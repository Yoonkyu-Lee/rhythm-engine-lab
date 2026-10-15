//! 판정 스레드 ("우리 방식"): 입력 시각 → 청취 곡 시간 → 순수 판정 → 키음 요청.
//!
//! - 청취 곡 시간 H(q) = (장치 시계로 본 그 순간의 스트림 프레임 − origin) / rate + 출력 보정.
//!   장치 시계는 `SharedClock`(오디오 스레드의 위치 관측으로 맞춘 직선)이다. 화면·프레임과 무관하다.
//! - 키음: 입력 순간의 스트림 프레임을 목표로 오디오 스레드에 요청한다. 오디오 스레드가 받을 때는 이미 지났으므로
//!   믹서가 "가장 이른 안전한 프레임"에 낸다 (최소 지연 모드, 설계안 8-3). 지각량은 믹서 사건에 남는다.
//! - MISS 확정은 입력 스레드의 watermark가 지나간 뒤에만 한다.
//! - 결과는 SPSC 링으로 기록 스레드에, 집계는 화면용 공유 상태(`JudgeShared`)에 올린다.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rcore::judge::{Grade, Judged, Lane, Tally, Windows};
use rcore::ring::{Consumer, Producer};
use rcore::runtime::Compiled;
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use synchronizer::shared_clock::SharedClock;

use crate::audio::KeyReq;

/// 판정 하나의 기록.
#[derive(Clone, Copy, Debug, Default)]
pub struct JudgeLog {
    /// 0 노트, 1 빈 누름, 2 MISS, 3 롱노트 끝(성공), 4 롱노트 끊김, 5 뗌, 6 watermark 위반(이미 지난 시각의 입력),
    /// 7 무효(재동기화 중의 노트 판정·MISS: grade는 원래 받았을 등급)
    pub kind: u8,
    pub note: u32,
    /// 0 PERFECT 1 GREAT 2 GOOD 3 BAD 4 MISS
    pub grade: u8,
    pub err_ns: i64,
    /// 입력의 청취 곡 시간 (ns).
    pub t_ns: i64,
    /// 입력 시각 추정 (QPC 틱)과 판정 스레드가 처리한 시각.
    pub q_in: i64,
    pub q_proc: i64,
    pub vkey: u16,
    /// 요청한 키음 목표 스트림 프레임 (없으면 −1).
    pub key_frame: i64,
    /// 판정할 때의 게임 상태 (0 Running, 1 Suspect, 2 Resyncing).
    pub state: u8,
}

/// 화면이 읽는 판정 상태 (자주 바뀌지 않으므로 잠금으로 충분: 판정 스레드는 try_lock만 한다).
#[derive(Clone, Debug, Default)]
pub struct JudgeView {
    pub tally: Tally,
    /// 레인별로 아직 판정하지 않은 첫 노트.
    pub next_note: Vec<usize>,
    pub last: Option<(u8, i64)>,
    pub pressed: bool,
}

pub struct JudgeShared {
    pub view: Mutex<JudgeView>,
    /// 마지막으로 처리한 watermark의 곡 시간 (ns).
    pub watermark_song_ns: AtomicI64,
    pub violations: AtomicU32,
    /// 고장 주입: 0이 아니면 판정 스레드가 그만큼(ms) 잔다.
    pub inject_stall_ms: AtomicU32,
}

impl JudgeShared {
    pub fn new(lanes: usize) -> Arc<JudgeShared> {
        Arc::new(JudgeShared {
            view: Mutex::new(JudgeView { next_note: vec![0; lanes], ..Default::default() }),
            watermark_song_ns: AtomicI64::new(i64::MIN),
            violations: AtomicU32::new(0),
            inject_stall_ms: AtomicU32::new(0),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct JudgeFault {
    pub at_s: f64,
    pub stall_ms: u32,
}

pub struct JudgeCfg {
    pub origin: i64,
    pub rate: u32,
    pub out_offset_ns: i64,
    pub windows: Windows,
    pub keys: Vec<u16>,
    pub faults: Vec<JudgeFault>,
    /// 재동기화(Resyncing) 중의 노트 판정·MISS를 무효로 한다 (CP2 2-5 결정: 그 순간 플레이어는 밀리기 전 박자를 따르고 있다).
    pub resync_void: bool,
}

const STATE_RESYNCING: u8 = 2;

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
        view: Mutex::new(JudgeView { next_note: vec![0; chart.chart.lanes.len()], ..Default::default() }),
        watermark_song_ns: AtomicI64::new(i64::MIN),
        violations: AtomicU32::new(0),
        inject_stall_ms: AtomicU32::new(0),
    });
    let sh = shared.clone();
    let join = thread::Builder::new()
        .name("judge".into())
        .spawn(move || {
            let notes = &chart.chart.lanes[0].notes;
            let mut lane = Lane::new();
            let mut tally = Tally::default();
            let mut lost = 0u64;
            let mut fired = vec![false; cfg.faults.len()];
            let mut last_wm_song = i64::MIN;
            let mut last_wm_q = 0i64;
            let mut pressed = false;
            let mut last: Option<(u8, i64)> = None;
            let freq = qpc::frequency();
            let emit = |log: &mut Producer<JudgeLog>, lost: &mut u64, mut l: JudgeLog| {
                l.state = monitor.state.load(Ordering::Relaxed);
                if !log.push(l) {
                    *lost += 1;
                }
            };
            while !stop.load(Ordering::Relaxed) {
                // 고장 주입: 곡 시각에 판정 스레드를 재운다
                let wm_q = watermark.load(Ordering::Acquire);
                if let Some((song_now, _)) = (wm_q > 0).then(|| song_ns(&clock, wm_q, cfg.origin, cfg.rate, cfg.out_offset_ns)).flatten() {
                    for (i, f) in cfg.faults.iter().enumerate() {
                        if !fired[i] && song_now as f64 / 1e9 >= f.at_s {
                            fired[i] = true;
                            thread::sleep(Duration::from_millis(f.stall_ms as u64));
                        }
                    }
                }
                beats.judge.store(qpc::now(), Ordering::Release);
                let req = sh.inject_stall_ms.swap(0, Ordering::AcqRel);
                if req > 0 {
                    thread::sleep(Duration::from_millis(req as u64));
                }
                // 1) 입력 (도착 순서대로)
                while let Some(ev) = keys_in.pop() {
                    if !cfg.keys.contains(&ev.vkey) {
                        continue;
                    }
                    let q_in = ev.estimated_qpc(freq);
                    let q_proc = qpc::now();
                    let Some((t, frame)) = song_ns(&clock, q_in, cfg.origin, cfg.rate, cfg.out_offset_ns) else { continue };
                    if q_in < last_wm_q {
                        // watermark 계약 위반: 이미 지났다고 선언한 시각의 입력이 늦게 왔다 (MISS 확정 뒤일 수 있음)
                        sh.violations.fetch_add(1, Ordering::Relaxed);
                        emit(&mut log, &mut lost, JudgeLog { kind: 6, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, ..Default::default() });
                    }
                    if ev.down {
                        if pressed {
                            continue; // 키 반복
                        }
                        pressed = true;
                        let j = lane.press(notes, &cfg.windows, t);
                        let void = cfg.resync_void && matches!(j, Judged::Hit { .. }) && monitor.state.load(Ordering::Relaxed) == STATE_RESYNCING;
                        if void {
                            tally.void += 1;
                        } else {
                            tally.add(&j);
                        }
                        let (kind, note, grade, err, sound_note) = match j {
                            Judged::Hit { note, grade, err } => (if void { 7 } else { 0 }, note, grade, err, Some(note)),
                            Judged::Empty { sound_note, .. } => (1, sound_note.unwrap_or(u32::MAX), Grade::Miss, 0, sound_note),
                            _ => unreachable!(),
                        };
                        let key_frame = if let Some(n) = sound_note {
                            let f = frame.round() as i64;
                            let _ = key_out.push(KeyReq::note(f, 0, n));
                            f
                        } else {
                            -1
                        };
                        if kind == 0 {
                            last = Some((grade as u8, err));
                        }
                        emit(&mut log, &mut lost, JudgeLog { kind, note, grade: grade as u8, err_ns: err, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame, state: 0 });
                    } else {
                        pressed = false;
                        if let Some(Judged::LongEnd { note, ok, err }) = lane.release(notes, &cfg.windows, t) {
                            let j = Judged::LongEnd { note, ok, err };
                            tally.add(&j);
                            emit(&mut log, &mut lost, JudgeLog { kind: if ok { 3 } else { 4 }, note, err_ns: err, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, ..Default::default() });
                        } else {
                            emit(&mut log, &mut lost, JudgeLog { kind: 5, t_ns: t, q_in, q_proc, vkey: ev.vkey, key_frame: -1, ..Default::default() });
                        }
                    }
                }
                // 2) watermark 진행 → MISS·롱노트 완료 확정
                let wm_q = watermark.load(Ordering::Acquire);
                if wm_q > 0 {
                    if let Some((wm_song, _)) = song_ns(&clock, wm_q, cfg.origin, cfg.rate, cfg.out_offset_ns) {
                        if wm_song > last_wm_song {
                            last_wm_song = wm_song;
                            last_wm_q = wm_q;
                            sh.watermark_song_ns.store(wm_song, Ordering::Release);
                            let q_proc = qpc::now();
                            let st = monitor.state.load(Ordering::Relaxed);
                            lane.advance(notes, &cfg.windows, wm_song, &mut |j| {
                                let void = cfg.resync_void && matches!(j, Judged::Miss { .. }) && st == STATE_RESYNCING;
                                if void {
                                    tally.void += 1;
                                } else {
                                    tally.add(&j);
                                }
                                let l = match j {
                                    Judged::Miss { note } => JudgeLog { kind: if void { 7 } else { 2 }, note, grade: Grade::Miss as u8, t_ns: wm_song, q_in: wm_q, q_proc, key_frame: -1, ..Default::default() },
                                    Judged::LongEnd { note, ok, err } => JudgeLog { kind: if ok { 3 } else { 4 }, note, err_ns: err, t_ns: wm_song, q_in: wm_q, q_proc, key_frame: -1, ..Default::default() },
                                    _ => return,
                                };
                                let l = JudgeLog { state: monitor.state.load(Ordering::Relaxed), ..l };
                                if !log.push(l) {
                                    lost += 1;
                                }
                            });
                        }
                    }
                }
                // 3) 화면용 상태 (화면이 잡고 있으면 다음 번에)
                if let Ok(mut v) = sh.view.try_lock() {
                    v.tally = tally.clone();
                    v.next_note[0] = lane.next_note();
                    v.last = last;
                    v.pressed = pressed;
                }
                thread::sleep(Duration::from_micros(500));
            }
            lost
        })
        .expect("판정 스레드");
    (shared, join)
}
