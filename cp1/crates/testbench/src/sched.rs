//! 2번 단계: 예약기 정책 비교.
//!
//!   testbench sched --mode sample|immediate [--fps 60] [--secs 60] [--lookahead-ms 100] [--late cut|whole]
//!
//! 왼쪽 채널: 시험 이벤트. 게임 스레드가 정책에 따라 예약기로 보낸다.
//!   sample    : 이벤트 시각보다 lookahead 만큼 앞서 "프레임 N에서 시작"(At)을 보낸다.
//!   immediate : fps 주기로 도는 게임 루프가 시계를 보고, 이벤트 시각이 지났으면 "지금 울려"(Now)를 보낸다.
//! 오른쪽 채널: 기준 클릭. 렌더 스레드가 정해진 프레임에 직접 쓴다. 루프백 위치를 재생 프레임 번호로 바꾸는 기준.
//! 분석: cp1/tools/analyze_sched.py

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use synchronizer::clock::{ClockModel, ClockPolicy};
use synchronizer::qpc;
use synchronizer::scheduler::{Cmd, LatePolicy, Mixer};
use synchronizer::wasapi::{self, IAudioClock, IAudioRenderClient, Stream};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::{arg, capture_loop, Meta};

const EVENT_START_S: f64 = 1.0;
const EVENT_PERIOD_S: f64 = 0.5007; // 게임 프레임 주기와 맞물리지 않게
const PILOT_START_S: f64 = 1.25;
const PILOT_PERIOD_S: f64 = 0.5;
const CLICK: [f32; 3] = [0.25; 3];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Sample,
    Immediate,
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mode = match arg(args, "--mode", "sample").as_str() {
        "sample" => Mode::Sample,
        "immediate" => Mode::Immediate,
        m => return Err(format!("--mode 는 sample 또는 immediate: {m}").into()),
    };
    let fps: f64 = arg(args, "--fps", "60").parse()?;
    let secs: f64 = arg(args, "--secs", "60").parse()?;
    let lookahead_ms: f64 = arg(args, "--lookahead-ms", "100").parse()?;
    let late = match arg(args, "--late", "cut").as_str() {
        "whole" => LatePolicy::PlayWhole,
        _ => LatePolicy::CutFront,
    };
    let render_name = arg(args, "--render", "Playback 1/2");
    let capture_name = arg(args, "--capture", "Loopback 1/2");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = match mode {
        Mode::Sample => format!("sample-la{lookahead_ms:.0}"),
        Mode::Immediate => format!("immediate-{fps:.0}fps"),
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

    let clock: Arc<Mutex<Option<ClockModel>>> = Arc::new(Mutex::new(None));
    let rate_cell: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let (tx, rx) = channel::<Cmd>();
    let render_done = Arc::new(AtomicBool::new(false));
    let ren = {
        let (out, name, clock, rate_cell, done) = (out.clone(), render_name.clone(), clock.clone(), rate_cell.clone(), render_done.clone());
        thread::spawn(move || render_loop(&name, &out, secs, late, rx, clock, rate_cell, done))
    };
    let game = game_loop(&out, mode, fps, secs, lookahead_ms, tx, clock, rate_cell, render_done.clone());

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
        "{{\"mode\": \"{}\", \"fps\": {fps}, \"lookahead_ms\": {lookahead_ms}, \"late\": \"{}\", \"secs\": {secs}, \"qpc_freq\": {}, \
         \"event\": {{\"start_s\": {EVENT_START_S}, \"period_s\": {EVENT_PERIOD_S}}}, \"pilot\": {{\"start_s\": {PILOT_START_S}, \"period_s\": {PILOT_PERIOD_S}}}, \
         \"render\": {}, \"capture\": {}}}",
        if mode == Mode::Sample { "sample" } else { "immediate" },
        if late == LatePolicy::CutFront { "cut" } else { "whole" },
        qpc::frequency(), m(&rmeta), m(&cmeta)
    )?;
    println!("기록: {}", out.display());
    Ok(())
}

fn events(rate: f64, secs: f64) -> Vec<u64> {
    let mut v = Vec::new();
    let mut k = 0;
    loop {
        let t = EVENT_START_S + k as f64 * EVENT_PERIOD_S;
        if t > secs - 0.5 {
            break;
        }
        v.push((t * rate).round() as u64);
        k += 1;
    }
    v
}

#[allow(clippy::too_many_arguments)]
fn game_loop(
    out: &Path, mode: Mode, fps: f64, secs: f64, lookahead_ms: f64, tx: Sender<Cmd>,
    clock: Arc<Mutex<Option<ClockModel>>>, rate_cell: Arc<Mutex<u32>>, render_done: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe { timeBeginPeriod(1) };
    // 시계가 생길 때까지 기다린다.
    let rate = loop {
        let r = *rate_cell.lock().unwrap();
        if r > 0 && clock.lock().unwrap().as_ref().is_some_and(|m| m.is_valid()) {
            break r as f64;
        }
        if render_done.load(Ordering::Acquire) {
            unsafe { timeEndPeriod(1) };
            return Err("렌더가 시계를 만들기 전에 끝남".into());
        }
        thread::sleep(Duration::from_millis(1));
    };
    let ev = events(rate, secs);
    let lookahead = lookahead_ms * 1e-3 * rate;
    let qf = qpc::frequency() as f64;
    let mut log = BufWriter::new(File::create(out.join("game_sends.csv"))?);
    writeln!(log, "k,event_frame,send_qpc,now_frame")?;
    let period = Duration::from_secs_f64(1.0 / fps);
    let mut next = Instant::now();
    let mut k = 0usize;
    while k < ev.len() && !render_done.load(Ordering::Acquire) {
        let q = qpc::now();
        let now_frame = clock.lock().unwrap().as_ref().map(|m| m.frame_at((q as f64 * 1e7 / qf) as i64)).unwrap_or(0.0);
        while k < ev.len() {
            let due = match mode {
                Mode::Sample => ev[k] as f64 <= now_frame + lookahead,
                Mode::Immediate => ev[k] as f64 <= now_frame,
            };
            if !due {
                break;
            }
            let cmd = match mode {
                Mode::Sample => Cmd::At { frame: ev[k], sound: 0, channel: 0 },
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_loop(
    name: &str, out: &Path, secs: f64, late: LatePolicy, rx: Receiver<Cmd>,
    clock: Arc<Mutex<Option<ClockModel>>>, rate_cell: Arc<Mutex<u32>>, done: Arc<AtomicBool>,
) -> Result<Meta, String> {
    let r = (|| -> Result<Meta, Box<dyn std::error::Error>> {
        wasapi::com_init()?;
        let mmcss = wasapi::mmcss_pro_audio();
        let ep = wasapi::find(false, name)?;
        let s = Stream::open(&ep.device)?;
        if s.bits != 32 || s.channels < 2 {
            return Err(format!("믹스 형식이 32비트 float 스테레오가 아님: {} bit, {} ch", s.bits, s.channels).into());
        }
        let rc: IAudioRenderClient = s.service()?;
        let ac: IAudioClock = s.service()?;
        let clock_freq = unsafe { ac.GetFrequency()? };
        let rate = s.rate as u64;
        let ch = s.channels as usize;
        *clock.lock().unwrap() = Some(ClockModel::new(s.rate as f64, ClockPolicy::default()));
        *rate_cell.lock().unwrap() = s.rate;
        let total = (secs * rate as f64).round() as u64;
        let pilot_start = (PILOT_START_S * rate as f64).round() as u64;
        let pilot_period = (PILOT_PERIOD_S * rate as f64).round() as u64;
        let mut mixer = Mixer::new(vec![CLICK.to_vec()], late);
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
        let mut fill = |n: u32, written: &mut u64, mixer: &mut Mixer| -> Result<(), Box<dyn std::error::Error>> {
            let buf = &mut scratch[..n as usize * ch];
            buf.fill(0.0);
            mixer.fill(buf, ch, *written, n as usize, &rx);
            for i in 0..n as u64 {
                let f = *written + i;
                if f >= pilot_start && (f - pilot_start) % pilot_period < CLICK.len() as u64 && f < total {
                    buf[i as usize * ch + 1] += CLICK[0];
                }
            }
            unsafe {
                let p = rc.GetBuffer(n)? as *mut f32;
                std::ptr::copy_nonoverlapping(buf.as_ptr(), p, buf.len());
                rc.ReleaseBuffer(n, 0)?;
            }
            *written += n as u64;
            Ok(())
        };
        fill(s.buffer_frames, &mut written, &mut mixer)?;
        unsafe { s.client.Start()? };
        let mut rlog = BufWriter::new(File::create(out.join("render_packets.csv"))?);
        writeln!(rlog, "qpc,written_before,padding,frames,clock_pos,clock_qpc100ns")?;
        while written < total + rate / 2 {
            unsafe { WaitForSingleObject(s.event, 2000) };
            let now = qpc::now();
            let pad = unsafe { s.client.GetCurrentPadding()? };
            let n = s.buffer_frames - pad;
            let before = written;
            if n > 0 {
                fill(n, &mut written, &mut mixer)?;
            }
            let (mut pos, mut qpos) = (0u64, 0u64);
            unsafe { ac.GetPosition(&mut pos, Some(&mut qpos))? };
            let frame = (pos as f64 * rate as f64 / clock_freq as f64).round() as u64;
            if frame > rate / 20 {
                // 시험용: 오디오 스레드에서 잠금을 잡는다. 제품 경로에서는 잠금 없는 스냅샷으로 바꿀 것.
                if let Some(m) = clock.lock().unwrap().as_mut() {
                    m.observe(frame, qpos as i64);
                }
            }
            writeln!(rlog, "{now},{before},{pad},{n},{pos},{qpos}")?;
        }
        done.store(true, Ordering::Release);
        rlog.flush()?;
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
