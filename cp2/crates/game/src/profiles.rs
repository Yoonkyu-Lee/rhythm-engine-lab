//! 구동 프로필. "우리 방식"은 오디오 콜백·판정 스레드가 따로 돌고(audio.rs, judging.rs), 나머지 셋은 화면 프레임에서 로직을 돈다.
//!
//! 공통: 시작할 때 한 번 장치 시계로 "지금 곡 몇 초"를 맞춘 뒤, 그다음은 프로필의 시계로만 진행한다.
//! 자동재생은 그 시계로 시각이 지난 소리를 프레임에서 "지금 울려"(mixer::NOW)로 보낸다. 판정·MISS도 프레임에서.
//!
//! - BMS식 (beatoraja·LR2 계열, CP0 0-4): 시계 = OS 단조 시계(QPC)로 개방 루프. 입력 = 1 kHz 폴링 시각. 같은 소리 다시 울리면 끊기(오디오 쪽 설정).
//! - DJMAX식 (CP0 DJMAX 분석): 시계 = 프레임 시계(프레임 간격 누적, 프레임당 0.333초 상한). 누름 판정 = 200 Hz 고정 스텝 시계,
//!   입력은 그 프레임의 첫 고정 스텝에서 한꺼번에 처리. 소리는 겹치기. (트랙당 프레임당 1발화는 실행 데이터에 트랙이 없어 재현하지 않음)
//! - IIDX 개량형 (CP0 0-3 + 설계안 13장): 시계 = 프레임 = 틱(틱 길이 = 측정한 화면 주기). 2프레임마다 장치 시계와 비교해
//!   17 ms 넘게 늦으면 틱 하나 더(최대 1.5배), 34 ms 넘게 앞서면 틱 하나 쉼(원본은 앞서는 쪽을 고치지 않음 → 개량: 양방향).
//!   곡 끝 주사율 재추정은 하지 않는다(정지를 진행률로 배우는 문제를 피함). 입력은 프레임에서 그때의 틱 시각으로 판정.
//! - IIDX 미세 보정형 (`iidx-fine`, CP2 뒤 추가 실험): IIDX 개량형 + 데드밴드 안에서 어긋남에 비례해 틱 길이를 조금(최대 ±5%) 고쳐
//!   0으로 수렴시킨다 (시정수 0.5초, PLL처럼). 큰 어긋남은 그대로 1.5배/0.5배로 따라잡는다.

use std::sync::Arc;

use rcore::judge::{Grade, Judged, Lane, Tally, Windows};
use rcore::mixer::NOW;
use rcore::ring::{Consumer, Producer};
use rcore::runtime::Compiled;
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use synchronizer::shared_clock::SharedClock;

use crate::audio::KeyReq;
use crate::judging::{song_ns, JudgeLog, JudgeView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Ours,
    Iidx,
    IidxFine,
    Djmax,
    Bms,
}

impl Profile {
    pub fn parse(s: &str) -> Result<Profile, String> {
        Ok(match s {
            "ours" => Profile::Ours,
            "iidx" => Profile::Iidx,
            "iidx-fine" => Profile::IidxFine,
            "djmax" => Profile::Djmax,
            "bms" => Profile::Bms,
            _ => return Err(format!("프로필: ours | iidx | iidx-fine | djmax | bms — {s}")),
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Profile::Ours => "ours",
            Profile::Iidx => "iidx",
            Profile::IidxFine => "iidx-fine",
            Profile::Djmax => "djmax",
            Profile::Bms => "bms",
        }
    }
    pub fn title(&self) -> &'static str {
        match self {
            Profile::Ours => "우리 방식 (기록 시계 C = 장치 시계 + D · 키음·반주 샘플 예약 · 오디오 정지는 건너뜀)",
            Profile::Iidx => "IIDX 개량형 (프레임=틱 · 오디오로 양방향 slew · 틱에서 발음·판정)",
            Profile::IidxFine => "IIDX 미세 보정형 (IIDX 개량형 + 데드밴드 안 비례 보정으로 0에 수렴)",
            Profile::Djmax => "DJMAX식 (프레임 시계 0.333초 상한 · 즉시 재생 · 200 Hz 고정 스텝 판정)",
            Profile::Bms => "BMS식 (OS 시계 개방 루프 · 즉시 재생+끊기 · 1 kHz 폴링 입력)",
        }
    }
}

const STEP_NS: i64 = 5_000_000; // DJMAX 고정 스텝 200 Hz
const MAX_DT_NS: i64 = 333_333_333; // Unity maximumDeltaTime
const SLEW_LATE_NS: i64 = 17_000_000;
const SLEW_EARLY_NS: i64 = 34_000_000;
/// 미세 보정의 시정수: 어긋남을 이 시간에 걸쳐 지수적으로 줄인다 (속도 변화 = 어긋남 / 시정수, 최대 ±5%).
const FINE_TAU_NS: i64 = 500_000_000;

pub struct FrameLogic {
    pub kind: Profile,
    chart: Arc<Compiled>,
    w: Windows,
    keys: Vec<u16>,
    origin: i64,
    rate: u32,
    out_offset_ns: i64,
    lane: Lane,
    tally: Tally,
    pressed: bool,
    last: Option<(u8, i64)>,
    auto_cur: usize,
    started: bool,
    // BMS: 곡 시간 0의 QPC
    start_q: i64,
    // DJMAX: 프레임 시계·고정 스텝 시계 (ns)
    t_frame: i64,
    t_fixed: i64,
    last_q: i64,
    // IIDX: 틱
    base_ns: i64,
    tick: i64,
    tick_ns: i64,
    /// 아직 틱으로 바꾸지 않은 경과 시간 (QPC)
    vb_acc_q: i64,
    frame_no: u64,
    pub slew_extra: u64,
    pub slew_hold: u64,
    /// 마지막 비교의 (차트 − 오디오) ns
    pub slew_err_ns: i64,
    keys_in: Consumer<KeyEvent>,
    key_out: Producer<KeyReq>,
    log: Producer<JudgeLog>,
    pub log_lost: u64,
}

impl FrameLogic {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: Profile,
        chart: Arc<Compiled>,
        w: Windows,
        keys: Vec<u16>,
        origin: i64,
        rate: u32,
        out_offset_ns: i64,
        keys_in: Consumer<KeyEvent>,
        key_out: Producer<KeyReq>,
        log: Producer<JudgeLog>,
    ) -> FrameLogic {
        FrameLogic {
            kind,
            chart,
            w,
            keys,
            origin,
            rate,
            out_offset_ns,
            lane: Lane::new(),
            tally: Tally::default(),
            pressed: false,
            last: None,
            auto_cur: 0,
            started: false,
            start_q: 0,
            t_frame: 0,
            t_fixed: 0,
            last_q: 0,
            base_ns: 0,
            tick: 0,
            tick_ns: 16_666_667,
            vb_acc_q: 0,
            frame_no: 0,
            slew_extra: 0,
            slew_hold: 0,
            slew_err_ns: 0,
            keys_in,
            key_out,
            log,
            log_lost: 0,
        }
    }

    pub fn view(&self) -> JudgeView {
        JudgeView { tally: self.tally.clone(), next_note: vec![self.lane.next_note()], last: self.last, pressed: self.pressed }
    }

    fn emit(&mut self, l: JudgeLog) {
        if !self.log.push(l) {
            self.log_lost += 1;
        }
    }

    /// 입력 이벤트 하나를 시각 `t`(이 프로필의 곡 시간)로 판정한다.
    fn judge_event(&mut self, ev: KeyEvent, t: i64, state: u8) {
        let notes = &self.chart.chart.lanes[0].notes;
        let q_proc = qpc::now();
        if ev.down {
            if self.pressed {
                return;
            }
            self.pressed = true;
            // 누름 직전에 그 시각까지 MISS를 확정한다 (늦은 창이 지난 노트가 다음 누름을 가로막지 않게)
            let mut outs: Vec<Judged> = Vec::new();
            self.lane.advance(notes, &self.w, t, &mut |j| outs.push(j));
            for j in outs {
                self.tally.add(&j);
                let l = match j {
                    Judged::Miss { note } => JudgeLog { kind: 2, note, grade: Grade::Miss as u8, t_ns: t, q_in: q_proc, q_proc, key_frame: -1, state, ..Default::default() },
                    Judged::LongEnd { note, ok, err } => JudgeLog { kind: if ok { 3 } else { 4 }, note, err_ns: err, t_ns: t, q_in: q_proc, q_proc, key_frame: -1, state, ..Default::default() },
                    _ => continue,
                };
                self.emit(l);
            }
            let notes = &self.chart.chart.lanes[0].notes;
            let j = self.lane.press(notes, &self.w, t);
            self.tally.add(&j);
            let (kind, note, grade, err, sound_note) = match j {
                Judged::Hit { note, grade, err } => (0u8, note, grade, err, Some(note)),
                Judged::Empty { sound_note, .. } => (1, sound_note.unwrap_or(u32::MAX), Grade::Miss, 0, sound_note),
                _ => unreachable!(),
            };
            if let Some(n) = sound_note {
                let _ = self.key_out.push(KeyReq::note(NOW, 0, n));
            }
            if kind == 0 {
                self.last = Some((grade as u8, err));
            }
            self.emit(JudgeLog { kind, note, grade: grade as u8, err_ns: err, t_ns: t, q_in: ev.qpc, q_proc, vkey: ev.vkey, key_frame: -1, state });
        } else {
            self.pressed = false;
            if let Some(Judged::LongEnd { note, ok, err }) = self.lane.release(notes, &self.w, t) {
                self.tally.add(&Judged::LongEnd { note, ok, err });
                self.emit(JudgeLog { kind: if ok { 3 } else { 4 }, note, err_ns: err, t_ns: t, q_in: ev.qpc, q_proc, vkey: ev.vkey, key_frame: -1, state, ..Default::default() });
            } else {
                self.emit(JudgeLog { kind: 5, t_ns: t, q_in: ev.qpc, q_proc, vkey: ev.vkey, key_frame: -1, state, ..Default::default() });
            }
        }
    }

    /// 한 프레임. `q_now` = 프레임 시작 QPC, `period_q` = 측정한 화면 주기(QPC 틱). 화면에 쓸 곡 시각(ns)을 돌려준다.
    pub fn frame(&mut self, q_now: i64, period_q: i64, clock: &SharedClock, state: u8) -> Option<i64> {
        let qf = qpc::frequency();
        let to_ns = |dq: i64| (dq as i128 * 1_000_000_000 / qf as i128) as i64;
        if !self.started {
            let (s_dev, _) = song_ns(clock, q_now, self.origin, self.rate, self.out_offset_ns)?;
            self.start_q = q_now - (s_dev as i128 * qf as i128 / 1_000_000_000) as i64;
            self.t_frame = s_dev;
            self.t_fixed = s_dev.div_euclid(STEP_NS) * STEP_NS;
            self.last_q = q_now;
            self.base_ns = s_dev;
            self.tick = 0;
            self.started = true;
        }
        self.frame_no += 1;
        // 1) 이 프로필의 시계
        let t = match self.kind {
            Profile::Bms => to_ns(q_now - self.start_q),
            Profile::Djmax => {
                self.t_frame += to_ns(q_now - self.last_q).min(MAX_DT_NS);
                self.last_q = q_now;
                self.t_frame
            }
            Profile::Iidx | Profile::IidxFine => {
                // 틱 = 화면 갱신(vblank) 한 번. 프레임 수로 세면 안 된다: 이 장비는 vblank보다 프레임을 1% 더 그린다
                // (33422 프레임 / 33100 vblank) → 차트가 오디오보다 빨리 달려 slew가 계속 잡아당긴다.
                // 지난 프레임 이후 흐른 vblank 수만큼 틱을 세되, 한 프레임에 최대 1틱 (아케이드: 프레임이 밀리면 그만큼 차트도 늦어지고 slew가 따라잡는다).
                // 틱마다 그때의 틱 길이를 더한다 (틱 수 × 틱 길이로 계산하면, 화면 주기 추정이 바뀌는 순간 시각이 통째로 튄다)
                self.tick_ns = to_ns(period_q).max(1);
                self.vb_acc_q += q_now - self.last_q;
                self.last_q = q_now;
                let n = ((self.vb_acc_q + period_q / 2) / period_q.max(1)).min(1);
                self.vb_acc_q = (self.vb_acc_q - n * period_q).min(period_q); // 멈춤 동안 쌓인 몫은 버린다
                self.tick += n;
                self.base_ns += n * self.tick_ns;
                let fine = self.kind == Profile::IidxFine;
                let coarse_turn = self.frame_no % 2 == 0;
                if coarse_turn || fine {
                    if let Some((a, _)) = song_ns(clock, q_now, self.origin, self.rate, self.out_offset_ns) {
                        let e = self.base_ns - a;
                        self.slew_err_ns = e;
                        if coarse_turn && e < -SLEW_LATE_NS {
                            self.base_ns += self.tick_ns; // 2프레임에 한 틱 더 = 1.5배
                            self.slew_extra += 1;
                        } else if coarse_turn && e > SLEW_EARLY_NS {
                            self.base_ns -= self.tick_ns; // 한 틱 쉼 = 0.5배
                            self.slew_hold += 1;
                        } else if fine && n > 0 && e.abs() <= SLEW_EARLY_NS {
                            // 데드밴드 안: 이번 틱 길이를 어긋남에 비례해 고친다 (최대 ±5%)
                            let adj = (-(e as i128) * self.tick_ns as i128 / FINE_TAU_NS as i128) as i64;
                            let cap = self.tick_ns / 20;
                            self.base_ns += adj.clamp(-cap, cap);
                        }
                    }
                }
                self.base_ns
            }
            Profile::Ours => return None,
        };
        // 2) 자동재생: 시각이 지난 소리를 지금 울린다
        let auto = &self.chart.chart.auto;
        while self.auto_cur < auto.len() && auto[self.auto_cur].t <= t {
            let e = auto[self.auto_cur];
            let _ = self.key_out.push(KeyReq::sound(NOW, e.sound, e.gain, false));
            self.auto_cur += 1;
        }
        // 3) 입력과 판정
        match self.kind {
            Profile::Bms => {
                while let Some(ev) = self.keys_in.pop() {
                    if self.keys.contains(&ev.vkey) {
                        let te = to_ns(ev.qpc - self.start_q);
                        self.judge_event(ev, te, state);
                    }
                }
            }
            Profile::Djmax => {
                // 이 프레임에 돌 고정 스텝들. 입력은 첫 스텝에서 한꺼번에 그 스텝 시각으로.
                let mut first = true;
                while self.t_fixed + STEP_NS <= self.t_frame {
                    self.t_fixed += STEP_NS;
                    if first {
                        first = false;
                        while let Some(ev) = self.keys_in.pop() {
                            if self.keys.contains(&ev.vkey) {
                                let tf = self.t_fixed;
                                self.judge_event(ev, tf, state);
                            }
                        }
                    }
                }
            }
            Profile::Iidx | Profile::IidxFine => {
                while let Some(ev) = self.keys_in.pop() {
                    if self.keys.contains(&ev.vkey) {
                        self.judge_event(ev, t, state);
                    }
                }
            }
            Profile::Ours => {}
        }
        // 4) MISS·롱노트 완료: 프레임 시각으로
        let notes = &self.chart.chart.lanes[0].notes;
        let mut outs: Vec<Judged> = Vec::new();
        self.lane.advance(notes, &self.w, t, &mut |j| outs.push(j));
        for j in outs {
            self.tally.add(&j);
            let l = match j {
                Judged::Miss { note } => JudgeLog { kind: 2, note, grade: Grade::Miss as u8, t_ns: t, q_in: q_now, q_proc: qpc::now(), key_frame: -1, state, ..Default::default() },
                Judged::LongEnd { note, ok, err } => JudgeLog { kind: if ok { 3 } else { 4 }, note, err_ns: err, t_ns: t, q_in: q_now, q_proc: qpc::now(), key_frame: -1, state, ..Default::default() },
                _ => continue,
            };
            self.emit(l);
        }
        Some(t)
    }
}
