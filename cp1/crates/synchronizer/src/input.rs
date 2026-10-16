//! 입력 도장: Raw Input 키보드 이벤트에 시각을 찍는다.
//!
//! Windows의 키보드 Raw Input은 이벤트에 고정밀 시각을 붙여 주지 않는다. 그래서 두 가지를 함께 남긴다.
//! - `qpc`: 이 스레드가 메시지를 받은 순간의 QPC (µs 이하 해상도, 스레드가 늦게 깨면 그만큼 늦음)
//! - `msg_time_ms`: Windows가 메시지에 붙인 시각 (`GetMessageTime`, ms 단위 틱). 스레드가 늦어도 보존되는지 3-2 단계에서 잰다.
//!
//! 전용 스레드가 메시지 전용 창을 만들고 `RIDEV_INPUTSINK`로 등록해, 창이 앞에 없어도 키보드 입력을 받는다.
//!
//! CP1-4: 메시지 시각으로 되돌리는 추정은 틱 경계를 넘으면 거의 기다리지 않은 입력도 한 틱(15–16 ms) 과거로 보낸다
//! (CP2 1차 시도에서 발견). 그래서 "큐가 비어 있음을 마지막으로 확인한 순간"(`floor_qpc`)보다 과거로 가지 않게 자른다.
//! 그 순간 이전의 입력은 이미 모두 넘겼으므로, 이 입력은 그 뒤에 들어온 것이 확실하다. 같은 값이 watermark다.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK,
    RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageTime, MsgWaitForMultipleObjects, PeekMessageW,
    PostThreadMessageW, RegisterClassW, HWND_MESSAGE, MSG, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_INPUT, WM_QUIT, WNDCLASSW,
};

use crate::qpc;

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub vkey: u16,
    pub down: bool,
    pub qpc: i64,
    pub msg_time_ms: u32,
    /// 메시지를 받은 순간의 GetTickCount. msg_time_ms와 같은 틱 시계라서, 둘의 차이 = 메시지가 기다린 시간(틱 해상도).
    pub tick_ms: u32,
    /// 이 입력을 꺼내기 전, 큐가 비어 있음을 마지막으로 확인한 QPC. 누른 시각은 이보다 이를 수 없다.
    pub floor_qpc: i64,
}

impl KeyEvent {
    /// 누른 시각 추정 (QPC 틱). 메시지가 큐에서 기다리지 않았으면 받은 순간의 QPC 그대로 (µs 수준),
    /// 기다렸으면 기다린 틱 수만큼 되돌린다 (틱 해상도 15–16 ms만큼 거칠지만, 입력 스레드가 멈춰도 오차가 그 안에 묶인다).
    /// 3-2 단계 측정: 평상시 +0.04 ms, 입력 스레드 100·300 ms 멈춤 중 +2.9 … +7.2 ms.
    pub fn estimated_qpc(&self, qpc_freq: i64) -> i64 {
        let waited_ms = self.tick_ms.wrapping_sub(self.msg_time_ms) as i64;
        (self.qpc - waited_ms * qpc_freq / 1000).max(self.floor_qpc)
    }
}

/// 입력 스레드에 보내는 "이만큼 멈춰라" 메시지 (고장 C 주입용).
const WM_STALL: u32 = WM_APP + 1;

pub struct RawKeyboard {
    pub thread_id: u32,
    /// 큐가 비어 있음을 마지막으로 확인한 QPC (이 시각 이전의 입력은 모두 보냈다는 생산자 계약). 1 ms마다라도 오른다.
    pub watermark: Arc<AtomicI64>,
    handle: Option<JoinHandle<()>>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

impl RawKeyboard {
    /// 입력 스레드를 띄운다. 키 이벤트는 `tx`로 보낸다.
    pub fn spawn(tx: Sender<KeyEvent>) -> Result<RawKeyboard> {
        let (id_tx, id_rx) = std::sync::mpsc::channel::<std::result::Result<u32, String>>();
        let watermark = Arc::new(AtomicI64::new(0));
        let wm = watermark.clone();
        let handle = thread::spawn(move || {
            let r = unsafe { run(&tx, &id_tx, &wm) };
            if let Err(e) = r {
                let _ = id_tx.send(Err(e.to_string()));
            }
        });
        match id_rx.recv() {
            Ok(Ok(thread_id)) => Ok(RawKeyboard { thread_id, watermark, handle: Some(handle) }),
            Ok(Err(e)) => Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e)),
            Err(_) => Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "입력 스레드가 시작하지 못함")),
        }
    }

    /// 입력 스레드를 `ms` 동안 멈춘다 (고장 C). 그동안 들어온 입력은 OS 메시지 큐에 쌓인다.
    pub fn stall(&self, ms: u32) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_STALL, WPARAM(ms as usize), LPARAM(0));
        }
    }

    pub fn stop(mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

unsafe fn run(tx: &Sender<KeyEvent>, id_tx: &std::sync::mpsc::Sender<std::result::Result<u32, String>>, wm: &AtomicI64) -> Result<()> {
    let hinst = GetModuleHandleW(None)?;
    let class = w!("cp1_raw_keyboard");
    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, hinst, None)?;
    let dev = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32)?;
    let _ = id_tx.send(Ok(GetCurrentThreadId()));

    let mut msg = MSG::default();
    let mut buf = [0u8; 256];
    let mut last_empty = qpc::now();
    wm.store(last_empty, Ordering::Release);
    loop {
        // 1 ms마다라도 깨어 watermark를 올린다 (입력이 없어도 받는 쪽이 "그 시각까지 입력 없음"을 알 수 있게)
        MsgWaitForMultipleObjects(None, false, 1, QS_ALLINPUT);
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let q = qpc::now();
            match msg.message {
                WM_QUIT => return Ok(()),
                WM_INPUT => {
                    let mt = GetMessageTime() as u32;
                    let tk = GetTickCount();
                    let mut size = buf.len() as u32;
                    let n = GetRawInputData(
                        HRAWINPUT(msg.lParam.0 as _),
                        RID_INPUT,
                        Some(buf.as_mut_ptr() as *mut _),
                        &mut size,
                        std::mem::size_of::<RAWINPUTHEADER>() as u32,
                    );
                    if n != u32::MAX && n as usize >= std::mem::size_of::<RAWINPUTHEADER>() {
                        let raw = &*(buf.as_ptr() as *const RAWINPUT);
                        if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
                            let kb = raw.data.keyboard;
                            let down = kb.Flags & 0x01 == 0; // RI_KEY_BREAK = 1
                            let _ = tx.send(KeyEvent { vkey: kb.VKey, down, qpc: q, msg_time_ms: mt, tick_ms: tk, floor_qpc: last_empty });
                        }
                    }
                    DispatchMessageW(&msg);
                }
                m if m == WM_STALL => {
                    std::thread::sleep(std::time::Duration::from_millis(msg.wParam.0 as u64));
                }
                _ => {
                    DispatchMessageW(&msg);
                }
            }
        }
        // 큐가 비었다: 지금 이전의 입력은 모두 넘겼다
        last_empty = qpc::now();
        wm.store(last_empty, Ordering::Release);
    }
}
