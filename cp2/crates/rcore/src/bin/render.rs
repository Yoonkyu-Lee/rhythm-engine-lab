//! 오프라인 정답 렌더: 모든 노트를 제때 쳤다고 보고 곡 전체를 32비트 float WAV로 쓴다.
//!   render <파일.rtc> <출력.wav> [--block 512] [--check 1,64,441,4096] [--rate 48000]
//! `--check`: 블록 크기를 바꿔 다시 렌더해 비트 단위로 같은지 확인한다 (믹서 결정성).
//! 옆에 <출력>.json: 해시, 길이, 발음 수, 늦음·버림·넘침 수, 렌더 시간.

use rcore::mixer::{render_perfect, MixEvent};
use rcore::{rtc, runtime};
use std::io::Write;
use std::time::Instant;

fn arg(args: &[String], k: &str, d: &str) -> String {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| d.to_string())
}

fn write_wav_f32(path: &str, pcm: &[f32], rate: u32, ch: u16) -> std::io::Result<()> {
    let data = (pcm.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // IEEE float
    f.write_all(&ch.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * ch as u32 * 4).to_le_bytes())?;
    f.write_all(&(ch * 4).to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for x in pcm {
        f.write_all(&x.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("사용법: render <파일.rtc> <출력.wav> [--block 512] [--check 1,64,441,4096]");
        std::process::exit(2);
    }
    let mut src = rtc::decode(&std::fs::read(&args[1])?)?;
    // --rate: 실시간 재생과 같은 장치 속도로 리샘플링한 정답 (게임과 같은 리샘플러)
    let rate: u32 = arg(&args, "--rate", "0").parse()?;
    if rate != 0 && rate != src.bank.rate {
        src = rcore::resample::resample_chart(&src, rate)?;
    }
    let c = runtime::compile(src)?;
    let block: usize = arg(&args, "--block", "512").parse()?;
    let t0 = Instant::now();
    let (pcm, log) = render_perfect(&c, block);
    let secs = t0.elapsed().as_secs_f64();
    let mut check = Vec::new();
    for b in arg(&args, "--check", "").split(',').filter(|s| !s.is_empty()) {
        let b: usize = b.parse()?;
        let (x, _) = render_perfect(&c, b);
        let n = pcm.len().min(x.len());
        let same = pcm[..n] == x[..n] && pcm[n..].iter().chain(x[n..].iter()).all(|&v| v == 0.0);
        println!("  블록 {b}: {}", if same { "비트 단위로 같음" } else { "다름" });
        check.push(format!("{{\"block\": {b}, \"identical\": {same}}}"));
        if !same {
            std::process::exit(1);
        }
    }
    write_wav_f32(&args[2], &pcm, c.chart.bank.rate, 2)?;
    let (mut starts, mut late, mut dropped, mut overflow) = (0, 0, 0, 0);
    for e in &log {
        match e {
            MixEvent::Start { late: l, .. } => {
                starts += 1;
                late += (*l > 0) as u32;
            }
            MixEvent::Dropped { .. } => dropped += 1,
            MixEvent::Overflow { .. } => overflow += 1,
            MixEvent::Cut { .. } => {}
        }
    }
    let peak = pcm.iter().fold(0f32, |m, &x| m.max(x.abs()));
    let frames = pcm.len() / 2;
    let json = format!(
        "{{\"hash\": \"{:016x}\", \"rate\": {}, \"frames\": {frames}, \"block\": {block}, \"starts\": {starts}, \"late\": {late}, \"dropped\": {dropped}, \"overflow\": {overflow}, \"peak\": {peak}, \"render_s\": {secs:.4}, \"check\": [{}]}}\n",
        c.hash,
        c.chart.bank.rate,
        check.join(", ")
    );
    std::fs::write(format!("{}.json", args[2]), &json)?;
    println!(
        "{}: {:.1}초, 발음 {starts} (늦음 {late}, 버림 {dropped}, 넘침 {overflow}), 최대 {peak:.3}, 렌더 {:.0} ms (실시간의 {:.0}배)",
        args[2],
        frames as f64 / c.chart.bank.rate as f64,
        secs * 1e3,
        frames as f64 / c.chart.bank.rate as f64 / secs
    );
    Ok(())
}
