//! 2번 단계: 예약기 정책 비교.
//!
//!   testbench sched --mode sample|immediate [--fps 60] [--secs 60] [--lookahead-ms 100] [--late cut|whole]
//!                   [--sound click|noise|long] [--event-period-s 0.5007] [--faults A@10:50,B@30:300]
//!   noise = 50 ms 노이즈 버스트, long = 1초 노이즈 (늦은 긴 소리의 앞부분 자르기 확인용)
//!
//! 고장 주입 (`--faults`, 시각은 오디오 시간 초, 길이는 ms):
//!   A@t:ms  렌더(오디오) 스레드가 t초에 ms 동안 버퍼를 채우지 않는다 → 언더런.
//!   B@t:ms  게임 스레드가 t초에 ms 동안 멈춘다.
//!
//! 왼쪽 채널: 시험 이벤트. 게임 스레드가 정책에 따라 예약기로 보낸다.
//!   sample    : 이벤트 시각보다 lookahead 만큼 앞서 "프레임 N에서 시작"(At)을 보낸다.
//!   immediate : fps 주기로 도는 게임 루프가 시계를 보고, 이벤트 시각이 지났으면 "지금 울려"(Now)를 보낸다.
//! 오른쪽 채널: 기준 클릭. 렌더 스레드가 정해진 프레임에 직접 쓴다. 루프백 위치를 재생 프레임 번호로 바꾸는 기준.
//! 분석: cp1/tools/analyze_sched.py
//!
//! CP1-4 요청 탐침 (`--probe-hz N`): 탐침 스레드가 무작위 순간에 "지금 장치 위치 A(q)에 무음 소리(1번)를 놓아 달라"를 보낸다.
//! 믹서가 그 요청을 처리한 블록의 첫 프레임(= 그 순간 소리를 놓을 수 있는 가장 이른 위치)을 기록하므로,
//! starts.csv의 sound=1 행에서 (block − requested)가 "요청 → 쓰기 위치" 지연, 곧 기록 시계 D가 덮어야 하는 양이다. 분석: tools/analyze_d.py

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use synchronizer::clock::ClockPolicy;
use synchronizer::shared_clock::{obs_ring, spawn_clock_thread, SharedClock};
use synchronizer::timeline::ContentSkip;
use synchronizer::qpc;
use synchronizer::scheduler::{Cmd, LatePolicy, Mixer};
use synchronizer::wasapi::{self, IAudioClock, IAudioRenderClient, Stream};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::{arg, capture_loop, Meta};

pub(crate) const EVENT_START_S: f64 = 1.0;
pub(crate) const EVENT_PERIOD_S: f64 = 0.5007; // 기본값. 게임 프레임 주기와 맞물리지 않게
pub(crate) const PILOT_START_S: f64 = 1.25;
pub(crate) const PILOT_PERIOD_S: f64 = 0.5;
pub(crate) const CLICK: [f32; 3] = [0.25; 3];

/// 50 ms 결정적 노이즈 버스트 (양끝 1 ms 페이드). 앞부분이 잘려도 남은 부분의 위치를 상호상관으로 잴 수 있다.
/// CP1-4 요청 탐침 스레드. 평균 `hz`번/초, 간격은 [0.5, 1.5]/hz 균등 무작위 (블록 경계와 위상이 고르게 섞이게).
pub(crate) fn spawn_req_probe(clock: Arc<SharedClock>, tx: Sender<Cmd>, hz: f64, stop: Arc<AtomicBool>) -> thread::JoinHandle<u64> {
    thread::spawn(move || {
        unsafe { timeBeginPeriod(1) };
        let (mut x, mut sent) = (0x9e37_79b9u32, 0u64);
        let qf = qpc::frequency() as i128;
        while !stop.load(Ordering::Relaxed) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let gap = (0.5 + x as f64 / u32::MAX as f64) / hz;
            thread::sleep(Duration::from_secs_f64(gap));
            let snap = clock.load();
            if !snap.valid {
                continue;
            }
            let t100 = (qpc::now() as i128 * 10_000_000 / qf) as i64;
            let f = snap.frame_at(t100);
            if f > 0.0 && tx.send(Cmd::At { frame: f.round() as u64, sound: 1, channel: 0 }).is_ok() {
                sent += 1;
            }
        }
        unsafe { timeEndPeriod(1) };
        sent
    })
}

pub(crate) fn noise_burst(rate: f64, secs: f64) -> Vec<f32> {
    let n = (secs * rate) as usize;
    let fade = (0.001 * rate) as usize;
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let v = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let g = if i < fade { i as f32 / fade as f32 } else if i + fade > n { (n - i) as f32 / fade as f32 } else { 1.0 };
            0.2 * v * g
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct Fault {
    pub kind: char,
    pub at_s: f64,
    pub ms: u64,
}

pub(crate) fn parse_faults(s: &str) -> Result<Vec<Fault>, String> {
    if s.is_empty() {
        return Ok(vec![]);
    }
    s.split(',')
        .map(|f| {
            let (k, rest) = f.split_once('@').ok_or(format!("고장 형식: {f}"))?;
            let (t, ms) = rest.split_once(':').ok_or(format!("고장 형식: {f}"))?;
            Ok(Fault {
                kind: k.trim().chars().next().unwrap_or('?'),
                at_s: t.parse().map_err(|_| f.to_string())?,
                ms: ms.parse().map_err(|_| f.to_string())?,
            })
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Sample,
    Immediate,
    /// CP1-4 4-3: PERFECT 누름 (perfect.rs)
    Perfect,
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mode = match arg(args, "--mode", "sample").as_str() {
        "sample" => Mode::Sample,
        "immediate" => Mode::Immediate,
        "perfect" => Mode::Perfect,
        m => return Err(format!("--mode 는 sample · immediate · perfect: {m}").into()),
    };
    let d_ms: f64 = arg(args, "--d-ms", "0").parse()?;
    if mode == Mode::Perfect && d_ms <= 0.0 {
        return Err("--mode perfect 에는 --d-ms (기록 시계 D) 가 필요".into());
    }
    let fps: f64 = arg(args, "--fps", "60").parse()?;
    let secs: f64 = arg(args, "--secs", "60").parse()?;
    let lookahead_ms: f64 = arg(args, "--lookahead-ms", "100").parse()?;
    let late = match arg(args, "--late", "cut").as_str() {
        "whole" => LatePolicy::PlayWhole,
        _ => LatePolicy::CutFront,
    };
    let sound_kind = arg(args, "--sound", "click");
    let noise_s = match sound_kind.as_str() { "noise" => 0.05, "long" => 1.0, _ => 0.0 };
    let event_period: f64 = arg(args, "--event-period-s", &EVENT_PERIOD_S.to_string()).parse()?;
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    let render_name = arg(args, "--render", "Playback 1/2");
    let wmode = wasapi::Mode::parse(&arg(args, "--wasapi", "shared")).ok_or("--wasapi shared|lowlat|exclusive")?;
    // 오디오 정지 정책 (CP1-4): skip = 콘텐츠를 건너뛰어 시간축을 잇는다(기본), push = 스트림이 밀리는 대로 둔다(CP1 기존)
    let skip = match arg(args, "--audio-stall", "skip").as_str() {
        "skip" => true,
        "push" => false,
        m => return Err(format!("--audio-stall skip|push: {m}").into()),
    };
    let capture_name = arg(args, "--capture", "Loopback 1/2");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = match mode {
        Mode::Sample => format!("sample-la{lookahead_ms:.0}-{}", if late == LatePolicy::CutFront { "cut" } else { "whole" }),
        Mode::Immediate => format!("immediate-{fps:.0}fps"),
        Mode::Perfect => format!("perfect-{}-d{d_ms}", arg(args, "--wasapi", "shared")),
    };
    let out: PathBuf = Path::new(&arg(args, "--out", "runs")).join(format!("sched-{tag}-{unix}"));
    fs::create_dir_all(&out)?;

    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let cap = {
        let (stop, started, out, name) = (stop.clone(), started.clone(), out.clone(), capture_name.clone());
        thread::spawn(move || capture_loop(&name, &out, stop, started, true))
    };
    while !started.load(Ordering::Acquire) && !cap.is_finished() {
        thread::sleep(Duration::from_millis(5));
    }

    // 시계 스냅샷: 렌더 스레드가 띄운 시계 스레드가 쓰고, 게임 스레드가 잠금 없이 읽는다.
    let clock = Arc::new(SharedClock::new());
    let (tx, rx) = channel::<Cmd>();
    let render_done = Arc::new(AtomicBool::new(false));
    let ren = {
        let (out, name, clock, done) = (out.clone(), render_name.clone(), clock.clone(), render_done.clone());
        let fa: Vec<Fault> = faults.iter().copied().filter(|f| f.kind == 'A').collect();
        let pilots = mode != Mode::Perfect;
        thread::spawn(move || render_loop(&name, wmode, skip, pilots, &out, secs, late, noise_s, fa, rx, clock, done))
    };
    let fb: Vec<Fault> = faults.iter().copied().filter(|f| f.kind == 'B').collect();
    let probe_hz: f64 = arg(args, "--probe-hz", "0").parse()?;
    let probe = (probe_hz > 0.0).then(|| spawn_req_probe(clock.clone(), tx.clone(), probe_hz, render_done.clone()));
    let game = if mode == Mode::Perfect {
        let fbc: Vec<Fault> = faults.iter().copied().filter(|f| f.kind == 'B' || f.kind == 'C').collect();
        crate::perfect::perfect_loop(&out, secs, event_period, d_ms, fbc, tx, clock, render_done.clone())
    } else {
        game_loop(&out, mode, fps, secs, lookahead_ms, event_period, fb, tx, clock, render_done.clone())
    };
    if let Some(p) = probe {
        println!("  요청 탐침: {}개 보냄", p.join().map_err(|_| "탐침 스레드 패닉")?);
    }

    let rmeta = ren.join().map_err(|_| "렌더 스레드 패닉")??;
    game?;
    thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Release);
    let cmeta = cap.join().map_err(|_| "녹음 스레드 패닉")??;

    let m = |x: &Meta| {
        format!(
            "{{\"name\": \"{}\", \"rate\": {}, \"channels\": {}, \"buffer_frames\": {}, \"default_period_100ns\": {}, \"clock_freq\": {}, \"mmcss\": {}}}",
            x.name.replace('"', "'"), x.rate, x.channels, x.buffer_frames, x.default_period_100ns, x.clock_freq, x.mmcss
        )
    };
    let mut f = File::create(out.join("meta.json"))?;
    writeln!(
        f,
        "{{\"path\": \"{}\", \"mode\": \"{}\", \"fps\": {fps}, \"lookahead_ms\": {lookahead_ms}, \"late\": \"{}\", \"sound\": \"{}\", \"faults\": \"{}\", \"secs\": {secs}, \"d_ms\": {d_ms}, \"qpc_freq\": {}, \
         \"event\": {{\"start_s\": {EVENT_START_S}, \"period_s\": {event_period}}}, \"pilot\": {{\"start_s\": {PILOT_START_S}, \"period_s\": {PILOT_PERIOD_S}}}, \
         \"render\": {}, \"capture\": {}}}",
        wmode.name(),
        match mode { Mode::Sample => "sample", Mode::Immediate => "immediate", Mode::Perfect => "perfect" },
        if late == LatePolicy::CutFront { "cut" } else { "whole" },
        sound_kind,
        arg(args, "--faults", ""),
        qpc::frequency(), m(&rmeta), m(&cmeta)
    )?;
    println!("기록: {}", out.display());
    Ok(())
}

fn events(rate: f64, secs: f64, period: f64) -> Vec<u64> {
    let mut v = Vec::new();
    let mut k = 0;
    loop {
        let t = EVENT_START_S + k as f64 * period;
        if t > secs - 0.5 {
            break;
        }
        v.push((t * rate).round() as u64);
        k += 1;
    }
    v
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn game_loop(
    out: &Path, mode: Mode, fps: f64, secs: f64, lookahead_ms: f64, event_period: f64, faults: Vec<Fault>, tx: Sender<Cmd>,
    clock: Arc<SharedClock>, render_done: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe { timeBeginPeriod(1) };
    // 시계가 생길 때까지 기다린다 (첫 관측이 반영된 스냅샷).
    let rate = loop {
        let s = clock.load();
        if s.valid {
            break s.nominal_rate;
        }
        if render_done.load(Ordering::Acquire) {
            unsafe { timeEndPeriod(1) };
            return Err("렌더가 시계를 만들기 전에 끝남".into());
        }
        thread::sleep(Duration::from_millis(1));
    };
    let ev = events(rate, secs, event_period);
    let lookahead = lookahead_ms * 1e-3 * rate;
    let qf = qpc::frequency() as f64;
    let mut log = BufWriter::new(File::create(out.join("game_sends.csv"))?);
    writeln!(log, "k,event_frame,send_qpc,now_frame")?;
    let period = Duration::from_secs_f64(1.0 / fps);
    let mut next = Instant::now();
    let mut k = 0usize;
    let mut fired = vec![false; faults.len()];
    let mut flog = BufWriter::new(File::create(out.join("faults_game.csv"))?);
    writeln!(flog, "kind,at_s,ms,qpc_before,qpc_after,frame_before")?;
    while k < ev.len() && !render_done.load(Ordering::Acquire) {
        let q = qpc::now();
        let now_frame = clock.load().frame_at((q as f64 * 1e7 / qf) as i64);
        for (i, f) in faults.iter().enumerate() {
            if !fired[i] && now_frame >= f.at_s * rate {
                fired[i] = true;
                thread::sleep(Duration::from_millis(f.ms)); // 고장 B: 게임 스레드 멈춤
                writeln!(flog, "B,{},{},{q},{},{now_frame:.1}", f.at_s, f.ms, qpc::now())?;
            }
        }
        let q = qpc::now();
        let now_frame = clock.load().frame_at((q as f64 * 1e7 / qf) as i64);
        while k < ev.len() {
            let due = match mode {
                Mode::Sample | Mode::Perfect => ev[k] as f64 <= now_frame + lookahead,
                Mode::Immediate => ev[k] as f64 <= now_frame,
            };
            if !due {
                break;
            }
            let cmd = match mode {
                Mode::Sample | Mode::Perfect => Cmd::At { frame: ev[k], sound: 0, channel: 0 },
                Mode::Immediate => Cmd::Now { sound: 0, channel: 0 },
            };
            tx.send(cmd)?;
            writeln!(log, "{k},{},{q},{now_frame:.1}", ev[k])?;
            k += 1;
        }
        // 게임 프레임 주기 (VSync 대신 잠들기). 밀리면 다음 프레임부터 다시 맞춘다.
        next += period;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    unsafe { timeEndPeriod(1) };
    log.flush()?;
    flog.flush()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_loop(
    name: &str, wmode: wasapi::Mode, skip: bool, pilots_on: bool, out: &Path, secs: f64, late: LatePolicy, noise_s: f64, faults: Vec<Fault>, rx: Receiver<Cmd>,
    clock: Arc<SharedClock>, done: Arc<AtomicBool>,
) -> Result<Meta, String> {
    let r = (|| -> Result<Meta, Box<dyn std::error::Error>> {
        wasapi::com_init()?;
        let mmcss = wasapi::mmcss_pro_audio();
        let ep = wasapi::find(false, name)?;
        let excl_period: Option<i64> = std::env::var("CP1_EXCL_PERIOD_MS").ok().and_then(|v| v.parse::<f64>().ok()).map(|ms| (ms * 1e4) as i64);
        let s = Stream::open_mode_period(&ep.device, wmode, excl_period)?;
        if s.channels < 2 {
            return Err(format!("스테레오가 아님: {} ch", s.channels).into());
        }
        println!("  출력: {} {:?} {} bit, 버퍼 {} 프레임, 주기 {} 프레임", wmode.name(), s.sample, s.bits, s.buffer_frames, s.period_frames);
        let rc: IAudioRenderClient = s.service()?;
        let ac: IAudioClock = s.service()?;
        let clock_freq = unsafe { ac.GetFrequency()? };
        let rate = s.rate as u64;
        let ch = s.channels as usize;
        // 관측은 링에 넣기만 하고, 다시 맞추는 계산은 시계 스레드가 한다 (오디오 스레드에서 잠금·할당 없음).
        let (mut obs, obs_rx) = obs_ring(4096);
        let clock_thread = spawn_clock_thread(s.rate as f64, ClockPolicy::default(), obs_rx, clock.clone());
        let total = (secs * rate as f64).round() as u64;
        // 기준 클릭은 오른쪽 채널: PERFECT 누름 시험에서는 오른쪽을 키음이 쓰므로 끈다
        let pilot_start = if pilots_on { (PILOT_START_S * rate as f64).round() as u64 } else { u64::MAX / 2 };
        let pilot_period = (PILOT_PERIOD_S * rate as f64).round() as u64;
        let sound0 = if noise_s > 0.0 { noise_burst(s.rate as f64, noise_s) } else { CLICK.to_vec() };
        {
            let mut f = BufWriter::new(File::create(out.join("sound0.f32"))?);
            for v in &sound0 {
                f.write_all(&v.to_le_bytes())?;
            }
        }
        let mut mixer = Mixer::new(vec![sound0, Vec::new(), CLICK.to_vec()], late); // 1번 = 요청 탐침용 무음, 2번 = 키음 (CP1-4 4-3)
        let mut fired = vec![false; faults.len()];
        let mut flog = BufWriter::new(File::create(out.join("faults_render.csv"))?);
        writeln!(flog, "kind,at_s,ms,qpc_before,qpc_after,written")?;
        let mut scratch = vec![0.0f32; s.buffer_frames as usize * ch];

        let mut pilots = BufWriter::new(File::create(out.join("pilots.csv"))?);
        writeln!(pilots, "frame")?;
        let mut k = pilot_start;
        while k < total {
            writeln!(pilots, "{k}")?;
            k += pilot_period;
        }
        pilots.flush()?;

        let mut written: u64 = 0;
        // 시간축 = 스트림 + offset. 오디오가 멈춰 장치가 뒤처지면 offset을 늘려 콘텐츠를 건너뛴다 (2 ms 넘게 뒤처졌을 때만: 위치 보고의 1 ms 톱니보다 크게)
        let mut cskip = ContentSkip::new(0.002 * rate as f64);
        let mut fill = |n: u32, written: &mut u64, off: i64, mixer: &mut Mixer| -> Result<(), Box<dyn std::error::Error>> {
            let buf = &mut scratch[..n as usize * ch];
            buf.fill(0.0);
            let tl = (*written as i64 + off) as u64; // 이 블록의 시간축 프레임
            mixer.fill(buf, ch, tl, n as usize, &rx);
            for i in 0..n as u64 {
                let f = tl + i;
                if f >= pilot_start && (f - pilot_start) % pilot_period < CLICK.len() as u64 && f < total {
                    buf[i as usize * ch + 1] += CLICK[0];
                }
            }
            s.write(&rc, buf, n)?;
            *written += n as u64;
            Ok(())
        };
        fill(s.buffer_frames, &mut written, 0, &mut mixer)?;
        unsafe { s.client.Start()? };
        let mut rlog = BufWriter::new(File::create(out.join("render_packets.csv"))?);
        writeln!(rlog, "qpc,written_before,padding,frames,clock_pos,clock_qpc100ns")?;
        while written < total + rate / 2 {
            unsafe { WaitForSingleObject(s.event, 2000) };
            for (i, f) in faults.iter().enumerate() {
                if !fired[i] && written as f64 >= f.at_s * rate as f64 {
                    fired[i] = true;
                    let q0 = qpc::now();
                    thread::sleep(Duration::from_millis(f.ms)); // 고장 A: 버퍼를 채우지 않고 잠듦 → 언더런
                    writeln!(flog, "A,{},{},{q0},{},{written}", f.at_s, f.ms, qpc::now())?;
                }
            }
            let now = qpc::now();
            // 먼저 장치 위치를 보고 (뒤처졌으면) 건너뛴 다음에 채운다: 이번 블록부터 제 시간축 위치의 콘텐츠를 쓴다
            let (mut pos, mut qpos) = (0u64, 0u64);
            unsafe { ac.GetPosition(&mut pos, Some(&mut qpos))? };
            let raw = (pos as f64 * rate as f64 / clock_freq as f64).round() as u64;
            let frame = if skip { cskip.observe(raw, qpos as i64, &clock.load()) } else { raw };
            if raw > rate / 20 {
                obs.push(frame, qpos as i64);
            }
            let pad = if s.mode == wasapi::Mode::Exclusive { 0 } else { unsafe { s.client.GetCurrentPadding()? } };
            let n = s.frames_to_write()?;
            let before = written;
            if n > 0 {
                fill(n, &mut written, cskip.offset, &mut mixer)?;
            }
            writeln!(rlog, "{now},{before},{pad},{n},{pos},{qpos}")?;
        }
        done.store(true, Ordering::Release);
        drop(obs);
        let cs = clock_thread.join().map_err(|_| "시계 스레드 패닉")?;
        if cs.dropped > 0 {
            println!("  경고: 관측 링이 가득 차서 관측 {}개를 버림", cs.dropped);
        }
        rlog.flush()?;
        flog.flush()?;
        let mut klog = BufWriter::new(File::create(out.join("skips.csv"))?);
        writeln!(klog, "qpc100ns,frames")?;
        for (t, d) in &cskip.skips {
            writeln!(klog, "{t},{d}")?;
        }
        klog.flush()?;
        let mut slog = BufWriter::new(File::create(out.join("starts.csv"))?);
        writeln!(slog, "sound,requested,start,block")?;
        for l in &mixer.log {
            writeln!(slog, "{},{},{},{}", l.sound, l.requested.map(|r| r as i64).unwrap_or(-1), l.start, l.block)?;
        }
        slog.flush()?;
        Ok(Meta { name: ep.name, rate: s.rate, channels: s.channels, bits: s.bits, buffer_frames: s.buffer_frames,
                  default_period_100ns: s.default_period_100ns, clock_freq, mmcss })
    })();
    done.store(true, Ordering::Release);
    r.map_err(|e| format!("렌더: {e}"))
}
