//! CP4 실험 F1·E1: 일러스트(재킷·스킨 그림·이미지 BGA)를 GPU 압축 텍스처(BC7)로 미리 바꿔 두면 무엇이 달라지나.
//!
//! imgbench <list.txt> <out.csv> [<blob_dir>]
//!   그림마다: 원본 크기·디코딩 시간(WIC), BC7 인코딩(간이: 모드 6만) 뒤 화질(PSNR), 파일 크기·GPU 메모리,
//!   불러오기 시간 두 길 (가) 원본 파일 읽기 + 디코딩 + RGBA 텍스처 올리기 (나) BC7 파일 읽기 + BC7 텍스처 올리기.
//!   간이 인코더라 화질은 하한(실제 인코더는 모드 여러 개를 써서 더 좋다). 크기·불러오기 시간은 형식의 성질이라 인코더와 무관.

#[allow(dead_code)]
mod common;

use std::io::Write as _;

use synchronizer::qpc;
use windows::core::HSTRING;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_BC7_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

const W4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn interp(a: u32, b: u32, w: u32) -> u32 {
    ((64 - w) * a + w * b + 32) >> 6
}

/// BC7 모드 6 블록 하나 (RGBA 16픽셀 → 16바이트). 주성분 축 위 양 끝점 → 7비트+p비트 네 조합 시도 → 최소제곱 한 번 다듬기
fn bc7_block(px: &[[f32; 4]; 16]) -> [u8; 16] {
    let mut mean = [0f32; 4];
    for p in px {
        for c in 0..4 {
            mean[c] += p[c] / 16.0;
        }
    }
    let mut cov = [[0f32; 4]; 4];
    for p in px {
        for i in 0..4 {
            for j in 0..4 {
                cov[i][j] += (p[i] - mean[i]) * (p[j] - mean[j]);
            }
        }
    }
    let mut axis = [1f32, 1.0, 1.0, 0.0];
    for _ in 0..8 {
        let mut n = [0f32; 4];
        for i in 0..4 {
            for j in 0..4 {
                n[i] += cov[i][j] * axis[j];
            }
        }
        let l = (n.iter().map(|v| v * v).sum::<f32>()).sqrt();
        if l < 1e-6 {
            break;
        }
        axis = n.map(|v| v / l);
    }
    let (mut tmin, mut tmax) = (f32::MAX, f32::MIN);
    for p in px {
        let t: f32 = (0..4).map(|c| (p[c] - mean[c]) * axis[c]).sum();
        tmin = tmin.min(t);
        tmax = tmax.max(t);
    }
    let mut e0: [f32; 4] = std::array::from_fn(|c| mean[c] + tmin * axis[c]);
    let mut e1: [f32; 4] = std::array::from_fn(|c| mean[c] + tmax * axis[c]);
    let mut best = (u64::MAX, [0u8; 16]);
    for round in 0..2 {
        for p0 in 0..2u32 {
            for p1 in 0..2u32 {
                let q = |v: f32, p: u32| -> u32 { (((v.clamp(0.0, 255.0) - p as f32) / 2.0).round().clamp(0.0, 127.0)) as u32 };
                let a: [u32; 4] = std::array::from_fn(|c| q(e0[c], p0));
                let b: [u32; 4] = std::array::from_fn(|c| q(e1[c], p1));
                let ea: [u32; 4] = std::array::from_fn(|c| (a[c] << 1) | p0);
                let eb: [u32; 4] = std::array::from_fn(|c| (b[c] << 1) | p1);
                let pal: [[u32; 4]; 16] = std::array::from_fn(|k| std::array::from_fn(|c| interp(ea[c], eb[c], W4[k])));
                let mut idx = [0u32; 16];
                let mut err = 0u64;
                for (i, p) in px.iter().enumerate() {
                    let (mut bi, mut be) = (0, u64::MAX);
                    for (k, q) in pal.iter().enumerate() {
                        let e: u64 = (0..4).map(|c| ((p[c] - q[c] as f32).powi(2)) as u64).sum();
                        if e < be {
                            be = e;
                            bi = k as u32;
                        }
                    }
                    idx[i] = bi;
                    err += be;
                }
                if err < best.0 {
                    best = (err, pack(a, b, p0, p1, idx));
                }
                if round == 0 && p0 == 0 && p1 == 0 {
                    // 최소제곱으로 양 끝점 다듬기 (색인 고정)
                    let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
                    let (mut xa, mut xb) = ([0f32; 4], [0f32; 4]);
                    for (i, p) in px.iter().enumerate() {
                        let w = W4[idx[i] as usize] as f32 / 64.0;
                        aa += (1.0 - w) * (1.0 - w);
                        ab += (1.0 - w) * w;
                        bb += w * w;
                        for c in 0..4 {
                            xa[c] += (1.0 - w) * p[c];
                            xb[c] += w * p[c];
                        }
                    }
                    let det = aa * bb - ab * ab;
                    if det.abs() > 1e-6 {
                        let n0: [f32; 4] = std::array::from_fn(|c| (bb * xa[c] - ab * xb[c]) / det);
                        let n1: [f32; 4] = std::array::from_fn(|c| (aa * xb[c] - ab * xa[c]) / det);
                        e0 = n0;
                        e1 = n1;
                    }
                }
            }
        }
        if round == 0 && best.0 == 0 {
            break;
        }
    }
    best.1
}

fn pack(mut a: [u32; 4], mut b: [u32; 4], mut p0: u32, mut p1: u32, mut idx: [u32; 16]) -> [u8; 16] {
    // 첫 픽셀 색인의 최상위 비트는 0이어야 함 → 아니면 끝점을 맞바꾸고 색인을 뒤집는다
    if idx[0] >= 8 {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut p0, &mut p1);
        for i in idx.iter_mut() {
            *i = 15 - *i;
        }
    }
    let mut bits: u128 = 0;
    let mut pos = 0;
    let mut put = |v: u32, n: u32| {
        bits |= (v as u128 & ((1u128 << n) - 1)) << pos;
        pos += n;
    };
    put(1 << 6, 7); // 모드 6
    for c in 0..4 {
        put(a[c], 7);
        put(b[c], 7);
    }
    put(p0, 1);
    put(p1, 1);
    put(idx[0], 3);
    for i in &idx[1..] {
        put(*i, 4);
    }
    bits.to_le_bytes()
}

fn decode_block(blk: &[u8; 16]) -> [[u32; 4]; 16] {
    let bits = u128::from_le_bytes(*blk);
    let mut pos = 7;
    let mut get = |n: u32| -> u32 {
        let v = ((bits >> pos) & ((1u128 << n) - 1)) as u32;
        pos += n;
        v
    };
    let mut a = [0u32; 4];
    let mut b = [0u32; 4];
    for c in 0..4 {
        a[c] = get(7);
        b[c] = get(7);
    }
    let (p0, p1) = (get(1), get(1));
    let ea: [u32; 4] = a.map(|v| (v << 1) | p0);
    let eb: [u32; 4] = b.map(|v| (v << 1) | p1);
    std::array::from_fn(|i| {
        let k = if i == 0 { get(3) } else { get(4) };
        std::array::from_fn(|c| interp(ea[c], eb[c], W4[k as usize]))
    })
}

/// BGRA 픽셀 → BC7 블록들 (여러 스레드) + RGB PSNR
fn encode(bgra: &[u8], w: usize, h: usize) -> (Vec<u8>, f64) {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = vec![0u8; bw * bh * 16];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let rows_per = bh.div_ceil(threads);
    let sse: f64 = std::thread::scope(|s| {
        let hs: Vec<_> = out
            .chunks_mut(rows_per * bw * 16)
            .enumerate()
            .map(|(t, chunk)| {
                s.spawn(move || {
                    let mut sse = 0f64;
                    for (k, blk) in chunk.chunks_mut(16).enumerate() {
                        let (bx, by) = (k % bw, t * rows_per + k / bw);
                        let mut px = [[0f32; 4]; 16];
                        for i in 0..16 {
                            let (x, y) = ((bx * 4 + i % 4).min(w - 1), (by * 4 + i / 4).min(h - 1));
                            let o = (y * w + x) * 4;
                            px[i] = [bgra[o + 2] as f32, bgra[o + 1] as f32, bgra[o] as f32, bgra[o + 3] as f32];
                        }
                        let b = bc7_block(&px);
                        blk.copy_from_slice(&b);
                        let d = decode_block(&b);
                        for i in 0..16 {
                            let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                            if x < w && y < h {
                                for c in 0..3 {
                                    sse += (px[i][c] as f64 - d[i][c] as f64).powi(2);
                                }
                            }
                        }
                    }
                    sse
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).sum()
    });
    let mse = sse / (w * h * 3) as f64;
    let psnr = if mse <= 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    (out, psnr)
}

fn wic_decode(wic: &IWICImagingFactory, path: &str) -> windows::core::Result<(Vec<u8>, usize, usize)> {
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        Ok((px, w as usize, h as usize))
    }
}

/// 텍스처를 만들고 GPU가 다 받을 때까지 (이벤트 질의로 확인)
fn upload(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, q: &ID3D11Query, data: &[u8], w: usize, h: usize, fmt: DXGI_FORMAT, pitch: usize) -> windows::core::Result<ID3D11Texture2D> {
    unsafe {
        let td = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut t = None;
        dev.CreateTexture2D(&td, Some(&D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, SysMemPitch: pitch as u32, SysMemSlicePitch: 0 }), Some(&mut t))?;
        let t = t.unwrap();
        // 텍스처를 실제로 한 번 쓰게(복사) 해서 상주시키고, 끝날 때까지 기다린다
        let mut srv = None;
        dev.CreateShaderResourceView(&t, None, Some(&mut srv))?;
        ctx.PSSetShaderResources(0, Some(&[srv]));
        ctx.End(q);
        ctx.Flush();
        let mut done = 0i32;
        while ctx.GetData(q, Some(&mut done as *mut _ as *mut _), 4, 0).is_err() || done == 0 {
            std::hint::spin_loop();
        }
        Ok(t)
    }
}

/// 텍스처만 만들고 기다리지 않음 (묶음 불러오기용)
fn create(dev: &ID3D11Device, data: &[u8], w: usize, h: usize, fmt: DXGI_FORMAT, pitch: usize) -> ID3D11Texture2D {
    unsafe {
        let td = D3D11_TEXTURE2D_DESC {
            Width: w as u32,
            Height: h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut t = None;
        dev.CreateTexture2D(&td, Some(&D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, SysMemPitch: pitch as u32, SysMemSlicePitch: 0 }), Some(&mut t)).unwrap();
        t.unwrap()
    }
}

/// 묶음 불러오기: 그림 전부를 (가) 원본 디코딩 + RGBA 텍스처 (나) BC7 파일 + BC7 텍스처로 올리고, GPU가 다 받을 때까지의 벽시계 시간.
/// 곡·스킨 로딩 화면에서 실제로 일어나는 일. 세 번씩 번갈아 재고 가장 짧은 값
fn batch(blob_dir: &std::path::Path) {
    let index = std::fs::read_to_string(blob_dir.join("index.txt")).unwrap();
    let items: Vec<(String, usize, usize, String)> = index
        .lines()
        .map(|l| {
            let v: Vec<&str> = l.splitn(4, '\t').collect();
            (v[0].to_string(), v[1].parse().unwrap(), v[2].parse().unwrap(), v[3].to_string())
        })
        .collect();
    let f = qpc::frequency();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).unwrap();
        let (dev, ctx): (ID3D11Device, ID3D11DeviceContext) = (dev.unwrap(), ctx.unwrap());
        let mut q = None;
        dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q)).unwrap();
        let q = q.unwrap();
        let wait = || {
            ctx.End(&q);
            ctx.Flush();
            let mut done = 0i32;
            while ctx.GetData(&q, Some(&mut done as *mut _ as *mut _), 4, 0).is_err() || done == 0 {
                std::hint::spin_loop();
            }
        };
        let (mut best_a, mut best_b, mut cpu_a, mut cpu_b) = (f64::MAX, f64::MAX, 0f64, 0f64);
        for _ in 0..3 {
            let c0 = common::cpu_time();
            let t0 = qpc::now();
            let mut keep = Vec::with_capacity(items.len());
            for (_, w, h, path) in &items {
                let (px, _, _) = wic_decode(&wic, path).unwrap();
                keep.push(create(&dev, &px, *w, *h, DXGI_FORMAT_B8G8R8A8_UNORM, w * 4));
            }
            wait();
            let t1 = qpc::now();
            let c1 = common::cpu_time();
            drop(keep);
            let mut keep = Vec::with_capacity(items.len());
            for (n, w, h, _) in &items {
                let data = std::fs::read(blob_dir.join(format!("{n}.bc7"))).unwrap();
                keep.push(create(&dev, &data, w.div_ceil(4) * 4, h.div_ceil(4) * 4, DXGI_FORMAT_BC7_UNORM, w.div_ceil(4) * 16));
            }
            wait();
            let t2 = qpc::now();
            let c2 = common::cpu_time();
            drop(keep);
            let (a, b) = ((t1 - t0) as f64 * 1e3 / f as f64, (t2 - t1) as f64 * 1e3 / f as f64);
            if a < best_a {
                best_a = a;
                cpu_a = (c1 - c0) as f64 / 1e4;
            }
            if b < best_b {
                best_b = b;
                cpu_b = (c2 - c1) as f64 / 1e4;
            }
        }
        println!("images,orig_wall_ms,orig_cpu_ms,bc7_wall_ms,bc7_cpu_ms");
        println!("{},{best_a:.1},{cpu_a:.0},{best_b:.1},{cpu_b:.0}", items.len());
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "batch" {
        batch(std::path::Path::new(&a[2]));
        return;
    }
    let list = std::fs::read_to_string(&a[1]).unwrap();
    let mut out = std::fs::File::create(&a[2]).unwrap();
    let blob_dir = std::path::PathBuf::from(a.get(3).cloned().unwrap_or_else(|| std::env::temp_dir().join("cp4_bc7").to_string_lossy().into_owned()));
    std::fs::create_dir_all(&blob_dir).unwrap();
    let mut index = std::fs::File::create(blob_dir.join("index.txt")).unwrap();
    writeln!(out, "path,ext,w,h,file_bytes,decode_ms,rgba_bytes,bc7_bytes,psnr,encode_ms,load_orig_ms,load_bc7_ms,upload_rgba_ms,upload_bc7_ms").unwrap();
    let f = qpc::frequency();
    let ms = |a: i64, b: i64| (b - a) as f64 * 1e3 / f as f64;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).unwrap();
        let (dev, ctx): (ID3D11Device, ID3D11DeviceContext) = (dev.unwrap(), ctx.unwrap());
        let mut q = None;
        dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q)).unwrap();
        let q = q.unwrap();
        for (n, path) in list.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let path = path.trim();
            let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let t0 = qpc::now();
            let Ok((px, w, h)) = wic_decode(&wic, path) else {
                eprintln!("디코딩 실패: {path}");
                continue;
            };
            let t1 = qpc::now();
            if w < 4 || h < 4 {
                continue;
            }
            let (bc7, psnr) = encode(&px, w, h);
            let t2 = qpc::now();
            let blob = blob_dir.join(format!("{n}.bc7"));
            std::fs::write(&blob, &bc7).unwrap();
            // 새로 쓴 파일을 처음 열 때의 검사(백신 등) 비용은 빼고, 원본처럼 OS 캐시에 있는 상태에서 잰다
            let _ = std::fs::read(&blob);
            let _ = wic_decode(&wic, path);
            // (가) 원본: 파일 읽기 + 디코딩 + 올리기 (파일은 OS 캐시에 있는 상태)
            let a0 = qpc::now();
            let (px2, _, _) = wic_decode(&wic, path).unwrap();
            let a1 = qpc::now();
            let _t = upload(&dev, &ctx, &q, &px2, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, w * 4).unwrap();
            let a2 = qpc::now();
            // (나) BC7: 파일 읽기 + 올리기 (블록 4×4 단위라 크기를 4의 배수로)
            let b0 = qpc::now();
            let data = std::fs::read(&blob).unwrap();
            let b1 = qpc::now();
            let _t2 = upload(&dev, &ctx, &q, &data, w.div_ceil(4) * 4, h.div_ceil(4) * 4, DXGI_FORMAT_BC7_UNORM, w.div_ceil(4) * 16).unwrap();
            let b2 = qpc::now();
            writeln!(index, "{n}\t{w}\t{h}\t{path}").unwrap();
            writeln!(
                out,
                "{},{ext},{w},{h},{file_bytes},{:.3},{},{},{:.2},{:.1},{:.3},{:.3},{:.3},{:.3}",
                path.replace(',', "_"),
                ms(t0, t1),
                w * h * 4,
                bc7.len(),
                psnr,
                ms(t1, t2),
                ms(a0, a2),
                ms(b0, b2),
                ms(a1, a2),
                ms(b1, b2)
            )
            .unwrap();
            if n % 50 == 0 {
                eprintln!("{n}: {w}x{h} psnr {psnr:.1}");
            }
        }
    }
}
