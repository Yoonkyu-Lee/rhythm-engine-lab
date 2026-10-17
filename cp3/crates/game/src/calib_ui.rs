//! 보정 도구 (3-4, `cp3/설정창_계획.md` 5장): 합성 차분(엔진 `calib::calib_chart`)을 플레이 경로 그대로 돌리고(`cp3game run calib --calib …`),
//! 판정 기록의 누름(judgments.csv kind 8)을 박과 비교해 오프셋을 제안한다.
//! - 소리로 맞추기(A): 클릭을 들으며 누른다. 실행은 오프셋 A = 0으로 돌린다(game.rs) → 중앙값 = 이 사람·장비의 늦음 그대로.
//!   제안 A = −중앙값.
//! - 화면으로 맞추기(B): 클릭 없이 노트가 판정선에 닿을 때 누른다. A는 지금 값 그대로(귀를 먼저 맞춘 뒤) → 제안 B = 지금 B + 중앙값
//!   (늦게 보고 늦게 누르면 중앙값 +, 노트를 그만큼 앞당겨 그려야 하므로 B도 + 쪽).
//! - 봇 검증: `--bot-direct --bot-offset-ms 20`이면 소리 보정 제안 A = −20 ms, 화면 보정 제안 B = B + 20 ms (±1).

use std::path::Path;

use engine::calib::{analyze, beat_grid, calib_chart, Calib};
use engine::runtime::Compiled;

pub const BPM: f64 = 120.0;
pub const BEATS: usize = 40;
/// 처음 몇 박은 버린다 (맞춰 가는 중)
pub const WARMUP: usize = 4;
/// 이보다 먼 누름은 엉뚱한 박을 노린 것으로 버린다
pub const REJECT_NS: i64 = 150_000_000;
pub const HIST_MS: i64 = 60;

pub fn chart(rate: u32, with_click: bool) -> Compiled {
    calib_chart(rate, BPM, BEATS, with_click)
}

/// 실행 폴더의 judgments.csv에서 누름의 곡 시각(오프셋 A 포함 = 판정이 본 시각)을 모은다. 키 반복(같은 키의 연속 누름)은 거른다.
pub fn presses(run_dir: &Path) -> Result<Vec<i64>, String> {
    let csv = std::fs::read_to_string(run_dir.join("judgments.csv")).map_err(|e| e.to_string())?;
    let mut down: std::collections::HashMap<u16, bool> = Default::default();
    let mut out = Vec::new();
    for l in csv.lines().skip(1) {
        let f: Vec<&str> = l.split(',').collect();
        if f.first() != Some(&"8") || f.len() < 9 {
            continue;
        }
        let (Ok(t), Ok(vkey)) = (f[5].parse::<i64>(), f[8].parse::<u16>()) else { continue };
        let is_down = f[3] == "1";
        if down.insert(vkey, is_down).unwrap_or(false) == is_down {
            continue;
        }
        if is_down {
            out.push(t);
        }
    }
    Ok(out)
}

pub fn result(run_dir: &Path) -> Result<Calib, String> {
    Ok(analyze(&beat_grid(2_000_000_000, BPM, BEATS), &presses(run_dir)?, WARMUP, REJECT_NS, HIST_MS))
}

/// 보정 결과 → 새 오프셋 (ms). `sound`면 A의 새 값, 아니면 B의 새 값(`cur_b_ms` 기준).
pub fn proposal_ms(c: &Calib, sound: bool, cur_b_ms: f64) -> f64 {
    let m = c.median as f64 / 1e6;
    if sound {
        -m
    } else {
        cur_b_ms + m
    }
}

pub fn describe(c: &Calib, sound: bool, cur_b_ms: f64) -> String {
    let what = if sound { "소리로 맞추기 (A)" } else { "화면으로 맞추기 (B)" };
    if c.used < 8 {
        return format!("{what}: 쓸 수 있는 누름이 {}개뿐 (버림 {}) — 다시 해 주세요", c.used, c.dropped);
    }
    format!(
        "{what}: 누름 {} (버림 {}), 중앙값 {:+.1} ms, 흔들림(MAD) {:.1} ms → 제안 {} {:+.0} ms  {}",
        c.used,
        c.dropped,
        c.median as f64 / 1e6,
        c.mad as f64 / 1e6,
        if sound { "A" } else { "B" },
        proposal_ms(c, sound, cur_b_ms),
        sparkline(c)
    )
}

/// 히스토그램을 한 줄 문자로 (설정 화면 표시용): 4 ms씩 묶는다
pub fn sparkline(c: &Calib) -> String {
    let bins: Vec<u32> = c.hist.chunks(4).map(|ch| ch.iter().sum()).collect();
    let max = bins.iter().copied().max().unwrap_or(1).max(1);
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    bins.iter().map(|&v| if v == 0 { '·' } else { bars[((v * 7) / max) as usize] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_skip_repeats_and_proposals() {
        let dir = std::env::temp_dir().join(format!("cp3calib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let beats = beat_grid(2_000_000_000, BPM, BEATS);
        let mut csv = String::from("kind,lane,note,grade,err_ns,t_ns,q_in,q_proc,vkey,key_frame,state,q_recv,q_floor\n");
        for &b in &beats {
            let t = b + 20_000_000;
            csv += &format!("8,0,0,1,0,{t},0,0,124,-1,0,0,0\n");
            csv += &format!("8,0,0,1,0,{},0,0,124,-1,0,0,0\n", t + 30_000_000); // 키 반복: 버린다
            csv += &format!("8,0,0,0,0,{},0,0,124,-1,0,0,0\n", t + 80_000_000);
        }
        std::fs::write(dir.join("judgments.csv"), csv).unwrap();
        assert_eq!(presses(&dir).unwrap().len(), BEATS);
        let c = result(&dir).unwrap();
        assert_eq!(c.used, BEATS - WARMUP);
        assert_eq!(c.median, 20_000_000);
        assert_eq!(proposal_ms(&c, true, 0.0), -20.0);
        assert_eq!(proposal_ms(&c, false, 5.0), 25.0);
    }
}
