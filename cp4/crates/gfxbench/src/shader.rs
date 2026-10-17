//! 스프라이트 셰이더 (D3D11·D3D12 공용 HLSL). 인스턴스 하나 = 사각형 하나: 정점 셰이더가 정점 번호(0~3)로 네 모서리를 만든다.

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::ID3DBlob;

pub const HLSL: &str = r#"
cbuffer CB : register(b0) { float2 inv; float2 pad; };
Texture2D tex : register(t0);
SamplerState smp : register(s0);
struct I { float4 rect : RECT; float4 uv : UVRECT; float4 col : COLOR; };
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD; float4 col : COLOR; };
V vs(uint vid : SV_VertexID, I i) {
    float2 c = float2(vid & 1, vid >> 1);
    float2 p = i.rect.xy + c * i.rect.zw;
    V o;
    o.pos = float4(p.x * inv.x - 1.0, 1.0 - p.y * inv.y, 0.0, 1.0);
    o.uv = lerp(i.uv.xy, i.uv.zw, c);
    o.col = i.col;
    return o;
}
float4 ps(V v) : SV_Target { float4 t = tex.Sample(smp, v.uv); return float4(t.rgb * v.col.rgb, t.a) * v.col.a; }
"#;

pub fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        let r = D3DCompile(HLSL.as_ptr() as *const _, HLSL.len(), s!("sprite.hlsl"), None, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut code, Some(&mut err));
        if let Some(e) = err {
            let msg = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
            eprintln!("셰이더: {}", String::from_utf8_lossy(msg));
        }
        r?;
        Ok(code.unwrap())
    }
}

pub fn bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}
