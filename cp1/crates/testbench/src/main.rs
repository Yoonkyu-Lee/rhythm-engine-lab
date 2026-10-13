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

mod asioprobe;
mod asiorun;
mod clockjump;
mod input;
mod sched;

use synchronizer::qpc;
use synchronizer::wasapi::{self, IAudioCaptureClient, IAudioClock, IAudioRenderClient, Stream};
use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
use windows::Win32::System::Threading::WaitForSingleObject;

/// 클릭: 시작 1초 뒤부터 0.5초마다, 3샘플 길이의 사각 펄스.
const CLICK_START_S: f64 = 1.0;
const CLICK_PERIOD_S: f64 = 0.5;
const CLICK_LEN: u64 = 3;
const CLICK_AMP: f32 = 0.25;

pub(crate) fn arg(args: &[String], key: &str, default: &str) -> String {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
    let r = match cmd {
        "list" => list(),
        "step0" => step0(&args),
        "clockeval" => clockeval(&args),
        "sched" => sched::run(&args),
        "input" => input::run(&args),
        "clockjump" => clockjump::run(&args),
        "asioprobe" => asioprobe::run(&args),
        "asiorun" => asiorun::run(&args),
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

/// 기록된 렌더 시계 보고값을 시간 순서대로 시계 모델에 다시 넣어 정책별 예측 오차를 잰다.
/// 기준(진실의 최선 추정): 기록 전체로 맞춘 직선. 각 관측 직전에 모델이 그 위치의 시각을 예측하고, 직선과 비교한다.
fn clockeval(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use synchronizer::clock::{ClockModel, ClockPolicy};
    let run = PathBuf::from(args.get(2).ok_or("사용법: testbench clockeval <runs\\step0-...>")?);
    let meta = fs::read_to_string(run.join("meta.json"))?;
    let num = |key: &str, from: &str| -> f64 {
        let s = &meta[meta.find(from).unwrap_or(0)..];
        let i = s.find(&format!("\"{key}\":")).expect(key) + key.len() + 3;
        s[i..].trim_start().split(|c: char| c == ',' || c == '}').next().unwrap().trim().parse().unwrap()
    };
    let rate = num("rate", "\"render\"");
    let clock_freq = num("clock_freq", "\"render\"");
    let mut obs: Vec<(u64, i64)> = Vec::new();
    for line in fs::read_to_string(run.join("render_packets.csv"))?.lines().skip(1) {
        let v: Vec<&str> = line.split(',').collect();
        let (pos, q): (u64, i64) = (v[4].parse()?, v[5].parse()?);
        let frame = (pos as f64 * rate / clock_freq).round() as u64;
        if frame as f64 > rate * 0.05 {
            obs.push((frame, q));
        }
    }
    // 기록 전체 직선: t = c + d·frame
    let n = obs.len() as f64;
    let (mf, mt) = obs.iter().fold((0.0, 0.0), |(a, b), &(f, t)| (a + f as f64, b + t as f64));
    let (mf, mt) = (mf / n, mt / n);
    let (sft, sff) = obs.iter().fold((0.0, 0.0), |(a, b), &(f, t)| {
        (a + (f as f64 - mf) * (t as f64 - mt), b + (f as f64 - mf).powi(2))
    });
    let d = sft / sff;
    let line = |f: f64| mt + d * (f - mf);
    // 정답 두 가지:
    // ① 물리 직선 (주 정답): 기록 전체의 직선. 보고값은 실제 시계를 중심으로 1 ms 톱니를 그리므로 (USB 프레임 단위 보정),
    //    여러 톱니 주기를 덮는 직선이 실제 소리의 시계에 가장 가깝다 (추정, 근거 강함).
    // ② 국소 보고값 (참고): 앞뒤 ±0.5초 보고값의 평균. 톱니까지 따라간 값.
    let resid: Vec<f64> = obs.iter().map(|&(f, t)| t as f64 - line(f as f64)).collect();
    let half = 50usize;
    let mut prefix = vec![0.0; resid.len() + 1];
    for (i, r) in resid.iter().enumerate() {
        prefix[i + 1] = prefix[i] + r;
    }
    let local: Vec<f64> = (0..obs.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(half), (i + half + 1).min(obs.len()));
            line(obs[i].0 as f64) + (prefix[hi] - prefix[lo]) / (hi - lo) as f64
        })
        .collect();
    let physical: Vec<f64> = obs.iter().map(|&(f, _)| line(f as f64)).collect();
    let span_s = (obs.last().unwrap().1 - obs[0].1) as f64 * 1e-7;
    println!("{}: 관측 {}개, {:.0}초, 전체 직선 기울기 {:+.2} ppm", run.display(), obs.len(), span_s, ((1e7 / rate) / d - 1.0) * 1e6);

    let policies: Vec<(&str, Option<ClockPolicy>)> = vec![
        ("원시값 (직전 보고 + 명목 비율)", None),
        ("명목 비율 + 1초 평균 절편", Some(ClockPolicy { window_s: 1.0, min_slope_span_s: f64::INFINITY, ..Default::default() })),
        ("명목 비율 + 600초 평균 절편", Some(ClockPolicy { min_slope_span_s: f64::INFINITY, ..Default::default() })),
        ("120초 창, 기울기 30초부터", Some(ClockPolicy { window_s: 120.0, min_slope_span_s: 30.0, ..Default::default() })),
        ("300초 창, 기울기 150초부터", Some(ClockPolicy { window_s: 300.0, ..Default::default() })),
        ("600초 창, 기울기 150초부터 (기본)", Some(ClockPolicy::default())),
    ];
    let buckets = [(0.0, 10.0), (10.0, 150.0), (150.0, 300.0), (300.0, f64::INFINITY)];
    for (title, truth) in [("① 물리 직선", &physical), ("② 국소 보고값 (참고)", &local)] {
        println!("\n예측 오차 |모델 − {title}| (ms): 구간별 p50 / p99 / 최대");
        print!("  {:<34}", "정책");
        for (lo, hi) in buckets {
            print!(" {:>22}", if hi.is_finite() { format!("{lo:.0}–{hi:.0}초") } else { format!("{lo:.0}초 이후") });
        }
        println!();
        for (name, pol) in &policies {
            let mut errs: Vec<Vec<f64>> = vec![Vec::new(); buckets.len()];
            let mut model = pol.map(|p| ClockModel::new(rate, p));
            let mut last: Option<(u64, i64)> = None;
            for (i, &(f, t)) in obs.iter().enumerate() {
                let el = (t - obs[0].1) as f64 * 1e-7;
                let pred = match (&model, last) {
                    (Some(m), _) if m.is_valid() => Some(m.t100_at(f as f64)),
                    (None, Some((lf, lt))) => Some(lt as f64 + (f as f64 - lf as f64) / rate * 1e7),
                    _ => None,
                };
                if let Some(p) = pred {
                    let bi = buckets.iter().position(|&(lo, hi)| el >= lo && el < hi).unwrap();
                    errs[bi].push((p - truth[i]).abs() / 1e4);
                }
                if let Some(m) = model.as_mut() {
                    m.observe(f, t);
                }
                last = Some((f, t));
            }
            print!("  {:<34}", name);
            for e in errs.iter_mut() {
                if e.is_empty() {
                    print!(" {:>22}", "-");
                    continue;
                }
                e.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let q = |p: f64| e[((e.len() - 1) as f64 * p) as usize];
                print!(" {:>22}", format!("{:.3} / {:.3} / {:.3}", q(0.5), q(0.99), e[e.len() - 1]));
            }
            println!();
        }
    }
    Ok(())
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

pub(crate) struct Meta {
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub buffer_frames: u32,
    pub default_period_100ns: i64,
    pub clock_freq: u64,
    pub mmcss: bool,
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
        thread::spawn(move || capture_loop(&name, &out, stop, started, false))
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

pub(crate) fn capture_loop(name: &str, out: &Path, stop: Arc<AtomicBool>, started: Arc<AtomicBool>, stereo: bool) -> Result<Meta, String> {
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
        let mut wav = BufWriter::new(File::create(out.join(if stereo { "capture_2ch.f32" } else { "capture_ch0.f32" }))?);
        let keep = if stereo { ch } else { 1 };
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
                    for c in 0..keep {
                        let v = if silent { 0.0 } else { unsafe { *p.add(i * ch + c) } };
                        wav.write_all(&v.to_le_bytes())?;
                    }
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
