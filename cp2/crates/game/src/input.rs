//! 입력 스레드: Raw Input 키보드 + watermark.
//!
//! - 시각: CP1 입력 도장 (c). 받은 순간의 QPC에서 메시지가 큐에서 기다린 틱만큼 되돌린 값 (`KeyEvent::estimated_qpc`).
//!   단, 틱(약 15.6 ms) 해상도라 기다리는 사이 틱 경계를 넘으면 거의 안 기다렸어도 한 틱만큼 과거로 간다 (CP2 2-2에서 발견).
//!   이 스레드가 "큐가 비어 있다"고 확인한 마지막 QPC보다 앞일 수는 없으므로, 그 시각으로 자른다.
//!   자른 결과를 `qpc`에 넣고 `msg_time_ms = tick_ms`로 두어, 받는 쪽의 `estimated_qpc()`가 이 값을 그대로 돌려주게 한다.
//! - watermark: 메시지 큐가 비어 있음을 확인한 순간의 QPC. "이 시각 이전에 일어난 키 입력은 모두 넘겼다"는 생산자 계약이다.
//!   이 스레드가 멈추면 watermark도 멈춰서, 판정 스레드는 MISS 확정을 보류한다 (설계안 7장).
//! - 키 이벤트는 SPSC 링으로 판정 스레드에 넘긴다. 링이 가득 차면 버리고 센다 (손실은 기록으로 남는다).
//! - 창이 앞에 없어도 받도록 `RIDEV_INPUTSINK`. 고장 주입: `stall(ms)` 메시지를 받으면 그만큼 잔다 (OS 큐에 입력이 쌓임).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use rcore::ring::Producer;
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT,
    RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageTime, MsgWaitForMultipleObjects, PeekMessageW,
    PostThreadMessageW, RegisterClassW, HWND_MESSAGE, MSG, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_INPUT, WM_QUIT, WNDCLASSW,
};

const WM_STALL: u32 = WM_APP + 1;

pub struct Input {
    /// Raw Input 스레드 ID (폴링이면 0).
    pub thread_id: u32,
    /// 폴링 스레드용 정지 요청 (ms).
    poll_stall: Arc<std::sync::atomic::AtomicU32>,
    /// QPC 틱. 0이면 아직 없음.
    pub watermark: Arc<AtomicI64>,
    pub lost: Arc<AtomicU64>,
    /// 입력 스레드가 깨어난 간격: 최대(µs)와 2 ms 넘은 횟수 (1 ms 대기가 지켜지는지)
    pub gap_max_us: Arc<AtomicU64>,
    pub gap_over_2ms: Arc<AtomicU64>,
    /// 입력 스레드를 MMCSS(Pro Audio)에 등록했나
    pub mmcss: bool,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

impl Input {
    pub fn spawn(mut tx: Producer<KeyEvent>, mmcss: bool) -> Result<Input, String> {
        let watermark = Arc::new(AtomicI64::new(0));
        let lost = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let gap_max_us = Arc::new(AtomicU64::new(0));
        let gap_over_2ms = Arc::new(AtomicU64::new(0));
        let (id_tx, id_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        let (wm, lo, st, gm, go) = (watermark.clone(), lost.clone(), stop.clone(), gap_max_us.clone(), gap_over_2ms.clone());
        let join = thread::Builder::new()
            .name("input".into())
            .spawn(move || {
                if mmcss {
                    let _ = synchronizer::wasapi::mmcss_pro_audio();
                }
                let r = unsafe { run(&mut tx, &wm, &lo, &st, &id_tx, &gm, &go) };
                if let Err(e) = r {
                    let _ = id_tx.send(Err(e.to_string()));
                }
            })
            .map_err(|e| e.to_string())?;
        let thread_id = id_rx.recv().map_err(|_| "입력 스레드가 시작하지 못함".to_string())??;
        Ok(Input { thread_id, poll_stall: Arc::new(std::sync::atomic::AtomicU32::new(0)), watermark, lost, gap_max_us, gap_over_2ms, mmcss, stop, join: Some(join) })
    }

    /// BMS 구동기식 입력: 1 kHz로 키 상태를 폴링하고, 바뀐 것을 본 폴링 시각을 입력 시각으로 쓴다.
    /// 폴링 사이(최대 1 ms)에 누르고 뗀 짧은 입력은 놓친다 (CP1 3-2). watermark = 마지막 폴링 시각.
    pub fn spawn_poll(mut tx: Producer<KeyEvent>, keys: Vec<u16>) -> Result<Input, String> {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        let watermark = Arc::new(AtomicI64::new(0));
        let lost = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let poll_stall = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (wm, lo, st, ps) = (watermark.clone(), lost.clone(), stop.clone(), poll_stall.clone());
        let join = thread::Builder::new()
            .name("input-poll".into())
            .spawn(move || {
                let freq = qpc::frequency();
                let mut down = vec![false; keys.len()];
                let mut next = qpc::now();
                while !st.load(Ordering::Relaxed) {
                    let req = ps.swap(0, Ordering::AcqRel);
                    if req > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(req as u64));
                    }
                    let q = qpc::now();
                    for (i, &k) in keys.iter().enumerate() {
                        let d = unsafe { GetAsyncKeyState(k as i32) } as u16 & 0x8000 != 0;
                        if d != down[i] {
                            down[i] = d;
                            let t = (q * 1000 / freq) as u32;
                            if !tx.push(KeyEvent { vkey: k, down: d, qpc: q, msg_time_ms: t, tick_ms: t, floor_qpc: q, recv_qpc: q }) {
                                lo.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    wm.store(q, Ordering::Release);
                    next += freq / 1000;
                    let now = qpc::now();
                    if next > now {
                        std::thread::sleep(std::time::Duration::from_micros(((next - now) * 1_000_000 / freq) as u64));
                    } else {
                        next = now;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Input { thread_id: 0, poll_stall, watermark, lost, gap_max_us: Arc::new(AtomicU64::new(0)), gap_over_2ms: Arc::new(AtomicU64::new(0)), mmcss: false, stop, join: Some(join) })
    }

    /// 입력 스레드를 `ms` 동안 재운다 (고장 C).
    pub fn stall(&self, ms: u32) {
        if self.thread_id == 0 {
            self.poll_stall.store(ms, Ordering::Release);
            return;
        }
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_STALL, WPARAM(ms as usize), LPARAM(0));
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

unsafe fn run(
    tx: &mut Producer<KeyEvent>,
    wm: &AtomicI64,
    lost: &AtomicU64,
    stop: &AtomicBool,
    id_tx: &std::sync::mpsc::Sender<Result<u32, String>>,
    gap_max_us: &AtomicU64,
    gap_over_2ms: &AtomicU64,
) -> windows::core::Result<()> {
    let hinst = GetModuleHandleW(None)?;
    let class = w!("cp2_raw_keyboard");
    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, hinst, None)?;
    let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32)?;
    let _ = id_tx.send(Ok(GetCurrentThreadId()));
    let mut msg = MSG::default();
    let mut buf = [0u8; 256];
    let freq = qpc::frequency();
    let mut last_empty = qpc::now();
    while !stop.load(Ordering::Relaxed) {
        // 1 ms마다라도 깨어 watermark를 올린다 (입력이 없어도 MISS 확정이 진행되게)
        MsgWaitForMultipleObjects(None, false, 1, QS_ALLINPUT);
        // 깨어난 간격 (직전 '큐가 비었다' 확인부터): 1 ms 대기가 실제로 몇 ms였나
        let gap_us = ((qpc::now() - last_empty) as i128 * 1_000_000 / freq as i128) as u64;
        gap_max_us.fetch_max(gap_us, Ordering::Relaxed);
        if gap_us > 2000 {
            gap_over_2ms.fetch_add(1, Ordering::Relaxed);
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let q = qpc::now();
            match msg.message {
                WM_QUIT => return Ok(()),
                WM_INPUT => {
                    let mt = GetMessageTime() as u32;
                    let tk = GetTickCount();
                    let mut size = buf.len() as u32;
                    let n = GetRawInputData(HRAWINPUT(msg.lParam.0 as _), RID_INPUT, Some(buf.as_mut_ptr() as *mut _), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
                    if n != u32::MAX && n as usize >= std::mem::size_of::<RAWINPUTHEADER>() {
                        let raw = &*(buf.as_ptr() as *const RAWINPUT);
                        if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
                            let kb = raw.data.keyboard;
                            let down = kb.Flags & 0x01 == 0;
                            let raw_ev = KeyEvent { vkey: kb.VKey, down, qpc: q, msg_time_ms: mt, tick_ms: tk, floor_qpc: last_empty, recv_qpc: q };
                            let est = raw_ev.estimated_qpc(freq).max(last_empty);
                            if !tx.push(KeyEvent { qpc: est, msg_time_ms: tk, ..raw_ev }) {
                                lost.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    DispatchMessageW(&msg);
                }
                m if m == WM_STALL => std::thread::sleep(std::time::Duration::from_millis(msg.wParam.0 as u64)),
                _ => {
                    DispatchMessageW(&msg);
                }
            }
        }
        // 큐가 비었다: 지금 이전의 입력은 모두 넘겼다
        last_empty = qpc::now();
        wm.store(last_empty, Ordering::Release);
    }
    Ok(())
}
