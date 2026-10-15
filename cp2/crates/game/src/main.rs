//! CP2 시험장. 건반 하나짜리 리듬게임과 측정 도구.
//!
//!   cp2game play <곡.rtc> [--secs 0] [--autoplay-notes] [--faults 30:150,60:300] [--out runs] [--tag 이름]
//!                [--render "Playback 1/2"] [--capture "Loopback 1/2"] [--no-capture] [--lead-in 1.5]
//!
//! play: 곡을 실시간으로 재생하면서 루프백으로 녹음하고, 오디오 블록·믹서 사건을 기록한다 (창·입력 없음, 2-2 첫 단계).
//!   --faults 곡초:ms,...  오디오 스레드가 그 곡 시각에 ms 동안 버퍼를 채우지 않는다 (언더런).
//! 출력: runs/<tag>-<시각>/ meta.json, audio_blocks.csv, mix_events.csv, capture_2ch.f32, capture_packets.csv

mod audio;
mod capture;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcore::mixer::{render_perfect, LatePolicy, MixEvent};
use rcore::ring::ring;
use rcore::{rtc, runtime};
use synchronizer::qpc;

pub(crate) fn arg(args: &[String], key: &str, default: &str) -> String {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.to_string())
}

fn flag(args: &[String], key: &str) -> bool {
    args.iter().any(|a| a == key)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("play") => play(&args),
        _ => {
            eprintln!("사용법: cp2game play <곡.rtc> [--autoplay-notes] [--faults 30:150] [--out runs]");
            Ok(())
        }
    };
    if let Err(e) = r {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}

/// 헤드룸 정책: 정답 렌더의 최댓값이 −1 dBFS를 넘으면 그만큼 줄인다 (곡마다 고정, 플레이 중 바꾸지 않음).
fn master_gain(c: &runtime::Compiled) -> (f32, f32) {
    let (pcm, _) = render_perfect(c, 4096);
    let peak = pcm.iter().fold(0f32, |m, &x| m.max(x.abs()));
    let limit = 10f32.powf(-1.0 / 20.0);
    (if peak > limit { limit / peak } else { 1.0 }, peak)
}

fn parse_faults(s: &str) -> Result<Vec<audio::AudioFault>, String> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|f| {
            let (t, ms) = f.split_once(':').ok_or(format!("고장 형식 곡초:ms — {f}"))?;
            Ok(audio::AudioFault { at_s: t.parse().map_err(|_| f.to_string())?, stall_ms: ms.parse().map_err(|_| f.to_string())? })
        })
        .collect()
}

fn play(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.get(2).ok_or("곡 파일이 필요함")?;
    let render_name = arg(args, "--render", "Playback 1/2");
    let src = rtc::decode(&fs::read(path)?)?;
    let src_rate = src.bank.rate;
    let src_hash = runtime::compile(src.clone())?.hash;
    // 곡 준비 (제어 경로): 장치 속도로 리샘플링한 뒤 컴파일한다. 플레이 중에는 디코딩·변환이 없다.
    let dev_rate = audio::device_rate(&render_name)?;
    let t_rs = std::time::Instant::now();
    let ready = if dev_rate != src_rate { rcore::resample::resample_chart(&src, dev_rate)? } else { src };
    let resample_ms = t_rs.elapsed().as_secs_f64() * 1e3;
    let chart = Arc::new(runtime::compile(ready)?);
    let (gain, peak) = master_gain(&chart);
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = arg(args, "--tag", "play");
    let out = PathBuf::from(arg(args, "--out", "runs")).join(format!("{tag}-{unix}"));
    fs::create_dir_all(&out)?;
    println!("{} → {}", chart.chart.title, out.display());
    if dev_rate != src_rate {
        println!("  뱅크 {src_rate} → {dev_rate} Hz 리샘플링 {resample_ms:.0} ms");
    }
    println!("  정답 렌더 최댓값 {peak:.3} → 마스터 이득 {gain:.3} ({:.1} dB)", 20.0 * gain.log10());

    // 녹음 먼저 (재생 시작을 놓치지 않게)
    let cap_stop = Arc::new(AtomicBool::new(false));
    let cap_started = Arc::new(AtomicBool::new(false));
    let cap = if flag(args, "--no-capture") {
        None
    } else {
        let (stop, started, dir, name) = (cap_stop.clone(), cap_started.clone(), out.clone(), arg(args, "--capture", "Loopback 1/2"));
        let h = thread::Builder::new().name("capture".into()).spawn(move || capture::capture_loop(&name, &dir, stop, started))?;
        while !cap_started.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        Some(h)
    };

    let (_key_tx, key_rx) = ring::<audio::KeyReq>(256);
    let (blk_tx, mut blk_rx) = ring::<audio::BlockLog>(1 << 14);
    let (mix_tx, mut mix_rx) = ring::<MixEvent>(1 << 15);
    let cfg = audio::AudioCfg {
        render: render_name.clone(),
        lead_in_s: arg(args, "--lead-in", "1.5").parse()?,
        tail_s: 1.0,
        master_gain: gain,
        late: LatePolicy::DEFAULT,
        autoplay_notes: flag(args, "--autoplay-notes"),
        faults: faults.clone(),
    };
    let autoplay = cfg.autoplay_notes;
    let t_start = qpc::now();
    let a = audio::spawn(chart.clone(), cfg, key_rx, blk_tx, mix_tx)?;
    println!(
        "  출력 {} — {} Hz {}ch, 버퍼 {} 프레임, 주기 {:.2} ms, MMCSS {}",
        a.info.device,
        a.info.rate,
        a.info.channels,
        a.info.buffer_frames,
        a.info.period_100ns as f64 / 1e4,
        a.info.mmcss
    );

    // 기록 스레드 역할: 링을 비워 CSV로 쓴다 (실시간 스레드가 파일을 만지지 않게)
    let mut fb = BufWriter::new(File::create(out.join("audio_blocks.csv"))?);
    writeln!(fb, "qpc_wake,written,padding,frames,dev_frame,dev_qpc100,fill_ns,voices,injected_ms")?;
    let mut fm = BufWriter::new(File::create(out.join("mix_events.csv"))?);
    writeln!(fm, "kind,sound,due,start,block,late,player")?;
    let secs_limit: f64 = arg(args, "--secs", "0").parse()?;
    let mut n_blocks = 0u64;
    let mut secs_limit_hit = false;
    loop {
        let finished = a.done.load(Ordering::Acquire);
        while let Some(b) = blk_rx.pop() {
            n_blocks += 1;
            writeln!(fb, "{},{},{},{},{},{},{},{},{}", b.qpc_wake, b.written, b.padding, b.frames, b.dev_frame, b.dev_qpc100, b.fill_ns, b.voices, b.injected_ms)?;
        }
        while let Some(e) = mix_rx.pop() {
            match e {
                MixEvent::Start { sound, due, start, block, late, player } => writeln!(fm, "start,{sound},{due},{start},{block},{late},{}", player as u8)?,
                MixEvent::Dropped { sound, due, block, player } => writeln!(fm, "dropped,{sound},{due},,{block},,{}", player as u8)?,
                MixEvent::Overflow { sound, due, player } => writeln!(fm, "overflow,{sound},{due},,,,{}", player as u8)?,
            }
        }
        if finished {
            break;
        }
        if !secs_limit_hit && secs_limit > 0.0 && (qpc::now() - t_start) as f64 / qpc::frequency() as f64 > secs_limit {
            println!("  --secs 제한으로 중단");
            a.stop.store(true, Ordering::Release);
            secs_limit_hit = true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    fb.flush()?;
    fm.flush()?;
    let st = a.join.join().map_err(|_| "오디오 스레드 패닉")??;
    cap_stop.store(true, Ordering::Release);
    let capm = match cap {
        Some(h) => Some(h.join().map_err(|_| "녹음 스레드 패닉")??),
        None => None,
    };
    let snap = a.clock.load();
    let faults_json: Vec<String> = faults.iter().map(|f| format!("{{\"at_s\": {}, \"stall_ms\": {}}}", f.at_s, f.stall_ms)).collect();
    let meta = format!(
        "{{\n \"chart\": \"{}\", \"chart_hash\": \"{:016x}\", \"source_hash\": \"{src_hash:016x}\", \"source_rate\": {src_rate}, \"resample_ms\": {resample_ms:.1}, \"title\": \"{}\",\n \"device\": \"{}\", \"rate\": {}, \"channels\": {}, \"buffer_frames\": {}, \"period_100ns\": {}, \"mmcss\": {},\n \"origin\": {}, \"master_gain\": {gain}, \"render_peak\": {peak}, \"autoplay_notes\": {autoplay},\n \"faults\": [{}],\n \"written\": {}, \"blocks\": {}, \"overflows\": {}, \"log_lost\": {}, \"mix_lost\": {},\n \"clock\": {{\"valid\": {}, \"epoch\": {}, \"t0\": {}, \"f0\": {}, \"a\": {}, \"b\": {}}},\n \"qpc_freq\": {},\n \"capture\": {}\n}}\n",
        path.replace('\\', "/"),
        chart.hash,
        chart.chart.title,
        a.info.device,
        a.info.rate,
        a.info.channels,
        a.info.buffer_frames,
        a.info.period_100ns,
        a.info.mmcss,
        a.info.origin,
        faults_json.join(", "),
        st.written,
        st.blocks,
        st.overflows,
        st.log_lost,
        st.mix_lost,
        snap.valid,
        snap.epoch,
        snap.t0,
        snap.f0,
        snap.a,
        snap.b,
        qpc::frequency(),
        capm.map(|m| format!("{{\"name\": \"{}\", \"rate\": {}, \"channels\": {}, \"frames\": {}, \"silent_packets\": {}}}", m.name, m.rate, m.channels, m.frames, m.silent_packets))
            .unwrap_or_else(|| "null".into())
    );
    fs::write(out.join("meta.json"), meta)?;
    println!("  끝: 블록 {n_blocks}, 보이스 넘침 {}, 기록 손실 {}/{}", st.overflows, st.log_lost, st.mix_lost);
    Ok(())
}
