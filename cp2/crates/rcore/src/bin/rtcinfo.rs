//! `.rtc` 파일을 읽어 검증·컴파일하고 요약을 출력한다.
//!   rtcinfo <파일.rtc> [...]

use rcore::{rtc, runtime};

fn main() {
    let mut fail = false;
    for path in std::env::args().skip(1) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: 읽기 실패: {e}");
                fail = true;
                continue;
            }
        };
        let r = rtc::decode(&bytes).map_err(|e| e.to_string()).and_then(|c| runtime::compile(c).map_err(|e| e.to_string()));
        match r {
            Ok(c) => {
                let s = &c.stats;
                let b = &c.chart.bank;
                println!(
                    "{path}: \"{}\" 해시 {:016x}\n  뱅크 {} Hz {}ch, 소리 {}, {:.1}초 분량 ({:.1} MB)\n  자동재생 {}, 레인 {}, 노트 {} (롱 {}), 끝 {:.3}초\n  동시 발음 최대 {} → 보이스 예산 {}",
                    c.chart.title,
                    c.hash,
                    b.rate,
                    b.channels,
                    s.sounds,
                    s.bank_frames as f64 / b.rate as f64,
                    b.pcm.len() as f64 * 4.0 / 1e6,
                    s.auto_events,
                    c.chart.lanes.len(),
                    s.notes,
                    s.long_notes,
                    c.chart.end as f64 / 1e9,
                    s.peak_voices,
                    c.voice_budget
                );
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                fail = true;
            }
        }
    }
    if fail {
        std::process::exit(1);
    }
}
