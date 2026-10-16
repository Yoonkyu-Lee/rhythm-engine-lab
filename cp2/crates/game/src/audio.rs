//! 오디오 출력 (CP2 2차): 경로 셋 — WASAPI 공유, WASAPI 배타, ASIO — 이 같은 "소리 코어"를 쓴다.
//!
//! 시간 (CP1-4 기록 시계):
//! - 시간축 프레임: 장치 시계 직선(`SharedClock`)이 쓰는 프레임 번호. 판정·화면·봇은 "곡 시간 0 = 시간축 프레임 `origin`"으로 읽는다.
//!   WASAPI는 스트림에 쓴 순번 + 건너뛴 양(`ContentSkip::offset`), ASIO는 드라이버의 샘플 위치 그대로.
//! - 소리 시간축: 곡 시각 t의 자동재생은 시간축 프레임 `origin + D + t`에 놓는다. 키음은 누른 순간의 장치 위치 + D에 놓는다.
//!   → 기록 시점 C = 장치 시계 + D. PERFECT(누른 순간 장치 위치 = origin + t)면 키음과 반주가 같은 샘플이다.
//! - D: 블록을 채울 때마다 "직전 블록이 요청을 꺼낸 순간의 장치 위치 → 이 블록의 시작"을 잰다. 그 직후에 들어온 요청이
//!   가장 손해를 보므로, 이 값의 최대가 곧 요청 → 쓰기 위치 지연의 최대다(CP1-4 4-1의 탐침과 같은 양). 리드인 동안 잰 최대 + 2 ms로
//!   정하고 플레이 중에는 바꾸지 않는다. 그 뒤에도 계속 재서 기록한다.
//! - 오디오 정지: WASAPI는 깨어날 때마다 장치 위치를 직선 예측과 비교해 뒤처진 만큼 콘텐츠를 건너뛴다(시간축이 끊기지 않음).
//!   ASIO는 장치가 계속 흐르므로 샘플 위치를 그대로 쓰면 저절로 건너뛴다.
//!
//! 이 스레드(또는 ASIO 콜백)는 할당·잠금·파일 작업을 하지 않는다 (링이 가득 차면 버리고 센다).

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rcore::mixer::{LatePolicy, MixEvent, Mixer};
use rcore::ring::{ring, Consumer, Producer};
use rcore::runtime::{ns_to_frame, Compiled};
use synchronizer::asio::{self, AsioBufferInfo, AsioCallbacks, Driver, ASIOST_INT32_LSB};
use synchronizer::clock::ClockPolicy;
use synchronizer::qpc;
use synchronizer::shared_clock::{obs_ring, spawn_clock_thread, ClockSnapshot, ObsProducer, SharedClock};
use synchronizer::timeline::ContentSkip;
use synchronizer::wasapi::{self, IAudioClock, IAudioRenderClient, Mode, Stream};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::WaitForSingleObject;

/// 출력 경로.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioPath {
    Shared,
    Exclusive,
    Asio,
}

impl AudioPath {
    pub fn parse(s: &str) -> Result<AudioPath, String> {
        Ok(match s {
            "shared" => AudioPath::Shared,
            "exclusive" | "excl" => AudioPath::Exclusive,
            "asio" => AudioPath::Asio,
            _ => return Err(format!("--audio shared | exclusive | asio — {s}")),
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            AudioPath::Shared => "WASAPI 공유",
            AudioPath::Exclusive => "WASAPI 배타",
            AudioPath::Asio => "ASIO",
        }
    }
    pub fn key(&self) -> &'static str {
        match self {
            AudioPath::Shared => "shared",
            AudioPath::Exclusive => "exclusive",
            AudioPath::Asio => "asio",
        }
    }
}

/// 게임 로직 → 오디오 요청.
/// - 노트 소리: "노트 (lane, note)의 소리를 시간축 프레임 frame에 내라" (판정 스레드의 플레이어 키음).
/// - 소리 하나: 프레임에서 소리를 내는 구동 방식의 자동재생·키음 (frame = mixer::NOW면 즉시).
#[derive(Clone, Copy, Debug)]
pub struct KeyReq {
    pub frame: i64,
    pub lane: u16,
    pub note: u32,
    /// u32::MAX면 노트 소리, 아니면 이 소리 하나.
    pub sound: u32,
    pub gain: f32,
    pub player: bool,
}

impl KeyReq {
    pub fn note(frame: i64, lane: u16, note: u32) -> KeyReq {
        KeyReq { frame, lane, note, sound: u32::MAX, gain: 1.0, player: true }
    }
    pub fn sound(frame: i64, sound: u32, gain: f32, player: bool) -> KeyReq {
        KeyReq { frame, lane: 0, note: 0, sound, gain, player }
    }
}

/// 오디오 블록 하나의 기록.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockLog {
    /// 깨어난(ASIO: 콜백 진입) QPC.
    pub qpc_wake: i64,
    /// 이 블록의 첫 시간축 프레임.
    pub tl: i64,
    /// 시간축 − 스트림 (건너뛴 누적, WASAPI만).
    pub offset: i64,
    pub padding: u32,
    pub frames: u32,
    /// 장치 위치(시간축 프레임으로 바꾼 값)와 그 QPC(100 ns).
    pub dev_frame: u64,
    pub dev_qpc100: u64,
    /// 이 블록이 덮은 "요청 → 쓰기 위치" 필요량 (프레임, 직전 꺼냄 기준). 시계가 없으면 −1.
    pub need: i32,
    pub fill_ns: u32,
    pub voices: u16,
    pub injected_ms: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioFault {
    /// 곡 시간(초)에 도달하면
    pub at_s: f64,
    /// 오디오 스레드(ASIO: 콜백)가 그만큼 잔다.
    pub stall_ms: u16,
}

pub struct AudioCfg {
    pub path: AudioPath,
    /// WASAPI 장치 이름 (부분 일치).
    pub render: String,
    /// ASIO 버퍼 크기 (샘플).
    pub asio_buffer: i32,
    /// WASAPI 배타 주기 (ms). 이 장치는 3 ms 최소 주기에서 간헐적으로 33 ms씩 끊긴다(CP1-4) → 기본 5 ms.
    pub excl_period_ms: f64,
    pub lead_in_s: f64,
    pub tail_s: f64,
    pub master_gain: f32,
    pub late: LatePolicy,
    /// 노트 소리도 제때 자동으로 낸다 (입력 없이 정답과 비교할 때).
    pub autoplay_notes: bool,
    /// 자동재생을 믹서가 실행 데이터에서 직접 훑는가 (우리 방식 true, 프레임 구동 방식 false).
    pub auto_from_chart: bool,
    /// 같은 소리를 다시 울리면 앞 소리를 끊는가 (BMS식).
    pub cut_same: bool,
    /// D (ms). None이면 리드인 동안 재서 정한다 (측정 최대 + 2 ms).
    pub d_ms: Option<f64>,
    /// 오디오 정지 때 콘텐츠를 건너뛰는가 (우리 방식 true. 프레임 구동 프로필은 스트림이 밀리는 대로 둔다). ASIO는 늘 장치 위치.
    pub skip_on_stall: bool,
    /// ASIO일 때 루프백 입력을 녹음할 곳 (WASAPI는 game.rs의 녹음 스레드가 따로 한다).
    pub asio_capture: Option<PathBuf>,
    pub faults: Vec<AudioFault>,
}

pub struct AudioInfo {
    pub device: String,
    pub path: AudioPath,
    pub rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
    pub period_frames: u32,
    pub origin: i64,
    pub mmcss: bool,
}

pub struct AudioStats {
    pub written: i64,
    pub blocks: u64,
    pub overflows: u64,
    pub log_lost: u64,
    pub mix_lost: u64,
    pub cap_lost: u64,
}

fn zero_stats() -> AudioStats {
    AudioStats { written: 0, blocks: 0, overflows: 0, log_lost: 0, mix_lost: 0, cap_lost: 0 }
}

/// 화면 HUD·렉 감지·판정이 읽는 오디오 상태 (오디오 쪽이 원자 변수로만 쓴다).
#[derive(Default)]
pub struct AudioMonitor {
    pub blocks: AtomicU64,
    /// 공유 모드에서 버퍼가 비어 있던 블록 수 + 콘텐츠를 건너뛴 횟수.
    pub underruns: AtomicU64,
    /// 블록 사이 깨어난 간격의 최댓값 (µs). 읽는 쪽이 0으로 되돌린다.
    pub max_wake_gap_us: AtomicU64,
    pub voices: AtomicU64,
    /// 예약 프레임을 지나 도착한(늦은) 플레이어 키음 수와 마지막 지각량 (프레임).
    pub late_player: AtomicU64,
    pub last_player_late: AtomicI64,
    pub written: AtomicI64,
    /// 맥박: 마지막으로 깨어난 QPC.
    pub last_wake_qpc: AtomicI64,
    /// 고장 주입: 0이 아니면 다음 깨어날 때 그만큼(ms) 잔다.
    pub inject_stall_ms: std::sync::atomic::AtomicU32,
    /// 기록 시계 D (프레임). 정해지기 전에는 −1.
    pub d_frames: AtomicI64,
    /// 지금까지 잰 "요청 → 쓰기 위치" 필요량의 최대 (프레임).
    pub need_max: AtomicI64,
    /// 콘텐츠를 건너뛴 횟수와 누적 프레임.
    pub skips: AtomicU64,
    pub skipped_frames: AtomicI64,
}

pub struct AudioHandles {
    pub monitor: Arc<AudioMonitor>,
    pub info: AudioInfo,
    pub clock: Arc<SharedClock>,
    pub done: Arc<AtomicBool>,
    /// 세우면 다음 블록에서 끝낸다.
    pub stop: Arc<AtomicBool>,
    pub join: JoinHandle<Result<AudioStats, String>>,
}

/// QPC 틱 → 100 ns (128비트로 곱해 넘치지 않게).
fn to_100ns(q: i64) -> i64 {
    (q as i128 * 10_000_000 / qpc::frequency() as i128) as i64
}

/// 출력 장치의 샘플레이트 (곡을 그 속도로 준비하기 위해 먼저 묻는다).
pub fn device_rate(path: AudioPath, render: &str) -> Result<u32, String> {
    match path {
        AudioPath::Shared | AudioPath::Exclusive => (|| -> windows::core::Result<u32> {
            wasapi::com_init()?;
            let ep = wasapi::find(false, render)?;
            Ok(Stream::open(&ep.device)?.rate)
        })()
        .map_err(|e| format!("장치 {render}: {e}")),
        AudioPath::Asio => thread::spawn(|| -> Result<u32, String> {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().map_err(|e| e.to_string())? };
            let entry = asio::drivers().into_iter().find(|d| d.name.contains("TOPPING")).ok_or("TOPPING ASIO 드라이버 없음")?;
            let drv = Driver::open(&entry)?;
            let r = drv.sample_rate().map_err(|e| format!("ASIO 샘플레이트 조회 실패 {e}"))?;
            Ok(r.round() as u32)
        })
        .join()
        .map_err(|_| "ASIO 조회 스레드 패닉".to_string())?,
    }
}

/// 세 경로가 같이 쓰는 소리 코어: 키음 요청 꺼내기, 자동 연주, 섞기, D 측정, 기록.
struct Core<'a> {
    chart: &'a Compiled,
    mixer: Mixer<'a>,
    keys: Consumer<KeyReq>,
    blocks: Producer<BlockLog>,
    mix: Producer<MixEvent>,
    mon: Arc<AudioMonitor>,
    rate: i64,
    origin: i64,
    master_gain: f32,
    autoplay_notes: bool,
    note_cur: Vec<usize>,
    /// 직전 블록이 요청을 꺼낸 QPC.
    last_pop: Option<i64>,
    /// D를 리드인 동안 재는 중이면 그 최대·표본 수·끝 시간축 프레임.
    calibrating: bool,
    cal_max: f64,
    cal_n: u32,
    cal_end: i64,
    need_max: f64,
    stats: AudioStats,
}

impl<'a> Core<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        chart: &'a Compiled, cfg: &AudioCfg, rate: u32, origin: i64, keys: Consumer<KeyReq>, blocks: Producer<BlockLog>, mix: Producer<MixEvent>,
        mon: Arc<AudioMonitor>,
    ) -> Core<'a> {
        // D가 정해지기 전에는 자동재생을 아무 데도 놓지 않는다 (곡 시간 0을 아주 뒤로)
        let mut mixer = Mixer::new(chart, i64::MAX / 4, cfg.late, 4096);
        mixer.set_auto_from_chart(cfg.auto_from_chart);
        mixer.set_cut_same(cfg.cut_same);
        let mut c = Core {
            chart,
            mixer,
            keys,
            blocks,
            mix,
            mon,
            rate: rate as i64,
            origin,
            master_gain: cfg.master_gain,
            autoplay_notes: cfg.autoplay_notes,
            note_cur: vec![0; chart.chart.lanes.len()],
            last_pop: None,
            calibrating: cfg.d_ms.is_none(),
            cal_max: 0.0,
            cal_n: 0,
            // 리드인이 끝나기 0.3초 전까지 잰다
            cal_end: origin - (0.3 * rate as f64) as i64,
            need_max: 0.0,
            stats: zero_stats(),
        };
        if let Some(d) = cfg.d_ms {
            c.set_d((d * rate as f64 / 1e3).round() as i64);
        }
        c
    }

    fn set_d(&mut self, d: i64) {
        self.mixer.set_origin(self.origin + d);
        self.mon.d_frames.store(d, Ordering::Release);
    }

    /// 시간축 프레임 `tl`부터 `n` 프레임을 `out`(끼워 넣은 f32, ch 채널)에 채운다. `now`는 이 블록이 요청을 꺼내는 QPC.
    /// 돌려주는 값: 이 블록이 덮은 필요량 (프레임, 모르면 −1).
    fn block(&mut self, tl: i64, n: usize, out: &mut [f32], ch: usize, now: i64, snap: &ClockSnapshot) -> i32 {
        // 1) D 측정: 직전 꺼냄 직후에 들어온 요청이 이 블록에서 놓일 때 필요했던 앞당김
        let mut need_out = -1;
        if let (Some(lp), true) = (self.last_pop, snap.valid) {
            let need = tl as f64 - snap.frame_at(to_100ns(lp));
            need_out = need.round() as i32;
            if need > self.need_max {
                self.need_max = need;
                self.mon.need_max.store(need.round() as i64, Ordering::Relaxed);
            }
            if self.calibrating {
                self.cal_max = self.cal_max.max(need);
                self.cal_n += 1;
            }
        }
        if self.calibrating && tl >= self.cal_end && (self.cal_n >= 20 || tl >= self.origin - (0.1 * self.rate as f64) as i64) {
            // 측정 최대 + 2 ms, ms 단위로 올림
            let ms = ((self.cal_max * 1e3 / self.rate as f64) + 2.0).ceil();
            self.set_d((ms * self.rate as f64 / 1e3).round() as i64);
            self.calibrating = false;
        }
        // 2) 요청 꺼내기
        self.last_pop = Some(now);
        let blk_end = tl + n as i64;
        while let Some(k) = self.keys.pop() {
            if k.sound == u32::MAX {
                self.mixer.play_note(k.lane as usize, k.note as usize, k.frame);
            } else {
                self.mixer.play_sound(k.sound, k.gain, k.frame, k.player);
            }
        }
        if self.autoplay_notes {
            let d = self.mon.d_frames.load(Ordering::Relaxed);
            if d >= 0 {
                for (l, cur) in self.note_cur.iter_mut().enumerate() {
                    let notes = &self.chart.chart.lanes[l].notes;
                    while *cur < notes.len() {
                        let f = self.origin + d + ns_to_frame(notes[*cur].t, self.rate as u32);
                        if f >= blk_end {
                            break;
                        }
                        self.mixer.play_note(l, *cur, f);
                        *cur += 1;
                    }
                }
            }
        }
        // 3) 섞기
        let buf = &mut out[..n * ch];
        buf.fill(0.0);
        self.mixer.fill(buf, ch, tl, n);
        if self.master_gain != 1.0 {
            for v in buf.iter_mut() {
                *v *= self.master_gain;
            }
        }
        let (mon, mix, st) = (&self.mon, &mut self.mix, &mut self.stats);
        self.mixer.drain_each(|e| {
            if let MixEvent::Start { late, player: true, .. } = e {
                if late > 0 {
                    mon.late_player.fetch_add(1, Ordering::Relaxed);
                    mon.last_player_late.store(late, Ordering::Relaxed);
                }
            }
            if !mix.push(e) {
                st.mix_lost += 1;
            }
        });
        self.stats.blocks += 1;
        self.mon.blocks.fetch_add(1, Ordering::Relaxed);
        self.mon.voices.store(self.mixer.active_voices() as u64, Ordering::Relaxed);
        need_out
    }

    fn log(&mut self, l: BlockLog) {
        if !self.blocks.push(l) {
            self.stats.log_lost += 1;
        }
    }
}

pub fn spawn(
    chart: Arc<Compiled>, cfg: AudioCfg, keys: Consumer<KeyReq>, blocks: Producer<BlockLog>, mix: Producer<MixEvent>,
) -> Result<AudioHandles, String> {
    match cfg.path {
        AudioPath::Shared | AudioPath::Exclusive => spawn_wasapi(chart, cfg, keys, blocks, mix),
        AudioPath::Asio => spawn_asio(chart, cfg, keys, blocks, mix),
    }
}

// ───────────────────────────── WASAPI (공유·배타) ─────────────────────────────

fn spawn_wasapi(
    chart: Arc<Compiled>, cfg: AudioCfg, keys: Consumer<KeyReq>, blocks: Producer<BlockLog>, mix: Producer<MixEvent>,
) -> Result<AudioHandles, String> {
    let clock = Arc::new(SharedClock::new());
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let monitor = Arc::new(AudioMonitor { d_frames: AtomicI64::new(-1), ..Default::default() });
    let (info_tx, info_rx) = std::sync::mpsc::channel::<Result<AudioInfo, String>>();
    let (clock2, done2, stop2, mon) = (clock.clone(), done.clone(), stop.clone(), monitor.clone());
    let join = thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let r = (|| -> Result<AudioStats, Box<dyn std::error::Error>> {
                wasapi::com_init()?;
                let mmcss = wasapi::mmcss_pro_audio();
                let ep = wasapi::find(false, &cfg.render)?;
                let mode = if cfg.path == AudioPath::Exclusive { Mode::Exclusive } else { Mode::Shared };
                let s = Stream::open_mode_period(&ep.device, mode, Some((cfg.excl_period_ms * 1e4) as i64))?;
                if s.channels < 2 {
                    return Err(format!("스테레오가 아님: {} ch", s.channels).into());
                }
                if s.rate != chart.chart.bank.rate {
                    return Err(format!("장치 {} Hz ≠ 곡 뱅크 {} Hz", s.rate, chart.chart.bank.rate).into());
                }
                let rc: IAudioRenderClient = s.service()?;
                let ac: IAudioClock = s.service()?;
                let clock_freq = unsafe { ac.GetFrequency()? };
                let rate = s.rate as i64;
                let ch = s.channels as usize;
                let origin = (cfg.lead_in_s * rate as f64).round() as i64;
                let end = origin + ns_to_frame(chart.chart.end, s.rate) + (cfg.tail_s * rate as f64).round() as i64;
                let _ = info_tx.send(Ok(AudioInfo {
                    device: ep.name.clone(),
                    path: cfg.path,
                    rate: s.rate,
                    channels: s.channels,
                    buffer_frames: s.buffer_frames,
                    period_frames: s.period_frames,
                    origin,
                    mmcss,
                }));
                let (mut obs, obs_rx) = obs_ring(4096);
                let clock_thread = spawn_clock_thread(s.rate as f64, ClockPolicy::default(), obs_rx, clock2.clone());
                let mut core = Core::new(&chart, &cfg, s.rate, origin, keys, blocks, mix, mon.clone());
                // 오디오 정지 때 건너뛰기: 2 ms 넘게 뒤처졌을 때만 (공유 모드 위치 보고의 1 ms 톱니보다 크게)
                let mut cskip = ContentSkip::new(0.002 * rate as f64);
                let mut scratch = vec![0.0f32; s.buffer_frames as usize * ch];
                let mut fired = vec![false; cfg.faults.len()];
                let mut written: i64 = 0;

                // 미리 채우기 (시작 전): 시계가 없으니 D 측정은 다음 블록부터
                let t0 = qpc::now();
                core.block(0, s.buffer_frames as usize, &mut scratch, ch, t0, &ClockSnapshot::default());
                s.write(&rc, &scratch[..s.buffer_frames as usize * ch], s.buffer_frames)?;
                written += s.buffer_frames as i64;
                unsafe { s.client.Start()? };
                let mut prev_wake = qpc::now();
                while written + cskip.offset < end && !stop2.load(Ordering::Relaxed) {
                    unsafe { WaitForSingleObject(s.event, 2000) };
                    let wake = qpc::now();
                    mon.last_wake_qpc.store(wake, Ordering::Release);
                    let mut injected = 0u16;
                    let req = mon.inject_stall_ms.swap(0, Ordering::AcqRel);
                    if req > 0 {
                        thread::sleep(Duration::from_millis(req as u64));
                        injected += req as u16;
                    }
                    let song_s = (written + cskip.offset - origin) as f64 / rate as f64;
                    for (i, f) in cfg.faults.iter().enumerate() {
                        if !fired[i] && song_s >= f.at_s {
                            fired[i] = true;
                            thread::sleep(Duration::from_millis(f.stall_ms as u64));
                            injected += f.stall_ms;
                        }
                    }
                    // 장치 위치를 먼저 보고 (뒤처졌으면) 건너뛴 다음에 채운다
                    let (mut pos, mut qpos) = (0u64, 0u64);
                    unsafe { ac.GetPosition(&mut pos, Some(&mut qpos))? };
                    let raw = (pos as f64 * rate as f64 / clock_freq as f64).round() as u64;
                    let before_off = cskip.offset;
                    let frame = if cfg.skip_on_stall { cskip.observe(raw, qpos as i64, &clock2.load()) } else { raw };
                    if cskip.offset != before_off {
                        mon.skips.fetch_add(1, Ordering::Relaxed);
                        mon.skipped_frames.fetch_add(cskip.offset - before_off, Ordering::Relaxed);
                        mon.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    if raw > (rate / 20) as u64 {
                        obs.push(frame, qpos as i64);
                    }
                    let pad = if s.mode == Mode::Exclusive { 0 } else { unsafe { s.client.GetCurrentPadding()? } };
                    if s.mode != Mode::Exclusive && pad == 0 {
                        mon.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    let n = s.frames_to_write()?;
                    let tl = written + cskip.offset;
                    let mut need = -1;
                    let tf = qpc::now();
                    if n > 0 {
                        need = core.block(tl, n as usize, &mut scratch, ch, tf, &clock2.load());
                        s.write(&rc, &scratch[..n as usize * ch], n)?;
                        written += n as i64;
                    }
                    let fill_ns = ((qpc::now() - tf) * 1_000_000_000 / qpc::frequency()) as u32;
                    let gap_us = ((wake - prev_wake) * 1_000_000 / qpc::frequency()) as u64;
                    prev_wake = wake;
                    mon.max_wake_gap_us.fetch_max(gap_us, Ordering::Relaxed);
                    mon.written.store(written + cskip.offset, Ordering::Relaxed);
                    let voices = core.mixer.active_voices() as u16;
                    core.log(BlockLog {
                        qpc_wake: wake,
                        tl,
                        offset: cskip.offset,
                        padding: pad,
                        frames: n,
                        dev_frame: frame,
                        dev_qpc100: qpos,
                        need,
                        fill_ns,
                        voices,
                        injected_ms: injected,
                    });
                }
                // 남은 버퍼가 다 나갈 때까지 기다린 뒤 멈춘다
                thread::sleep(Duration::from_millis((s.buffer_frames as u64 * 1000 / rate as u64) + 50));
                unsafe { s.client.Stop()? };
                drop(obs);
                let _ = clock_thread.join();
                let mut st = std::mem::replace(&mut core.stats, zero_stats());
                st.written = written + cskip.offset;
                st.overflows = core.mixer.overflows;
                Ok(st)
            })();
            done2.store(true, Ordering::Release);
            r.map_err(|e| {
                let msg = format!("오디오: {e}");
                let _ = info_tx.send(Err(msg.clone()));
                msg
            })
        })
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "오디오 스레드가 시작 정보를 보내지 못함".to_string())??;
    Ok(AudioHandles { monitor, info, clock, done, stop, join })
}

// ───────────────────────────── ASIO ─────────────────────────────

const OUT_L: i32 = 0; // Playback 1
const OUT_R: i32 = 1; // Playback 2
const IN_L: i32 = 4; // Loopback 1
const IN_R: i32 = 5; // Loopback 2
const CAP_MAX: usize = 512; // 콜백 하나의 녹음 조각 (스테레오 256 프레임까지)

/// ASIO 콜백 → 녹음 스레드: 루프백 입력 버퍼 하나.
#[derive(Clone, Copy)]
struct CapChunk {
    spos: i64,
    qpc: i64,
    n: u16,
    data: [f32; CAP_MAX],
}

struct AsioState {
    driver: *mut c_void,
    bufs: [AsioBufferInfo; 4],
    n: usize,
    core: Core<'static>,
    obs: ObsProducer,
    cap: Option<Producer<CapChunk>>,
    clock: Arc<SharedClock>,
    mon: Arc<AudioMonitor>,
    scratch: Vec<f32>,
    faults: Vec<AudioFault>,
    fired: Vec<bool>,
    origin: i64,
    rate: i64,
    end: i64,
    prev_wake: i64,
    finished: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

static ASIO_STATE: AtomicPtr<AsioState> = AtomicPtr::new(std::ptr::null_mut());

fn sample_position(drv: *mut c_void) -> u64 {
    // Driver 래퍼는 Drop에서 Release를 부르므로, 콜백에서는 함수표를 직접 부른다 (CP1 asiorun과 같음).
    #[repr(C)]
    struct V {
        _p: [usize; 17],
        get_sample_position: extern "system" fn(*mut c_void, *mut asio::AsioSamples, *mut asio::AsioSamples) -> i32,
    }
    unsafe {
        let vt = &**(drv as *const *const V);
        let (mut s, mut t) = (asio::AsioSamples::default(), asio::AsioSamples::default());
        (vt.get_sample_position)(drv, &mut s, &mut t);
        s.get()
    }
}

fn output_ready(drv: *mut c_void) {
    #[repr(C)]
    struct V {
        _p: [usize; 23],
        output_ready: extern "system" fn(*mut c_void) -> i32,
    }
    unsafe {
        let vt = &**(drv as *const *const V);
        (vt.output_ready)(drv);
    }
}

extern "C" fn buffer_switch(index: i32, _direct: i32) {
    let q_in = qpc::now();
    let p = ASIO_STATE.load(Ordering::Acquire);
    if p.is_null() {
        return;
    }
    let st = unsafe { &mut *p };
    let n = st.n;
    let idx = index as usize;
    if st.finished.load(Ordering::Relaxed) {
        // 끝난 뒤에는 무음
        unsafe {
            for b in 0..2 {
                std::ptr::write_bytes(st.bufs[b].buffers[idx] as *mut i32, 0, n);
            }
        }
        return;
    }
    st.mon.last_wake_qpc.store(q_in, Ordering::Release);
    let mut injected = 0u16;
    let req = st.mon.inject_stall_ms.swap(0, Ordering::AcqRel);
    if req > 0 {
        thread::sleep(Duration::from_millis(req as u64));
        injected += req as u16;
    }
    // 장치 위치 = 시간축 (장치는 콜백이 늦어도 흐르므로 그대로 쓰면 콘텐츠가 저절로 건너뛰어진다)
    let mut spos = sample_position(st.driver) as i64;
    let song_s = (spos - st.origin) as f64 / st.rate as f64;
    for i in 0..st.faults.len() {
        if !st.fired[i] && song_s >= st.faults[i].at_s {
            st.fired[i] = true;
            thread::sleep(Duration::from_millis(st.faults[i].stall_ms as u64));
            injected += st.faults[i].stall_ms;
        }
    }
    if injected > 0 {
        spos = sample_position(st.driver) as i64;
    }
    if spos > st.rate / 20 {
        st.obs.push(spos as u64, to_100ns(q_in));
    }
    // 녹음: 루프백 L/R (Int32 → f32)
    if let Some(cap) = st.cap.as_mut() {
        let mut c = CapChunk { spos, qpc: q_in, n: n as u16, data: [0.0; CAP_MAX] };
        unsafe {
            let l = st.bufs[2].buffers[idx] as *const i32;
            let r = st.bufs[3].buffers[idx] as *const i32;
            for i in 0..n.min(CAP_MAX / 2) {
                c.data[i * 2] = *l.add(i) as f32 / 2147483648.0;
                c.data[i * 2 + 1] = *r.add(i) as f32 / 2147483648.0;
            }
        }
        if !cap.push(c) {
            st.core.stats.cap_lost += 1;
        }
    }
    let snap = st.clock.load();
    let tf = qpc::now();
    let need = st.core.block(spos, n, &mut st.scratch, 2, tf, &snap);
    unsafe {
        let l = st.bufs[0].buffers[idx] as *mut i32;
        let r = st.bufs[1].buffers[idx] as *mut i32;
        for i in 0..n {
            *l.add(i) = (st.scratch[i * 2].clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32;
            *r.add(i) = (st.scratch[i * 2 + 1].clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32;
        }
    }
    output_ready(st.driver);
    let gap_us = ((q_in - st.prev_wake) * 1_000_000 / qpc::frequency()) as u64;
    st.prev_wake = q_in;
    st.mon.max_wake_gap_us.fetch_max(gap_us, Ordering::Relaxed);
    st.mon.written.store(spos + n as i64, Ordering::Relaxed);
    let voices = st.core.mixer.active_voices() as u16;
    let fill_ns = ((qpc::now() - tf) * 1_000_000_000 / qpc::frequency()) as u32;
    st.core.log(BlockLog {
        qpc_wake: q_in,
        tl: spos,
        offset: 0,
        padding: 0,
        frames: n as u32,
        dev_frame: spos as u64,
        dev_qpc100: to_100ns(q_in) as u64,
        need,
        fill_ns,
        voices,
        injected_ms: injected,
    });
    if spos >= st.end || st.stop.load(Ordering::Relaxed) {
        st.finished.store(true, Ordering::Release);
    }
}

extern "C" fn sample_rate_did_change(_r: f64) {}

extern "C" fn asio_message(selector: i32, value: i32, _msg: *mut c_void, _opt: *mut f64) -> i32 {
    match selector {
        1 => matches!(value, 2 | 3 | 5 | 6) as i32, // selector supported?
        2 => 2,                                      // engine version
        3 | 5 | 6 => 1,                              // reset / resync / latencies changed
        _ => 0,
    }
}

extern "C" fn buffer_switch_time_info(params: *mut c_void, index: i32, direct: i32) -> *mut c_void {
    buffer_switch(index, direct);
    params
}

static CALLBACKS: AsioCallbacks = AsioCallbacks { buffer_switch, sample_rate_did_change, asio_message, buffer_switch_time_info };

fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// ASIO 녹음 조각을 파일로 (WASAPI 녹음과 같은 형식: device_pos = 장치 샘플 위치).
fn spawn_asio_capture_writer(dir: PathBuf, mut rx: Consumer<CapChunk>, stop: Arc<AtomicBool>) -> JoinHandle<Result<u64, String>> {
    thread::spawn(move || -> Result<u64, String> {
        use std::io::{BufWriter, Write};
        let r = (|| -> std::io::Result<u64> {
            let mut wav = BufWriter::new(std::fs::File::create(dir.join("capture_2ch.f32"))?);
            let mut log = BufWriter::new(std::fs::File::create(dir.join("capture_packets.csv"))?);
            writeln!(log, "qpc,device_pos,qpc100ns,frames,flags")?;
            let mut total = 0u64;
            loop {
                let mut got = false;
                while let Some(c) = rx.pop() {
                    got = true;
                    for v in &c.data[..c.n as usize * 2] {
                        wav.write_all(&v.to_le_bytes())?;
                    }
                    writeln!(log, "{},{},{},{},0", c.qpc, c.spos, to_100ns(c.qpc), c.n)?;
                    total += c.n as u64;
                }
                if !got {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            wav.flush()?;
            log.flush()?;
            Ok(total)
        })();
        r.map_err(|e| format!("ASIO 녹음: {e}"))
    })
}

fn spawn_asio(
    chart: Arc<Compiled>, cfg: AudioCfg, keys: Consumer<KeyReq>, blocks: Producer<BlockLog>, mix: Producer<MixEvent>,
) -> Result<AudioHandles, String> {
    let clock = Arc::new(SharedClock::new());
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let monitor = Arc::new(AudioMonitor { d_frames: AtomicI64::new(-1), ..Default::default() });
    let (info_tx, info_rx) = std::sync::mpsc::channel::<Result<AudioInfo, String>>();
    let (clock2, done2, stop2, mon) = (clock.clone(), done.clone(), stop.clone(), monitor.clone());
    let join = thread::Builder::new()
        .name("asio".into())
        .spawn(move || {
            let r = (|| -> Result<AudioStats, Box<dyn std::error::Error>> {
                // 드라이버를 만든 이 스레드(STA)는 메시지를 계속 처리해야 콜백이 멈추지 않는다 (CP1-2)
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
                let entry = asio::drivers().into_iter().find(|d| d.name.contains("TOPPING")).ok_or("TOPPING ASIO 드라이버 없음")?;
                let drv = Driver::open(&entry)?;
                let rate_f = drv.sample_rate().map_err(|e| format!("샘플레이트 조회 실패 {e}"))?;
                let rate = rate_f.round() as u32;
                if rate != chart.chart.bank.rate {
                    return Err(format!("ASIO {} Hz ≠ 곡 뱅크 {} Hz", rate, chart.chart.bank.rate).into());
                }
                for (c, is_in) in [(OUT_L, false), (OUT_R, false), (IN_L, true), (IN_R, true)] {
                    let (_, t, _) = drv.channel_info(c, is_in).map_err(|e| format!("채널 정보 실패 {e}"))?;
                    if t != ASIOST_INT32_LSB {
                        return Err(format!("채널 {c}가 Int32LSB가 아님 (형식 {t})").into());
                    }
                }
                let origin = (cfg.lead_in_s * rate as f64).round() as i64;
                let end = origin + ns_to_frame(chart.chart.end, rate) + (cfg.tail_s * rate as f64).round() as i64;
                let (obs, obs_rx) = obs_ring(8192);
                let clock_thread = spawn_clock_thread(rate as f64, ClockPolicy::default(), obs_rx, clock2.clone());
                let cap_stop = Arc::new(AtomicBool::new(false));
                let (cap_tx, cap_writer) = match cfg.asio_capture.clone() {
                    Some(dir) => {
                        let (tx, rx) = ring::<CapChunk>(8192);
                        (Some(tx), Some(spawn_asio_capture_writer(dir, rx, cap_stop.clone())))
                    }
                    None => (None, None),
                };
                // 콜백 상태는 이 스레드가 소유한다. 코어는 곡을 빌리므로, 드라이버를 멈출 때까지 `chart`가 살아 있게 한다.
                let chart_ref: &'static Compiled = unsafe { &*Arc::as_ptr(&chart) };
                let core = Core::new(chart_ref, &cfg, rate, origin, keys, blocks, mix, mon.clone());
                let null = std::ptr::null_mut();
                let finished = Arc::new(AtomicBool::new(false));
                let mut st = Box::new(AsioState {
                    driver: drv.raw(),
                    bufs: [
                        AsioBufferInfo { is_input: 0, channel_num: OUT_L, buffers: [null; 2] },
                        AsioBufferInfo { is_input: 0, channel_num: OUT_R, buffers: [null; 2] },
                        AsioBufferInfo { is_input: 1, channel_num: IN_L, buffers: [null; 2] },
                        AsioBufferInfo { is_input: 1, channel_num: IN_R, buffers: [null; 2] },
                    ],
                    n: cfg.asio_buffer as usize,
                    core,
                    obs,
                    cap: cap_tx,
                    clock: clock2.clone(),
                    mon: mon.clone(),
                    scratch: vec![0.0; 4096 * 2],
                    faults: cfg.faults.clone(),
                    fired: vec![false; cfg.faults.len()],
                    origin,
                    rate: rate as i64,
                    end,
                    prev_wake: qpc::now(),
                    finished: finished.clone(),
                    stop: stop2.clone(),
                });
                unsafe {
                    drv.create_buffers(&mut st.bufs, cfg.asio_buffer, &CALLBACKS).map_err(|e| format!("createBuffers 실패 {e} ({})", asio::error_name(e)))?;
                }
                let _ = info_tx.send(Ok(AudioInfo {
                    device: entry.name.clone(),
                    path: AudioPath::Asio,
                    rate,
                    channels: 2,
                    buffer_frames: cfg.asio_buffer as u32,
                    period_frames: cfg.asio_buffer as u32,
                    origin,
                    mmcss: false,
                }));
                ASIO_STATE.store(&mut *st as *mut AsioState, Ordering::Release);
                drv.start().map_err(|e| format!("start 실패 {e}"))?;
                while !finished.load(Ordering::Acquire) {
                    pump_messages();
                    thread::sleep(Duration::from_millis(1));
                }
                // 마지막 소리가 나가도록 잠깐 더
                for _ in 0..100 {
                    pump_messages();
                    thread::sleep(Duration::from_millis(1));
                }
                let _ = drv.stop();
                let _ = drv.dispose_buffers();
                ASIO_STATE.store(std::ptr::null_mut(), Ordering::Release);
                cap_stop.store(true, Ordering::Release);
                let mut stats = std::mem::replace(&mut st.core.stats, zero_stats());
                stats.overflows = st.core.mixer.overflows;
                stats.written = mon.written.load(Ordering::Relaxed);
                drop(st); // 관측 생산자가 사라지면 시계 스레드가 끝난다
                let _ = clock_thread.join();
                if let Some(w) = cap_writer {
                    w.join().map_err(|_| "ASIO 녹음 스레드 패닉")??;
                }
                drop(drv);
                drop(chart);
                Ok(stats)
            })();
            done2.store(true, Ordering::Release);
            r.map_err(|e| {
                let msg = format!("오디오(ASIO): {e}");
                let _ = info_tx.send(Err(msg.clone()));
                msg
            })
        })
        .map_err(|e| e.to_string())?;
    let info = info_rx.recv().map_err(|_| "오디오 스레드가 시작 정보를 보내지 못함".to_string())??;
    Ok(AudioHandles { monitor, info, clock, done, stop, join })
}
