//! 렌더러 인터페이스와 D3D11 백엔드.
//!
//! 게임은 `Renderer`만 쓴다: 프레임 대기 → 그리기 목록 → present → 프레임 시각 보고. CP3-1에서 같은 인터페이스 뒤에 wgpu를 붙인다.
//! D3D11 백엔드: DXGI flip 모델(FLIP_DISCARD), 대기 가능한 스왑체인 + 최대 프레임 지연 1, Direct2D/DirectWrite로 사각형·글자.
//! 프레임 시각: present 전후 QPC와 DXGI 프레임 통계(마지막 vblank의 SyncQPCTime, PresentCount)를 보고한다.
//! SyncQPCTime은 refresh에 연결된 QPC이지 픽셀이 빛난 시각이 아니다 (설계안 10장) — 등급 "dxgi_stats"로 표시한다.

use windows::core::{w, Interface};
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, IDXGISwapChain2, DXGI_FRAME_STATISTICS, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use synchronizer::qpc;

#[derive(Clone, Copy, Debug)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

pub enum Cmd {
    Rect { x: f32, y: f32, w: f32, h: f32, c: Rgba },
    Text { x: f32, y: f32, size: f32, c: Rgba, s: String },
}

#[derive(Default)]
pub struct DrawList {
    pub clear: Option<Rgba>,
    pub cmds: Vec<Cmd>,
}

impl DrawList {
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
        self.cmds.push(Cmd::Rect { x, y, w, h, c });
    }
    pub fn text(&mut self, x: f32, y: f32, size: f32, c: Rgba, s: impl Into<String>) {
        self.cmds.push(Cmd::Text { x, y, size, c, s: s.into() });
    }
}

/// 한 프레임의 시각 보고.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    pub q_wait_start: i64,
    pub q_wait_end: i64,
    pub q_present_start: i64,
    pub q_present_end: i64,
    /// DXGI 프레임 통계 (없으면 0). sync_qpc = 마지막 vblank의 QPC.
    pub present_count: u32,
    pub present_refresh: u32,
    pub sync_refresh: u32,
    pub sync_qpc: i64,
}

pub trait Renderer {
    /// 프레임을 시작해도 될 때까지 기다린다 (대기 가능한 스왑체인). 기다린 시작·끝 QPC.
    fn wait(&mut self) -> (i64, i64);
    fn draw(&mut self, list: &DrawList) -> windows::core::Result<()>;
    /// present 하고 시각을 보고한다.
    fn present(&mut self, vsync: bool) -> FrameTiming;
    /// 프레임 시각 정보의 등급 (백엔드마다 다름).
    fn timing_grade(&self) -> &'static str;
}

pub struct D3D11Renderer {
    _device: ID3D11Device,
    swap: IDXGISwapChain2,
    waitable: HANDLE,
    dc: ID2D1DeviceContext,
    _target: ID2D1Bitmap1,
    brush: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    formats: Vec<(u32, IDWriteTextFormat)>,
    pending: FrameTiming,
}

impl D3D11Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32) -> windows::core::Result<D3D11Renderer> {
        unsafe {
            let mut device = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
            let device: ID3D11Device = device.unwrap();
            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter: IDXGIAdapter = dxgi_device.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let swap1: IDXGISwapChain1 = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
            let swap: IDXGISwapChain2 = swap1.cast()?;
            swap.SetMaximumFrameLatency(1)?;
            let waitable = swap.GetFrameLatencyWaitableObject();
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let d2d_device = d2d.CreateDevice(&dxgi_device)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let surface: IDXGISurface = swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let target = dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            dc.SetTarget(&target);
            let brush = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(D3D11Renderer { _device: device, swap, waitable, dc, _target: target, brush, dwrite, formats: Vec::new(), pending: FrameTiming::default() })
        }
    }

    fn format(&mut self, size: f32) -> windows::core::Result<IDWriteTextFormat> {
        let key = (size * 10.0) as u32;
        if let Some((_, f)) = self.formats.iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let f = unsafe {
            self.dwrite.CreateTextFormat(w!("Malgun Gothic"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("ko-kr"))?
        };
        self.formats.push((key, f.clone()));
        Ok(f)
    }
}

impl Renderer for D3D11Renderer {
    fn wait(&mut self) -> (i64, i64) {
        let a = qpc::now();
        unsafe { WaitForSingleObjectEx(self.waitable, 1000, true) };
        let b = qpc::now();
        self.pending.q_wait_start = a;
        self.pending.q_wait_end = b;
        (a, b)
    }

    fn draw(&mut self, list: &DrawList) -> windows::core::Result<()> {
        unsafe {
            self.dc.BeginDraw();
            if let Some(c) = list.clear {
                self.dc.Clear(Some(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 }));
            }
            for cmd in &list.cmds {
                match cmd {
                    Cmd::Rect { x, y, w, h, c } => {
                        self.brush.SetColor(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 });
                        self.dc.FillRectangle(&D2D_RECT_F { left: *x, top: *y, right: x + w, bottom: y + h }, &self.brush);
                    }
                    Cmd::Text { x, y, size, c, s } => {
                        let f = self.format(*size)?;
                        self.brush.SetColor(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 });
                        let wide: Vec<u16> = s.encode_utf16().collect();
                        let rect = D2D_RECT_F { left: *x, top: *y, right: x + 2000.0, bottom: y + size * 2.0 };
                        self.dc.DrawText(&wide, &f, &rect, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
            }
            self.dc.EndDraw(None, None)
        }
    }

    fn present(&mut self, vsync: bool) -> FrameTiming {
        let mut t = self.pending;
        t.q_present_start = qpc::now();
        unsafe {
            let _ = self.swap.Present(if vsync { 1 } else { 0 }, DXGI_PRESENT(0));
        }
        t.q_present_end = qpc::now();
        let mut st = DXGI_FRAME_STATISTICS::default();
        if unsafe { self.swap.GetFrameStatistics(&mut st) }.is_ok() {
            t.present_count = st.PresentCount;
            t.present_refresh = st.PresentRefreshCount;
            t.sync_refresh = st.SyncRefreshCount;
            t.sync_qpc = st.SyncQPCTime;
        }
        self.pending = FrameTiming::default();
        t
    }

    fn timing_grade(&self) -> &'static str {
        "dxgi_stats"
    }
}

