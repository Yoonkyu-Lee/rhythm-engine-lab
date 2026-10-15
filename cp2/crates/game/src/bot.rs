//! 자동 플레이어 (측정용). 노트가 "들리는 순간"에 맞춰 OS 입력(SendInput, F24)을 넣는다.
//!
//! 목표 QPC = 장치 시계의 역변환: 곡 시각 T(+오프셋+오차)가 청취되는 스트림 프레임 → 그 프레임이 나가는 QPC.
//! 그래서 봇은 게임 내부 상태(프레임, 판정)를 보지 않고 OS 입력 경로를 그대로 탄다. 판정 오차 = 입력 도장 오차 + 시계 오차 + 의도한 오차.
//! 롱노트는 끝까지 누르고, 단노트는 30 ms 뒤에 뗀다. 실제로 보낸 QPC를 기록한다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rcore::ring::Producer;
use rcore::runtime::Compiled;
use synchronizer::qpc;
use synchronizer::shared_clock::SharedClock;
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_F24};

pub const BOT_KEY: u16 = VK_F24.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct BotLog {
    pub note: u32,
    pub down: bool,
    /// 노리는 청취 곡 시각 (ns, 의도한 오차 포함).
    pub target_ns: i64,
    pub q_target: i64,
    pub q_sent: i64,
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
}

fn send(down: bool) {
    let ki = KEYBDINPUT { wVk: VK_F24, dwFlags: if down { Default::default() } else { KEYEVENTF_KEYUP }, ..Default::default() };
    let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}

/// 청취 곡 시각(ns) → 그 순간의 QPC 틱 (시계가 없으면 None).
fn qpc_for_song(clock: &SharedClock, t_ns: i64, cfg: &BotCfg) -> Option<i64> {
    let s = clock.load();
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

pub fn spawn(chart: Arc<Compiled>, cfg: BotCfg, clock: Arc<SharedClock>, mut log: Producer<BotLog>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
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
            // 이벤트: (청취 시각, 누름?, 노트)
            let mut evs: Vec<(i64, bool, u32)> = Vec::new();
            for (i, n) in chart.chart.lanes[0].notes.iter().enumerate() {
                let j = if cfg.jitter_ns > 0 { (next_rand() % (2 * cfg.jitter_ns as u64 + 1)) as i64 - cfg.jitter_ns } else { 0 };
                let t = n.t + cfg.offset_ns + j;
                let up = if n.end > n.t { n.end + cfg.offset_ns } else { t + 30_000_000 };
                evs.push((t, true, i as u32));
                evs.push((up, false, i as u32));
            }
            evs.sort_by_key(|e| (e.0, e.1)); // 같은 시각이면 뗌 먼저
            for (t, down, note) in evs {
                // 시계가 생길 때까지, 그리고 목표 2초 전까지는 시계를 다시 읽으며 기다린다 (최신 직선 사용)
                let q = loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    match qpc_for_song(&clock, t, &cfg) {
                        Some(q) if q - qpc::now() < 2 * qpc::frequency() => break q,
                        _ => thread::sleep(Duration::from_millis(20)),
                    }
                };
                if !wait_until(q, &stop) {
                    return;
                }
                let q_sent = qpc::now();
                send(down);
                let _ = log.push(BotLog { note, down, target_ns: t, q_target: q, q_sent });
            }
        })
        .expect("봇 스레드")
}
