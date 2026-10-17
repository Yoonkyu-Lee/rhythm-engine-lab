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

/// 보정 실행 한 번의 결과: 분석 + 측정 범위(장치 열쇠, 실행에 쓴 B) + 적용을 막는 문제들 (리뷰 3-4 #2·#4)
#[derive(Clone, Debug)]
pub struct CalibOut {
    pub c: Calib,
    pub sound: bool,
    /// 측정한 장치 범위 ("<경로>:<장치>") — 제안은 이 범위에만 적용한다
    pub device_key: String,
    /// 실행에 쓴 B (ms): 화면 보정의 제안 기준
    pub b_ms: f64,
    pub problems: Vec<String>,
}

impl CalibOut {
    /// 적용할 수 있는 제안값 (ms, 정수로 반올림). 문제가 있거나 표본이 모자라면 None.
    pub fn proposal(&self) -> Option<f64> {
        (self.problems.is_empty() && self.c.used >= MIN_USED).then(|| proposal_ms(&self.c, self.sound, self.b_ms).round())
    }
}

/// 적용에 필요한 최소 표본 (박 수)
pub const MIN_USED: usize = 24;

/// 실행 폴더의 judgments.csv에서 누름의 곡 시각(오프셋 A 포함 = 판정이 본 시각)을 모은다.
/// 판정 스레드·리플레이와 같게: 같은 키의 반복은 거르고, 같은 레인에 이미 눌린 키가 있으면 겹친 누름으로 거른다.
/// 돌려주는 값 = (누름 시각들, 재동기화 중에 들어온 누름 수).
pub fn presses(run_dir: &Path) -> Result<(Vec<i64>, usize), String> {
    let csv = std::fs::read_to_string(run_dir.join("judgments.csv")).map_err(|e| e.to_string())?;
    let mut down: std::collections::HashMap<u16, bool> = Default::default();
    let mut held: std::collections::HashMap<u16, u32> = Default::default();
    let (mut out, mut resync) = (Vec::new(), 0);
    for l in csv.lines().skip(1) {
        let f: Vec<&str> = l.split(',').collect();
        if f.first() != Some(&"8") || f.len() < 11 {
            continue;
        }
        let (Ok(lane), Ok(t), Ok(vkey)) = (f[1].parse::<u16>(), f[5].parse::<i64>(), f[8].parse::<u16>()) else { continue };
        let is_down = f[3] == "1";
        if down.insert(vkey, is_down).unwrap_or(false) == is_down {
            continue; // 키 반복
        }
        let h = held.entry(lane).or_insert(0);
        if is_down {
            *h += 1;
            if *h == 1 {
                out.push(t);
                resync += (f[10] == "2") as usize;
            }
        } else {
            *h = h.saturating_sub(1);
        }
    }
    Ok((out, resync))
}

fn meta_num(meta: &str, k: &str) -> Option<i64> {
    let i = meta.find(&format!("\"{k}\": "))? + k.len() + 4;
    meta[i..].split([',', '\n', '}']).next()?.trim().parse().ok()
}

/// 실행 폴더 → 보정 결과. 입력·판정 기록 유실, 재동기화 중 누름, 오디오 끊김이 있으면 적용 불가로 표시한다.
pub fn result(run_dir: &Path, sound: bool, device_key: &str, b_ms: f64) -> Result<CalibOut, String> {
    let (ps, resync) = presses(run_dir)?;
    let c = analyze(&beat_grid(2_000_000_000, BPM, BEATS), &ps, WARMUP, REJECT_NS, HIST_MS);
    let meta = std::fs::read_to_string(run_dir.join("meta.json")).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    for (k, what) in [("input_lost", "입력 유실"), ("judge_log_lost", "판정 기록 유실"), ("log_lost", "오디오 기록 유실"), ("skips", "오디오 끊김")] {
        match meta_num(&meta, k) {
            Some(0) => {}
            Some(n) => problems.push(format!("{what} {n}")),
            None => problems.push(format!("meta.json에 {k} 없음")),
        }
    }
    if resync > 0 {
        problems.push(format!("재동기화 중 누름 {resync}"));
    }
    Ok(CalibOut { c, sound, device_key: device_key.to_string(), b_ms, problems })
}

/// 보정 결과 → 새 오프셋 (ms). `sound`면 A의 새 값, 아니면 B의 새 값(`cur_b_ms` = 실행에 쓴 B 기준).
pub fn proposal_ms(c: &Calib, sound: bool, cur_b_ms: f64) -> f64 {
    let m = c.median as f64 / 1e6;
    if sound {
        -m
    } else {
        cur_b_ms + m
    }
}

pub fn describe(o: &CalibOut) -> String {
    let c = &o.c;
    let what = if o.sound { "소리로 맞추기 (A)" } else { "화면으로 맞추기 (B)" };
    if !o.problems.is_empty() {
        return format!("{what}: 적용 불가 — {} (다시 해 주세요)", o.problems.join(", "));
    }
    if c.used < MIN_USED {
        return format!("{what}: 쓸 수 있는 누름이 {}개뿐 (버림 {}, {MIN_USED}개 필요) — 다시 해 주세요", c.used, c.dropped);
    }
    format!(
        "{what} [{}]: 누름 {} (버림 {}), 중앙값 {:+.1} ms, 흔들림(MAD) {:.1} ms → 제안 {} {:+.0} ms  {}",
        o.device_key,
        c.used,
        c.dropped,
        c.median as f64 / 1e6,
        c.mad as f64 / 1e6,
        if o.sound { "A" } else { "B" },
        proposal_ms(c, o.sound, o.b_ms),
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
            csv += &format!("8,0,0,1,0,{},0,0,90,-1,0,0,0\n", t + 80_000_000); // 같은 레인의 다른 키가 겹쳐 누름: 버린다
            csv += &format!("8,0,0,0,0,{},0,0,124,-1,0,0,0\n", t + 90_000_000);
            csv += &format!("8,0,0,0,0,{},0,0,90,-1,0,0,0\n", t + 95_000_000);
        }
        std::fs::write(dir.join("judgments.csv"), &csv).unwrap();
        let meta = "{\n \"skips\": 0, \"log_lost\": 0, \"judge_log_lost\": 0, \"input_lost\": 0,\n}";
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        assert_eq!(presses(&dir).unwrap().0.len(), BEATS);
        let o = result(&dir, true, "shared:X", 0.0).unwrap();
        assert_eq!(o.c.used, BEATS - WARMUP);
        assert_eq!(o.c.median, 20_000_000);
        assert_eq!(o.proposal(), Some(-20.0));
        let o = result(&dir, false, "shared:X", 5.0).unwrap();
        assert_eq!(o.proposal(), Some(25.0));
        // 유실이 있으면 적용 불가
        std::fs::write(dir.join("meta.json"), meta.replace("\"input_lost\": 0", "\"input_lost\": 3")).unwrap();
        let o = result(&dir, true, "shared:X", 0.0).unwrap();
        assert_eq!(o.proposal(), None);
        assert!(describe(&o).contains("입력 유실 3"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
