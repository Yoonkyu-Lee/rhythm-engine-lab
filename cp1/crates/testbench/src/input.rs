//! 3-2 단계: 입력 도장 정책 비교와 고장 C (입력 스레드 멈춤).
//!
//!   testbench input [--secs 30] [--stalls 10:100,20:300] [--period-ms 250]
//!
//! 가짜 플레이어가 SendInput으로 F24 키를 누르고 뗀다 (누른 길이 5 / 30 / 80 ms 반복). 정답 = SendInput을 부른 순간의 QPC.
//! 정책 세 가지를 동시에 기록한다:
//!   (a) 폴링: 1 kHz로 GetAsyncKeyState를 읽어 바뀐 순간의 QPC (beatoraja · LVF 방식)
//!   (b) 수신 시각: Raw Input 메시지를 받은 순간의 QPC
//!   (c) 메시지 시각: Windows가 메시지에 붙인 GetMessageTime (ms 틱)
//! 고장 C: --stalls 시각(초)에 폴링 스레드와 Raw Input 스레드를 함께 ms 동안 멈춘다.
//! 분석: cp1/tools/analyze_input.py

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use synchronizer::input::{KeyEvent, RawKeyboard};
use synchronizer::qpc;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_F24,
};

use crate::arg;

const HOLDS_MS: [u64; 3] = [5, 30, 80];

fn send_key(down: bool) -> u32 {
    let ki = KEYBDINPUT { wVk: VK_F24, wScan: 0, dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP }, time: 0, dwExtraInfo: 0 };
    let inp = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } };
    unsafe { SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) }
}

/// 남은 시간이 2 ms보다 길면 잠들고, 마지막은 바쁘게 기다려 정해진 QPC에 맞춘다.
fn wait_until(q_target: i64, qf: i64) {
    loop {
        let left_ms = (q_target - qpc::now()) as f64 * 1e3 / qf as f64;
        if left_ms <= 0.0 {
            return;
        }
        if left_ms > 2.0 {
            thread::sleep(Duration::from_secs_f64((left_ms - 1.5) / 1e3));
        } else {
            std::hint::spin_loop();
        }
    }
}

fn parse_stalls(s: &str) -> Vec<(f64, u32)> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.split_once(':').and_then(|(t, ms)| Some((t.parse().ok()?, ms.parse().ok()?))))
        .collect()
}

/// GetTickCount가 바뀌는 순간의 QPC를 `ms` 동안 기록한다 (틱 해상도와 틱 ↔ QPC 대응용).
fn tick_samples(ms: u64, out: &mut Vec<(u32, i64)>) {
    let qf = qpc::frequency();
    let end = qpc::now() + (ms as i64) * qf / 1000;
    let mut last = unsafe { GetTickCount() };
    while qpc::now() < end {
        let t = unsafe { GetTickCount() };
        if t != last {
            out.push((t, qpc::now()));
            last = t;
        }
    }
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let secs: f64 = arg(args, "--secs", "30").parse()?;
    let period_ms: f64 = arg(args, "--period-ms", "250").parse()?;
    let stalls = parse_stalls(&arg(args, "--stalls", "10:100,20:300"));
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let out: PathBuf = Path::new(&arg(args, "--out", "runs")).join(format!("input-{unix}"));
    fs::create_dir_all(&out)?;
    let qf = qpc::frequency();
    unsafe { timeBeginPeriod(1) };

    let mut ticks = Vec::new();
    tick_samples(100, &mut ticks);

    // (b)(c) Raw Input 스레드
    let (ktx, krx) = channel::<KeyEvent>();
    let kb = RawKeyboard::spawn(ktx)?;

    // (a) 폴링 스레드
    let stop = Arc::new(AtomicBool::new(false));
    let q0 = qpc::now() + qf / 2; // 0.5초 뒤를 시간 원점으로
    let poll = {
        let (stop, stalls) = (stop.clone(), stalls.clone());
        let out = out.clone();
        thread::spawn(move || -> std::io::Result<()> {
            unsafe { timeBeginPeriod(1) };
            let mut log = BufWriter::new(File::create(out.join("poll.csv"))?);
            writeln!(log, "down,qpc")?;
            let mut fired = vec![false; stalls.len()];
            let mut last = false;
            while !stop.load(Ordering::Acquire) {
                let q = qpc::now();
                for (i, &(t, ms)) in stalls.iter().enumerate() {
                    if !fired[i] && q >= q0 + (t * qf as f64) as i64 {
                        fired[i] = true;
                        thread::sleep(Duration::from_millis(ms as u64)); // 고장 C: 폴링 스레드 멈춤
                    }
                }
                let q = qpc::now();
                let down = unsafe { GetAsyncKeyState(VK_F24.0 as i32) } as u16 & 0x8000 != 0;
                if down != last {
                    writeln!(log, "{},{q}", down as u8)?;
                    last = down;
                }
                thread::sleep(Duration::from_millis(1));
            }
            unsafe { timeEndPeriod(1) };
            log.flush()
        })
    };

    // 고장 C: Raw Input 스레드 멈춤 (폴링과 같은 시각)
    let ctl = {
        let stalls = stalls.clone();
        let tid = kb.thread_id;
        thread::spawn(move || {
            for (t, ms) in stalls {
                wait_until(q0 + (t * qf as f64) as i64, qf);
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                        tid, windows::Win32::UI::WindowsAndMessaging::WM_APP + 1,
                        windows::Win32::Foundation::WPARAM(ms as usize), windows::Win32::Foundation::LPARAM(0));
                }
            }
        })
    };

    // 가짜 플레이어
    let mut bot = BufWriter::new(File::create(out.join("bot.csv"))?);
    writeln!(bot, "k,down,hold_ms,qpc_before,qpc_after,sent")?;
    let mut k = 0usize;
    loop {
        let t = 1.0 + k as f64 * period_ms / 1e3;
        if t > secs - 0.5 {
            break;
        }
        let hold = HOLDS_MS[k % HOLDS_MS.len()];
        let qd = q0 + (t * qf as f64) as i64;
        wait_until(qd, qf);
        let b = qpc::now();
        let n = send_key(true);
        writeln!(bot, "{k},1,{hold},{b},{},{n}", qpc::now())?;
        wait_until(qd + hold as i64 * qf / 1000, qf);
        let b = qpc::now();
        let n = send_key(false);
        writeln!(bot, "{k},0,{hold},{b},{},{n}", qpc::now())?;
        k += 1;
    }
    bot.flush()?;
    thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Release);
    poll.join().map_err(|_| "폴링 스레드 패닉")??;
    let _ = ctl.join();
    kb.stop();
    tick_samples(100, &mut ticks);
    unsafe { timeEndPeriod(1) };

    let mut raw = BufWriter::new(File::create(out.join("raw.csv"))?);
    writeln!(raw, "vkey,down,qpc,msg_time_ms,tick_ms")?;
    for e in krx.try_iter() {
        writeln!(raw, "{},{},{},{},{}", e.vkey, e.down as u8, e.qpc, e.msg_time_ms, e.tick_ms)?;
    }
    raw.flush()?;
    let mut tk = BufWriter::new(File::create(out.join("ticks.csv"))?);
    writeln!(tk, "tick,qpc")?;
    for (t, q) in &ticks {
        writeln!(tk, "{t},{q}")?;
    }
    tk.flush()?;
    let st: Vec<String> = stalls.iter().map(|(t, ms)| format!("[{t}, {ms}]")).collect();
    fs::write(out.join("meta.json"), format!(
        "{{\"qpc_freq\": {qf}, \"q0\": {q0}, \"secs\": {secs}, \"period_ms\": {period_ms}, \"holds_ms\": [5, 30, 80], \"stalls\": [{}], \"vkey\": {}}}\n",
        st.join(", "), VK_F24.0
    ))?;
    println!("기록: {}", out.display());
    Ok(())
}
