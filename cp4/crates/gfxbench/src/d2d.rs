//! 백엔드 ① Direct2D (CP3 방식을 그림까지 늘린 것): 스프라이트마다 DrawBitmap, 글자는 줄마다 DrawText (DirectWrite가 매 프레임 배치).

use windows::core::{w, Interface};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};

use crate::common::{Sprite, ATLAS};
use crate::swap::{PresentInfo, Swap};

pub struct D2D {
    swap: Swap,
    dc: ID2D1DeviceContext,
    atlas: ID2D1Bitmap1,
    brush: ID2D1SolidColorBrush,
    fmt: IDWriteTextFormat,
    w: f32,
    h: f32,
}

impl D2D {
    pub fn new(hwnd: HWND, w: u32, h: u32, atlas_px: &[u8], tearing: bool) -> windows::core::Result<D2D> {
        unsafe {
            let mut dev: Option<ID3D11Device> = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)?;
            let dev = dev.unwrap();
            let swap = Swap::new(&dev.cast()?, hwnd, w, h, 2, tearing)?;
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let dxgi: IDXGIDevice = dev.cast()?;
            let d2dev = d2d.CreateDevice(&dxgi)?;
            let dc = d2dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let surf: IDXGISurface = swap.swap.GetBuffer(0)?;
            let bp = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let target = dc.CreateBitmapFromDxgiSurface(&surf, Some(&bp))?;
            dc.SetTarget(&target);
            let ap = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let atlas = dc.CreateBitmap(D2D_SIZE_U { width: ATLAS, height: ATLAS }, Some(atlas_px.as_ptr() as *const _), ATLAS * 4, &ap)?;
            let brush = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fmt = dw.CreateTextFormat(w!("Malgun Gothic"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 18.0, w!("ko-kr"))?;
            Ok(D2D { swap, dc, atlas, brush, fmt, w: w as f32, h: h as f32 })
        }
    }

    pub fn wait(&self) {
        self.swap.wait();
    }

    pub fn tearing(&self) -> bool {
        self.swap.tearing
    }

    /// `text` = None이면 글자도 스프라이트(글리프 아틀라스)로, Some이면 DrawText
    pub fn frame(&mut self, sprites: &[Sprite], text: Option<&[Vec<u16>]>, vsync: bool) -> windows::core::Result<PresentInfo> {
        unsafe {
            self.dc.BeginDraw();
            self.dc.Clear(Some(&D2D1_COLOR_F { r: 0.04, g: 0.04, b: 0.06, a: 1.0 }));
            let a = ATLAS as f32;
            for s in sprites {
                let dst = D2D_RECT_F { left: s.rect[0], top: s.rect[1], right: s.rect[0] + s.rect[2], bottom: s.rect[1] + s.rect[3] };
                let src = D2D_RECT_F { left: s.uv[0] * a, top: s.uv[1] * a, right: s.uv[2] * a, bottom: s.uv[3] * a };
                self.dc.DrawBitmap(&self.atlas, Some(&dst), s.col[3], D2D1_INTERPOLATION_MODE_LINEAR, Some(&src), None);
            }
            if let Some(lines) = text {
                for (i, l) in lines.iter().enumerate() {
                    let y = (20.0 + i as f32 * 22.0) % self.h;
                    let r = D2D_RECT_F { left: 20.0, top: y, right: self.w, bottom: y + 24.0 };
                    self.dc.DrawText(l, &self.fmt, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
            }
            self.dc.EndDraw(None, None)?;
        }
        Ok(self.swap.present(vsync))
    }
}
