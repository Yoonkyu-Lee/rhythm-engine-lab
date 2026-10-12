//! WASAPI 공유 모드, 이벤트 구동 스트림.
//!
//! WASAPI가 돌려주는 QPC 위치(qpcPosition)는 QPC 틱이 아니라 100 ns 단위다.

use windows::core::{w, Interface, Result};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, eRender, EDataFlow, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{AvSetMmThreadCharacteristicsW, CreateEventW};

pub use windows::Win32::Media::Audio::{IAudioCaptureClient, IAudioClock, IAudioRenderClient};

/// 스레드마다 한 번 부른다 (MTA).
pub fn com_init() -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
}

/// 현재 스레드를 MMCSS "Pro Audio" 작업으로 등록한다. 실패해도 진행할 수 있다.
pub fn mmcss_pro_audio() -> bool {
    let mut task = 0u32;
    unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task).is_ok() }
}

pub struct Endpoint {
    pub name: String,
    pub id: String,
    pub device: IMMDevice,
}

pub fn endpoints(capture: bool) -> Result<Vec<Endpoint>> {
    let flow: EDataFlow = if capture { eCapture } else { eRender };
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let coll = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..coll.GetCount()? {
            let device = coll.Item(i)?;
            let store = device.OpenPropertyStore(STGM_READ)?;
            let name = store.GetValue(&PKEY_Device_FriendlyName)?.to_string();
            let idp = device.GetId()?;
            let id = idp.to_string().unwrap_or_default();
            CoTaskMemFree(Some(idp.0 as *const _));
            out.push(Endpoint { name, id, device });
        }
        Ok(out)
    }
}

/// 이름에 `needle`이 들어간 첫 장치.
pub fn find(capture: bool, needle: &str) -> Result<Endpoint> {
    endpoints(capture)?
        .into_iter()
        .find(|e| e.name.contains(needle))
        .ok_or_else(|| windows::core::Error::new(windows::Win32::Foundation::E_INVALIDARG, format!("장치 없음: {needle}")))
}

pub struct Stream {
    pub client: IAudioClient,
    pub event: HANDLE,
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub buffer_frames: u32,
    pub default_period_100ns: i64,
    pub min_period_100ns: i64,
}

impl Stream {
    /// 장치의 믹스 형식(공유 모드에서는 보통 32비트 float)으로 연다.
    pub fn open(device: &IMMDevice) -> Result<Stream> {
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let fmt: *mut WAVEFORMATEX = client.GetMixFormat()?;
            let (rate, channels, bits) = ((*fmt).nSamplesPerSec, (*fmt).nChannels, (*fmt).wBitsPerSample);
            let mut def = 0i64;
            let mut min = 0i64;
            client.GetDevicePeriod(Some(&mut def), Some(&mut min))?;
            let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 0, 0, fmt, None);
            CoTaskMemFree(Some(fmt as *const _));
            r?;
            let event = CreateEventW(None, false, false, None)?;
            client.SetEventHandle(event)?;
            let buffer_frames = client.GetBufferSize()?;
            Ok(Stream { client, event, rate, channels, bits, buffer_frames, default_period_100ns: def, min_period_100ns: min })
        }
    }

    pub fn service<T: Interface>(&self) -> Result<T> {
        unsafe { self.client.GetService::<T>() }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}
