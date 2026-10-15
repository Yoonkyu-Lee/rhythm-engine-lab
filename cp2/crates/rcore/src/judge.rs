//! 판정: 순수 함수. 같은 입력 순서와 같은 watermark 순서면 항상 같은 결과다 (리플레이 재현의 근거).
//!
//! - 시각은 모두 정규 곡 시간 ns. 입력 시각을 곡 시간으로 바꾸는 일(청취 곡 시간 H(q))은 호출하는 쪽이 한다.
//! - 노트 고르기(P7): 아직 판정하지 않은 노트 중 가장 이른 것이 창 안에 있으면 그것.
//! - MISS 확정: watermark(이 시각 이전의 입력은 모두 받았다는 표시)가 노트의 늦은 창 끝을 지나야 확정한다 (설계안 7장).
//!   입력이 늦게 도착해도(입력 스레드 정지 등) watermark가 멈춰 있으면 MISS로 바꾸지 않는다.
//! - 빈 누름(창 안에 노트 없음): 판정 없이, 다음 노트의 소리를 울린다 (키음이 비지 않게).
//! - 롱노트: 머리는 단노트처럼, 끝은 떼는 시각이 (끝 − GOOD) 이후거나 계속 누르고 있으면 성공, 너무 일찍 떼면 끊김.
//! 판정 창 값은 CP2 임시 고정값이다 (CP3 기본형에서 정한다).

use crate::runtime::Note;

/// 대칭 창의 반폭 (ns). 바깥 BAD를 넘으면 그 노트를 고르지 않는다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Windows {
    pub perfect: i64,
    pub great: i64,
    pub good: i64,
    pub bad: i64,
}

impl Windows {
    pub const CP2: Windows = Windows { perfect: 20_000_000, great: 50_000_000, good: 100_000_000, bad: 150_000_000 };

    pub fn grade(&self, err: i64) -> Option<Grade> {
        let a = err.abs();
        if a <= self.perfect {
            Some(Grade::Perfect)
        } else if a <= self.great {
            Some(Grade::Great)
        } else if a <= self.good {
            Some(Grade::Good)
        } else if a <= self.bad {
            Some(Grade::Bad)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Grade {
    Perfect,
    Great,
    Good,
    Bad,
    Miss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judged {
    /// 노트(머리)를 쳤다. `err` = 입력 시각 − 노트 시각.
    Hit { note: u32, grade: Grade, err: i64 },
    /// 창 안에 노트가 없는 누름. 울릴 소리는 `sound_note`(다음 노트)의 것.
    Empty { t: i64, sound_note: Option<u32> },
    /// 늦은 창이 지나도록 치지 않았다 (watermark로 확정).
    Miss { note: u32 },
    /// 롱노트 끝: 성공(제때 떼거나 끝까지 누름) 또는 끊김(너무 일찍 뗌).
    LongEnd { note: u32, ok: bool, err: i64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lane {
    /// 아직 판정하지 않은 첫 노트.
    next: usize,
    /// 누르고 있는 롱노트.
    holding: Option<usize>,
    pressed: bool,
}

impl Lane {
    pub fn new() -> Lane {
        Lane::default()
    }

    pub fn next_note(&self) -> usize {
        self.next
    }

    /// 누름. 결과 하나를 낸다 (울릴 소리: Hit → 그 노트, Empty → sound_note).
    pub fn press(&mut self, notes: &[Note], w: &Windows, t: i64) -> Judged {
        self.pressed = true;
        if let Some(i) = notes.get(self.next).map(|_| self.next) {
            let n = notes[i];
            if let Some(grade) = w.grade(t - n.t) {
                self.next += 1;
                if n.end > n.t {
                    self.holding = Some(i);
                }
                return Judged::Hit { note: i as u32, grade, err: t - n.t };
            }
        }
        Judged::Empty { t, sound_note: (self.next < notes.len()).then_some(self.next as u32) }
    }

    /// 뗌. 롱노트를 누르고 있었으면 끝 판정을 낸다.
    pub fn release(&mut self, notes: &[Note], w: &Windows, t: i64) -> Option<Judged> {
        self.pressed = false;
        let i = self.holding.take()?;
        let n = notes[i];
        let ok = t >= n.end - w.good;
        Some(Judged::LongEnd { note: i as u32, ok, err: t - n.end })
    }

    /// watermark 진행: 늦은 창이 지난 노트를 MISS로, 끝까지 누른 롱노트를 성공으로 확정한다. 결과는 `out`에 시간순으로.
    pub fn advance(&mut self, notes: &[Note], w: &Windows, watermark: i64, out: &mut impl FnMut(Judged)) {
        if let Some(i) = self.holding {
            if watermark > notes[i].end {
                self.holding = None;
                out(Judged::LongEnd { note: i as u32, ok: true, err: 0 });
            }
        }
        while self.next < notes.len() && watermark > notes[self.next].t + w.bad {
            out(Judged::Miss { note: self.next as u32 });
            self.next += 1;
        }
    }
}

/// 판정 결과 집계.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub perfect: u32,
    pub great: u32,
    pub good: u32,
    pub bad: u32,
    pub miss: u32,
    pub empty: u32,
    pub ln_ok: u32,
    pub ln_break: u32,
    /// 무효 처리한 노트 (재동기화 중의 판정: 점수·콤보에 넣지 않는다, CP2 2-5).
    pub void: u32,
    pub combo: u32,
    pub max_combo: u32,
}

impl Tally {
    pub fn add(&mut self, j: &Judged) {
        match *j {
            Judged::Hit { grade, .. } => {
                match grade {
                    Grade::Perfect => self.perfect += 1,
                    Grade::Great => self.great += 1,
                    Grade::Good => self.good += 1,
                    Grade::Bad => self.bad += 1,
                    Grade::Miss => self.miss += 1,
                }
                if matches!(grade, Grade::Bad | Grade::Miss) {
                    self.combo = 0;
                } else {
                    self.combo += 1;
                }
            }
            Judged::Miss { .. } => {
                self.miss += 1;
                self.combo = 0;
            }
            Judged::Empty { .. } => self.empty += 1,
            Judged::LongEnd { ok, .. } => {
                if ok {
                    self.ln_ok += 1;
                } else {
                    self.ln_break += 1;
                    self.combo = 0;
                }
            }
        }
        self.max_combo = self.max_combo.max(self.combo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    fn notes() -> Vec<Note> {
        vec![
            Note { t: 1000 * MS, end: 1000 * MS, sounds_start: 0, sounds_len: 1 },
            Note { t: 1500 * MS, end: 2500 * MS, sounds_start: 1, sounds_len: 1 },
            Note { t: 3000 * MS, end: 3000 * MS, sounds_start: 2, sounds_len: 1 },
        ]
    }

    #[test]
    fn grades_and_ln() {
        let (n, w) = (notes(), Windows::CP2);
        let mut l = Lane::new();
        assert_eq!(l.press(&n, &w, 1012 * MS), Judged::Hit { note: 0, grade: Grade::Perfect, err: 12 * MS });
        l.release(&n, &w, 1050 * MS);
        assert_eq!(l.press(&n, &w, 1460 * MS), Judged::Hit { note: 1, grade: Grade::Great, err: -40 * MS });
        // 롱노트를 끝 400 ms 전에 떼면 끊김
        assert_eq!(l.release(&n, &w, 2100 * MS), Some(Judged::LongEnd { note: 1, ok: false, err: -400 * MS }));
        // 창 밖 누름은 빈 누름, 다음 노트(2)의 소리
        assert_eq!(l.press(&n, &w, 2700 * MS), Judged::Empty { t: 2700 * MS, sound_note: Some(2) });
    }

    #[test]
    fn miss_waits_for_watermark() {
        let (n, w) = (notes(), Windows::CP2);
        let mut l = Lane::new();
        let mut out = vec![];
        // watermark가 노트 0의 늦은 창 끝(1150 ms)을 넘지 않으면 MISS가 아니다
        l.advance(&n, &w, 1149 * MS, &mut |j| out.push(j));
        assert!(out.is_empty());
        // 늦게 도착한 입력(1140 ms)은 아직 판정할 수 있다 → BAD
        assert_eq!(l.press(&n, &w, 1140 * MS), Judged::Hit { note: 0, grade: Grade::Bad, err: 140 * MS });
        l.advance(&n, &w, 1700 * MS, &mut |j| out.push(j));
        assert_eq!(out, vec![Judged::Miss { note: 1 }]);
    }

    #[test]
    fn ln_held_to_end_completes_on_watermark() {
        let (n, w) = (notes(), Windows::CP2);
        let mut l = Lane::new();
        let mut out = vec![];
        l.advance(&n, &w, 1000 * MS, &mut |j| out.push(j));
        l.press(&n, &w, 1000 * MS);
        l.release(&n, &w, 1010 * MS);
        l.press(&n, &w, 1500 * MS);
        l.advance(&n, &w, 2600 * MS, &mut |j| out.push(j));
        assert_eq!(out, vec![Judged::LongEnd { note: 1, ok: true, err: 0 }]);
        let mut t = Tally::default();
        t.add(&Judged::Hit { note: 0, grade: Grade::Perfect, err: 0 });
        t.add(&Judged::Miss { note: 1 });
        t.add(&Judged::Hit { note: 2, grade: Grade::Great, err: 0 });
        assert_eq!((t.perfect, t.great, t.miss, t.combo, t.max_combo), (1, 1, 1, 1, 1));
    }
}
