//! CP1-4 4-3: "PERFECT 누름" — 정확히 친 키음이 반주와 같은 샘플에 놓이는가.
//!
//!   testbench sched   --mode perfect --d-ms 43 [--wasapi shared|exclusive] [--faults A@t:ms,B@t:ms,C@t:ms]
//!   testbench asiorun --mode perfect --d-ms 3 --buffer 64 [--faults ...]
//!
//! - 반주(왼쪽 채널): 시간축 프레임 T_k(0.5007초마다)에 클릭을 미리 예약한다. 실제 엔진에서 채보를 오디오 스레드가 직접 훑는 것과 같은 자리.
//! - 봇: 기록 시점 C(q) = 장치 시계(q) + D 가 T_k가 되는 순간(= 장치 시계가 T_k − D인 순간)에 실제 F24 키를 SendInput으로 누른다.
//!   누르기 직전까지 시계를 다시 읽는다 (오디오 정지로 시간축이 건너뛰어도 따라감).
//! - 판정 스레드: Raw Input으로 받은 입력 시각(도장) q를 C(q)로 바꿔, 그 프레임에 키음(오른쪽 채널)을 예약한다.
//! - 루프백 녹음에서 오른쪽 시작점 − 왼쪽 시작점 = 키음 vs 반주. 엔진이 맞으면 = 입력 전달 지연(SendInput → 도장)뿐이고,
//!   그것을 뺀 나머지는 0 샘플이어야 한다. 분석: tools/analyze_perfect.py
//!
//! 고장: A = 오디오 스레드 정지(렌더 루프), B = 판정 스레드 정지, C = 입력 스레드 정지.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use synchronizer::input::{KeyEvent, RawKeyboard};
use synchronizer::qpc;
use synchronizer::scheduler::Cmd;
use synchronizer::shared_clock::SharedClock;
use synchronizer::timeline::commit_frame;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_F24};

use crate::sched::{Fault, EVENT_START_S};

/// 키음 소리 번호 (믹서: 0 = 반주, 1 = 요청 탐침 무음, 2 = 키음).
pub(crate) const KEYSOUND: u32 = 2;

fn send_key(down: bool) {
    let ki = KEYBDINPUT { wVk: VK_F24, wScan: 0, dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP }, time: 0, dwExtraInfo: 0 };
    let inp = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
    unsafe { SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) };
}

fn t100(q: i64) -> i64 {
    (q as i128 * 10_000_000 / qpc::frequency() as i128) as i64
}

fn qpc_of_t100(t: f64) -> i64 {
    (t * qpc::frequency() as f64 / 1e7) as i64
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn perfect_loop(
    out: &Path, secs: f64, event_period: f64, d_ms: f64, faults: Vec<Fault>, tx: Sender<Cmd>, clock: Arc<SharedClock>,
    render_done: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe { timeBeginPeriod(1) };
    let rate = loop {
        let s = clock.load();
        if s.valid {
            break s.nominal_rate;
        }
        if render_done.load(Ordering::Acquire) {
            unsafe { timeEndPeriod(1) };
            return Err("렌더가 시계를 만들기 전에 끝남".into());
        }
        thread::sleep(Duration::from_millis(1));
    };
    let d = d_ms * 1e-3 * rate;
    let mut ev = Vec::new();
    let mut k = 0;
    loop {
        let t = EVENT_START_S + 1.0 + k as f64 * event_period;
        if t > secs - 0.5 {
            break;
        }
        ev.push((t * rate).round() as u64);
        k += 1;
    }
    let qf = qpc::frequency();
    let stop = Arc::new(AtomicBool::new(false));

    // 판정 스레드: 입력 도장 → C → 키음 예약. 고장 B(판정 정지)와 C(입력 정지)도 여기서 건다.
    let (ktx, krx) = channel::<KeyEvent>();
    let kb = RawKeyboard::spawn(ktx)?;
    let judge = {
        let (clock, tx, stop, out) = (clock.clone(), tx.clone(), stop.clone(), out.to_path_buf());
        let faults = faults.clone();
        let input_thread = kb.thread_id;
        thread::spawn(move || -> Result<(), String> {
            let mut log = BufWriter::new(File::create(out.join("judge.csv")).map_err(|e| e.to_string())?);
            writeln!(log, "q_recv,q_stamp,c_frame,q_proc,vkey").map_err(|e| e.to_string())?;
            let mut flog = BufWriter::new(File::create(out.join("faults_judge.csv")).map_err(|e| e.to_string())?);
            writeln!(flog, "kind,at_s,ms,qpc").map_err(|e| e.to_string())?;
            let mut fired = vec![false; faults.len()];
            while !stop.load(Ordering::Relaxed) {
                let now_frame = clock.load().frame_at(t100(qpc::now()));
                for (i, f) in faults.iter().enumerate() {
                    if !fired[i] && now_frame >= f.at_s * rate {
                        fired[i] = true;
                        let _ = writeln!(flog, "{},{},{},{}", f.kind, f.at_s, f.ms, qpc::now());
                        match f.kind {
                            'B' => thread::sleep(Duration::from_millis(f.ms)),
                            'C' => unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                                    input_thread,
                                    windows::Win32::UI::WindowsAndMessaging::WM_APP + 1,
                                    windows::Win32::Foundation::WPARAM(f.ms as usize),
                                    windows::Win32::Foundation::LPARAM(0),
                                );
                            },
                            _ => {}
                        }
                    }
                }
                match krx.recv_timeout(Duration::from_millis(1)) {
                    Ok(e) if e.down && e.vkey == VK_F24.0 => {
                        let q = e.estimated_qpc(qf);
                        let c = commit_frame(&clock.load(), t100(q), d);
                        let _ = tx.send(Cmd::At { frame: c.round().max(0.0) as u64, sound: KEYSOUND, channel: 1 });
                        let _ = writeln!(log, "{},{q},{c:.3},{},{}", e.qpc, qpc::now(), e.vkey);
                    }
                    _ => {}
                }
            }
            log.flush().map_err(|e| e.to_string())?;
            flog.flush().map_err(|e| e.to_string())?;
            Ok(())
        })
    };

    // 봇: 장치 시계가 T_k − D가 되는 순간에 누른다
    let bot = {
        let (clock, stop, out, ev) = (clock.clone(), stop.clone(), out.to_path_buf(), ev.clone());
        thread::spawn(move || -> Result<(), String> {
            let mut log = BufWriter::new(File::create(out.join("bot.csv")).map_err(|e| e.to_string())?);
            writeln!(log, "k,event_frame,q_target,q_sent").map_err(|e| e.to_string())?;
            for (k, &f) in ev.iter().enumerate() {
                let target = |c: &SharedClock| qpc_of_t100(c.load().t100_at(f as f64 - d));
                let mut q;
                // 2 ms 전까지는 시계를 다시 읽으며 잠들고, 마지막은 돌면서 기다린다
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    q = target(&clock);
                    let left = q - qpc::now();
                    if left <= 3 * qf / 1000 {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                while qpc::now() < q {
                    std::hint::spin_loop();
                }
                let sent = qpc::now();
                send_key(true);
                writeln!(log, "{k},{f},{q},{sent}").map_err(|e| e.to_string())?;
                thread::sleep(Duration::from_millis(30));
                send_key(false);
            }
            log.flush().map_err(|e| e.to_string())?;
            Ok(())
        })
    };

    // 반주: 채보를 오디오 쪽이 미리 아는 것처럼 0.5초 앞서 예약 (게임·판정 정지와 무관하게)
    let mut slog = BufWriter::new(File::create(out.join("game_sends.csv"))?);
    writeln!(slog, "k,event_frame,send_qpc,now_frame")?;
    let lookahead = 0.5 * rate;
    let mut k = 0;
    while k < ev.len() && !render_done.load(Ordering::Acquire) {
        let q = qpc::now();
        let now_frame = clock.load().frame_at(t100(q));
        while k < ev.len() && ev[k] as f64 <= now_frame + lookahead {
            tx.send(Cmd::At { frame: ev[k], sound: 0, channel: 0 })?;
            writeln!(slog, "{k},{},{q},{now_frame:.1}", ev[k])?;
            k += 1;
        }
        thread::sleep(Duration::from_millis(2));
    }
    slog.flush()?;
    let b = bot.join().map_err(|_| "봇 스레드 패닉")?;
    thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Release);
    let j = judge.join().map_err(|_| "판정 스레드 패닉")?;
    kb.stop();
    unsafe { timeEndPeriod(1) };
    b?;
    j?;
    Ok(())
}
