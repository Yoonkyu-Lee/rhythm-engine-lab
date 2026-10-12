//! CP1 테스트벤치.
//!
//!   testbench list
//!   testbench step0 [--secs 20] [--render "Playback 1/2"] [--capture "Loopback 1/2"] [--out runs]
//!
//! step0: 렌더 장치에 정해진 프레임 위치마다 짧은 클릭을 쓰고, 루프백 장치를 동시에 녹음한다.
//! 분석은 cp1/tools/analyze_step0.py 가 한다. 이 프로그램은 원시 기록만 남긴다.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use synchronizer::qpc;
use synchronizer::wasapi::{self, IAudioCaptureClient, IAudioClock, IAudioRenderClient, Stream};
use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
use windows::Win32::System::Threading::WaitForSingleObject;

/// 클릭: 시작 1초 뒤부터 0.5초마다, 3샘플 길이의 사각 펄스.
const CLICK_START_S: f64 = 1.0;
const CLICK_PERIOD_S: f64 = 0.5;
const CLICK_LEN: u64 = 3;
const CLICK_AMP: f32 = 0.25;

fn arg(args: &[String], key: &str, default: &str) -> String {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
    let r = match cmd {
        "list" => list(),
        "step0" => step0(&args),
        _ => {
            eprintln!("사용법: testbench list | testbench step0 [--secs 20] [--render 이름] [--capture 이름] [--out runs]");
            Ok(())
        }
    };
    if let Err(e) = r {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}

fn list() -> Result<(), Box<dyn std::error::Error>> {
    wasapi::com_init()?;
    for (cap, label) in [(false, "재생"), (true, "녹음")] {
        println!("[{label}]");
        for e in wasapi::endpoints(cap)? {
            let s = Stream::open(&e.device)?;
            println!(
                "  {}  ({} Hz, {} ch, {} bit, 버퍼 {} 프레임, 기본 주기 {:.2} ms, 최소 {:.2} ms)",
                e.name, s.rate, s.channels, s.bits, s.buffer_frames,
                s.default_period_100ns as f64 / 1e4, s.min_period_100ns as f64 / 1e4
            );
        }
    }
    Ok(())
}

struct Meta {
    name: String,
    rate: u32,
    channels: u16,
    bits: u16,
    buffer_frames: u32,
    default_period_100ns: i64,
    clock_freq: u64,
    mmcss: bool,
}

fn step0(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let secs: f64 = arg(args, "--secs", "20").parse()?;
    let render_name = arg(args, "--render", "Playback 1/2");
    let capture_name = arg(args, "--capture", "Loopback 1/2");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let out: PathBuf = Path::new(&arg(args, "--out", "runs")).join(format!("step0-{unix}"));
    fs::create_dir_all(&out)?;

    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));

    // 녹음을 먼저 시작해 첫 클릭을 놓치지 않는다.
    let cap = {
        let (stop, started, out, name) = (stop.clone(), started.clone(), out.clone(), capture_name.clone());
        thread::spawn(move || capture_loop(&name, &out, stop, started))
    };
    while !started.load(Ordering::Acquire) {
        if cap.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let ren = {
        let (stop, out, name) = (stop.clone(), out.clone(), render_name.clone());
        thread::spawn(move || render_loop(&name, &out, secs, stop))
    };

    let rmeta = ren.join().map_err(|_| "렌더 스레드 패닉")??;
    thread::sleep(Duration::from_millis(500)); // 마지막 클릭이 루프백으로 돌아올 시간
    stop.store(true, Ordering::Release);
    let cmeta = cap.join().map_err(|_| "녹음 스레드 패닉")??;

    let mut f = File::create(out.join("meta.json"))?;
    let m = |x: &Meta| {
        format!(
            "{{\"name\": \"{}\", \"rate\": {}, \"channels\": {}, \"bits\": {}, \"buffer_frames\": {}, \"default_period_100ns\": {}, \"clock_freq\": {}, \"mmcss\": {}}}",
            x.name.replace('"', "'"), x.rate, x.channels, x.bits, x.buffer_frames, x.default_period_100ns, x.clock_freq, x.mmcss
        )
    };
    writeln!(
        f,
        "{{\"qpc_freq\": {}, \"secs\": {}, \"click\": {{\"start_s\": {}, \"period_s\": {}, \"len\": {}, \"amp\": {}}}, \"render\": {}, \"capture\": {}}}",
        qpc::frequency(), secs, CLICK_START_S, CLICK_PERIOD_S, CLICK_LEN, CLICK_AMP, m(&rmeta), m(&cmeta)
    )?;
    println!("기록: {}", out.display());
    Ok(())
}

fn render_loop(name: &str, out: &Path, secs: f64, stop: Arc<AtomicBool>) -> Result<Meta, String> {
    let r = (|| -> Result<Meta, Box<dyn std::error::Error>> {
        wasapi::com_init()?;
        let mmcss = wasapi::mmcss_pro_audio();
        let ep = wasapi::find(false, name)?;
        let s = Stream::open(&ep.device)?;
        if s.bits != 32 {
            return Err(format!("믹스 형식이 32비트 float가 아님: {} bit", s.bits).into());
        }
        let rc: IAudioRenderClient = s.service()?;
        let clock: IAudioClock = s.service()?;
        let clock_freq = unsafe { clock.GetFrequency()? };
        let rate = s.rate as u64;
        let ch = s.channels as usize;
        let start = (CLICK_START_S * rate as f64).round() as u64;
        let period = (CLICK_PERIOD_S * rate as f64).round() as u64;
        let total = (secs * rate as f64).round() as u64;
        let sample = |f: u64| -> f32 { if f >= start && (f - start) % period < CLICK_LEN && f < total { CLICK_AMP } else { 0.0 } };

        let mut log = BufWriter::new(File::create(out.join("render_packets.csv"))?);
        writeln!(log, "qpc,written_before,padding,frames,clock_pos,clock_qpc100ns")?;
        let mut clicks = BufWriter::new(File::create(out.join("clicks.csv"))?);
        writeln!(clicks, "frame")?;
        let mut k = start;
        while k < total {
            writeln!(clicks, "{k}")?;
            k += period;
        }

        let mut written: u64 = 0;
        let fill = |n: u32, written: &mut u64| -> Result<(), Box<dyn std::error::Error>> {
            unsafe {
                let p = rc.GetBuffer(n)? as *mut f32;
                for i in 0..n as usize {
                    let v = sample(*written + i as u64);
                    for c in 0..ch {
                        *p.add(i * ch + c) = v;
                    }
                }
                rc.ReleaseBuffer(n, 0)?;
            }
            *written += n as u64;
            Ok(())
        };
        // 시작 전에 버퍼 전체를 채운다 (처음 1초는 무음).
        fill(s.buffer_frames, &mut written)?;
        unsafe { s.client.Start()? };
        while written < total + rate / 2 && !stop.load(Ordering::Acquire) {
            unsafe { WaitForSingleObject(s.event, 2000) };
            let now = qpc::now();
            let pad = unsafe { s.client.GetCurrentPadding()? };
            let n = s.buffer_frames - pad;
            let before = written;
            if n > 0 {
                fill(n, &mut written)?;
            }
            let (mut pos, mut qpos) = (0u64, 0u64);
            unsafe { clock.GetPosition(&mut pos, Some(&mut qpos))? };
            writeln!(log, "{now},{before},{pad},{n},{pos},{qpos}")?;
        }
        log.flush()?;
        clicks.flush()?;
        Ok(Meta { name: ep.name, rate: s.rate, channels: s.channels, bits: s.bits, buffer_frames: s.buffer_frames,
                  default_period_100ns: s.default_period_100ns, clock_freq, mmcss })
    })();
    r.map_err(|e| format!("렌더: {e}"))
}

fn capture_loop(name: &str, out: &Path, stop: Arc<AtomicBool>, started: Arc<AtomicBool>) -> Result<Meta, String> {
    let r = (|| -> Result<Meta, Box<dyn std::error::Error>> {
        wasapi::com_init()?;
        let mmcss = wasapi::mmcss_pro_audio();
        let ep = wasapi::find(true, name)?;
        let s = Stream::open(&ep.device)?;
        if s.bits != 32 {
            return Err(format!("믹스 형식이 32비트 float가 아님: {} bit", s.bits).into());
        }
        let cc: IAudioCaptureClient = s.service()?;
        let clock: IAudioClock = s.service()?;
        let clock_freq = unsafe { clock.GetFrequency()? };
        let ch = s.channels as usize;
        let mut log = BufWriter::new(File::create(out.join("capture_packets.csv"))?);
        writeln!(log, "qpc,device_pos,qpc100ns,frames,flags")?;
        let mut wav = BufWriter::new(File::create(out.join("capture_ch0.f32"))?);
        unsafe { s.client.Start()? };
        started.store(true, Ordering::Release);
        while !stop.load(Ordering::Acquire) {
            unsafe { WaitForSingleObject(s.event, 2000) };
            loop {
                let n = unsafe { cc.GetNextPacketSize()? };
                if n == 0 {
                    break;
                }
                let now = qpc::now();
                let (mut data, mut frames, mut flags, mut dpos, mut qpos) = (std::ptr::null_mut(), 0u32, 0u32, 0u64, 0u64);
                unsafe { cc.GetBuffer(&mut data, &mut frames, &mut flags, Some(&mut dpos), Some(&mut qpos))? };
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let p = data as *const f32;
                for i in 0..frames as usize {
                    let v = if silent { 0.0 } else { unsafe { *p.add(i * ch) } };
                    wav.write_all(&v.to_le_bytes())?;
                }
                unsafe { cc.ReleaseBuffer(frames)? };
                writeln!(log, "{now},{dpos},{qpos},{frames},{flags}")?;
            }
        }
        log.flush()?;
        wav.flush()?;
        Ok(Meta { name: ep.name, rate: s.rate, channels: s.channels, bits: s.bits, buffer_frames: s.buffer_frames,
                  default_period_100ns: s.default_period_100ns, clock_freq, mmcss })
    })();
    if r.is_err() {
        started.store(true, Ordering::Release);
    }
    r.map_err(|e| format!("녹음: {e}"))
}
