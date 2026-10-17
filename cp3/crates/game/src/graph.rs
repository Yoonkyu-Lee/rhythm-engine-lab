//! 실시간 싱크 그래프: 채보대로 낸 소리(키음·새 자동재생)가 지금 울리는 반주 조각들과 얼마나 어긋났나.
//!
//! - x: 실제 흐른 시간 (최근 10초, 오른쪽 끝 = 지금). 어떤 방식·어떤 오류에서도 끊기거나 갈라지지 않는 유일한 시간이다.
//! - y: 어긋남 ms (+면 채보 쪽 소리가 반주보다 늦음). ±5 ms 안은 그대로, 밖은 로그로 눌러서 ±1 ms와 ±1초를 한 화면에 둔다.
//! - 선: 반주 조각(지금 울리는 2초 넘는 자동재생)마다 하나 = 채보의 대응 − 그 조각의 대응. 조각끼리 어긋나면 갈라진다.
//! - 점: 키음이 놓일 때마다 = 키음의 대응 − 가장 크게 어긋난 조각. 조각이 없으면 최근 새 자동재생 기준으로 속이 빈 점.
//! - 아래 회색 띠: 기준(반주 조각) 없음. 표본이 아예 없는 곳 = 오디오가 멈춰 소리가 나지 않은 동안.
//! - 표시: 주입한 고장(세로 띠), 콘텐츠 건너뜀, 감지한 렉, IIDX식 빨리감기(주황)·느리게감기(파랑).
//! 값은 오디오 스레드가 믹서가 놓은 자리로 계산한다 (`audio::SyncSample`). 같은 값을 `live_sync.csv`로 남긴다.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use engine::ring::Consumer;

use crate::audio::SyncSample;
use crate::render::{DrawList, Rgba};

const SPAN_S: f64 = 10.0;
const LIN_MS: f64 = 5.0;
const MAX_MS: f64 = 2000.0;
const COL_PX: f32 = 2.0;

pub struct Mark {
    pub q: i64,
    /// 띠 길이 (QPC, 0이면 세로선 하나)
    pub len_q: i64,
    pub color: Rgba,
    pub label: String,
}

pub struct LiveGraph {
    samples: VecDeque<SyncSample>,
    marks: VecDeque<Mark>,
    /// IIDX식 slew: (QPC, +1 빨리감기 / −1 느리게감기)
    slew: VecDeque<(i64, i8)>,
    pub visible: bool,
    last_skip: Option<(i64, f64)>,
    out: BufWriter<File>,
    qf: i64,
}

fn sym(ms: f64) -> f64 {
    let a = ms.abs();
    let v = if a <= LIN_MS { a / LIN_MS } else { 1.0 + (a / LIN_MS).log10() };
    v.min(1.0 + (MAX_MS / LIN_MS).log10()) * ms.signum()
}

const STEM_COLORS: [Rgba; 6] = [
    Rgba(0.35, 0.85, 0.95, 1.0),
    Rgba(0.95, 0.75, 0.30, 1.0),
    Rgba(0.75, 0.55, 0.95, 1.0),
    Rgba(0.45, 0.95, 0.55, 1.0),
    Rgba(0.95, 0.50, 0.60, 1.0),
    Rgba(0.85, 0.85, 0.55, 1.0),
];

impl LiveGraph {
    pub fn new(dir: &Path, qf: i64) -> std::io::Result<LiveGraph> {
        let mut out = BufWriter::new(File::create(dir.join("live_sync.csv"))?);
        writeln!(out, "q,key,chart_ms,n,key_ms,new_ms,stems")?;
        Ok(LiveGraph { samples: VecDeque::new(), marks: VecDeque::new(), slew: VecDeque::new(), visible: true, last_skip: None, out, qf })
    }

    /// 오디오 스레드가 보낸 표본을 모두 꺼내 기록하고, 10초 넘게 지난 것은 버린다.
    pub fn ingest(&mut self, rx: &mut Consumer<SyncSample>, now: i64) -> std::io::Result<()> {
        while let Some(s) = rx.pop() {
            let stems: Vec<String> = s.stems[..s.n as usize].iter().map(|(id, ms)| format!("{id}:{ms:.3}")).collect();
            writeln!(self.out, "{},{},{},{},{},{},{}", s.q, s.key as u8, s.chart_ms, s.n, s.key_ms, s.new_ms, stems.join(";"))?;
            self.samples.push_back(s);
        }
        let old = now - ((SPAN_S + 1.0) * self.qf as f64) as i64;
        while self.samples.front().is_some_and(|s| s.q < old) {
            self.samples.pop_front();
        }
        while self.marks.front().is_some_and(|m| m.q + m.len_q < old) {
            self.marks.pop_front();
        }
        while self.slew.front().is_some_and(|s| s.0 < old) {
            self.slew.pop_front();
        }
        Ok(())
    }

    pub fn mark(&mut self, m: Mark) {
        self.marks.push_back(m);
    }

    /// 콘텐츠 건너뜀 표시 (0.5초 안에 이어진 것은 합친다).
    pub fn skip(&mut self, q: i64, ms: f64) {
        if let Some((q0, total)) = self.last_skip {
            if q - q0 < self.qf / 2 {
                let total = total + ms;
                if let Some(m) = self.marks.iter_mut().rev().find(|m| m.q == q0) {
                    m.label = format!("+{total:.0} ms 건너뜀");
                }
                self.last_skip = Some((q0, total));
                return;
            }
        }
        self.last_skip = Some((q, ms));
        self.marks.push_back(Mark { q, len_q: 0, color: Rgba(1.0, 1.0, 1.0, 0.7), label: format!("+{ms:.0} ms 건너뜀") });
    }

    pub fn slew(&mut self, q: i64, dir: i8) {
        self.slew.push_back((q, dir));
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    pub fn draw(&self, list: &mut DrawList, now: i64, x0: f32, y0: f32, w: f32, h: f32) {
        if !self.visible {
            list.text(x0, y0, 13.0, Rgba(0.6, 0.6, 0.65, 1.0), "싱크 그래프 숨김 (G)");
            return;
        }
        let span_q = SPAN_S * self.qf as f64;
        let t0 = now as f64 - span_q;
        let xs = |q: i64| x0 + ((q as f64 - t0) / span_q) as f32 * w;
        let top = y0 + 24.0;
        let ph = h - 24.0 - 26.0; // 위 제목, 아래 띠 두 줄
        let mid = top + ph / 2.0;
        let ymax = sym(MAX_MS);
        let ys = |ms: f64| mid - (sym(ms) / ymax) as f32 * (ph / 2.0);
        // 바탕과 눈금
        list.rect(x0, top, w, ph, Rgba(0.07, 0.07, 0.10, 1.0));
        for v in [0.0, 1.0, -1.0, 5.0, -5.0, 20.0, -20.0, 100.0, -100.0, 500.0, -500.0, 2000.0, -2000.0] {
            let y = ys(v);
            let c = if v == 0.0 { Rgba(0.6, 0.6, 0.6, 0.8) } else { Rgba(0.3, 0.3, 0.35, 0.6) };
            list.rect(x0, y, w, 1.0, c);
            if v.abs() >= 1.0 || v == 0.0 {
                list.text(x0 + w + 4.0, y - 8.0, 11.0, Rgba(0.6, 0.6, 0.65, 1.0), format!("{v:+.0}"));
            }
        }
        list.text(x0, y0, 13.0, Rgba(0.85, 0.85, 0.9, 1.0), "채보대로 낸 소리 − 반주 (ms, +면 늦음) · 선: 반주 조각 · 점: 키음 · G: 숨기기");
        // 고장 띠
        for m in &self.marks {
            let (a, b) = (xs(m.q).max(x0), xs(m.q + m.len_q).min(x0 + w));
            if b < x0 || a > x0 + w {
                continue;
            }
            list.rect(a, top, (b - a).max(2.0), ph, m.color);
            if !m.label.is_empty() {
                let ly = if m.len_q > 0 { top + 2.0 } else { top + 16.0 };
                list.text(a + 3.0, ly, 11.0, Rgba(m.color.0, m.color.1, m.color.2, 1.0), m.label.as_str());
            }
        }
        // 선(반주 조각마다)과 점(키음): 2픽셀 칸마다 조각별 최소·최대를 세로 막대 하나로
        let ncol = (w / COL_PX) as usize;
        let mut cur_col = usize::MAX;
        let mut acc: Vec<(i64, f32, f32)> = Vec::with_capacity(16);
        let mut has_any = vec![false; ncol + 1];
        let mut has_ref = vec![false; ncol + 1];
        let flush_col = |list: &mut DrawList, col: usize, acc: &mut Vec<(i64, f32, f32)>| {
            let x = x0 + col as f32 * COL_PX;
            for &(id, lo, hi) in acc.iter() {
                let c = STEM_COLORS[(id.unsigned_abs() as usize / 7) % STEM_COLORS.len()];
                let (ya, yb) = (ys(hi as f64), ys(lo as f64));
                list.rect(x, ya - 1.0, COL_PX, (yb - ya).max(0.0) + 2.0, c);
            }
            acc.clear();
        };
        for s in &self.samples {
            if (s.q as f64) < t0 {
                continue;
            }
            let col = (((s.q as f64 - t0) / span_q) * ncol as f64) as usize;
            if col > ncol {
                continue;
            }
            if col != cur_col {
                if cur_col != usize::MAX {
                    flush_col(list, cur_col, &mut acc);
                }
                cur_col = col;
            }
            has_any[col] = true;
            if s.n > 0 {
                has_ref[col] = true;
            }
            if s.chart_ms.is_finite() {
                for &(id, ms) in &s.stems[..s.n as usize] {
                    let v = s.chart_ms - ms;
                    match acc.iter_mut().find(|e| e.0 == id) {
                        Some(e) => {
                            e.1 = e.1.min(v);
                            e.2 = e.2.max(v);
                        }
                        None => acc.push((id, v, v)),
                    }
                }
            }
        }
        if cur_col != usize::MAX {
            flush_col(list, cur_col, &mut acc);
        }
        for s in self.samples.iter().filter(|s| s.key && s.q as f64 >= t0) {
            let x = xs(s.q);
            let worst = s.stems[..s.n as usize].iter().map(|&(_, m)| s.key_ms - m).fold(f32::NAN, |a: f32, b| if a.is_nan() || b.abs() > a.abs() { b } else { a });
            if worst.is_finite() {
                let y = ys(worst as f64);
                list.rect(x - 3.0, y - 3.0, 6.0, 6.0, Rgba(1.0, 1.0, 1.0, 1.0));
            } else if s.new_ms.is_finite() {
                let y = ys((s.key_ms - s.new_ms) as f64);
                let c = Rgba(1.0, 1.0, 1.0, 1.0);
                list.rect(x - 3.0, y - 3.0, 6.0, 1.0, c);
                list.rect(x - 3.0, y + 2.0, 6.0, 1.0, c);
                list.rect(x - 3.0, y - 3.0, 1.0, 6.0, c);
                list.rect(x + 2.0, y - 3.0, 1.0, 6.0, c);
            }
        }
        // 아래 띠 1: 기준 없음(회색) / 소리 없음(빈 칸). 띠 2: IIDX식 빨리감기·느리게감기
        let by = top + ph + 4.0;
        for col in 0..ncol {
            if has_any[col] && !has_ref[col] {
                list.rect(x0 + col as f32 * COL_PX, by, COL_PX, 8.0, Rgba(0.5, 0.5, 0.5, 0.9));
            }
        }
        for &(q, d) in &self.slew {
            let x = xs(q);
            if x >= x0 && x <= x0 + w {
                let c = if d > 0 { Rgba(1.0, 0.6, 0.2, 1.0) } else { Rgba(0.3, 0.6, 1.0, 1.0) };
                list.rect(x, by + 12.0, 2.0, 8.0, c);
            }
        }
        list.text(x0, by + 22.0 - 2.0, 11.0, Rgba(0.6, 0.6, 0.65, 1.0), "회색: 기준(반주 조각) 없음 · 주황/파랑: IIDX식 빨리/느리게감기");
    }
}
