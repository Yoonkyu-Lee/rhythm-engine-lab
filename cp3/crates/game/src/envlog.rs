//! 실행 환경 기록 (`env.csv`): 게임 창이 포그라운드인가 / 앞에 있는 프로그램 / 최소화 / 가려짐(DXGI가 그릴 곳이 안 보인다고 보고) /
//! 시스템 타이머 해상도 / 입력 스레드가 깨어난 간격. 상태가 바뀔 때와 1초마다 한 줄.
//! 왜: Windows 11은 창이 가려지거나 최소화된 프로그램의 타이머 해상도 요청을 무시할 수 있고, 그러면 입력 스레드의 1 ms 대기가
//! 15.6 ms가 되어 누름 도장이 이르게 찍힌다 (CP2 2026-10-16 비교 배치의 ASIO 실행). 그 상태를 실행 기록에서 바로 가리기 위해.
//! 앞 프로그램은 실행 파일 이름만 남긴다 (창 제목은 남기지 않음).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsIconic};

#[link(name = "ntdll")]
extern "system" {
    /// 100 ns 단위: 가장 거친 해상도, 가장 고운 해상도, 지금 해상도.
    fn NtQueryTimerResolution(maximum: *mut u32, minimum: *mut u32, current: *mut u32) -> i32;
}

/// 지금 시스템 타이머 해상도 (µs).
pub fn timer_resolution_us() -> f64 {
    let (mut mx, mut mn, mut cur) = (0u32, 0u32, 0u32);
    unsafe { NtQueryTimerResolution(&mut mx, &mut mn, &mut cur) };
    cur as f64 / 10.0
}

fn foreground_exe() -> (bool, String) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return (false, String::new());
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return (true, String::new());
        }
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return (false, format!("pid {pid}")) };
        let mut buf = [0u16; 512];
        let mut n = buf.len() as u32;
        let name = if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok() {
            let full = String::from_utf16_lossy(&buf[..n as usize]);
            full.rsplit('\\').next().unwrap_or("").to_string()
        } else {
            format!("pid {pid}")
        };
        let _ = CloseHandle(h);
        (false, name)
    }
}

#[derive(Default)]
pub struct EnvSummary {
    pub not_foreground_s: f64,
    pub iconic_s: f64,
    pub occluded_s: f64,
    pub timer_res_max_us: f64,
    pub input_gap_max_us: u64,
}

pub struct EnvLog {
    out: BufWriter<File>,
    qf: i64,
    last_q: i64,
    last_row_q: i64,
    last_state: (bool, String, bool, bool, u32),
    pub sum: EnvSummary,
}

impl EnvLog {
    pub fn new(dir: &Path, qf: i64) -> std::io::Result<EnvLog> {
        let mut out = BufWriter::new(File::create(dir.join("env.csv"))?);
        writeln!(out, "q,song_s,foreground,foreground_exe,iconic,occluded,timer_res_us,input_gap_max_us")?;
        Ok(EnvLog { out, qf, last_q: 0, last_row_q: 0, last_state: (true, String::new(), false, false, 0), sum: EnvSummary::default() })
    }

    /// 프레임마다 부른다. `input_gap_us` = 지난 호출 뒤 입력 스레드가 깨어난 가장 긴 간격 (읽고 0으로 되돌린 값).
    pub fn sample(&mut self, q: i64, song_s: f64, hwnd: HWND, occluded: bool, input_gap_us: u64, gap_acc: &mut u64) -> std::io::Result<()> {
        let (fg, exe) = foreground_exe();
        let iconic = unsafe { IsIconic(hwnd).as_bool() };
        let res = timer_resolution_us();
        let dt = if self.last_q > 0 { (q - self.last_q) as f64 / self.qf as f64 } else { 0.0 };
        self.last_q = q;
        if !fg {
            self.sum.not_foreground_s += dt;
        }
        if iconic {
            self.sum.iconic_s += dt;
        }
        if occluded {
            self.sum.occluded_s += dt;
        }
        self.sum.timer_res_max_us = self.sum.timer_res_max_us.max(res);
        self.sum.input_gap_max_us = self.sum.input_gap_max_us.max(input_gap_us);
        *gap_acc = (*gap_acc).max(input_gap_us);
        let state = (fg, exe, iconic, occluded, res.round() as u32);
        if state != self.last_state || q - self.last_row_q >= self.qf {
            writeln!(self.out, "{q},{song_s:.3},{},{},{},{},{:.1},{}", fg as u8, state.1, iconic as u8, occluded as u8, res, *gap_acc)?;
            *gap_acc = 0;
            self.last_row_q = q;
            self.last_state = state;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}
