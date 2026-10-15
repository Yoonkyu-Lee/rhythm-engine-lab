//! 오디오 출력 스레드 (WASAPI 공유 모드, 이벤트 구동). "우리 방식"의 소리 경로.
//!
//! - 믹서(`rcore::mixer`)가 실행 데이터를 스트림 프레임에 직접 섞는다 (자동재생은 프레임을 기다리지 않음).
//! - 곡 시간 0 = 스트림 프레임 `origin` (리드인 뒤). 콘텐츠 대응 C(f) = (f − origin) / rate.
//! - 장치 위치 관측 → 관측 링 → 시계 스레드 → `SharedClock` (CP1 그대로). 다른 스레드는 이걸로 "지금 곡 몇 초"를 계산한다.
//! - 판정 스레드의 키음 요청은 SPSC 링으로 받는다. 블록 기록·믹서 사건은 SPSC 링으로 기록 스레드에 넘긴다.
//! - 이 스레드는 할당·잠금·파일 작업을 하지 않는다 (링이 가득 차면 버리고 센다).
//! - 마스터 이득: 섞은 뒤 곱한다 (헤드룸 정책은 main이 정함).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rcore::mixer::{LatePolicy, MixEvent, Mixer};
use rcore::ring::{Consumer, Producer};
use rcore::runtime::{ns_to_frame, Compiled};
use synchronizer::clock::ClockPolicy;
use synchronizer::qpc;
use synchronizer::shared_clock::{obs_ring, spawn_clock_thread, SharedClock};
use synchronizer::wasapi::{self, IAudioClock, IAudioRenderClient, Stream};
use windows::Win32::System::Threading::WaitForSingleObject;

/// 판정 스레드 → 오디오: "노트 (lane, note)의 소리를 스트림 프레임 frame에 내라".
#[derive(Clone, Copy, Debug)]
pub struct KeyReq {
    pub frame: i64,
    pub lane: u16,
    pub note: u32,
}

/// 오디오 블록 하나의 기록.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockLog {
    /// 이벤트로 깨어난 QPC (틱).
    pub qpc_wake: i64,
    /// 이 블록 전까지 쓴 프레임 수 (= 이 블록의 첫 스트림 프레임).
    pub written: i64,
    pub padding: u32,
    pub frames: u32,
    /// 장치가 보고한 재생 위치(프레임)와 그 QPC(100 ns).
    pub dev_frame: u64,
    pub dev_qpc100: u64,
    /// 섞기에 걸린 시간 (ns).
    pub fill_ns: u32,
    pub voices: u16,
    /// 고장 주입으로 일부러 잔 시간 (ms, 0이면 없음).
    pub injected_ms: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioFault {
    /// 곡 시간(초)에 도달하면
    pub at_s: f64,
    /// 오디오 스레드가 버퍼를 채우지 않고 잔다 (언더런 유발).
    pub stall_ms: u16,
}

pub struct AudioCfg {
    pub render: String,
    pub lead_in_s: f64,
    pub tail_s: f64,
    pub master_gain: f32,
    pub late: LatePolicy,
    /// 노트 소리도 제때 자동으로 낸다 (입력 없이 정답과 비교할 때).
    pub autoplay_notes: bool,
    pub faults: Vec<AudioFault>,
}

pub struct AudioInfo {
    pub device: String,
    pub rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
    pub period_100ns: i64,
    pub origin: i64,
    pub mmcss: bool,
}

pub struct AudioStats {
    pub written: i64,
    pub blocks: u64,
    pub overflows: u64,
    pub log_lost: u64,
    pub mix_lost: u64,
}

/// 화면 HUD·렉 감지가 읽는 오디오 상태 (오디오 스레드가 원자 변수로만 쓴다).
#[derive(Default)]
pub struct AudioMonitor {
    pub blocks: AtomicU64,
    /// 버퍼가 비어 있던(padding 0) 블록 수 = 언더런 의심.
    pub underruns: AtomicU64,
    /// 블록 사이 깨어난 간격의 최댓값 (µs). 읽는 쪽이 0으로 되돌린다.
    pub max_wake_gap_us: AtomicU64,
    pub voices: AtomicU64,
    /// 늦게 시작한 키음 수와 마지막 지각량 (프레임).
    pub late_player: AtomicU64,
    pub last_player_late: AtomicI64,
    pub written: AtomicI64,
    /// 맥박: 마지막으로 깨어난 QPC (감시 스레드가 오디오 정지를 알아챈다).
    pub last_wake_qpc: AtomicI64,
    /// 고장 주입: 0이 아니면 다음 깨어날 때 그만큼(ms) 버퍼를 채우지 않고 잔다.
    pub inject_stall_ms: std::sync::atomic::AtomicU32,
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

/// 출력 장치의 공유 모드 믹스 속도 (곡을 그 속도로 준비하기 위해 먼저 묻는다).
pub fn device_rate(render: &str) -> Result<u32, String> {
    (|| -> windows::core::Result<u32> {
        wasapi::com_init()?;
        let ep = wasapi::find(false, render)?;
        Ok(Stream::open(&ep.device)?.rate)
    })()
    .map_err(|e| format!("장치 {render}: {e}"))
}

pub fn spawn(
    chart: Arc<Compiled>,
    cfg: AudioCfg,
    mut keys: Consumer<KeyReq>,
    mut blocks: Producer<BlockLog>,
    mut mix: Producer<MixEvent>,
) -> Result<AudioHandles, String> {
    let clock = Arc::new(SharedClock::new());
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let monitor = Arc::new(AudioMonitor::default());
    let mon = monitor.clone();
    let (info_tx, info_rx) = std::sync::mpsc::channel::<Result<AudioInfo, String>>();
    let (clock2, done2) = (clock.clone(), done.clone());
    let join = thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let r = (|| -> Result<AudioStats, Box<dyn std::error::Error>> {
                wasapi::com_init()?;
                let mmcss = wasapi::mmcss_pro_audio();
                let ep = wasapi::find(false, &cfg.render)?;
                let s = Stream::open(&ep.device)?;
                if s.bits != 32 || s.channels < 2 {
                    return Err(format!("믹스 형식이 32비트 float 스테레오가 아님: {} bit {} ch", s.bits, s.channels).into());
                }
                if s.rate != chart.chart.bank.rate {
                    return Err(format!("장치 {} Hz ≠ 곡 뱅크 {} Hz (리샘플링 미지원)", s.rate, chart.chart.bank.rate).into());
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
                    rate: s.rate,
                    channels: s.channels,
                    buffer_frames: s.buffer_frames,
                    period_100ns: s.default_period_100ns,
                    origin,
                    mmcss,
                }));
                let (mut obs, obs_rx) = obs_ring(4096);
                let clock_thread = spawn_clock_thread(s.rate as f64, ClockPolicy::default(), obs_rx, clock2.clone());
                let mut mixer = Mixer::new(&chart, origin, cfg.late, 4096);
                let mut scratch = vec![0.0f32; s.buffer_frames as usize * ch];
                // 노트 자동 연주 커서 (레인별)
                let lanes = chart.chart.lanes.len();
                let mut note_cur = vec![0usize; lanes];
                let mut fired = vec![false; cfg.faults.len()];
                let mut st = AudioStats { written: 0, blocks: 0, overflows: 0, log_lost: 0, mix_lost: 0 };
                let mut written: i64 = 0;

                let mut fill = |n: u32, written: &mut i64, mixer: &mut Mixer, st: &mut AudioStats| -> windows::core::Result<u32> {
                    let t0 = qpc::now();
                    let buf = &mut scratch[..n as usize * ch];
                    buf.fill(0.0);
                    let blk_end = *written + n as i64;
                    while let Some(k) = keys.pop() {
                        mixer.play_note(k.lane as usize, k.note as usize, k.frame);
                    }
                    if cfg.autoplay_notes {
                        for (l, cur) in note_cur.iter_mut().enumerate() {
                            let notes = &chart.chart.lanes[l].notes;
                            while *cur < notes.len() {
                                let f = origin + ns_to_frame(notes[*cur].t, s.rate);
                                if f >= blk_end {
                                    break;
                                }
                                mixer.play_note(l, *cur, f);
                                *cur += 1;
                            }
                        }
                    }
                    mixer.fill(buf, ch, *written, n as usize);
                    if cfg.master_gain != 1.0 {
                        for v in buf.iter_mut() {
                            *v *= cfg.master_gain;
                        }
                    }
                    unsafe {
                        let p = rc.GetBuffer(n)? as *mut f32;
                        std::ptr::copy_nonoverlapping(buf.as_ptr(), p, buf.len());
                        rc.ReleaseBuffer(n, 0)?;
                    }
                    mixer.drain_each(|e| {
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
                    *written = blk_end;
                    Ok(((qpc::now() - t0) * 1_000_000_000 / qpc::frequency()) as u32)
                };

                let fill_ns = fill(s.buffer_frames, &mut written, &mut mixer, &mut st)?;
                let _ = blocks.push(BlockLog { qpc_wake: qpc::now(), written: 0, padding: 0, frames: s.buffer_frames, fill_ns, ..Default::default() });
                unsafe { s.client.Start()? };
                let mut prev_wake = qpc::now();
                while written < end && !stop2.load(Ordering::Relaxed) {
                    unsafe { WaitForSingleObject(s.event, 2000) };
                    let wake = qpc::now();
                    mon.last_wake_qpc.store(wake, Ordering::Release);
                    let mut injected = 0u16;
                    let req = mon.inject_stall_ms.swap(0, Ordering::AcqRel);
                    if req > 0 {
                        thread::sleep(Duration::from_millis(req as u64));
                        injected += req as u16;
                    }
                    let song_s = (written - origin) as f64 / rate as f64;
                    for (i, f) in cfg.faults.iter().enumerate() {
                        if !fired[i] && song_s >= f.at_s {
                            fired[i] = true;
                            thread::sleep(Duration::from_millis(f.stall_ms as u64));
                            injected += f.stall_ms;
                        }
                    }
                    let pad = unsafe { s.client.GetCurrentPadding()? };
                    let n = s.buffer_frames - pad;
                    let before = written;
                    let fill_ns = if n > 0 { fill(n, &mut written, &mut mixer, &mut st)? } else { 0 };
                    let (mut pos, mut qpos) = (0u64, 0u64);
                    unsafe { ac.GetPosition(&mut pos, Some(&mut qpos))? };
                    let frame = (pos as f64 * rate as f64 / clock_freq as f64).round() as u64;
                    if frame > (rate / 20) as u64 {
                        obs.push(frame, qpos as i64);
                    }
                    st.blocks += 1;
                    mon.blocks.fetch_add(1, Ordering::Relaxed);
                    if pad == 0 {
                        mon.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    let gap_us = ((wake - prev_wake) * 1_000_000 / qpc::frequency()) as u64;
                    prev_wake = wake;
                    mon.max_wake_gap_us.fetch_max(gap_us, Ordering::Relaxed);
                    mon.voices.store(mixer.active_voices() as u64, Ordering::Relaxed);
                    mon.written.store(written, Ordering::Relaxed);
                    let log = BlockLog {
                        qpc_wake: wake,
                        written: before,
                        padding: pad,
                        frames: n,
                        dev_frame: frame,
                        dev_qpc100: qpos,
                        fill_ns,
                        voices: mixer.active_voices() as u16,
                        injected_ms: injected,
                    };
                    if !blocks.push(log) {
                        st.log_lost += 1;
                    }
                }
                // 남은 버퍼가 다 나갈 때까지 기다린 뒤 멈춘다
                thread::sleep(Duration::from_millis((s.buffer_frames as u64 * 1000 / rate as u64) + 50));
                unsafe { s.client.Stop()? };
                drop(obs);
                let _ = clock_thread.join();
                st.written = written;
                st.overflows = mixer.overflows;
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
