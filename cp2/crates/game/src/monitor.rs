//! 렉 감지와 상태 머신 (감시 스레드). 게임이 스스로 렉을 알아채고, 기록하고, 판정에 알린다.
//!
//! 감시 스레드는 1 ms마다 각 스레드의 맥박(마지막으로 일한 QPC)과 오디오 상태를 본다. 화면 스레드가 멈춰도 감지는 계속된다.
//! - 오디오 정지: 마지막 깨어남에서 `AUDIO_STALL_MS` 경과. 언더런: 버퍼 여유 0 블록 수 증가. 시계 점프: 장치 시계 epoch 변경.
//! - 화면 정지 `RENDER_STALL_MS`, 판정 정지 `JUDGE_STALL_MS`, 입력 정지(watermark 정체) `INPUT_STALL_MS`.
//! 상태 (설계안 9장의 최소 버전):
//! - Running: 정상.
//! - Suspect: 오디오·입력·판정 경로의 정지가 진행 중. 판정은 계속하되 이 동안의 판정에 상태를 붙여 기록한다.
//! - Resyncing: 시계 점프(장치 시계 epoch 변경) 뒤, 새 대응이 `RESYNC_STABLE_MS` 동안 다시 안정될 때까지.
//!   CP2 2차: 우리 방식은 오디오가 멈추면 콘텐츠를 건너뛰어 시간축을 잇는다(시계가 튀지 않음) → "콘텐츠 건너뜀" 사건만 남고
//!   재동기화할 것이 없다. 시계 점프는 건너뛰기를 쓰지 않는 프레임 구동 프로필에서만 난다.
//! 화면만 멈춘 것은 판정·소리에 영향이 없으므로 상태를 바꾸지 않고 사건만 남긴다.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rcore::ring::Producer;
use synchronizer::qpc;
use synchronizer::shared_clock::SharedClock;

use crate::audio::AudioMonitor;

pub const AUDIO_STALL_MS: i64 = 30;
pub const RENDER_STALL_MS: i64 = 50;
pub const JUDGE_STALL_MS: i64 = 20;
pub const INPUT_STALL_MS: i64 = 20;
pub const RESYNC_STABLE_MS: i64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Running = 0,
    Suspect = 1,
    Resyncing = 2,
}

pub const STATE_NAMES: [&str; 3] = ["Running", "Suspect", "Resyncing"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LagKind {
    AudioStall = 0,
    Underrun = 1,
    ClockJump = 2,
    RenderStall = 3,
    JudgeStall = 4,
    InputStall = 5,
    StateChange = 6,
    /// 재동기화가 끝난 순간, 밀리기 전 직선과 지금 직선이 같은 순간을 어디로 보는지의 차이 (확정된 총 밀림).
    ResyncDone = 7,
    /// 오디오 정지 뒤 콘텐츠를 건너뛰었다 (magnitude = 건너뛴 길이 µs). 시간축은 끊기지 않았다.
    ContentSkip = 8,
}

pub const LAG_NAMES: [&str; 9] = ["오디오 정지", "언더런", "시계 점프", "화면 정지", "판정 정지", "입력 정지", "상태", "재동기화 완료", "콘텐츠 건너뜀"];

/// 렉 사건 하나. 정지류: start = 마지막 맥박, detect = 기준을 넘긴 것을 본 순간, end = 맥박이 돌아온 순간 (0이면 진행 중).
/// 시계 점프: magnitude_us = 새 직선과 옛 직선이 같은 순간에 가리키는 위치 차이(µs, +면 장치가 뒤처짐). 상태: magnitude = 새 상태.
#[derive(Clone, Copy, Debug, Default)]
pub struct LagEvent {
    pub kind: u8,
    pub q_start: i64,
    pub q_detect: i64,
    pub q_end: i64,
    pub magnitude_us: i64,
}

/// 다른 스레드가 맥박을 남기는 곳.
#[derive(Default)]
pub struct Heartbeats {
    pub render: AtomicI64,
    pub judge: AtomicI64,
}

/// 화면이 읽는 감시 결과.
#[derive(Default)]
pub struct MonitorShared {
    pub state: AtomicU8,
    pub counts: [AtomicU32; 9],
    /// 최근 사건 (화면 표시용, 최대 6개).
    pub recent: Mutex<Vec<LagEvent>>,
}

struct Stall {
    active: bool,
    start: i64,
    detect: i64,
}

pub fn spawn(
    shared: Arc<MonitorShared>,
    audio: Arc<AudioMonitor>,
    clock: Arc<SharedClock>,
    watermark: Arc<AtomicI64>,
    beats: Arc<Heartbeats>,
    mut log: Producer<LagEvent>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let sh = shared;
    let join = thread::Builder::new()
        .name("monitor".into())
        .spawn(move || {
            let qf = qpc::frequency();
            let ms = |d: i64| d * 1000 / qf;
            let mut stalls: [Stall; 4] = std::array::from_fn(|_| Stall { active: false, start: 0, detect: 0 });
            let kinds = [LagKind::AudioStall, LagKind::RenderStall, LagKind::JudgeStall, LagKind::InputStall];
            let limits = [AUDIO_STALL_MS, RENDER_STALL_MS, JUDGE_STALL_MS, INPUT_STALL_MS];
            let mut underruns = audio.underruns.load(Ordering::Relaxed);
            let mut skipped = audio.skipped_frames.load(Ordering::Relaxed);
            let mut snap = clock.load();
            let mut state = State::Running;
            let mut resync_since: Option<i64> = None;
            // 이번 재동기화가 시작되기 전의 직선 (첫 언더런·점프 때 잡는다)
            let mut pre: Option<synchronizer::shared_clock::ClockSnapshot> = None;
            let push = |e: LagEvent, sh: &MonitorShared, log: &mut Producer<LagEvent>| {
                sh.counts[e.kind as usize].fetch_add(1, Ordering::Relaxed);
                if let Ok(mut r) = sh.recent.lock() {
                    r.push(e);
                    let n = r.len();
                    if n > 6 {
                        r.drain(..n - 6);
                    }
                }
                let _ = log.push(e);
            };
            while !stop.load(Ordering::Relaxed) {
                let now = qpc::now();
                // 맥박: 0이면 아직 시작 전
                let beats_now = [
                    audio.last_wake_qpc.load(Ordering::Acquire),
                    beats.render.load(Ordering::Acquire),
                    beats.judge.load(Ordering::Acquire),
                    watermark.load(Ordering::Acquire),
                ];
                for i in 0..4 {
                    let b = beats_now[i];
                    if b == 0 {
                        continue;
                    }
                    let s = &mut stalls[i];
                    if !s.active && ms(now - b) > limits[i] {
                        s.active = true;
                        s.start = b;
                        s.detect = now;
                    } else if s.active && b > s.start {
                        // 돌아왔다
                        s.active = false;
                        push(LagEvent { kind: kinds[i] as u8, q_start: s.start, q_detect: s.detect, q_end: b, magnitude_us: (b - s.start) * 1_000_000 / qf }, &sh, &mut log);
                    }
                }
                // 언더런
                let u = audio.underruns.load(Ordering::Relaxed);
                if u > underruns {
                    underruns = u;
                    push(LagEvent { kind: LagKind::Underrun as u8, q_start: audio.last_wake_qpc.load(Ordering::Relaxed), q_detect: now, q_end: 0, magnitude_us: 0 }, &sh, &mut log);
                }
                // 콘텐츠 건너뜀 (우리 방식: 시간축은 그대로)
                let sk = audio.skipped_frames.load(Ordering::Relaxed);
                if sk > skipped && snap.valid {
                    let us = ((sk - skipped) as f64 / snap.nominal_rate * 1e6) as i64;
                    skipped = sk;
                    push(LagEvent { kind: LagKind::ContentSkip as u8, q_start: audio.last_wake_qpc.load(Ordering::Relaxed), q_detect: now, q_end: 0, magnitude_us: us }, &sh, &mut log);
                }
                // 시계 점프 (epoch 변경): 같은 순간을 옛 직선과 새 직선이 어디로 보는지
                let ns = clock.load();
                if ns.valid && ns.epoch != snap.epoch {
                    let t100 = (now as i128 * 10_000_000 / qf as i128) as i64;
                    let jump_frames = snap.frame_at(t100) - ns.frame_at(t100);
                    let jump_us = (jump_frames / ns.nominal_rate * 1e6) as i64;
                    push(LagEvent { kind: LagKind::ClockJump as u8, q_start: now, q_detect: now, q_end: 0, magnitude_us: jump_us }, &sh, &mut log);
                    if pre.is_none() {
                        pre = Some(snap);
                    }
                    state = State::Resyncing;
                    resync_since = Some(now);
                }
                if ns.valid {
                    snap = ns;
                }
                // 상태 결정
                let path_stall = stalls[0].active || stalls[2].active || stalls[3].active;
                let next = if state == State::Resyncing {
                    match resync_since {
                        Some(t) if ms(now - t) >= RESYNC_STABLE_MS && !path_stall => State::Running,
                        _ => State::Resyncing,
                    }
                } else if path_stall {
                    State::Suspect
                } else {
                    State::Running
                };
                if state == State::Resyncing && next == State::Running {
                    if let Some(p) = pre.take() {
                        let t100 = (now as i128 * 10_000_000 / qf as i128) as i64;
                        let total_us = ((p.frame_at(t100) - snap.frame_at(t100)) / snap.nominal_rate * 1e6) as i64;
                        push(LagEvent { kind: LagKind::ResyncDone as u8, q_start: now, q_detect: now, q_end: 0, magnitude_us: total_us }, &sh, &mut log);
                    }
                }
                if next != state || sh.state.load(Ordering::Relaxed) != next as u8 {
                    if next != state {
                        push(LagEvent { kind: LagKind::StateChange as u8, q_start: now, q_detect: now, q_end: 0, magnitude_us: next as i64 }, &sh, &mut log);
                    }
                    state = next;
                    sh.state.store(next as u8, Ordering::Release);
                }
                thread::sleep(Duration::from_millis(1));
            }
        })
        .expect("감시 스레드");
    join
}
