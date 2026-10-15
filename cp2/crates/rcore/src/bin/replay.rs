//! 리플레이: 실행 기록의 입력 원본(judgments.csv kind 8)만으로 판정을 다시 해서 실행 중 판정과 노트마다 맞대 본다.
//!   replay <runs/...> [곡.rtc]
//!
//! "우리 방식" 판정은 순수 함수이고, 입력은 청취 곡 시각(장치 시계로 바꾼 값)으로 기록된다.
//! 그래서 같은 입력 순서면 스레드 타이밍·렉과 무관하게 같은 결과가 나와야 한다 (리플레이 재현 100%가 목표).
//! 실행 중에는 누름 직전과 watermark마다 MISS를 확정한다. 리플레이는 누름 직전과 마지막에만 확정한다 —
//! watermark는 아직 처리하지 않은 입력보다 앞서지 않으므로 MISS 집합·등급은 같다.
//! 무효(재동기화 중) 판정은 원래 등급으로 비교한다 (무효는 점수 규칙이지 판정이 아니다).
//! 빈 누름은 개수로, 롱노트 끝은 성공 여부만 비교한다 (watermark로 확정된 끝의 오차는 0으로 기록되므로).

use std::collections::BTreeMap;

use rcore::judge::{Judged, Lane, Windows};
use rcore::{rtc, runtime};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Res {
    Hit { grade: u8, err: i64 },
    Miss,
}

fn num(meta: &str, key: &str) -> Option<String> {
    let i = meta.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = meta[i..].trim_start_matches([':', ' ']);
    if let Some(r) = rest.strip_prefix('"') {
        return Some(r[..r.find('"')?].to_string());
    }
    if let Some(r) = rest.strip_prefix('[') {
        return Some(r[..r.find(']')?].to_string());
    }
    Some(rest[..rest.find([',', '\n', '}'])?].trim().to_string())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("사용법: replay <runs/...> [곡.rtc]");
        std::process::exit(2);
    }
    let run = std::path::Path::new(&a[1]);
    let meta = std::fs::read_to_string(run.join("meta.json"))?;
    let profile = num(&meta, "profile").unwrap_or_default();
    if profile != "ours" {
        eprintln!("리플레이는 \"우리 방식\" 실행만 (이 실행: {profile}). 프레임 구동 프로필의 판정은 프레임 시각에 묶여 입력만으로 재현되지 않는다.");
        std::process::exit(2);
    }
    let chart_path = a.get(2).cloned().unwrap_or_else(|| num(&meta, "chart").unwrap_or_default());
    let c = runtime::compile(rtc::decode(&std::fs::read(&chart_path)?)?)?;
    let w: Vec<i64> = num(&meta, "windows_ns").ok_or("windows_ns")?.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
    let win = Windows { perfect: w[0], great: w[1], good: w[2], bad: w[3] };
    let notes = &c.chart.lanes[0].notes;

    // 실행 기록: 입력 원본과 판정
    let text = std::fs::read_to_string(run.join("judgments.csv"))?;
    let mut inputs: Vec<(bool, i64)> = Vec::new();
    let mut live: BTreeMap<u32, Res> = BTreeMap::new();
    let mut live_ln: BTreeMap<u32, bool> = BTreeMap::new();
    let (mut live_empty, mut last_t) = (0u32, i64::MIN);
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let (kind, note, grade, err, t): (u8, u32, u8, i64, i64) = (f[0].parse()?, f[1].parse()?, f[2].parse()?, f[3].parse()?, f[4].parse()?);
        match kind {
            8 => inputs.push((grade == 1, t)),
            0 => drop(live.insert(note, Res::Hit { grade, err })),
            2 => drop(live.insert(note, Res::Miss)),
            7 if grade == 4 => drop(live.insert(note, Res::Miss)),
            7 => drop(live.insert(note, Res::Hit { grade, err })),
            1 => live_empty += 1,
            3 | 4 => drop(live_ln.insert(note, kind == 3)),
            _ => {}
        }
        if matches!(kind, 2 | 3 | 4 | 7) {
            last_t = last_t.max(t);
        }
    }

    if inputs.is_empty() {
        eprintln!("{}: 입력 원본(kind 8)이 없다 — CP2 2-6 이전에 만든 기록이라 리플레이할 수 없다.", run.display());
        std::process::exit(2);
    }

    // 다시 판정
    let mut lane = Lane::new();
    let mut pressed = false;
    let mut rep: BTreeMap<u32, Res> = BTreeMap::new();
    let mut rep_ln: BTreeMap<u32, bool> = BTreeMap::new();
    let mut rep_empty = 0u32;
    let take = |j: Judged, rep: &mut BTreeMap<u32, Res>, rep_ln: &mut BTreeMap<u32, bool>, empty: &mut u32| match j {
        Judged::Hit { note, grade, err } => drop(rep.insert(note, Res::Hit { grade: grade as u8, err })),
        Judged::Miss { note } => drop(rep.insert(note, Res::Miss)),
        Judged::LongEnd { note, ok, .. } => drop(rep_ln.insert(note, ok)),
        Judged::Empty { .. } => *empty += 1,
    };
    for &(down, t) in &inputs {
        if down {
            if pressed {
                continue;
            }
            pressed = true;
            let mut outs = Vec::new();
            lane.advance(notes, &win, t, &mut |j| outs.push(j));
            outs.push(lane.press(notes, &win, t));
            for j in outs {
                take(j, &mut rep, &mut rep_ln, &mut rep_empty);
            }
        } else {
            pressed = false;
            if let Some(j) = lane.release(notes, &win, t) {
                take(j, &mut rep, &mut rep_ln, &mut rep_empty);
            }
        }
    }
    let mut outs = Vec::new();
    lane.advance(notes, &win, last_t, &mut |j| outs.push(j));
    for j in outs {
        take(j, &mut rep, &mut rep_ln, &mut rep_empty);
    }

    // 맞대기
    let mut diffs = Vec::new();
    for (n, r) in &live {
        if rep.get(n) != Some(r) {
            diffs.push(format!("노트 {n}: 실행 {r:?} / 리플레이 {:?}", rep.get(n)));
        }
    }
    for (n, r) in &rep {
        if !live.contains_key(n) {
            diffs.push(format!("노트 {n}: 실행 없음 / 리플레이 {r:?}"));
        }
    }
    for (n, ok) in &live_ln {
        if rep_ln.get(n) != Some(ok) {
            diffs.push(format!("롱노트 끝 {n}: 실행 {ok} / 리플레이 {:?}", rep_ln.get(n)));
        }
    }
    if live_empty != rep_empty {
        diffs.push(format!("빈 누름: 실행 {live_empty} / 리플레이 {rep_empty}"));
    }
    let same = live.len() - diffs.iter().filter(|d| d.starts_with("노트") && !d.contains("실행 없음")).count();
    println!(
        "{}: 입력 {}개, 노트 판정 {}/{} 일치 (오차 ns까지), 롱노트 끝 {}, 빈 누름 {} → {}",
        run.display(),
        inputs.len(),
        same,
        live.len(),
        live_ln.len(),
        live_empty,
        if diffs.is_empty() { "재현 100%" } else { "불일치" }
    );
    for d in diffs.iter().take(20) {
        println!("  {d}");
    }
    std::process::exit(if diffs.is_empty() { 0 } else { 1 });
}
