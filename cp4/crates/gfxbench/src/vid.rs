//! CP4 실험 B2·B3: BGA 영상 디코딩 (Media Foundation 소스 리더 + D3D11 장치 관리자 = 하드웨어 디코더가 있으면 GPU로).
//!
//! vidbench <file>... 파일마다 CSV 한 줄:
//!   열기·첫 프레임 시간, 최대 속도 디코딩(실시간 대비 배수·프레임당 CPU), 탐색(seek) 시간,
//!   노래 시계에 맞춘 실시간 재생(프레임이 제시각보다 늦은 횟수·디코딩 시간) + 그동안 가짜 오디오 스레드의 늦음·CPU 사용률.

#[allow(dead_code)]
mod common;

use common::{cpu_time, pct, Jitter};
use synchronizer::qpc;
use windows::core::{Interface, GUID, HSTRING, PROPVARIANT, PWSTR};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

const VS: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

struct Dev {
    mgr: IMFDXGIDeviceManager,
}

fn device() -> windows::core::Result<Dev> {
    unsafe {
        let mut dev: Option<ID3D11Device> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)?;
        let dev = dev.unwrap();
        let mt: ID3D11Multithread = dev.cast()?;
        let _ = mt.SetMultithreadProtected(true);
        let mut token = 0u32;
        let mut mgr = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr)?;
        let mgr = mgr.unwrap();
        mgr.ResetDevice(&dev, token)?;
        Ok(Dev { mgr })
    }
}

fn fourcc(g: &GUID) -> String {
    let b = g.data1.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_graphic()) {
        String::from_utf8_lossy(&b).into_owned()
    } else {
        format!("{:08x}", g.data1)
    }
}

struct Reader {
    r: IMFSourceReader,
    w: u32,
    h: u32,
    codec: String,
    fps: f64,
    dur_s: f64,
    decoder: String,
    hw: bool,
}

fn open(path: &str, dev: &Dev, hw: bool) -> windows::core::Result<Reader> {
    unsafe {
        let mut attr = None;
        MFCreateAttributes(&mut attr, 4)?;
        let attr: IMFAttributes = attr.unwrap();
        if hw {
            attr.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &dev.mgr)?;
            attr.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        }
        attr.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let r = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attr)?;
        r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        r.SetStreamSelection(VS, true)?;
        let native = r.GetNativeMediaType(VS, 0)?;
        let codec = fourcc(&native.GetGUID(&MF_MT_SUBTYPE)?);
        let size = native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let rate = native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
        let fps = if rate & 0xffff_ffff != 0 { (rate >> 32) as f64 / (rate & 0xffff_ffff) as f64 } else { 0.0 };
        let out: IMFMediaType = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        r.SetCurrentMediaType(VS, None, &out)?;
        let dur = r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION).ok().and_then(|p| u64::try_from(&p).ok()).unwrap_or(0);
        // 실제로 끼워진 디코더 MFT 이름과 하드웨어 여부
        let (mut decoder, mut is_hw) = (String::from("?"), false);
        if let Ok(ex) = r.cast::<IMFSourceReaderEx>() {
            for i in 0..8 {
                let mut cat = GUID::zeroed();
                let mut t = None;
                if ex.GetTransformForStream(VS, i, Some(&mut cat), &mut t).is_err() {
                    break;
                }
                if cat != MFT_CATEGORY_VIDEO_DECODER {
                    continue;
                }
                if let Some(t) = t {
                    if let Ok(a) = t.GetAttributes() {
                        let mut p = PWSTR::null();
                        let mut n = 0u32;
                        if a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut n).is_ok() {
                            decoder = p.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(p.0 as *const _));
                        } else if let Ok(g) = a.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) {
                            decoder = format!("{g:?}");
                        } else {
                            decoder = "(이름 없음)".into();
                        }
                        is_hw = a.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok();
                    }
                }
            }
        }
        Ok(Reader { r, w: (size >> 32) as u32, h: (size & 0xffff_ffff) as u32, codec, fps, dur_s: dur as f64 / 1e7, decoder, hw: is_hw })
    }
}

/// 프레임 하나 읽기: (타임스탬프 100 ns, 끝인가, GPU 표면인가). 샘플 없는 알림(형식 바뀜·틈)은 건너뛴다
fn read(r: &IMFSourceReader) -> windows::core::Result<(i64, bool, bool)> {
    unsafe {
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut s: Option<IMFSample> = None;
            r.ReadSample(VS, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut s))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok((ts, true, false));
            }
            if let Some(s) = s {
                // GPU 표면이고 그 텍스처가 디코더 출력용(BIND_DECODER)이면 DXVA(하드웨어) 디코딩
                let mut gpu = false;
                if let Ok(b) = s.GetBufferByIndex(0) {
                    if let Ok(d) = b.cast::<IMFDXGIBuffer>() {
                        let mut t: Option<ID3D11Texture2D> = None;
                        if d.GetResource(&ID3D11Texture2D::IID, &mut t as *mut _ as *mut _).is_ok() {
                            if let Some(t) = t {
                                let mut desc = D3D11_TEXTURE2D_DESC::default();
                                t.GetDesc(&mut desc);
                                gpu = desc.BindFlags & D3D11_BIND_DECODER.0 as u32 != 0;
                            }
                        }
                    }
                }
                return Ok((ts, false, gpu));
            }
        }
    }
}

fn seek(r: &IMFSourceReader, t100: i64) -> windows::core::Result<()> {
    unsafe { r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(t100)) }
}

fn bench(path: &str, dev: &Dev, hw: bool) -> windows::core::Result<String> {
    let f = qpc::frequency();
    let ms = |a: i64, b: i64| (b - a) as f64 * 1e3 / f as f64;
    // 1) 열기 + 첫 프레임
    let q0 = qpc::now();
    let rd = open(path, dev, hw)?;
    let q1 = qpc::now();
    let (_, _, _) = read(&rd.r)?;
    let (_, _, gpu) = read(&rd.r)?;
    let q2 = qpc::now();
    // 2) 최대 속도 디코딩 (최대 20초 분량)
    let lim = 20.0f64.min(if rd.dur_s > 0.0 { rd.dur_s } else { 20.0 });
    let c0 = cpu_time();
    let q3 = qpc::now();
    let (mut n, mut last) = (0u64, 0i64);
    loop {
        let (ts, end, _) = read(&rd.r)?;
        if end || ts as f64 / 1e7 >= lim {
            break;
        }
        n += 1;
        last = ts;
    }
    let q4 = qpc::now();
    let c1 = cpu_time();
    let wall = ms(q3, q4) / 1e3;
    let speed = (last as f64 / 1e7) / wall.max(1e-9);
    let cpu_per_frame = (c1 - c0) as f64 / 1e4 / n.max(1) as f64; // ms
    // 3) 탐색: 길이를 8등분한 지점마다, 탐색 후 그 시각 이상의 첫 프레임까지
    let mut seeks = vec![];
    if rd.dur_s > 2.0 {
        for k in 1..=8 {
            let t = (rd.dur_s * k as f64 / 9.0 * 1e7) as i64;
            let a = qpc::now();
            seek(&rd.r, t)?;
            loop {
                let (ts, end, _) = read(&rd.r)?;
                if end || ts + (1e7 / rd.fps.max(1.0)) as i64 >= t {
                    break;
                }
            }
            seeks.push(ms(a, qpc::now()));
        }
    }
    // 4) 실시간 재생: 노래 시계(QPC)에 맞춰, 프레임 제시각 전에 디코딩이 끝났는지 (최대 5초)
    seek(&rd.r, 0)?;
    let _ = read(&rd.r)?;
    seek(&rd.r, 0)?;
    let jit = Jitter::start();
    let play = 5.0f64.min(if rd.dur_s > 0.0 { rd.dur_s } else { 10.0 });
    let c2 = cpu_time();
    let start = qpc::now();
    let (mut late, mut frames, mut slack, mut dec) = (0u32, 0u32, vec![], vec![]);
    loop {
        let a = qpc::now();
        let (ts, end, _) = read(&rd.r)?;
        let b = qpc::now();
        if end || ts as f64 / 1e7 >= play {
            break;
        }
        dec.push(ms(a, b));
        let due = start + (ts as f64 / 1e7 * f as f64) as i64;
        slack.push(ms(b, due)); // 양수 = 여유, 음수 = 늦음
        if b > due {
            late += 1;
        }
        frames += 1;
        while qpc::now() < due {
            let left = ms(qpc::now(), due);
            if left > 1.5 {
                std::thread::sleep(std::time::Duration::from_micros(((left - 1.0) * 1000.0) as u64));
            } else {
                std::hint::spin_loop();
            }
        }
    }
    let end = qpc::now();
    let c3 = cpu_time();
    let jl = jit.finish();
    let cpu_pct = (c3 - c2) as f64 / 1e7 / (ms(start, end) / 1e3) * 100.0;
    let name = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    Ok(format!(
        "{},{},{},{}x{},{:.2},{:.1},{},{},{},{:.2},{:.2},{:.1},{:.3},{:.1},{:.1},{},{},{:.2},{:.2},{:.2},{:.1},{:.3},{:.3}",
        path.replace(',', "_"),
        name,
        rd.codec,
        rd.w,
        rd.h,
        rd.fps,
        rd.dur_s,
        rd.decoder.replace(',', "_"),
        rd.hw,
        gpu,
        ms(q0, q1),
        ms(q1, q2),
        speed,
        cpu_per_frame,
        pct(&seeks, 0.5),
        seeks.iter().cloned().fold(f64::NAN, f64::max),
        frames,
        late,
        pct(&slack, 0.01),
        pct(&dec, 0.5),
        pct(&dec, 0.99),
        cpu_pct,
        pct(&jl, 0.99),
        jl.iter().cloned().fold(f64::NAN, f64::max),
    ))
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
    }
    let dev = device().unwrap();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let hw = !args.iter().any(|a| a == "--sw");
    args.retain(|a| a != "--sw");
    println!("path,ext,codec,size,fps,dur_s,decoder,hw_mft,dxva,open_ms,first_ms,speed_x,cpu_ms_per_frame,seek_p50,seek_max,frames,late,slack_p01,dec_p50,dec_p99,cpu_pct,jit_p99,jit_max");
    for p in args {
        let p = if p.starts_with('@') { std::fs::read_to_string(&p[1..]).unwrap().lines().map(String::from).collect() } else { vec![p] };
        for path in p {
            match bench(&path, &dev, hw) {
                Ok(row) => println!("{row}"),
                Err(e) => println!("{},실패,{}", path.replace(',', "_"), e.message().replace(',', " ")),
            }
        }
    }
}
