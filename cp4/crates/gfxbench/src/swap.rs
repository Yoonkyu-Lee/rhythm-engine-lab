//! DXGI 스왑체인 공통 (세 백엔드가 같은 출력 방식·같은 present 시각 측정을 쓰게).
//! 플립 모델(FLIP_DISCARD) + 대기 가능한 스왑체인(최대 프레임 지연 1) + 선택적 tearing 허용(가변 주사율·vsync 끔).

use synchronizer::qpc;
use windows::core::{IUnknown, Interface};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;

pub struct Swap {
    pub swap: IDXGISwapChain3,
    waitable: HANDLE,
    pub tearing: bool,
}

pub struct PresentInfo {
    /// present 호출 직전·직후 QPC
    pub q_call: i64,
    pub q_ret: i64,
    /// 이 present의 번호 (GetLastPresentCount)
    pub index: u32,
    /// 지금 시점의 프레임 통계 (마지막으로 화면에 나간 present 번호, 그 화면 갱신의 QPC) — 없으면 None
    pub stats: Option<(u32, i64)>,
}

impl Swap {
    /// `device` = D3D11 장치 또는 D3D12 명령 큐
    pub fn new(device: &IUnknown, hwnd: HWND, w: u32, h: u32, buffers: u32, want_tearing: bool) -> windows::core::Result<Swap> {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let mut allow = windows::Win32::Foundation::BOOL(0);
            let tearing = want_tearing
                && factory
                    .cast::<IDXGIFactory5>()
                    .and_then(|f5| f5.CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &mut allow as *mut _ as *mut _, std::mem::size_of_val(&allow) as u32))
                    .is_ok()
                && allow.as_bool();
            let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
            if tearing {
                flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32;
            }
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: buffers,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: flags,
                ..Default::default()
            };
            let s1 = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let swap: IDXGISwapChain3 = s1.cast()?;
            swap.SetMaximumFrameLatency(1)?;
            let waitable = swap.GetFrameLatencyWaitableObject();
            Ok(Swap { swap, waitable, tearing })
        }
    }

    /// 다음 프레임을 시작해도 될 때까지 기다린다 (대기 가능한 스왑체인)
    pub fn wait(&self) {
        unsafe {
            WaitForSingleObjectEx(self.waitable, 1000, true);
        }
    }

    pub fn present(&self, vsync: bool) -> PresentInfo {
        unsafe {
            let q_call = qpc::now();
            let flags = if !vsync && self.tearing { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
            let _ = self.swap.Present(if vsync { 1 } else { 0 }, flags);
            let q_ret = qpc::now();
            let index = self.swap.GetLastPresentCount().unwrap_or(0);
            let mut st = DXGI_FRAME_STATISTICS::default();
            let stats = self.swap.GetFrameStatistics(&mut st).ok().map(|_| (st.PresentCount, st.SyncQPCTime));
            PresentInfo { q_call, q_ret, index, stats }
        }
    }
}
