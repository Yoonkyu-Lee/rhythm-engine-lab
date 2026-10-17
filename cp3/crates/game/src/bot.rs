//! 자동 플레이어 (측정용). 노트가 "들리는 순간"에 맞춰 OS 입력(SendInput)을 넣는다. CP3: 레인마다 봇 키(`bot_key`) — F13~F24, 그 뒤는 숫자 패드.
//!
//! 목표 QPC = 장치 시계의 역변환: 곡 시각 T(+오프셋+오차)가 청취되는 스트림 프레임 → 그 프레임이 나가는 QPC.
//! 그래서 봇은 게임 내부 상태(프레임, 판정)를 보지 않고 OS 입력 경로를 그대로 탄다. 판정 오차 = 입력 도장 오차 + 시계 오차 + 의도한 오차.
//! 롱노트는 끝까지 누르고, 단노트는 30 ms 뒤에 뗀다. 실제로 보낸 QPC를 기록한다.
//! 프레임 구동 프로필(CP2 2차 2-4): `follow`가 있으면 장치 시계 대신 그 게임의 시계로 친다 — 그 게임 화면을 보고 PERFECT 순간에
//! 누르는 사람. 게임이 프레임마다 (프레임 QPC, 그때의 곡 시각)을 공개하고, 봇은 시각이 1배속으로 흐른다고 보고 목표 시각을 구한다.
//! 오토 토글: `enabled`가 꺼져 있으면 그 노트는 누르지 않는다 (이미 누른 노트는 끝까지 뗀다). 게임에서 F5로 켜고 끈다.
//! 사람 흉내(adapt_ns > 0): 음악이 밀려 장치 시계가 새 epoch로 바뀌어도 그 뒤 adapt_ns 동안은 옛 시계(옛 박자)로 치다가 그다음에 새 박자를 따른다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use engine::ring::Producer;
use std::sync::atomic::AtomicI64;
use synchronizer::input::KeyEvent;
use engine::runtime::{Compiled, NoteKind};
use synchronizer::qpc;
use synchronizer::shared_clock::{ClockSnapshot, SharedClock};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY};

/// 레인 `i`의 봇 키: F13(0x7C)부터 F24까지, 그 뒤는 숫자 패드 0(0x60)부터. 사람 키 배치와 겹치지 않는다.
pub fn bot_key(i: usize) -> u16 {
    if i < 12 { 0x7C + i as u16 } else { 0x60 + (i - 12) as u16 }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BotLog {
    pub lane: u16,
    pub note: u32,
    pub down: bool,
    /// 노리는 청취 곡 시각 (ns, 의도한 오차 포함).
    pub target_ns: i64,
    pub q_target: i64,
    pub q_sent: i64,
}

/// 프레임 구동 프로필이 공개하는 자기 시계 (마지막 프레임의 QPC와 그때의 곡 시각 ns). 쓰는 쪽: t → q 순서, 읽는 쪽: q·t·q 확인.
#[derive(Default)]
pub struct ProfileClock {
    pub q: std::sync::atomic::AtomicI64,
    pub t: std::sync::atomic::AtomicI64,
}

impl ProfileClock {
    pub fn publish(&self, q: i64, t_ns: i64) {
        self.q.store(0, Ordering::Release);
        self.t.store(t_ns, Ordering::Release);
        self.q.store(q, Ordering::Release);
    }
    pub fn read(&self) -> Option<(i64, i64)> {
        for _ in 0..4 {
            let q = self.q.load(Ordering::Acquire);
            let t = self.t.load(Ordering::Acquire);
            if q != 0 && q == self.q.load(Ordering::Acquire) {
                return Some((q, t));
            }
        }
        None
    }
}

pub struct BotCfg {
    pub origin: i64,
    pub rate: u32,
    pub out_offset_ns: i64,
    /// 모든 누름에 더하는 오차 (ns, +면 늦게).
    pub offset_ns: i64,
    /// 균등 무작위 오차 ±jitter_ns (0이면 없음), 결정적 시드.
    pub jitter_ns: i64,
    pub seed: u64,
    /// 사람 흉내: 시계 점프를 알아채기까지 걸리는 시간 (ns, 0이면 즉시 따름).
    pub adapt_ns: i64,
    /// 그 게임의 시계로 친다 (프레임 구동 프로필).
    pub follow: Option<Arc<ProfileClock>>,
    /// 오토 켜짐 (누름 순간에 본다).
    pub enabled: Arc<AtomicBool>,
    /// 직접 입력 (`--bot-direct`): OS 입력(SendInput) 대신 입력 링에 넣고 watermark를 올린다. 앞 창이 더 높은 권한이면
    /// SendInput이 조용히 버려지므로(UIPI) 사람이 없을 때의 기능 시험용. OS 입력 경로 지연은 재지 않는다.
    pub direct: Option<Arc<AtomicI64>>,
}

/// 봇이 믿는 시계: 같은 epoch 안에서는 최신 직선, 새 epoch는 처음 본 뒤 adapt_ns가 지나야 받아들인다.
struct View {
    seen: ClockSnapshot,
    pending_since: Option<i64>,
    adapt_q: i64,
}

impl View {
    fn get(&mut self, clock: &SharedClock) -> ClockSnapshot {
        let s = clock.load();
        if self.adapt_q == 0 || !self.seen.valid || s.epoch == self.seen.epoch {
            self.seen = s;
            self.pending_since = None;
        } else {
            let now = qpc::now();
            let since = *self.pending_since.get_or_insert(now);
            if now - since >= self.adapt_q {
                self.seen = s;
                self.pending_since = None;
            }
        }
        self.seen
    }
}

fn send(vk: u16, down: bool) {
    let ki = KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: if down { Default::default() } else { KEYEVENTF_KEYUP }, ..Default::default() };
    let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
    let n = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if n != 1 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| eprintln!("봇: SendInput 실패 (키 {vk:#x}, 오류 {:?})", unsafe { windows::Win32::Foundation::GetLastError() }));
    }
}

/// 청취 곡 시각(ns) → 그 순간의 QPC 틱 (시계가 없으면 None).
fn qpc_for_song(view: &mut View, clock: &SharedClock, t_ns: i64, cfg: &BotCfg) -> Option<i64> {
    if let Some(pc) = cfg.follow.as_ref() {
        let (q0, t0) = pc.read()?;
        return Some(q0 + ((t_ns - t0) as i128 * qpc::frequency() as i128 / 1_000_000_000) as i64);
    }
    let s = view.get(clock);
    if !s.valid {
        return None;
    }
    let frame = cfg.origin as f64 + (t_ns - cfg.out_offset_ns) as f64 * cfg.rate as f64 / 1e9;
    let t100 = s.t100_at(frame);
    Some((t100 as i128 * qpc::frequency() as i128 / 10_000_000) as i64)
}

/// 목표 QPC까지 기다린다: 2 ms 전까지는 잠들고 마지막은 돌면서.
fn wait_until(q: i64, stop: &AtomicBool) -> bool {
    let f = qpc::frequency();
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let left = q - qpc::now();
        if left <= 0 {
            return true;
        }
        if left > 2 * f / 1000 {
            thread::sleep(Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}

pub fn spawn(chart: Arc<Compiled>, cfg: BotCfg, clock: Arc<SharedClock>, mut log: Producer<BotLog>, stop: Arc<AtomicBool>, mut keys: Option<Producer<KeyEvent>>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("bot".into())
        .spawn(move || {
            let mut rng = cfg.seed | 1;
            let mut next_rand = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            let mut view = View { seen: ClockSnapshot::default(), pending_since: None, adapt_q: (cfg.adapt_ns as i128 * qpc::frequency() as i128 / 1_000_000_000) as i64 };
            // 이벤트: (청취 시각, 누름?, 레인, 노트). 지뢰는 누르지 않는다. 단노트는 30 ms 뒤(다음 노트가 더 가까우면 그 2 ms 전)에 뗀다.
            let mut evs: Vec<(i64, bool, usize, u32)> = Vec::new();
            for (li, lane) in chart.chart.lanes.iter().enumerate() {
                let playable: Vec<usize> = (0..lane.notes.len()).filter(|&i| lane.notes[i].kind != NoteKind::Mine).collect();
                for (k, &i) in playable.iter().enumerate() {
                    let n = lane.notes[i];
                    let j = if cfg.jitter_ns > 0 { (next_rand() % (2 * cfg.jitter_ns as u64 + 1)) as i64 - cfg.jitter_ns } else { 0 };
                    let t = n.t + cfg.offset_ns + j;
                    let next = playable.get(k + 1).map(|&x| lane.notes[x].t + cfg.offset_ns);
                    let up = if n.end > n.t { n.end + cfg.offset_ns } else { (t + 30_000_000).min(next.map_or(i64::MAX, |x| x - 2_000_000)).max(t + 1_000_000) };
                    evs.push((t, true, li, i as u32));
                    evs.push((up, false, li, i as u32));
                }
            }
            evs.sort_by_key(|e| (e.0, e.1)); // 같은 시각이면 뗌 먼저
            let mut held: Vec<Vec<bool>> = chart.chart.lanes.iter().map(|l| vec![false; l.notes.len()]).collect();
            for (t, down, lane, note) in evs {
                // 누르지 않은 노트는 떼지 않는다. 누름은 그 시각까지 기다린 뒤 오토가 켜져 있을 때만
                if !down && !held[lane][note as usize] {
                    continue;
                }
                // 시계가 생길 때까지, 그리고 목표 2초 전까지는 시계를 다시 읽으며 기다린다 (최신 직선 사용)
                let q = loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    match qpc_for_song(&mut view, &clock, t, &cfg) {
                        Some(q) if q - qpc::now() < 2 * qpc::frequency() => break q,
                        _ => {
                            if let Some(wm) = cfg.direct.as_ref() {
                                wm.store(qpc::now(), Ordering::Release);
                            }
                            thread::sleep(Duration::from_millis(5))
                        }
                    }
                };
                // 기다리는 동안에도 시계를 다시 읽는다: 언더런으로 음악이 밀리면 누를 시각도 같이 밀린다 (음악을 따라 치는 플레이어)
                let mut q = q;
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Some(nq) = qpc_for_song(&mut view, &clock, t, &cfg) {
                        q = nq;
                    }
                    let left = q - qpc::now();
                    if let Some(wm) = cfg.direct.as_ref() {
                        wm.store(qpc::now(), Ordering::Release);
                    }
                    if left <= 3 * qpc::frequency() / 1000 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                if !wait_until(q, &stop) {
                    return;
                }
                if down && !cfg.enabled.load(Ordering::Relaxed) {
                    continue; // 오토 꺼짐: 이 노트는 건너뜀
                }
                held[lane][note as usize] = down;
                let q_sent = qpc::now();
                match (keys.as_mut(), cfg.direct.as_ref()) {
                    (Some(k), Some(wm)) => {
                        let _ = k.push(KeyEvent { vkey: bot_key(lane), down, qpc: q_sent, msg_time_ms: 0, tick_ms: 0, floor_qpc: q_sent, recv_qpc: q_sent });
                        wm.store(qpc::now(), Ordering::Release);
                    }
                    _ => send(bot_key(lane), down),
                }
                let _ = log.push(BotLog { lane: lane as u16, note, down, target_ns: t, q_target: q, q_sent });
            }
            // 직접 입력: 봇이 끝나도 watermark를 계속 올린다 (남은 놓침·롱노트 확정)
            if let Some(wm) = cfg.direct.as_ref() {
                while !stop.load(Ordering::Relaxed) {
                    wm.store(qpc::now(), Ordering::Release);
                    thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .expect("봇 스레드")
}
