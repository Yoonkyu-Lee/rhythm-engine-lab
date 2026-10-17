//! 백엔드 ② Direct3D 11 직접: 모든 스프라이트를 인스턴스 버퍼 하나 + 그리기 호출 하나(인스턴싱)로. 글자도 글리프 아틀라스 스프라이트.

use windows::core::{s, Interface};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, D3D_DRIVER_TYPE_HARDWARE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC};

use crate::common::{Sprite, ATLAS};
use crate::shader;
use crate::swap::{PresentInfo, Swap};

pub const MAX: usize = 100_000;

pub struct D3D11 {
    swap: Swap,
    ctx: ID3D11DeviceContext,
    rtv: ID3D11RenderTargetView,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    il: ID3D11InputLayout,
    inst: ID3D11Buffer,
    cb: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    smp: ID3D11SamplerState,
    blend: ID3D11BlendState,
    w: u32,
    h: u32,
}

impl D3D11 {
    pub fn new(hwnd: HWND, w: u32, h: u32, atlas_px: &[u8], tearing: bool) -> windows::core::Result<D3D11> {
        unsafe {
            let mut dev: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
            let (dev, ctx) = (dev.unwrap(), ctx.unwrap());
            let swap = Swap::new(&dev.cast()?, hwnd, w, h, 2, tearing)?;
            let bb: ID3D11Texture2D = swap.swap.GetBuffer(0)?;
            let mut rtv = None;
            dev.CreateRenderTargetView(&bb, None, Some(&mut rtv))?;
            let vsb = shader::compile(s!("vs"), s!("vs_5_0"))?;
            let psb = shader::compile(s!("ps"), s!("ps_5_0"))?;
            let (mut vs, mut ps) = (None, None);
            dev.CreateVertexShader(shader::bytes(&vsb), None, Some(&mut vs))?;
            dev.CreatePixelShader(shader::bytes(&psb), None, Some(&mut ps))?;
            let el = |name: windows::core::PCSTR, off: u32| D3D11_INPUT_ELEMENT_DESC {
                SemanticName: name,
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: off,
                InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            };
            let elems = [el(s!("RECT"), 0), el(s!("UVRECT"), 16), el(s!("COLOR"), 32)];
            let mut il = None;
            dev.CreateInputLayout(&elems, shader::bytes(&vsb), Some(&mut il))?;
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: (MAX * std::mem::size_of::<Sprite>()) as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut inst = None;
            dev.CreateBuffer(&bd, None, Some(&mut inst))?;
            let cbd = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
            let cbdata = [2.0 / w as f32, 2.0 / h as f32, 0.0, 0.0];
            let mut cb = None;
            dev.CreateBuffer(&cbd, Some(&D3D11_SUBRESOURCE_DATA { pSysMem: cbdata.as_ptr() as *const _, ..Default::default() }), Some(&mut cb))?;
            let td = D3D11_TEXTURE2D_DESC {
                Width: ATLAS,
                Height: ATLAS,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut tex = None;
            dev.CreateTexture2D(&td, Some(&D3D11_SUBRESOURCE_DATA { pSysMem: atlas_px.as_ptr() as *const _, SysMemPitch: ATLAS * 4, SysMemSlicePitch: 0 }), Some(&mut tex))?;
            let mut srv = None;
            dev.CreateShaderResourceView(&tex.unwrap(), None, Some(&mut srv))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut smp = None;
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let mut bdsc = D3D11_BLEND_DESC::default();
            bdsc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            dev.CreateBlendState(&bdsc, Some(&mut blend))?;
            Ok(D3D11 {
                swap,
                ctx,
                rtv: rtv.unwrap(),
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                il: il.unwrap(),
                inst: inst.unwrap(),
                cb: cb.unwrap(),
                srv: srv.unwrap(),
                smp: smp.unwrap(),
                blend: blend.unwrap(),
                w,
                h,
            })
        }
    }

    pub fn wait(&self) {
        self.swap.wait();
    }

    pub fn tearing(&self) -> bool {
        self.swap.tearing
    }

    pub fn frame(&mut self, sprites: &[Sprite], vsync: bool) -> windows::core::Result<PresentInfo> {
        let n = sprites.len().min(MAX);
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&self.inst, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
            std::ptr::copy_nonoverlapping(sprites.as_ptr(), m.pData as *mut Sprite, n);
            self.ctx.Unmap(&self.inst, 0);
            self.ctx.ClearRenderTargetView(&self.rtv, &[0.04, 0.04, 0.06, 1.0]);
            self.ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            self.ctx.OMSetBlendState(&self.blend, None, 0xffff_ffff);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: self.w as f32, Height: self.h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
            self.ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.ctx.IASetInputLayout(&self.il);
            let stride = std::mem::size_of::<Sprite>() as u32;
            self.ctx.IASetVertexBuffers(0, 1, Some(&Some(self.inst.clone())), Some(&stride), Some(&0));
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            self.ctx.PSSetShader(&self.ps, None);
            self.ctx.PSSetShaderResources(0, Some(&[Some(self.srv.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            self.ctx.DrawInstanced(4, n as u32, 0, 0);
        }
        Ok(self.swap.present(vsync))
    }
}
