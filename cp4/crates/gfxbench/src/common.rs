//! 렌더러 벤치 공통: 창, 장면(스프라이트·글자), 텍스처 아틀라스, 글리프 아틀라스, 측정(프레임·present 시각·가짜 오디오 스레드).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use synchronizer::qpc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_FEATURE_LEVEL_DEFAULT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad, WICRect};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, TIMER_ALL_ACCESS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ATLAS: u32 = 2048;
/// 글리프 칸 (정사각, 아틀라스 아래 절반에 깔림)
pub const GLYPH: u32 = 32;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Sprite {
    pub rect: [f32; 4], // x, y, w, h (픽셀)
    pub uv: [f32; 4],   // u0, v0, u1, v1
    pub col: [f32; 4],
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

pub fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 창 만들기: `full`이면 주 모니터를 덮는 테두리 없는 창, 아니면 1280×720 창
pub fn window(full: bool) -> windows::core::Result<(HWND, u32, u32)> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("cp4_gfxbench");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW)?, ..Default::default() };
        RegisterClassW(&wc);
        let (style, x, y, wdt, hgt) = if full {
            let probe = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("gfxbench"), WS_POPUP, 0, 0, 10, 10, None, None, hinst, None)?;
            let mon = MonitorFromWindow(probe, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let _ = DestroyWindow(probe);
            let r = mi.rcMonitor;
            (WS_POPUP | WS_VISIBLE, r.left, r.top, r.right - r.left, r.bottom - r.top)
        } else {
            (WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE, 40, 40, 1280, 720)
        };
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("gfxbench"), style, x, y, wdt, hgt, None, None, hinst, None)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let mut rc = Default::default();
        GetClientRect(hwnd, &mut rc)?;
        Ok((hwnd, (rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32))
    }
}

pub fn close(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    pump();
}

/// 텍스처 아틀라스 (BGRA, 프리멀티플라이드): 위 절반 = 그림 칸 64개(256×256, 무늬), 아래 절반 = 글리프 칸.
/// 글리프는 `glyphs`의 글자를 DirectWrite로 그려 넣는다. 돌려주는 값 = (픽셀, 글자 → 칸 번호)
pub fn build_atlas(glyphs: &[char]) -> windows::core::Result<Vec<u8>> {
    let n = ATLAS as usize;
    let mut px = vec![0u8; n * n * 4];
    // 그림 칸: 8×8 칸, 칸마다 다른 무늬 (그라데이션 + 테두리)
    for ty in 0..8 {
        for tx in 0..8 {
            let (r0, g0, b0) = ((tx * 31 + 40) as u8, (ty * 29 + 60) as u8, ((tx ^ ty) * 23 + 80) as u8);
            for y in 0..128 {
                for x in 0..256 {
                    let (gx, gy) = (tx * 256 + x, ty * 128 + y);
                    let edge = x < 4 || y < 4 || x >= 252 || y >= 124;
                    let i = (gy * n + gx) * 4;
                    let f = (x + y) as f32 / 384.0;
                    let (r, g, b) = if edge { (255, 255, 255) } else { ((r0 as f32 * (0.5 + f)) as u8, (g0 as f32 * (0.5 + f)) as u8, (b0 as f32 * (0.5 + f)) as u8) };
                    px[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                }
            }
        }
    }
    // 글리프 칸: DirectWrite로 WIC 비트맵에 그린 뒤 복사
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let h = ATLAS / 2;
        let bmp = wic.CreateBitmap(ATLAS, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let fmt = dw.CreateTextFormat(w!("Malgun Gothic"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 26.0, w!("ko-kr"))?;
        let brush = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let per = ATLAS / GLYPH;
        for (i, c) in glyphs.iter().enumerate() {
            let (cx, cy) = ((i as u32 % per) * GLYPH, (i as u32 / per) * GLYPH);
            let mut buf = [0u16; 2];
            let s = c.encode_utf16(&mut buf);
            let r = D2D_RECT_F { left: cx as f32, top: cy as f32 - 2.0, right: (cx + GLYPH) as f32, bottom: (cy + GLYPH) as f32 };
            rt.DrawText(s, &fmt, &r, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        rt.EndDraw(None, None)?;
        let rect = WICRect { X: 0, Y: 0, Width: ATLAS as i32, Height: h as i32 };
        let off = (n * n / 2) * 4;
        bmp.CopyPixels(&rect, ATLAS * 4, &mut px[off..])?;
    }
    Ok(px)
}

/// 글자 표본: 한글·가나·한자·라틴 (곡 제목 같은 다국어 글자, 1,500자)
pub fn charset() -> Vec<char> {
    let mut v: Vec<char> = (0x20u32..0x7F).filter_map(char::from_u32).collect();
    v.extend((0xAC00u32..0xAC00 + 700).step_by(3).filter_map(char::from_u32)); // 한글
    v.extend((0x3041u32..0x3097).filter_map(char::from_u32)); // 히라가나
    v.extend((0x30A1u32..0x30FB).filter_map(char::from_u32)); // 가타카나
    v.extend((0x4E00u32..0x4E00 + 1600).step_by(3).filter_map(char::from_u32)); // 한자
    v.truncate(((ATLAS / GLYPH) * (ATLAS / 2 / GLYPH)) as usize);
    v
}

/// 장면: 프레임 `f`의 스프라이트들. `sprites`개의 노트·효과(움직임) + 글자 `chars`개(글리프 아틀라스)
pub fn scene(out: &mut Vec<Sprite>, f: u64, w: u32, h: u32, sprites: usize, chars: usize, nglyph: usize) {
    out.clear();
    let (wf, hf) = (w as f32, h as f32);
    // 레인 바탕 (16개)
    for l in 0..16 {
        out.push(Sprite { rect: [wf * 0.2 + l as f32 * wf * 0.03, 0.0, wf * 0.028, hf], uv: [0.0, 0.0, 0.125, 0.0625], col: [0.3, 0.3, 0.35, 1.0] });
    }
    // 노트·효과: 결정적 의사 난수로 위치, 프레임마다 아래로 이동
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..sprites {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let lane = (s % 16) as f32;
        let y0 = ((s >> 8) % 4096) as f32 / 4096.0;
        let y = ((y0 + f as f32 * 0.004) % 1.0) * hf;
        let tile = (i % 64) as f32;
        let (tu, tv) = ((tile % 8.0) / 8.0, (tile / 8.0).floor() / 16.0);
        let big = i % 50 == 0; // 가끔 큰 효과 (폭발)
        let (sw, sh) = if big { (wf * 0.08, wf * 0.08) } else { (wf * 0.026, hf * 0.012) };
        out.push(Sprite { rect: [wf * 0.2 + lane * wf * 0.03, y, sw, sh], uv: [tu, tv, tu + 0.125, tv + 0.0625], col: [1.0, 1.0, 1.0, if big { 0.6 } else { 1.0 }] });
    }
    // 글자: 줄 40자씩, 글리프 칸에서 (고정폭 근사)
    let per = (ATLAS / GLYPH) as usize;
    let g = GLYPH as f32 / ATLAS as f32;
    for i in 0..chars {
        let gi = (i * 7 + f as usize) % nglyph.max(1);
        let (gx, gy) = ((gi % per) as f32 * g, 0.5 + (gi / per) as f32 * g);
        let (x, y) = (20.0 + (i % 40) as f32 * 18.0, 20.0 + (i / 40) as f32 * 22.0);
        out.push(Sprite { rect: [x, y % hf, 20.0, 20.0], uv: [gx, gy, gx + g, gy + g], col: [1.0, 1.0, 1.0, 1.0] });
    }
}

/// 글자열 (Direct2D DrawText 비교용): 장면과 같은 글자를 40자 줄로
pub fn text_lines(chars: usize, glyphs: &[char], f: u64) -> Vec<Vec<u16>> {
    (0..chars.div_ceil(40))
        .map(|line| {
            let s: String = (0..40.min(chars - line * 40)).map(|k| glyphs[((line * 40 + k) * 7 + f as usize) % glyphs.len()]).collect();
            s.encode_utf16().collect()
        })
        .collect()
}

/// 가짜 오디오 스레드: 1 ms마다 깨어나(고해상도 대기 타이머) 늦음을 잰다. 오디오 콜백이 렌더링에 얼마나 방해받는지의 대리 지표.
pub struct Jitter {
    stop: Arc<AtomicBool>,
    h: Option<std::thread::JoinHandle<Vec<f64>>>,
}

impl Jitter {
    pub fn start() -> Jitter {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let h = std::thread::spawn(move || {
            let mut late = Vec::with_capacity(200_000);
            unsafe {
                let mut task = 0u32;
                let _ = windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task);
                let t = CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0).unwrap();
                let f = qpc::frequency();
                let period = f / 1000;
                let mut next = qpc::now() + period;
                while !st.load(Ordering::Relaxed) {
                    let now = qpc::now();
                    let due = -((next - now).max(0) * 10_000_000 / f); // 100 ns 단위, 음수 = 상대
                    let _ = SetWaitableTimer(t, &due, 0, None, None, false);
                    WaitForSingleObject(t, INFINITE);
                    let woke = qpc::now();
                    late.push((woke - next) as f64 * 1e3 / f as f64);
                    // 믹싱 흉내: 480프레임 × 2채널 곱셈합
                    let mut acc = 0f32;
                    for k in 0..960 {
                        acc += (k as f32 * 0.001).sin();
                    }
                    std::hint::black_box(acc);
                    next += period;
                    if woke - next > 10 * period {
                        next = woke + period; // 크게 밀리면 다시 맞춤 (늦음은 이미 기록)
                    }
                }
            }
            late
        });
        Jitter { stop, h: Some(h) }
    }

    pub fn finish(mut self) -> Vec<f64> {
        self.stop.store(true, Ordering::Relaxed);
        self.h.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    }
}

pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * p).round() as usize]
}

/// 프로세스 CPU 시간 (100 ns)
pub fn cpu_time() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u);
    }
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    t(k) + t(u)
}
