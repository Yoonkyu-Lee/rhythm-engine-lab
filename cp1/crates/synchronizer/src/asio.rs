//! ASIO 호스트 (최소 구현). Steinberg SDK 없이, 드라이버가 노출하는 COM 인터페이스(IASIO)를 직접 부른다.
//!
//! ASIO 드라이버는 `HKLM\SOFTWARE\ASIO\<이름>` 의 CLSID로 등록된 COM 객체다. CLSID를 인터페이스 ID로도 써서
//! CoCreateInstance로 만들고, C++ 가상 함수표를 순서대로 부른다 (x64에서는 호출 규약이 하나라 extern "system").
//! 함수표 순서와 구조체 배치는 공개된 ASIO 인터페이스를 기억으로 적은 것이라, 프로브(이름·채널·버퍼 조회)로 먼저 검증한다.
//!
//! 콜백에는 사용자 포인터가 없으므로 드라이버는 프로세스에 하나만 연다 (전역 상태).

use std::ffi::c_void;

use windows::core::{GUID, HRESULT, PCWSTR};
use windows::Win32::System::Com::{CLSIDFromString, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};

pub type AsioError = i32;
pub const ASE_OK: AsioError = 0;
pub const ASE_SUCCESS: AsioError = 0x3f48_47a0;

pub const ASIOST_INT16_LSB: i32 = 16;
pub const ASIOST_INT24_LSB: i32 = 17;
pub const ASIOST_INT32_LSB: i32 = 18;
pub const ASIOST_FLOAT32_LSB: i32 = 19;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AsioSamples {
    pub hi: u32,
    pub lo: u32,
}
impl AsioSamples {
    pub fn get(&self) -> u64 {
        ((self.hi as u64) << 32) | self.lo as u64
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsioChannelInfo {
    pub channel: i32,
    pub is_input: i32,
    pub is_active: i32,
    pub channel_group: i32,
    pub sample_type: i32,
    pub name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsioBufferInfo {
    pub is_input: i32,
    pub channel_num: i32,
    pub buffers: [*mut c_void; 2],
}

#[repr(C)]
pub struct AsioCallbacks {
    pub buffer_switch: extern "C" fn(index: i32, direct: i32),
    pub sample_rate_did_change: extern "C" fn(rate: f64),
    pub asio_message: extern "C" fn(selector: i32, value: i32, message: *mut c_void, opt: *mut f64) -> i32,
    pub buffer_switch_time_info: extern "C" fn(params: *mut c_void, index: i32, direct: i32) -> *mut c_void,
}

/// IASIO 가상 함수표 (IUnknown 3개 다음부터 ASIO 함수).
#[repr(C)]
struct Vtbl {
    query_interface: usize,
    add_ref: extern "system" fn(*mut c_void) -> u32,
    release: extern "system" fn(*mut c_void) -> u32,
    init: extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_driver_name: extern "system" fn(*mut c_void, *mut u8),
    get_driver_version: extern "system" fn(*mut c_void) -> i32,
    get_error_message: extern "system" fn(*mut c_void, *mut u8),
    start: extern "system" fn(*mut c_void) -> AsioError,
    stop: extern "system" fn(*mut c_void) -> AsioError,
    get_channels: extern "system" fn(*mut c_void, *mut i32, *mut i32) -> AsioError,
    get_latencies: extern "system" fn(*mut c_void, *mut i32, *mut i32) -> AsioError,
    get_buffer_size: extern "system" fn(*mut c_void, *mut i32, *mut i32, *mut i32, *mut i32) -> AsioError,
    can_sample_rate: extern "system" fn(*mut c_void, f64) -> AsioError,
    get_sample_rate: extern "system" fn(*mut c_void, *mut f64) -> AsioError,
    set_sample_rate: extern "system" fn(*mut c_void, f64) -> AsioError,
    get_clock_sources: usize,
    set_clock_source: usize,
    get_sample_position: extern "system" fn(*mut c_void, *mut AsioSamples, *mut AsioSamples) -> AsioError,
    get_channel_info: extern "system" fn(*mut c_void, *mut AsioChannelInfo) -> AsioError,
    create_buffers: extern "system" fn(*mut c_void, *mut AsioBufferInfo, i32, i32, *const AsioCallbacks) -> AsioError,
    dispose_buffers: extern "system" fn(*mut c_void) -> AsioError,
    control_panel: usize,
    future: extern "system" fn(*mut c_void, i32, *mut c_void) -> AsioError,
    output_ready: extern "system" fn(*mut c_void) -> AsioError,
}

#[link(name = "ole32")]
extern "system" {
    fn CoCreateInstance(clsid: *const GUID, outer: *mut c_void, ctx: u32, iid: *const GUID, out: *mut *mut c_void) -> HRESULT;
}

pub struct DriverEntry {
    pub name: String,
    pub clsid: GUID,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `HKLM\SOFTWARE\ASIO` 아래 등록된 드라이버.
pub fn drivers() -> Vec<DriverEntry> {
    let mut out = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        let path = wide("SOFTWARE\\ASIO");
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut key).is_err() {
            return out;
        }
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, windows::core::PWSTR(name.as_mut_ptr()), &mut len, None, windows::core::PWSTR::null(), None, None).is_err() {
                break;
            }
            let sub = String::from_utf16_lossy(&name[..len as usize]);
            let mut val = [0u16; 64];
            let mut bytes = (val.len() * 2) as u32;
            let subw = wide(&sub);
            let clsid_name = wide("CLSID");
            if RegGetValueW(key, PCWSTR(subw.as_ptr()), PCWSTR(clsid_name.as_ptr()), RRF_RT_REG_SZ, None, Some(val.as_mut_ptr() as *mut c_void), Some(&mut bytes)).is_ok() {
                if let Ok(clsid) = CLSIDFromString(PCWSTR(val.as_ptr())) {
                    out.push(DriverEntry { name: sub, clsid });
                }
            }
        }
        let _ = RegCloseKey(key);
    }
    out
}

/// 열린 ASIO 드라이버. 같은 스레드(STA로 COM 초기화한 스레드)에서만 쓴다.
pub struct Driver {
    this: *mut c_void,
}

impl Driver {
    pub fn open(entry: &DriverEntry) -> Result<Driver, String> {
        unsafe {
            let mut p: *mut c_void = std::ptr::null_mut();
            let hr = CoCreateInstance(&entry.clsid, std::ptr::null_mut(), CLSCTX_INPROC_SERVER.0, &entry.clsid, &mut p);
            if hr.is_err() || p.is_null() {
                return Err(format!("CoCreateInstance 실패: {hr:?}"));
            }
            let d = Driver { this: p };
            if (d.vt().init)(p, std::ptr::null_mut()) == 0 {
                let msg = d.error_message();
                return Err(format!("init 실패: {msg}"));
            }
            Ok(d)
        }
    }

    fn vt(&self) -> &Vtbl {
        unsafe { &**(self.this as *const *const Vtbl) }
    }

    pub fn raw(&self) -> *mut c_void {
        self.this
    }

    fn cstr(buf: &[u8]) -> String {
        let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    pub fn name(&self) -> String {
        let mut b = [0u8; 128];
        (self.vt().get_driver_name)(self.this, b.as_mut_ptr());
        Self::cstr(&b)
    }
    pub fn version(&self) -> i32 {
        (self.vt().get_driver_version)(self.this)
    }
    pub fn error_message(&self) -> String {
        let mut b = [0u8; 256];
        (self.vt().get_error_message)(self.this, b.as_mut_ptr());
        Self::cstr(&b)
    }
    pub fn channels(&self) -> Result<(i32, i32), AsioError> {
        let (mut i, mut o) = (0, 0);
        ok((self.vt().get_channels)(self.this, &mut i, &mut o)).map(|_| (i, o))
    }
    pub fn latencies(&self) -> Result<(i32, i32), AsioError> {
        let (mut i, mut o) = (0, 0);
        ok((self.vt().get_latencies)(self.this, &mut i, &mut o)).map(|_| (i, o))
    }
    /// (최소, 최대, 선호, 간격)
    pub fn buffer_size(&self) -> Result<(i32, i32, i32, i32), AsioError> {
        let (mut a, mut b, mut c, mut d) = (0, 0, 0, 0);
        ok((self.vt().get_buffer_size)(self.this, &mut a, &mut b, &mut c, &mut d)).map(|_| (a, b, c, d))
    }
    pub fn can_sample_rate(&self, r: f64) -> bool {
        (self.vt().can_sample_rate)(self.this, r) == ASE_OK
    }
    pub fn sample_rate(&self) -> Result<f64, AsioError> {
        let mut r = 0.0;
        ok((self.vt().get_sample_rate)(self.this, &mut r)).map(|_| r)
    }
    pub fn set_sample_rate(&self, r: f64) -> Result<(), AsioError> {
        ok((self.vt().set_sample_rate)(self.this, r))
    }
    pub fn channel_info(&self, channel: i32, is_input: bool) -> Result<(String, i32, bool), AsioError> {
        let mut ci = AsioChannelInfo { channel, is_input: is_input as i32, is_active: 0, channel_group: 0, sample_type: 0, name: [0; 32] };
        ok((self.vt().get_channel_info)(self.this, &mut ci)).map(|_| (Self::cstr(&ci.name), ci.sample_type, ci.is_active != 0))
    }
    /// (재생 위치 샘플, 그 위치의 시스템 시각 ns)
    pub fn sample_position(&self) -> Result<(u64, u64), AsioError> {
        let (mut s, mut t) = (AsioSamples::default(), AsioSamples::default());
        ok((self.vt().get_sample_position)(self.this, &mut s, &mut t)).map(|_| (s.get(), t.get()))
    }
    /// # Safety
    /// `infos`와 `callbacks`는 dispose_buffers 까지 살아 있어야 한다.
    pub unsafe fn create_buffers(&self, infos: &mut [AsioBufferInfo], size: i32, callbacks: &'static AsioCallbacks) -> Result<(), AsioError> {
        ok((self.vt().create_buffers)(self.this, infos.as_mut_ptr(), infos.len() as i32, size, callbacks))
    }
    pub fn dispose_buffers(&self) -> Result<(), AsioError> {
        ok((self.vt().dispose_buffers)(self.this))
    }
    pub fn start(&self) -> Result<(), AsioError> {
        ok((self.vt().start)(self.this))
    }
    pub fn stop(&self) -> Result<(), AsioError> {
        ok((self.vt().stop)(self.this))
    }
    /// 드라이버가 outputReady를 지원하면 Ok (지연을 버퍼 하나만큼 줄일 수 있음).
    pub fn output_ready(&self) -> Result<(), AsioError> {
        ok((self.vt().output_ready)(self.this))
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        (self.vt().release)(self.this);
    }
}

fn ok(e: AsioError) -> Result<(), AsioError> {
    if e == ASE_OK || e == ASE_SUCCESS {
        Ok(())
    } else {
        Err(e)
    }
}

pub fn error_name(e: AsioError) -> &'static str {
    match e {
        -1000 => "ASE_NotPresent",
        -999 => "ASE_HWMalfunction",
        -998 => "ASE_InvalidParameter",
        -997 => "ASE_InvalidMode",
        -996 => "ASE_SPNotAdvancing",
        -995 => "ASE_NoClock",
        -994 => "ASE_NoMemory",
        _ => "알 수 없음",
    }
}
