//! 판정 스레드: 입력 시각 → 청취 곡 시간 → 순수 판정(레인마다) → 키음 요청. CP2 judging.rs를 N레인으로.
//!
//! - 청취 곡 시간 H(q) = (장치 시계로 본 그 순간의 스트림 프레임 − origin) / rate + 출력 보정 (CP2와 같음).
//! - 키음: 입력 순간의 스트림 프레임 + D를 목표로 오디오 스레드에 요청한다 (PERFECT면 반주와 같은 샘플, CP1-4).
//!   노트를 쳤으면 그 노트의 소리들, 빈 누름이면 빈 레인 소리 규칙(`engine::judge::empty_sound`)이 고른 소리.
//! - MISS 확정은 입력 스레드의 watermark가 지난 뒤에만, 누름을 판정하기 직전에도 그 누름 시각까지 확정한다 (CP2 2-6).
//! - 키 → 레인은 `keymap`(레이아웃마다). 같은 레인에 키가 여럿이면 하나라도 눌려 있으면 누름(뗌은 모두 뗐을 때).
//! - 리플레이용으로 처리한 키 입력마다 kind 8 행(청취 곡 시각, 누름/뗌, 레인)을 남긴다.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use engine::judge::{empty_sound, EmptySound, Grade, Judge, Judged, Rules, Score};
use engine::ring::{Consumer, Producer};
use engine::runtime::{ns_to_frame, Compiled};
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use synchronizer::shared_clock::SharedClock;

use crate::audio::KeyReq;

/// 판정 하나의 기록.
#[derive(Clone, Copy, Debug, Default)]
pub struct JudgeLog {
    /// 0 노트, 1 빈 누름, 2 놓침 POOR, 3 롱노트 끝, 4 지뢰, 5 뗌, 6 watermark 위반, 7 무효(재동기화 중),
    /// 8 입력 원본(리플레이용: grade = 1 누름 / 0 뗌), 9 빈 POOR, 10 배치에 없는 키(진단), 11 마지막 watermark(t_ns)
    pub kind: u8,
    pub lane: u16,
    pub note: u32,
    /// 0 PGREAT 1 GREAT 2 GOOD 3 BAD 4 POOR, 롱노트 끝 성공(LN)은 5
    pub grade: u8,
    pub err_ns: i64,
    pub t_ns: i64,
    pub q_in: i64,
    pub q_proc: i64,
    pub vkey: u16,
    pub key_frame: i64,
    pub q_recv: i64,
    pub q_floor: i64,
    pub state: u8,
}

/// 화면이 읽는 판정 상태 (판정 스레드는 try_lock만 한다).
#[derive(Clone, Debug)]
pub struct JudgeView {
    pub score: Score,
    /// 레인별 그리기 시작 노트 (그 앞은 모두 판정됨)
    pub first_open: Vec<usize>,
    /// 레인별 판정한 노트 표시 (그리지 않음)
    pub done: Vec<Vec<bool>>,
    pub pressed: Vec<bool>,
    pub last: Option<(Grade, i64)>,
    /// 키음 요청 링이 가득 차 버린 수
    pub key_lost: u64,
}

pub struct JudgeShared {
    pub view: Mutex<JudgeView>,
    pub watermark_song_ns: AtomicI64,
    pub violations: AtomicU32,
    pub inject_stall_ms: AtomicU32,
}

pub struct JudgeCfg {
    pub origin: i64,
    pub rate: u32,
    pub out_offset_ns: i64,
    pub rules: Rules,
    /// (가상 키, 레인 번호)
    pub keymap: Vec<(u16, usize)>,
    pub resync_void: bool,
    pub audio: Arc<crate::audio::AudioMonitor>,
}

const STATE_RESYNCING: u8 = 2;

fn emit(log: &mut Producer<JudgeLog>, lost: &mut u64, l: JudgeLog) {
    if !log.push(l) {
        *lost += 1;
    }
}

fn grade_code(g: Grade) -> u8 {
    g as u8
}

/// QPC 틱 → 청취 곡 시간 (ns). 시계가 아직 없으면 None.
pub fn song_ns(clock: &SharedClock, q: i64, origin: i64, rate: u32, offset_ns: i64) -> Option<(i64, f64)> {
    let s = clock.load();
    if !s.valid {
        return None;
    }
    let t100 = (q as i128 * 10_000_000 / qpc::frequency() as i128) as i64;
    let frame = s.frame_at(t100);
    Some((((frame - origin as f64) / rate as f64 * 1e9) as i64 + offset_ns, frame))
}

pub fn new_view(c: &Compiled, rules: &Rules) -> JudgeView {
    JudgeView {
        score: Score::new(rules.gauge, c.chart.judge_count()),
        first_open: vec![0; c.chart.lanes.len()],
        done: c.chart.lanes.iter().map(|l| vec![false; l.notes.len()]).collect(),
        pressed: vec![false; c.chart.lanes.len()],
        last: None,
        key_lost: 0,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    chart: Arc<Compiled>,
    cfg: JudgeCfg,
    clock: Arc<SharedClock>,
    watermark: Arc<AtomicI64>,
    mut keys_in: Consumer<KeyEvent>,
    mut key_out: Producer<KeyReq>,
    mut log: Producer<JudgeLog>,
    stop: Arc<AtomicBool>,
    beats: Arc<crate::monitor::Heartbeats>,
    monitor: Arc<crate::monitor::MonitorShared>,
) -> (Arc<JudgeShared>, JoinHandle<u64>) {
    let shared = Arc::new(JudgeShared {
        view: Mutex::new(new_view(&chart, &cfg.rules)),
        watermark_song_ns: AtomicI64::new(i64::MIN),
        violations: AtomicU32::new(0),
        inject_stall_ms: AtomicU32::new(0),
    });
    let sh = shared.clone();
    let join = thread::Builder::new()
        .name("judge".into())
        .spawn(move || {
            let c = &chart.chart;
            let nl = c.lanes.len();
            // 판정기: 실시간·리플레이 같은 코드 (engine::judge::Judge — 확정 순서 규칙, 미리 잡은 버퍼)
            let mut judge = Judge::new(c, cfg.rules);
            let mut score = Score::new(cfg.rules.gauge, c.judge_count());
            let mut held: Vec<u32> = vec![0; nl]; // 레인마다 눌린 키 수
            let mut key_down: Vec<(u16, bool)> = cfg.keymap.iter().map(|(k, _)| (*k, false)).collect();
            let mut lost = 0u64;
            let mut key_lost = 0u64;
            let (mut last_wm_song, mut last_wm_q) = (i64::MIN, 0i64);
            let mut last: Option<(Grade, i64)> = None;
            let freq = qpc::frequency();
            let resync_void = cfg.resync_void;
            // 결과 하나를 집계·기록한다 (재동기화 중의 노트 판정·놓침은 무효). key_frame = 키음 요청의 예상 프레임 (리뷰 3-3 #14)
            let take = |j: Judged, lane: usize, t: i64, q: i64, st: u8, key_frame: i64, score: &mut Score, last: &mut Option<(Grade, i64)>, log: &mut Producer<JudgeLog>, lost: &mut u64| {
                let void = resync_void && st == STATE_RESYNCING && matches!(j, Judged::Hit { .. } | Judged::Miss { .. });
                if !void {
                    score.add(&j);
                }
                let base = JudgeLog { lane: lane as u16, t_ns: t, q_in: q, q_proc: qpc::now(), key_frame: -1, state: st, ..Default::default() };
                let l = match j {
                    Judged::Hit { note, grade, err } => {
                        *last = Some((grade, err));
                        JudgeLog { kind: if void { 7 } else { 0 }, note, grade: grade_code(grade), err_ns: err, key_frame, ..base }
                    }
                    Judged::Miss { note } => JudgeLog { kind: if void { 7 } else { 2 }, note, grade: 4, ..base },
                    Judged::Empty { poor, .. } => JudgeLog { kind: if poor { 9 } else { 1 }, note: u32::MAX, grade: 4, key_frame, ..base },
                    Judged::LongEnd { note, grade, err } => {
                        if let Some(g) = grade {
                            *last = Some((g, err));
                        }
                        JudgeLog { kind: 3, note, grade: grade.map_or(5, grade_code), err_ns: err, ..base }
                    }
                    Judged::Mine { note, damage } => JudgeLog { kind: 4, note, err_ns: damage as i64, ..base },
                };
                emit(log, lost, l);
            };
            while !stop.load(Ordering::Relaxed) {
                beats.judge.store(qpc::now(), Ordering::Release);
                let req = sh.inject_stall_ms.swap(0, Ordering::AcqRel);
                if req > 0 {
                    thread::sleep(Duration::from_millis(req as u64));
                }
                // 0) watermark를 **먼저** 읽는다 (리뷰 3-3 #3): 입력 스레드는 입력을 링에 넣은 뒤 watermark를 올리므로(Release),
                //    지금 읽은 watermark 이전의 입력은 아래에서 링을 비울 때 모두 꺼내진다. 확정은 그 입력들을 처리한 뒤.
                let wm_q = watermark.load(Ordering::Acquire);
                // 1) 입력 (도착 순서대로)
                while let Some(ev) = keys_in.pop() {
                    let Some(ki) = cfg.keymap.iter().position(|(k, _)| *k == ev.vkey) else {
                        // 배치에 없는 키 (진단: kind 10)
                        emit(&mut log, &mut lost, JudgeLog { kind: 10, vkey: ev.vkey, grade: ev.down as u8, key_frame: -1, ..Default::default() });
                        continue;
                    };
                    let li = cfg.keymap[ki].1;
                    let q_in = ev.estimated_qpc(freq);
                    let q_proc = qpc::now();
                    let Some((t, _frame)) = song_ns(&clock, q_in, cfg.origin, cfg.rate, cfg.out_offset_ns) else { continue };
                    if q_in < last_wm_q {
                        sh.violations.fetch_add(1, Ordering::Relaxed);
                        emit(&mut log, &mut lost, JudgeLog { kind: 6, lane: li as u16, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, ..Default::default() });
                    }
                    let st = monitor.state.load(Ordering::Relaxed);
                    emit(&mut log, &mut lost, JudgeLog { kind: 8, lane: li as u16, grade: ev.down as u8, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, q_recv: ev.recv_qpc, q_floor: ev.floor_qpc, state: st, ..Default::default() });
                    if key_down[ki].1 == ev.down {
                        continue; // 키 반복
                    }
                    key_down[ki].1 = ev.down;
                    if ev.down {
                        held[li] += 1;
                        if held[li] > 1 {
                            continue; // 같은 레인의 다른 키가 이미 눌려 있음
                        }
                        // 키음 예상 프레임 (기록용): 믹서 원점(origin + D) + 반올림(누름 곡 시각)
                        let d = cfg.audio.d_frames.load(Ordering::Relaxed).max(0);
                        let kf = cfg.origin + d + ns_to_frame(t, cfg.rate);
                        judge.press(c, li, t, &mut |lane, j| {
                            let mut kf_used = -1;
                            if lane == li {
                                let req = match j {
                                    Judged::Hit { note, .. } => Some(KeyReq::note_at(t, li as u16, note)),
                                    Judged::Empty { .. } => match empty_sound(&c.lanes[li], c.empty_lane, t) {
                                        EmptySound::Note(n) => Some(KeyReq::note_at(t, li as u16, n)),
                                        EmptySound::Sound(snd) => Some(KeyReq::sound_at(t, snd, 1.0)),
                                        EmptySound::None => None,
                                    },
                                    _ => None,
                                };
                                if let Some(r) = req {
                                    if key_out.push(r) {
                                        kf_used = kf;
                                    } else {
                                        key_lost += 1;
                                    }
                                }
                            }
                            take(j, lane, t, q_in, st, kf_used, &mut score, &mut last, &mut log, &mut lost);
                        });
                    } else {
                        held[li] = held[li].saturating_sub(1);
                        if held[li] > 0 {
                            continue;
                        }
                        let ended = judge.release(c, li, t, &mut |lane, j| take(j, lane, t, q_in, st, -1, &mut score, &mut last, &mut log, &mut lost));
                        if !ended {
                            emit(&mut log, &mut lost, JudgeLog { kind: 5, lane: li as u16, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, ..Default::default() });
                        }
                    }
                }
                // 2) watermark 진행 → 놓침·롱노트·지뢰 확정 (모든 레인, 정해진 순서)
                if wm_q > 0 {
                    if let Some((wm_song, _)) = song_ns(&clock, wm_q, cfg.origin, cfg.rate, cfg.out_offset_ns) {
                        if wm_song > last_wm_song {
                            last_wm_song = wm_song;
                            last_wm_q = wm_q;
                            sh.watermark_song_ns.store(wm_song, Ordering::Release);
                            let st = monitor.state.load(Ordering::Relaxed);
                            judge.advance(c, wm_song, &mut |lane, j| take(j, lane, wm_song, wm_q, st, -1, &mut score, &mut last, &mut log, &mut lost));
                        }
                    }
                }
                // 3) 화면용 상태 (try_lock: 기다리지 않음, 할당 없음)
                if let Ok(mut v) = sh.view.try_lock() {
                    v.score = score.clone();
                    for li in 0..nl {
                        let lj = judge.lane(li);
                        let n = c.lanes[li].notes.len();
                        let mut fo = v.first_open[li];
                        while fo < n && lj.is_done(fo) && lj.holding() != Some(fo) {
                            fo += 1;
                        }
                        v.first_open[li] = fo;
                        for i in fo..n.min(fo + 64) {
                            v.done[li][i] = lj.is_done(i) && lj.holding() != Some(i);
                        }
                        v.pressed[li] = held[li] > 0;
                    }
                    v.last = last;
                    v.key_lost = key_lost;
                }
                thread::sleep(Duration::from_micros(500));
            }
            // 마지막 watermark (리플레이가 같은 곳까지 확정하도록)
            emit(&mut log, &mut lost, JudgeLog { kind: 11, t_ns: last_wm_song, q_in: last_wm_q, key_frame: -1, ..Default::default() });
            lost
        })
        .expect("판정 스레드");
    (shared, join)
}
