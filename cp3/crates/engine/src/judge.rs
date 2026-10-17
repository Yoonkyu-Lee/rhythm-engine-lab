//! 판정·게이지·점수: 순수 함수. 같은 입력 순서면 항상 같은 결과다 (리플레이 재현의 근거, CP2와 같음).
//!
//! CP3 규칙은 IIDX식 기본값 하나다(README 9장 5, cp0 0-9). 엔진 안에서는 **데이터**(`Rules`: 등급별 빠름·늦음 경계 ns, 게이지 정책·증감표)로
//! 다뤄 CP4의 규칙 커스텀·프리셋이 코드 수정 없이 얹히게 한다(리뷰 3-3 #13). IIDX 값은 `Rules::iidx`가 만드는 프리셋이다.
//! - 판정 창(1프레임 = 1/60초): PGREAT ±1, GREAT ±2, GOOD ±7, BAD ±17 프레임. 스크래치 레인은 GREAT·GOOD·BAD가 2프레임 넓다.
//!   IIDX는 오차를 화면 틱 단위로 재지만 우리는 ns로 잰다(경계만 같게). 빠름·늦음 경계는 따로 둘 수 있다.
//! - 노트 고르기: 그 레인의 아직 판정 안 한 노트 중 오차(입력 − 노트)가 [−빈 POOR 범위, +늦은 BAD]인 것에서 |오차|가 가장 작은 것
//!   (같으면 이른 것). 그 노트가 빠른 BAD보다 이르면 빈 POOR(노트는 남는다). 후보가 없으면 판정 없는 빈 누름.
//! - 롱노트: 머리는 단노트처럼. LN(BMS)은 끝 판정이 없고 끝 − 빠른 GOOD보다 일찍 떼면 끊김(POOR). CN·HCN(HCN은 CP3에서 CN과 같음 ⚑)은
//!   뗄 때 끝 판정(같은 창), 끝 − 빠른 BAD보다 일찍 떼면 POOR, 끝 + 늦은 BAD가 지나도록 누르고 있으면 BAD (⚑ 원작 확인 전 임시).
//! - 지뢰: 지뢰 시각 ±GREAT 안에서 누르거나, 지뢰 시각에 누르고 있으면 터짐 (⚑ beatoraja식 임시). 피해 = 노트의 damage(게이지 %의 1/1000) 또는 BAD와 같음.
//! - **확정 순서 (리뷰 3-3 #3·#6)**: 입력(누름·뗌)마다 먼저 **모든 레인**을 그 입력 시각까지 확정하고, watermark마다 모든 레인을 그 시각까지
//!   확정한다. 한 번에 확정되는 결과는 **확정 가능해진 시각 → 레인 → 노트** 순으로 정렬해 집계한다(놓침 = 노트 + 늦은 BAD, 끝까지 누른
//!   롱노트 = 끝(+ 늦은 BAD), 누른 채 지난 지뢰 = 지뢰 시각). 확정 가능해진 시각은 watermark보다 작아야 확정되므로, watermark를 어떻게
//!   나눠 진행하든 집계 순서가 같다 → 실시간과 리플레이가 같은 순서로 같은 점수·게이지를 낸다.
//! - 게이지(5000 = 100%, `GaugeRule`): 그루브형(NORMAL·EASY·ASSIST)은 22% 시작, 회복 = floor(T × 5000 / (6N′)) (N′ = N, 350 이상이면
//!   350 + (N−350)/3), 피해 BAD·빈 POOR 100, POOR 300(EASY류 ×4/5 0 쪽 버림), 2% 바닥. 생존형(HARD·EX-HARD)은 100% 시작 +8, 피해 250·450,
//!   HARD는 1666 미만이면 피해 절반, EX-HARD 피해 ×2·완화 없음, **0 이하에서 탈락**(리뷰 3-3 #5). N = 판정 대상 수(CN 롱노트 = 2) ⚑ 원작 N+1 여부.
//! - EX 점수 = 2 × PGREAT + GREAT. 콤보: PG·GR·GD 이어짐, BAD·POOR 끊김, 빈 POOR는 콤보 유지.

use crate::runtime::{Chart, EmptyLane, Lane, LongRule, Note, NoteKind, Role};

/// 빈 누름(판정할 노트를 못 고른 누름)에 울릴 소리.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptySound {
    /// 이 노트의 소리들
    Note(u32),
    /// 빈 레인 소리 (차분 안 소리 번호)
    Sound(u32),
    None,
}

/// 빈 누름 소리 (명세 2-3 G5): 시각 `t` 이전의 마지막 빈 레인 소리가 그 레인의 마지막 노트(시각 ≤ t)보다 나중이면 그 소리,
/// 아니면 차분 규칙 — previous_note: 시각 ≤ t인 마지막 노트(없으면 다음 노트), next_note: 시각 > t인 첫 노트, none: 무음. 지뢰는 건너뛴다.
pub fn empty_sound(lane: &Lane, rule: EmptyLane, t: i64) -> EmptySound {
    let notes = &lane.notes;
    let k = notes.partition_point(|n| n.t <= t);
    let prev = notes[..k].iter().rposition(|n| n.kind != NoteKind::Mine);
    let next = notes[k..].iter().position(|n| n.kind != NoteKind::Mine).map(|i| i + k);
    let ls = lane.lane_sounds.partition_point(|x| x.t <= t);
    if ls > 0 {
        let x = lane.lane_sounds[ls - 1];
        if prev.map_or(true, |p| x.t >= notes[p].t) {
            return EmptySound::Sound(x.sound);
        }
    }
    let pick = match rule {
        EmptyLane::PreviousNote => prev.or(next),
        EmptyLane::NextNote => next,
        EmptyLane::None => None,
    };
    pick.map_or(EmptySound::None, |i| EmptySound::Note(i as u32))
}

pub const MS: i64 = 1_000_000;
/// 1/60초 프레임 (ns, 반올림)
pub const FRAME: i64 = 16_666_667;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Grade {
    PGreat,
    Great,
    Good,
    Bad,
    Poor,
}

/// 판정 창: 등급(PGREAT·GREAT·GOOD·BAD)별 빠름·늦음 경계 (ns, 양수). 경계 위는 안쪽 등급.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win {
    pub early: [i64; 4],
    pub late: [i64; 4],
}

impl Win {
    pub fn symmetric(w: [i64; 4]) -> Win {
        Win { early: w, late: w }
    }
    pub fn grade(&self, err: i64) -> Option<Grade> {
        let (b, a) = if err < 0 { (&self.early, -err) } else { (&self.late, err) };
        [Grade::PGreat, Grade::Great, Grade::Good, Grade::Bad].into_iter().zip(b.iter()).find(|(_, w)| a <= **w).map(|(g, _)| g)
    }
    pub fn bad_late(&self) -> i64 {
        self.late[3]
    }
    pub fn bad_early(&self) -> i64 {
        self.early[3]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GaugeKind {
    Normal,
    AssistEasy,
    Easy,
    Hard,
    ExHard,
}

/// 게이지 정책 (데이터). 값 단위는 5000 = 100%.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GaugeRule {
    pub start: i32,
    /// 그루브형 바닥 (생존형은 0 이하에서 탈락)
    pub floor: i32,
    pub cap: i32,
    /// 클리어 기준 (생존형은 탈락하지 않으면 클리어)
    pub clear: i32,
    pub survival: bool,
    /// 회복 [PG·GR, GOOD]: `per_note`면 floor(T × cap / (6N′)), 아니면 고정 증가량
    pub rec: [i32; 2],
    pub rec_per_note: bool,
    /// 피해 [BAD·빈 POOR, POOR] (양수)
    pub dmg: [i32; 2],
    /// 피해 배율 (분자, 분모; 0 쪽 버림)
    pub dmg_mul: (i32, i32),
    /// 이 값 미만이면 피해 × (분자/분모) — HARD의 저게이지 완화
    pub low: Option<(i32, i32, i32)>,
}

impl GaugeRule {
    pub fn iidx(kind: GaugeKind) -> GaugeRule {
        let groove = GaugeRule { start: 1100, floor: 100, cap: 5000, clear: 4000, survival: false, rec: [16, 8], rec_per_note: true, dmg: [100, 300], dmg_mul: (1, 1), low: None };
        let surv = GaugeRule { start: 5000, floor: 0, cap: 5000, clear: 1, survival: true, rec: [8, 0], rec_per_note: false, dmg: [250, 450], dmg_mul: (1, 1), low: None };
        match kind {
            GaugeKind::Normal => groove,
            GaugeKind::Easy => GaugeRule { dmg_mul: (4, 5), ..groove },
            GaugeKind::AssistEasy => GaugeRule { dmg_mul: (4, 5), clear: 3000, ..groove },
            GaugeKind::Hard => GaugeRule { low: Some((1666, 1, 2)), ..surv },
            GaugeKind::ExHard => GaugeRule { dmg_mul: (2, 1), ..surv },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub key: Win,
    pub scratch: Win,
    /// 빈 POOR 후보 범위 (입력보다 이 시간 안쪽 뒤에 있는 노트까지)
    pub empty_poor: i64,
    /// 게이지 이름 (표시·기록용)
    pub gauge_kind: GaugeKind,
    pub gauge: GaugeRule,
}

impl Rules {
    pub fn iidx(gauge: GaugeKind) -> Rules {
        Rules {
            key: Win::symmetric([FRAME, 2 * FRAME, 7 * FRAME, 17 * FRAME]),
            scratch: Win::symmetric([FRAME, 4 * FRAME, 9 * FRAME, 19 * FRAME]),
            empty_poor: 350 * MS,
            gauge_kind: gauge,
            gauge: GaugeRule::iidx(gauge),
        }
    }

    pub fn win_for(&self, role: Role) -> Win {
        if role == Role::Scratch {
            self.scratch
        } else {
            self.key
        }
    }

    /// 규칙 해시용 정규 바이트열 (3-4: 판정에 영향을 주는 값만, 고정 순서 little-endian)
    pub fn canonical_bytes(&self, long_rule: LongRule) -> Vec<u8> {
        let mut v = Vec::new();
        for w in [self.key, self.scratch] {
            w.early.iter().chain(w.late.iter()).for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        }
        v.extend_from_slice(&self.empty_poor.to_le_bytes());
        let g = self.gauge;
        for x in [g.start, g.floor, g.cap, g.clear, g.survival as i32, g.rec[0], g.rec[1], g.rec_per_note as i32, g.dmg[0], g.dmg[1], g.dmg_mul.0, g.dmg_mul.1] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        let (a, b, c) = g.low.unwrap_or((0, 1, 1));
        for x in [g.low.is_some() as i32, a, b, c, long_rule as i32] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judged {
    /// 노트(머리)를 쳤다. `err` = 입력 − 노트.
    Hit { note: u32, grade: Grade, err: i64 },
    /// 노트를 고르지 못한 누름. `poor`면 빈 POOR(빠른 BAD보다 이른 노트가 있었음).
    Empty { t: i64, poor: bool },
    /// 놓침 POOR (확정).
    Miss { note: u32 },
    /// 롱노트 끝. LN은 `grade`가 None(성공)이거나 Poor(끊김), CN은 끝 판정 등급.
    LongEnd { note: u32, grade: Option<Grade>, err: i64 },
    /// 지뢰가 터졌다.
    Mine { note: u32, damage: i32 },
}

/// 레인 하나의 판정 상태.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneJudge {
    done: Vec<bool>,
    /// 이 번호 앞의 노트는 모두 판정됨(놓침 포함)
    first_open: usize,
    holding: Option<usize>,
    down: bool,
    mine_cursor: usize,
}

impl LaneJudge {
    pub fn new(n: usize) -> LaneJudge {
        LaneJudge { done: vec![false; n], ..Default::default() }
    }
    pub fn is_down(&self) -> bool {
        self.down
    }
    pub fn holding(&self) -> Option<usize> {
        self.holding
    }
    pub fn is_done(&self, i: usize) -> bool {
        self.done[i]
    }
    pub fn first_open(&self) -> usize {
        self.first_open
    }

    /// 누름. 결과를 `out`에 (지뢰 → 노트 순).
    pub fn press(&mut self, notes: &[Note], w: &Win, r: &Rules, t: i64, out: &mut impl FnMut(Judged)) {
        self.down = true;
        for (i, n) in notes.iter().enumerate().skip(self.first_open) {
            if n.t > t + w.early[1] {
                break;
            }
            let e = t - n.t;
            if n.kind == NoteKind::Mine && !self.done[i] && e >= -w.early[1] && e <= w.late[1] {
                self.done[i] = true;
                out(Judged::Mine { note: i as u32, damage: n.damage });
            }
        }
        let mut best: Option<(usize, i64)> = None;
        for (i, n) in notes.iter().enumerate().skip(self.first_open) {
            let err = t - n.t;
            if err < -r.empty_poor {
                break;
            }
            if self.done[i] || n.kind == NoteKind::Mine || err > w.bad_late() {
                continue;
            }
            if best.map_or(true, |(_, e)| err.abs() < e.abs()) {
                best = Some((i, err));
            }
        }
        match best {
            Some((i, err)) => match w.grade(err) {
                Some(grade) => {
                    self.done[i] = true;
                    if notes[i].kind == NoteKind::Long {
                        self.holding = Some(i);
                    }
                    self.bump();
                    out(Judged::Hit { note: i as u32, grade, err });
                }
                None => out(Judged::Empty { t, poor: true }),
            },
            None => out(Judged::Empty { t, poor: false }),
        }
    }

    /// 뗌. 롱노트를 누르고 있었으면 끝 판정.
    pub fn release(&mut self, notes: &[Note], w: &Win, rule: LongRule, t: i64) -> Option<Judged> {
        self.down = false;
        let i = self.holding.take()?;
        let err = t - notes[i].end;
        let grade = match rule {
            LongRule::Ln => (err < -w.early[2]).then_some(Grade::Poor),
            LongRule::Cn | LongRule::Hcn => Some(if err < -w.bad_early() { Grade::Poor } else { w.grade(err).unwrap_or(Grade::Bad) }),
        };
        Some(Judged::LongEnd { note: i as u32, grade, err })
    }

    fn bump(&mut self) {
        while self.first_open < self.done.len() && self.done[self.first_open] {
            self.first_open += 1;
        }
    }

    /// 시각 `upto`까지 확정: 끝까지 누른 롱노트, 누르고 있는 동안 지난 지뢰, 놓친 노트. `out(확정 가능해진 시각, 결과)`.
    pub fn advance(&mut self, notes: &[Note], w: &Win, rule: LongRule, upto: i64, out: &mut impl FnMut(i64, Judged)) {
        if let Some(i) = self.holding {
            let n = notes[i];
            let limit = if rule == LongRule::Ln { n.end } else { n.end + w.bad_late() };
            if upto > limit {
                self.holding = None;
                let grade = if rule == LongRule::Ln { None } else { Some(Grade::Bad) };
                out(limit, Judged::LongEnd { note: i as u32, grade, err: limit - n.end });
            }
        }
        while self.mine_cursor < notes.len() && notes[self.mine_cursor].t < upto {
            let i = self.mine_cursor;
            if notes[i].kind == NoteKind::Mine && !self.done[i] && self.down {
                self.done[i] = true;
                out(notes[i].t, Judged::Mine { note: i as u32, damage: notes[i].damage });
            }
            self.mine_cursor += 1;
        }
        for i in self.first_open..notes.len() {
            let n = notes[i];
            if upto <= n.t + w.bad_late() {
                break;
            }
            if !self.done[i] {
                self.done[i] = true;
                if n.kind != NoteKind::Mine {
                    out(n.t + w.bad_late(), Judged::Miss { note: i as u32 });
                }
            }
        }
        self.bump();
    }
}

/// 여러 레인 판정기: 판정 스레드와 리플레이가 같은 순서 규칙(모듈 머리 주석)을 쓰는 하나의 코드.
/// 실시간 스레드에서 할당하지 않도록 확정 결과 버퍼를 미리 잡는다(노트 수 + 여유, 리뷰 3-3 #4).
pub struct Judge {
    lanes: Vec<LaneJudge>,
    wins: Vec<Win>,
    rule: LongRule,
    pub rules: Rules,
    buf: Vec<(i64, usize, Judged)>,
}

impl Judge {
    pub fn new(c: &Chart, rules: Rules) -> Judge {
        let total: usize = c.lanes.iter().map(|l| l.notes.len()).sum();
        Judge {
            lanes: c.lanes.iter().map(|l| LaneJudge::new(l.notes.len())).collect(),
            wins: c.lanes.iter().map(|l| rules.win_for(l.role)).collect(),
            rule: c.long_rule,
            rules,
            buf: Vec::with_capacity(total + 64),
        }
    }

    pub fn lane(&self, i: usize) -> &LaneJudge {
        &self.lanes[i]
    }

    /// 모든 레인을 `t`까지 확정하고, (확정 가능해진 시각, 레인, 노트) 순으로 `out(레인, 결과)`.
    pub fn advance(&mut self, c: &Chart, t: i64, out: &mut impl FnMut(usize, Judged)) {
        let buf = &mut self.buf;
        for (li, l) in self.lanes.iter_mut().enumerate() {
            l.advance(&c.lanes[li].notes, &self.wins[li], self.rule, t, &mut |k, j| {
                if buf.len() < buf.capacity() {
                    buf.push((k, li, j));
                }
            });
        }
        buf.sort_unstable_by_key(|&(k, li, j)| {
            let n = match j {
                Judged::Miss { note } | Judged::LongEnd { note, .. } | Judged::Mine { note, .. } | Judged::Hit { note, .. } => note,
                Judged::Empty { .. } => u32::MAX,
            };
            (k, li, n)
        });
        for &(_, li, j) in buf.iter() {
            out(li, j);
        }
        buf.clear();
    }

    /// 누름: 먼저 모든 레인을 `t`까지 확정한 뒤 그 레인의 누름 판정.
    pub fn press(&mut self, c: &Chart, lane: usize, t: i64, out: &mut impl FnMut(usize, Judged)) {
        self.advance(c, t, out);
        let (w, r) = (self.wins[lane], self.rules);
        self.lanes[lane].press(&c.lanes[lane].notes, &w, &r, t, &mut |j| out(lane, j));
    }

    /// 뗌: 먼저 모든 레인을 `t`까지 확정한 뒤 그 레인의 뗌 판정. 롱노트가 아니었으면 결과 없음(false).
    pub fn release(&mut self, c: &Chart, lane: usize, t: i64, out: &mut impl FnMut(usize, Judged)) -> bool {
        self.advance(c, t, out);
        let w = self.wins[lane];
        match self.lanes[lane].release(&c.lanes[lane].notes, &w, self.rule, t) {
            Some(j) => {
                out(lane, j);
                true
            }
            None => false,
        }
    }
}

/// 게이지 (5000 = 100%).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gauge {
    pub rule: GaugeRule,
    pub value: i32,
    /// 판정 대상 수 N에서 정한 회복량 (PG·GR, GOOD)
    rec: [i32; 2],
    pub failed: bool,
}

impl Gauge {
    pub fn new(rule: GaugeRule, n: u32) -> Gauge {
        let rec = if rule.rec_per_note {
            let n = n.max(1) as i64;
            let np = if n < 350 { n } else { 350 + (n - 350) / 3 };
            [(rule.rec[0] as i64 * rule.cap as i64 / (6 * np)) as i32, (rule.rec[1] as i64 * rule.cap as i64 / (6 * np)) as i32]
        } else {
            rule.rec
        };
        Gauge { rule, value: rule.start, rec, failed: false }
    }

    pub fn survival(&self) -> bool {
        self.rule.survival
    }

    /// 판정 종류: 0 PG·GR, 1 GOOD, 2 BAD·빈 POOR, 3 POOR. 지뢰는 `mine`(게이지 %의 1/1000, 음수면 BAD와 같음).
    pub fn apply(&mut self, kind: usize, mine: Option<i32>) {
        if self.failed {
            return;
        }
        let r = self.rule;
        let mut d: i32 = match kind {
            0 | 1 => self.rec[kind],
            2 => -r.dmg[0],
            _ => -r.dmg[1],
        };
        if let Some(m) = mine {
            d = if m >= 0 { -(m as i64 * r.cap as i64 / 100_000) as i32 } else { -r.dmg[0] };
        }
        if d < 0 {
            d = d * r.dmg_mul.0 / r.dmg_mul.1;
            if let Some((th, num, den)) = r.low {
                if self.value < th {
                    d = d * num / den;
                }
            }
        }
        self.value = (self.value + d).min(r.cap);
        if r.survival {
            if self.value <= 0 {
                self.value = 0;
                self.failed = true;
            }
        } else if self.value < r.floor {
            self.value = r.floor;
        }
    }

    pub fn cleared(&self) -> bool {
        if self.rule.survival {
            !self.failed && self.value > 0
        } else {
            self.value >= self.rule.clear
        }
    }

    /// 표시 % (2% 눈금)
    pub fn percent(&self) -> i32 {
        2 * (self.value / (self.rule.cap / 50).max(1))
    }
}

/// 판정 집계·EX 점수·콤보·게이지.
#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub pgreat: u32,
    pub great: u32,
    pub good: u32,
    pub bad: u32,
    pub poor: u32,
    pub empty_poor: u32,
    pub mines: u32,
    pub combo: u32,
    pub max_combo: u32,
    /// 빠름·늦음 (PGREAT 제외, IIDX식)
    pub fast: u32,
    pub slow: u32,
    pub gauge: Gauge,
}

impl Score {
    pub fn new(rule: GaugeRule, judge_count: u32) -> Score {
        Score { pgreat: 0, great: 0, good: 0, bad: 0, poor: 0, empty_poor: 0, mines: 0, combo: 0, max_combo: 0, fast: 0, slow: 0, gauge: Gauge::new(rule, judge_count) }
    }

    pub fn ex(&self) -> u32 {
        2 * self.pgreat + self.great
    }

    fn grade(&mut self, g: Grade, err: i64) {
        match g {
            Grade::PGreat => self.pgreat += 1,
            Grade::Great => self.great += 1,
            Grade::Good => self.good += 1,
            Grade::Bad => self.bad += 1,
            Grade::Poor => self.poor += 1,
        }
        if g != Grade::PGreat && g != Grade::Poor {
            if err < 0 {
                self.fast += 1;
            } else if err > 0 {
                self.slow += 1;
            }
        }
        if matches!(g, Grade::Bad | Grade::Poor) {
            self.combo = 0;
        } else {
            self.combo += 1;
        }
        self.max_combo = self.max_combo.max(self.combo);
        self.gauge.apply(
            match g {
                Grade::PGreat | Grade::Great => 0,
                Grade::Good => 1,
                Grade::Bad => 2,
                Grade::Poor => 3,
            },
            None,
        );
    }

    pub fn add(&mut self, j: &Judged) {
        match *j {
            Judged::Hit { grade, err, .. } => self.grade(grade, err),
            Judged::Miss { .. } => self.grade(Grade::Poor, 0),
            Judged::Empty { poor: true, .. } => {
                self.empty_poor += 1;
                self.gauge.apply(2, None);
            }
            Judged::Empty { poor: false, .. } => {}
            Judged::LongEnd { grade: Some(g), err, .. } => self.grade(g, err),
            Judged::LongEnd { grade: None, .. } => {}
            Judged::Mine { damage, .. } => {
                self.mines += 1;
                self.gauge.apply(2, Some(damage));
            }
        }
    }
}

/// 기록한 입력으로 다시 판정한다 (리플레이). `inputs` = (레인, 청취 곡 시각 ns, 누름?) — 처리한 순서 그대로.
/// 판정 스레드와 같은 `Judge`를 쓰므로 확정 순서가 같다. 끝에 `until`(실시간의 마지막 watermark 곡 시각)까지 확정.
/// 키 반복·같은 레인 겹친 키는 부르는 쪽이 거른다. 재동기화 무효(실시간 kind 7)는 기록된 상태로 `void`를 넘기면 집계에서 뺀다.
pub fn replay(c: &Chart, rules: &Rules, until: i64, inputs: &[(usize, i64, bool, bool)]) -> Score {
    let mut j = Judge::new(c, *rules);
    let mut score = Score::new(rules.gauge, c.judge_count());
    for &(li, t, down, void) in inputs {
        let mut take = |_: usize, x: Judged| {
            if !(void && matches!(x, Judged::Hit { .. } | Judged::Miss { .. })) {
                score.add(&x);
            }
        };
        if down {
            j.press(c, li, t, &mut take);
        } else {
            j.release(c, li, t, &mut take);
        }
    }
    j.advance(c, until, &mut |_, x| score.add(&x));
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(t: i64, end: i64, kind: NoteKind) -> Note {
        Note { t: t * MS, end: end * MS, sounds_start: 0, sounds_len: 0, kind, damage: -1 }
    }

    #[test]
    fn iidx_windows_and_choice() {
        let r = Rules::iidx(GaugeKind::Normal);
        let notes = vec![n(1000, 1000, NoteKind::Tap), n(1100, 1100, NoteKind::Tap)];
        let mut l = LaneJudge::new(2);
        let mut out = vec![];
        // 1060 ms: 노트 0(+60)과 노트 1(−40) 중 가까운 1을 고른다 → GOOD(−40 ms > GREAT 33.3)
        l.press(&notes, &r.key, &r, 1060 * MS, &mut |j| out.push(j));
        assert_eq!(out, vec![Judged::Hit { note: 1, grade: Grade::Good, err: -40 * MS }]);
        let mut out2 = vec![];
        l.advance(&notes, &r.key, LongRule::Cn, 1284 * MS, &mut |_, j| out2.push(j));
        assert_eq!(out2, vec![Judged::Miss { note: 0 }]);
    }

    #[test]
    fn empty_poor_before_bad() {
        let r = Rules::iidx(GaugeKind::Normal);
        let notes = vec![n(1000, 1000, NoteKind::Tap)];
        let mut l = LaneJudge::new(1);
        let mut out = vec![];
        l.press(&notes, &r.key, &r, 700 * MS, &mut |j| out.push(j)); // −300 ms: BAD(283.3) 밖, 350 안 → 빈 POOR
        l.press(&notes, &r.key, &r, 600 * MS, &mut |j| out.push(j)); // −400 ms: 후보 없음
        assert_eq!(out, vec![Judged::Empty { t: 700 * MS, poor: true }, Judged::Empty { t: 600 * MS, poor: false }]);
        out.clear();
        l.press(&notes, &r.scratch, &r, 700 * MS, &mut |j| out.push(j));
        assert_eq!(out, vec![Judged::Hit { note: 0, grade: Grade::Bad, err: -300 * MS }]);
    }

    #[test]
    fn asymmetric_windows() {
        let w = Win { early: [10 * MS, 20 * MS, 30 * MS, 40 * MS], late: [5 * MS, 10 * MS, 15 * MS, 20 * MS] };
        assert_eq!(w.grade(-25 * MS), Some(Grade::Good));
        assert_eq!(w.grade(25 * MS), None);
        assert_eq!(w.grade(8 * MS), Some(Grade::Great));
    }

    #[test]
    fn cn_and_ln_ends() {
        let r = Rules::iidx(GaugeKind::Normal);
        let notes = vec![n(1000, 2000, NoteKind::Long)];
        let mut l = LaneJudge::new(1);
        l.press(&notes, &r.key, &r, 1000 * MS, &mut |_| {});
        assert_eq!(l.release(&notes, &r.key, LongRule::Cn, 1990 * MS), Some(Judged::LongEnd { note: 0, grade: Some(Grade::PGreat), err: -10 * MS }));
        let mut l = LaneJudge::new(1);
        l.press(&notes, &r.key, &r, 1000 * MS, &mut |_| {});
        assert_eq!(l.release(&notes, &r.key, LongRule::Ln, 1500 * MS), Some(Judged::LongEnd { note: 0, grade: Some(Grade::Poor), err: -500 * MS }));
    }

    #[test]
    fn gauge_table() {
        // N = 1000 → N' = 566, PG 회복 = floor(80000 / 3396) = 23 (cp0 표와 같음)
        let mut g = Gauge::new(GaugeRule::iidx(GaugeKind::Normal), 1000);
        g.apply(0, None);
        assert_eq!(g.value, 1123);
        g.apply(3, None);
        assert_eq!(g.value, 823);
        let mut e = Gauge::new(GaugeRule::iidx(GaugeKind::Easy), 1000);
        e.apply(3, None);
        assert_eq!(e.value, 1100 - 240);
        let mut h = Gauge::new(GaugeRule::iidx(GaugeKind::Hard), 1000);
        h.value = 1600;
        h.apply(3, None);
        assert_eq!(h.value, 1600 - 225);
        // 리뷰 3-3 #5: HARD 300에서 POOR → 75 (2% 미만이어도 0 초과면 살아 있음)
        h.value = 300;
        h.apply(3, None);
        assert_eq!((h.value, h.failed), (75, false));
        h.apply(3, None);
        assert_eq!((h.value, h.failed), (0, true));
        let mut x = Gauge::new(GaugeRule::iidx(GaugeKind::ExHard), 1000);
        x.value = 1600;
        x.apply(3, None);
        assert_eq!(x.value, 1600 - 900);
        assert_eq!(Gauge::new(GaugeRule::iidx(GaugeKind::Normal), 10).percent(), 22);
        let mut s = Score::new(GaugeRule::iidx(GaugeKind::Normal), 10);
        s.add(&Judged::Hit { note: 0, grade: Grade::PGreat, err: 0 });
        s.add(&Judged::Hit { note: 1, grade: Grade::Great, err: -5 });
        s.add(&Judged::Empty { t: 0, poor: true });
        assert_eq!((s.ex(), s.combo, s.empty_poor, s.fast), (3, 2, 1, 1));
    }

    #[test]
    fn empty_sound_rules() {
        use crate::runtime::LaneSound;
        let lane = Lane {
            id: "K1".into(),
            role: Role::Key,
            notes: vec![n(1000, 1000, NoteKind::Tap), n(2000, 2000, NoteKind::Tap)],
            lane_sounds: vec![LaneSound { t: 1500 * MS, sound: 7 }],
        };
        assert_eq!(empty_sound(&lane, EmptyLane::PreviousNote, 500 * MS), EmptySound::Note(0));
        assert_eq!(empty_sound(&lane, EmptyLane::PreviousNote, 1200 * MS), EmptySound::Note(0));
        assert_eq!(empty_sound(&lane, EmptyLane::PreviousNote, 1600 * MS), EmptySound::Sound(7));
        assert_eq!(empty_sound(&lane, EmptyLane::PreviousNote, 2100 * MS), EmptySound::Note(1));
        assert_eq!(empty_sound(&lane, EmptyLane::NextNote, 1200 * MS), EmptySound::Note(1));
        assert_eq!(empty_sound(&lane, EmptyLane::None, 1200 * MS), EmptySound::None);
    }

    #[test]
    fn mine_when_held() {
        let r = Rules::iidx(GaugeKind::Normal);
        let notes = vec![n(1000, 1000, NoteKind::Mine)];
        let mut l = LaneJudge::new(1);
        let mut out = vec![];
        l.press(&notes, &r.key, &r, 500 * MS, &mut |j| out.push(j));
        l.advance(&notes, &r.key, LongRule::Ln, 1001 * MS, &mut |_, j| out.push(j));
        assert_eq!(out, vec![Judged::Empty { t: 500 * MS, poor: false }, Judged::Mine { note: 0, damage: -1 }]);
    }

    /// 리뷰 3-3 #6: watermark를 어떻게 나눠 진행해도(실시간) 한 번에 진행해도(리플레이) 집계 순서·점수·게이지가 같다.
    #[test]
    fn confirmation_order_is_watermark_independent() {
        use crate::runtime::{Chart, EmptyLane, Role, SoundBank};
        let mk = |id: &str, notes: Vec<Note>| Lane { id: id.into(), role: Role::Key, notes, lane_sounds: vec![] };
        // 레인 0: 1000 ms 노트(놓침), 1100 ms 롱노트 [1100, 1150] (누른 채 끝 지남 → BAD), 레인 1: 1050 ms 노트(놓침), 레인 2: 1500 ms 노트 (친다)
        let c = Chart {
            title: String::new(),
            chart_id: String::new(),
            layout: String::new(),
            bank: SoundBank::default(),
            auto: vec![],
            note_sounds: vec![],
            lanes: vec![mk("A", vec![n(1000, 1000, NoteKind::Tap), n(1100, 1150, NoteKind::Long)]), mk("B", vec![n(1050, 1050, NoteKind::Tap)]), mk("C", vec![n(1500, 1500, NoteKind::Tap)])],
            groups: vec![],
            scroll: vec![],
            bars: vec![],
            reference_scroll: 120.0,
            long_rule: LongRule::Cn,
            empty_lane: EmptyLane::PreviousNote,
            end: 2_000 * MS,
            chart_hash: [0; 32],
            variant_choice: vec![],
        };
        let rules = Rules::iidx(GaugeKind::Hard);
        let run = |steps: &[i64]| {
            let mut j = Judge::new(&c, rules);
            let mut seq = Vec::new();
            let mut s = Score::new(rules.gauge, c.judge_count());
            let mut take = |li: usize, x: Judged| {
                seq.push((li, x));
                s.add(&x);
            };
            j.press(&c, 0, 1100 * MS, &mut take); // 롱노트 머리 PG
            for &w in steps {
                j.advance(&c, w * MS, &mut take);
            }
            j.press(&c, 2, 1500 * MS, &mut take);
            j.advance(&c, 2000 * MS, &mut take);
            (seq, s)
        };
        let (a, sa) = run(&[]);
        let (b, sb) = run(&[1290, 1300, 1340, 1400, 1460]);
        assert_eq!(a, b);
        assert_eq!(sa, sb);
    }
}
