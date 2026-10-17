//! 보정 도구의 계산 (3-4, `cp3/설정창_계획.md` 5장): 박에 맞춘 누름들 → 오차 분포 → 오프셋 제안.
//!
//! - 오차 = 누름의 청취 곡 시각 − 가장 가까운 박. 처음 `warmup`박의 누름은 버린다(맞춰 가는 중).
//! - 박 간격의 절반보다 먼 누름, 그리고 |오차| > `reject`는 버린다(엉뚱한 박을 노린 누름).
//! - 박마다 표본은 하나(처리 순서로 먼저 온 누름). 한 박에 여러 번 누르면 나머지는 버린다(리뷰 3-4 #4).
//! - 제안 오프셋 = −중앙값 (A: 판정과 키음을 함께 옮겨 이 사람의 누름이 PERFECT 자리에 오게, B: 화면을 그만큼 옮김).
//! - 중앙값을 쓴다(평균은 실수 한두 번에 끌려간다). 안정성 지표로 MAD(중앙 절대 편차)를 함께 낸다.

use crate::runtime::*;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, PartialEq)]
pub struct Calib {
    /// 쓴 누름 수, 버린 누름 수
    pub used: usize,
    pub dropped: usize,
    /// 오차 중앙값 (ns, +면 늦게 누름)
    pub median: i64,
    /// 중앙 절대 편차 (ns)
    pub mad: i64,
    /// 제안 오프셋 (ns) = −median
    pub suggest: i64,
    /// 오차 히스토그램: 1 ms 칸, −range..+range ms
    pub hist: Vec<u32>,
    pub hist_range_ms: i64,
}

fn median_of(v: &mut [i64]) -> i64 {
    v.sort_unstable();
    let n = v.len();
    if n == 0 {
        0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        // 짝수: 가운데 둘의 평균 (0 쪽 버림 없이 내림)
        (v[n / 2 - 1] + v[n / 2]).div_euclid(2)
    }
}

/// `beats` = 박 시각(ns, 오름차순), `presses` = 누름 시각(ns, 처리한 순서).
pub fn analyze(beats: &[i64], presses: &[i64], warmup: usize, reject: i64, hist_range_ms: i64) -> Calib {
    let mut errs = Vec::new();
    let mut dropped = 0;
    let first_ok = beats.get(warmup).copied().unwrap_or(i64::MAX);
    let mut taken = vec![false; beats.len()];
    for &p in presses {
        let k = beats.partition_point(|&b| b < p);
        let near = [k.checked_sub(1), (k < beats.len()).then_some(k)].into_iter().flatten().min_by_key(|&i| (p - beats[i]).abs());
        let Some(i) = near else {
            dropped += 1;
            continue;
        };
        let half = match (i.checked_sub(1).map(|j| beats[i] - beats[j]), beats.get(i + 1).map(|b| b - beats[i])) {
            (Some(a), Some(b)) => a.min(b) / 2,
            (Some(a), None) | (None, Some(a)) => a / 2,
            (None, None) => i64::MAX,
        };
        let e = p - beats[i];
        if beats[i] < first_ok || e.abs() > half || e.abs() > reject || taken[i] {
            dropped += 1;
            continue;
        }
        taken[i] = true;
        errs.push(e);
    }
    let mut hist = vec![0u32; (2 * hist_range_ms + 1) as usize];
    for &e in &errs {
        let ms = (e as f64 / 1e6).round() as i64;
        if ms.abs() <= hist_range_ms {
            hist[(ms + hist_range_ms) as usize] += 1;
        }
    }
    let median = median_of(&mut errs.clone());
    let mut dev: Vec<i64> = errs.iter().map(|e| (e - median).abs()).collect();
    let mad = median_of(&mut dev);
    Calib { used: errs.len(), dropped, median, mad, suggest: -median, hist, hist_range_ms }
}

/// 보정용 박 일정: `bpm`으로 `count`박, 첫 박은 `start` ns.
pub fn beat_grid(start: i64, bpm: f64, count: usize) -> Vec<i64> {
    (0..count).map(|i| start + (i as f64 * 60e9 / bpm).round() as i64).collect()
}

/// 보정용 합성 차분 (3-4): 박마다 클릭(소리로 맞추기) 또는 클릭 없이(화면으로 맞추기), 레인 하나에 같은 박의 노트(소리 없음).
/// 클릭 = 1 kHz 사인 30 ms 지수 감쇠, 장치 속도로 바로 만든다(리샘플링 없음). 첫 박 2초.
pub fn calib_chart(rate: u32, bpm: f64, count: usize, with_click: bool) -> Compiled {
    let n = (rate as f64 * 0.03) as usize;
    let click: Vec<f32> = (0..n)
        .flat_map(|i| {
            let t = i as f64 / rate as f64;
            let v = (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * (-t / 0.008).exp()) as f32;
            [v, v]
        })
        .collect();
    let slot = OnceLock::new();
    let _ = slot.set(Pcm2::F32(click.into_boxed_slice()));
    let beats = beat_grid(2_000_000_000, bpm, count);
    let bank = SoundBank {
        rate,
        sounds: vec![SoundRef { data: 0, frames: n as u32, kind: SoundKind::Short, gain: 1.0, cut: false, choke: 0 }],
        data: vec![Arc::new(slot)],
        partial: vec![],
    };
    let notes = beats.iter().map(|&t| Note { t, end: t, sounds_start: 0, sounds_len: 0, kind: NoteKind::Tap, damage: -1 }).collect();
    let end = *beats.last().unwrap() + 1_000_000_000;
    Compiled {
        chart: Chart {
            title: if with_click { "보정: 소리".into() } else { "보정: 화면".into() },
            chart_id: "calib".into(),
            layout: "1".into(),
            bank,
            auto: if with_click { beats.iter().map(|&t| SoundEvent { t, sound: 0, gain: 1.0 }).collect() } else { vec![] },
            note_sounds: vec![],
            lanes: vec![Lane { id: "K1".into(), role: Role::Key, notes, lane_sounds: vec![] }],
            groups: vec![],
            scroll: vec![ScrollPoint { t: 0, pos: 0.0, speed: bpm / 60.0 }],
            bars: beats.iter().step_by(4).copied().collect(),
            reference_scroll: bpm,
            long_rule: LongRule::Cn,
            empty_lane: EmptyLane::None,
            end,
            chart_hash: [0; 32],
            variant_choice: vec![],
        },
        voice_budget: 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_offset_and_rejects_outliers() {
        let beats = beat_grid(1_000_000_000, 120.0, 40);
        // 모든 누름이 +20 ms 늦고, 결정적 흔들림 −3·0·+3 ms, 엉뚱한 누름 둘, 준비 박 4개
        let mut presses: Vec<i64> = beats.iter().enumerate().map(|(i, b)| b + 20_000_000 + (i as i64 % 3 - 1) * 3_000_000).collect();
        presses.push(beats[10] + 240_000_000); // 박 사이 (절반 250 ms 안이지만 reject 150 ms 밖)
        presses.push(beats[20] - 400_000_000); // 앞 박에 더 가까움 → 그 박 기준 +100 ms
        let c = analyze(&beats, &presses, 4, 150_000_000, 50);
        assert_eq!(c.suggest, -20_000_000);
        assert!(c.mad <= 3_000_000);
        assert_eq!(c.dropped, 4 + 1 + 1); // 준비 4 + reject 1 + 앞 박 기준 +100 ms (그 박은 이미 표본이 있음: 박마다 하나)
        assert_eq!(c.used, 36);
    }

    #[test]
    fn calib_chart_plays_clicks_on_beats() {
        let c = calib_chart(48_000, 120.0, 8, true);
        let (_out, log) = crate::mixer::render_perfect(&c, 256);
        let starts: Vec<i64> = log.iter().filter_map(|e| match e { crate::mixer::MixEvent::Start { start, player: false, .. } => Some(*start), _ => None }).collect();
        assert_eq!(starts, beat_grid(2_000_000_000, 120.0, 8).iter().map(|&t| ns_to_frame(t, 48_000)).collect::<Vec<_>>());
        assert_eq!(calib_chart(48_000, 120.0, 8, false).chart.auto.len(), 0);
    }
}
