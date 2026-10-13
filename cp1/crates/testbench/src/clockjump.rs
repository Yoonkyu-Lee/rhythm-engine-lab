//! 3-3 단계: 고장 D (시계 점프). 언더런으로 재생 위치 보고에 계단이 생긴 기록을 시계 모델에 다시 넣어,
//! 점프 감지(새 기준점) 정책별로 예측이 얼마나 틀리고 얼마나 빨리 돌아오는지 잰다.
//!
//!   testbench clockjump <runs\sched-...>   (faults_render.csv 가 있는 기록)
//!
//! 예측 오차 = 관측 직전 모델이 그 위치에 예측한 시각 − 실제 보고 시각. 평상시에도 보고값 톱니(±0.5 ms)만큼은 남는다.

use std::fs;
use std::path::PathBuf;

use synchronizer::clock::{ClockModel, ClockPolicy, Observe};

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let run = PathBuf::from(args.get(2).ok_or("사용법: testbench clockjump <runs\\sched-...>")?);
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
        let frame = (v[4].parse::<f64>()? * rate / clock_freq).round() as u64;
        if frame as f64 > rate * 0.05 {
            obs.push((frame, v[5].parse()?));
        }
    }
    let mut faults: Vec<(f64, u64)> = Vec::new();
    for line in fs::read_to_string(run.join("faults_render.csv"))?.lines().skip(1) {
        let v: Vec<&str> = line.split(',').collect();
        faults.push((v[1].parse()?, v[2].parse()?));
    }
    let t0 = obs[0].1;
    let ts = |t: i64| (t - t0) as f64 * 1e-7;
    println!("{}: 관측 {}개, 고장 A {:?}", run.display(), obs.len(), faults);

    let policies: Vec<(&str, ClockPolicy)> = vec![
        ("감지 없음 (그냥 맞추기)", ClockPolicy { jump_ms: f64::INFINITY, ..Default::default() }),
        ("새 기준점: 2 ms 벗어남 ×2회 (기본)", ClockPolicy::default()),
        ("새 기준점: 5 ms 벗어남 ×3회", ClockPolicy { jump_ms: 5.0, jump_count: 3, ..Default::default() }),
        ("새 기준점: 10 ms 벗어남 ×5회", ClockPolicy { jump_ms: 10.0, jump_count: 5, ..Default::default() }),
    ];
    for (pi, (name, pol)) in policies.into_iter().enumerate() {
        let mut m = ClockModel::new(rate, pol);
        let mut errs: Vec<(f64, f64)> = Vec::new(); // (시각 s, 오차 ms)
        let mut epochs: Vec<f64> = Vec::new();
        for &(f, t) in &obs {
            if m.is_valid() {
                errs.push((ts(t), (m.t100_at(f as f64) - t as f64) / 1e4));
            }
            if let Observe::NewEpoch(_) = m.observe(f, t) {
                epochs.push(ts(t));
            }
        }
        println!("\n  [{name}] 새 기준점 {}회: {:?}", epochs.len(), epochs.iter().map(|x| format!("{x:.2}초")).collect::<Vec<_>>());
        {
            use std::io::Write;
            let mut f = std::io::BufWriter::new(fs::File::create(run.join(format!("clockjump_{pi}.csv")))?);
            writeln!(f, "# {name}; epochs={}", epochs.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(" "))?;
            writeln!(f, "t_s,err_ms")?;
            for (t, e) in &errs {
                writeln!(f, "{t:.4},{e:.4}")?;
            }
        }
        // 고장 시각 = 오디오 시간. 관측 시각 축으로는 앞선 고장들의 밀림만큼 뒤에 있다 → 프레임으로 찾는다.
        let fts: Vec<f64> = faults.iter().map(|&(at, _)| {
            let i0 = obs.iter().position(|&(f, _)| f as f64 >= at * rate).unwrap_or(0);
            ts(obs[i0].1) - 0.1
        }).collect();
        for (fi, &(at, ms)) in faults.iter().enumerate() {
            let ft = fts[fi];
            let end = fts.get(fi + 1).copied().unwrap_or(f64::INFINITY).min(ft + 60.0);
            let win: Vec<&(f64, f64)> = errs.iter().filter(|e| e.0 >= ft && e.0 < end).collect();
            let peak = win.iter().map(|e| e.1.abs()).fold(0.0, f64::max);
            // 회복: 이후 오차가 1 ms 안으로 들어와 10초 동안 유지되기 시작한 시각
            let mut rec = None;
            for (j, e) in win.iter().enumerate() {
                if e.1.abs() < 1.0 && win[j..].iter().take_while(|x| x.0 < e.0 + 10.0).all(|x| x.1.abs() < 1.0) {
                    rec = Some(e.0 - ft);
                    break;
                }
            }
            let later: Vec<f64> = win.iter().filter(|e| e.0 >= ft + 1.0 && e.0 < ft + 10.0).map(|e| e.1.abs()).collect();
            let later_max = later.iter().copied().fold(0.0, f64::max);
            println!(
                "    고장 A {at}초 {ms}ms: 최대 오차 {peak:7.2} ms, 1 ms 안으로 회복 {}, 고장 1–10초 뒤 최대 {later_max:6.2} ms",
                rec.map(|r| format!("{:.2}초", r)).unwrap_or("못 함 (60초 안)".into())
            );
        }
        let base: Vec<f64> = errs.iter().filter(|e| faults.iter().all(|&(at, _)| (e.0 - at).abs() > 15.0) && e.0 > 3.0).map(|e| e.1.abs()).collect();
        if !base.is_empty() {
            let mut b = base.clone();
            b.sort_by(|a, c| a.partial_cmp(c).unwrap());
            println!("    고장과 먼 구간: p50 {:.3} ms, 최대 {:.3} ms", b[b.len() / 2], b[b.len() - 1]);
        }
    }
    Ok(())
}
