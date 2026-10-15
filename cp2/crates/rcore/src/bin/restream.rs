//! 믹서 사건 로그로 실제 출력 스트림을 다시 만든다 (출력 무결성 검사용).
//!   restream <파일.rtc> <mix_events.csv> <속도> <마스터 이득> <출력.wav>
//!
//! 믹서는 결정적이므로 "어느 소리를, 어느 프레임에(start), 어느 블록부터(block), 어떤 이득으로" 냈는지만 알면
//! 출력 스트림(스트림 프레임 0부터)을 그대로 재현할 수 있다. 앞부분을 자른 소리는 block 이전을 내지 않았다.
//! 같은 리샘플러로 장치 속도의 뱅크를 만든다. 이 결과를 루프백 녹음과 맞추면 "믹서가 냈다고 한 것 = 실제로 나간 것"인지 본다.

use rcore::{resample, rtc, runtime};
use std::io::{BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 6 {
        eprintln!("사용법: restream <파일.rtc> <mix_events.csv> <속도> <마스터 이득> <출력.wav>");
        std::process::exit(2);
    }
    let rate: u32 = a[3].parse()?;
    let gain: f32 = a[4].parse()?;
    let src = rtc::decode(&std::fs::read(&a[1])?)?;
    let c = runtime::compile(resample::resample_chart(&src, rate)?)?;
    let bank = &c.chart.bank;
    let mut starts = Vec::new();
    for line in std::io::BufReader::new(std::fs::File::open(&a[2])?).lines().skip(1) {
        let line = line?;
        let f: Vec<&str> = line.split(',').collect();
        if f[0] != "start" {
            continue;
        }
        let (sound, start, block, g): (u32, i64, i64, f32) = (f[1].parse()?, f[3].parse()?, f[4].parse()?, f[7].parse()?);
        starts.push((sound, start, block, g));
    }
    let end = starts.iter().map(|&(s, st, _, _)| st + bank.sounds[s as usize].frames as i64).max().unwrap_or(0).max(0) as usize;
    let mut out = vec![0f32; end * 2];
    // 믹서와 같은 순서(들어온 순서)로 더한다
    for &(s, start, block, g) in &starts {
        let snd = bank.sounds[s as usize];
        let lo = start.max(block).max(0);
        for f in lo..start + snd.frames as i64 {
            let src = (snd.offset as i64 + f - start) as usize * 2;
            out[f as usize * 2] += bank.pcm[src] * g;
            out[f as usize * 2 + 1] += bank.pcm[src + 1] * g;
        }
    }
    for v in out.iter_mut() {
        *v *= gain;
    }
    let data = (out.len() * 4) as u32;
    let mut w = std::io::BufWriter::new(std::fs::File::create(&a[5])?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    for v in [16u32] {
        w.write_all(&v.to_le_bytes())?;
    }
    w.write_all(&3u16.to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * 8).to_le_bytes())?;
    w.write_all(&8u16.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())?;
    for v in &out {
        w.write_all(&v.to_le_bytes())?;
    }
    println!("{}: 발음 {} 개, {:.1}초", a[5], starts.len(), end as f64 / rate as f64);
    Ok(())
}
