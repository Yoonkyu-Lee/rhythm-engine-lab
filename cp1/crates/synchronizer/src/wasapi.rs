//! WASAPI 이벤트 구동 스트림: 공유 / 공유 저지연(IAudioClient3, 엔진 주기를 장치 최소로) / 배타 (CP1-4).
//!
//! WASAPI가 돌려주는 QPC 위치(qpcPosition)는 QPC 틱이 아니라 100 ns 단위다.

use windows::core::{w, Interface, Result, GUID};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, eRender, EDataFlow, IAudioClient, IAudioClient3, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
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

/// 출력 경로.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 공유 모드, 기본 엔진 주기 (보통 10 ms).
    Shared,
    /// 공유 모드, IAudioClient3로 엔진 주기를 장치가 허락하는 최소로.
    SharedLowLatency,
    /// 배타 모드, 장치 최소 주기. 장치를 독점한다 (다른 앱 소리 안 남).
    Exclusive,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "shared" => Mode::Shared,
            "lowlat" | "shared-ll" => Mode::SharedLowLatency,
            "exclusive" | "excl" => Mode::Exclusive,
            _ => return None,
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Shared => "WASAPI 공유",
            Mode::SharedLowLatency => "WASAPI 공유 저지연",
            Mode::Exclusive => "WASAPI 배타",
        }
    }
}

/// 장치 버퍼의 샘플 형식.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFmt {
    F32,
    I32,
    I16,
}

const SUB_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUB_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

fn ext_format(rate: u32, channels: u16, f: SampleFmt) -> WAVEFORMATEXTENSIBLE {
    let bits: u16 = if f == SampleFmt::I16 { 16 } else { 32 };
    let mut w = WAVEFORMATEXTENSIBLE::default();
    w.Format.wFormatTag = 0xFFFE; // WAVE_FORMAT_EXTENSIBLE
    w.Format.nChannels = channels;
    w.Format.nSamplesPerSec = rate;
    w.Format.wBitsPerSample = bits;
    w.Format.nBlockAlign = channels * bits / 8;
    w.Format.nAvgBytesPerSec = rate * w.Format.nBlockAlign as u32;
    w.Format.cbSize = 22;
    w.Samples.wValidBitsPerSample = bits;
    w.dwChannelMask = if channels == 2 { 3 } else { 0 };
    w.SubFormat = if f == SampleFmt::F32 { SUB_FLOAT } else { SUB_PCM };
    w
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
    pub mode: Mode,
    pub sample: SampleFmt,
    /// 실제로 쓰는 엔진(또는 장치) 주기, 프레임.
    pub period_frames: u32,
}

impl Stream {
    /// 공유 모드, 장치의 믹스 형식(보통 32비트 float)으로 연다.
    pub fn open(device: &IMMDevice) -> Result<Stream> {
        Stream::open_mode(device, Mode::Shared)
    }

    pub fn open_mode(device: &IMMDevice, mode: Mode) -> Result<Stream> {
        unsafe {
            let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix: *mut WAVEFORMATEX = client.GetMixFormat()?;
            let (rate, mix_channels, mix_bits) = ((*mix).nSamplesPerSec, (*mix).nChannels, (*mix).wBitsPerSample);
            let mut def = 0i64;
            let mut min = 0i64;
            client.GetDevicePeriod(Some(&mut def), Some(&mut min))?;
            let (channels, bits, sample, period_frames);
            match mode {
                Mode::Shared => {
                    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 0, 0, mix, None);
                    CoTaskMemFree(Some(mix as *const _));
                    r?;
                    (channels, bits, sample) = (mix_channels, mix_bits, SampleFmt::F32);
                    period_frames = (def as f64 * rate as f64 / 1e7).round() as u32;
                }
                Mode::SharedLowLatency => {
                    let c3: IAudioClient3 = client.cast()?;
                    let (mut d, mut f, mut mn, mut mx) = (0u32, 0u32, 0u32, 0u32);
                    let r = c3
                        .GetSharedModeEnginePeriod(mix, &mut d, &mut f, &mut mn, &mut mx)
                        .and_then(|_| c3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, mn, mix, None));
                    CoTaskMemFree(Some(mix as *const _));
                    r?;
                    (channels, bits, sample) = (mix_channels, mix_bits, SampleFmt::F32);
                    period_frames = mn;
                }
                Mode::Exclusive => {
                    CoTaskMemFree(Some(mix as *const _));
                    let mut chosen = None;
                    for f in [SampleFmt::F32, SampleFmt::I32, SampleFmt::I16] {
                        let w = ext_format(rate, 2, f);
                        if client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, &w as *const _ as *const WAVEFORMATEX, None).is_ok() {
                            chosen = Some((f, w));
                            break;
                        }
                    }
                    let (f, w) = chosen.ok_or_else(|| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "배타 모드에서 받는 형식 없음 (f32/i32/i16, 2ch)"))?;
                    let wp = &w as *const _ as *const WAVEFORMATEX;
                    let mut period = min;
                    if let Err(e) = client.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, period, wp, None) {
                        if e.code() != AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
                            return Err(e);
                        }
                        // 장치가 정렬된 크기를 알려 준다: 그 크기에 맞는 주기로 다시
                        let frames = client.GetBufferSize()?;
                        period = (1e7 * frames as f64 / rate as f64).round() as i64;
                        client = device.Activate(CLSCTX_ALL, None)?;
                        client.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, period, wp, None)?;
                    }
                    (channels, bits, sample) = (2, w.Format.wBitsPerSample, f);
                    period_frames = (period as f64 * rate as f64 / 1e7).round() as u32;
                }
            }
            let event = CreateEventW(None, false, false, None)?;
            client.SetEventHandle(event)?;
            let buffer_frames = client.GetBufferSize()?;
            Ok(Stream { client, event, rate, channels, bits, buffer_frames, default_period_100ns: def, min_period_100ns: min, mode, sample, period_frames })
        }
    }

    /// 이번 이벤트에 쓸 프레임 수. 배타 이벤트 구동은 매번 버퍼 하나를 통째로 채운다 (패딩을 쓰지 않음).
    pub fn frames_to_write(&self) -> Result<u32> {
        if self.mode == Mode::Exclusive {
            return Ok(self.buffer_frames);
        }
        let pad = unsafe { self.client.GetCurrentPadding()? };
        Ok(self.buffer_frames - pad)
    }

    /// f32 끼워 넣은 샘플 `buf`(n 프레임 × channels)를 장치 형식으로 바꿔 쓴다.
    pub fn write(&self, rc: &IAudioRenderClient, buf: &[f32], n: u32) -> Result<()> {
        unsafe {
            let p = rc.GetBuffer(n)?;
            match self.sample {
                SampleFmt::F32 => std::ptr::copy_nonoverlapping(buf.as_ptr(), p as *mut f32, buf.len()),
                SampleFmt::I32 => {
                    let q = p as *mut i32;
                    for (i, v) in buf.iter().enumerate() {
                        *q.add(i) = (v.clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32;
                    }
                }
                SampleFmt::I16 => {
                    let q = p as *mut i16;
                    for (i, v) in buf.iter().enumerate() {
                        *q.add(i) = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                }
            }
            rc.ReleaseBuffer(n, 0)
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
