//! CP4 실험 A1·A2·A4·A5: 렌더러 세 가지(Direct2D / D3D11 직접 / D3D12 직접)를 같은 장면·같은 스왑체인으로 잰다.
//!
//! gfxbench run <backend> <mode> <scene> <secs>   한 구성을 재고 CSV 한 줄을 낸다
//! gfxbench idle <secs>                           그리기 없이 가짜 오디오 스레드만 (기준선)
//! gfxbench all <secs> <out.csv>                  모든 구성을 각자 새 프로세스로 돌려 모은다
//!
//! backend: d2d (스프라이트마다 DrawBitmap, 글자도 글리프 스프라이트) / d2dtext (글자는 DrawText) / d3d11 / d3d12
//! mode: win (1280×720 창, vsync) / full (테두리 없는 전체 화면, vsync) / tear (전체 화면, vsync 끔 + tearing)
//! scene: gear (노트 2천 + 글자 2천) / heavy (2만 + 2천) / stress (6만 + 2천) / text (200 + 글자 8천)

mod common;
mod d2d;
mod d3d11;
mod d3d12;
mod shader;
mod swap;

use std::collections::HashMap;
use std::io::Write as _;

use common::*;
use synchronizer::qpc;

const HEADER: &str = "backend,mode,scene,frames,fps,build_p50,submit_p50,submit_p99,interval_p50,interval_p99,interval_max,lat_p50,lat_p99,lat_n,jit_p50,jit_p99,jit_p999,jit_max,cpu_pct,tearing,phase_p50";

enum Backend {
    D2D(d2d::D2D, bool),
    D11(d3d11::D3D11),
    D12(d3d12::D3D12),
}

fn scene_size(s: &str) -> (usize, usize) {
    match s {
        "gear" => (2_000, 2_000),
        "heavy" => (20_000, 2_000),
        "stress" => (60_000, 2_000),
        "text" => (200, 8_000),
        _ => panic!("장면 이름: {s}"),
    }
}

fn ms(dq: i64, f: i64) -> f64 {
    dq as f64 * 1e3 / f as f64
}

fn run(backend: &str, mode: &str, scene_name: &str, secs: f64) -> windows::core::Result<String> {
    let (full, vsync, tear) = match mode {
        "win" => (false, true, false),
        "full" => (true, true, false),
        "tear" => (true, false, true),
        _ => panic!("모드 이름: {mode}"),
    };
    let (nspr, nchr) = scene_size(scene_name);
    let glyphs = charset();
    let atlas = build_atlas(&glyphs)?;
    let (hwnd, w, h) = window(full)?;
    let mut be = match backend {
        "d2d" => Backend::D2D(d2d::D2D::new(hwnd, w, h, &atlas, tear)?, false),
        "d2dtext" => Backend::D2D(d2d::D2D::new(hwnd, w, h, &atlas, tear)?, true),
        "d3d11" => Backend::D11(d3d11::D3D11::new(hwnd, w, h, &atlas, tear)?),
        "d3d12" => Backend::D12(d3d12::D3D12::new(hwnd, w, h, &atlas, tear)?),
        _ => panic!("백엔드 이름: {backend}"),
    };
    let tearing = match &be {
        Backend::D2D(b, _) => b.tearing(),
        Backend::D11(b) => b.tearing(),
        Backend::D12(b) => b.tearing(),
    };
    let f = qpc::frequency();
    let jit = Jitter::start();
    let mut sprites = Vec::with_capacity(nspr + nchr + 16);
    let (mut build, mut submit, mut interval, mut lat) = (vec![], vec![], vec![], vec![]);
    let mut starts: HashMap<u32, i64> = HashMap::new();
    let mut last_shown = 0u32;
    // 프레임 시작이 화면 갱신(vblank) 주기의 어디쯤인가: 마지막으로 알려진 갱신 시각부터 잰 시간 (끝에서 주기로 나머지)
    let (mut last_sync, mut since_sync) = (0i64, vec![]);
    let warm = qpc::now() + f; // 처음 1초는 버림
    let end = warm + (secs * f as f64) as i64;
    let (mut cpu0, mut q_cpu0) = (0u64, 0i64);
    let mut prev_start = 0i64;
    let mut frame = 0u64;
    let mut measured = 0u64;
    loop {
        pump();
        match &be {
            Backend::D2D(b, _) => b.wait(),
            Backend::D11(b) => b.wait(),
            Backend::D12(b) => b.wait(),
        }
        let t0 = qpc::now();
        if t0 >= end {
            break;
        }
        let on = t0 >= warm;
        if on && measured == 0 {
            cpu0 = cpu_time();
            q_cpu0 = t0;
        }
        let text_mode = matches!(be, Backend::D2D(_, true));
        scene(&mut sprites, frame, w, h, nspr, if text_mode { 0 } else { nchr }, glyphs.len());
        let lines = if text_mode { Some(text_lines(nchr, &glyphs, frame)) } else { None };
        let t1 = qpc::now();
        let info = match &mut be {
            Backend::D2D(b, _) => b.frame(&sprites, lines.as_deref(), vsync)?,
            Backend::D11(b) => b.frame(&sprites, vsync)?,
            Backend::D12(b) => b.frame(&sprites, vsync)?,
        };
        if on && last_sync > 0 {
            since_sync.push(ms(t0 - last_sync, f));
        }
        starts.insert(info.index, t0);
        if let Some((pc, sync)) = info.stats {
            last_sync = last_sync.max(sync);
            if pc != last_shown && pc != 0 {
                if let Some(s) = starts.get(&pc) {
                    if *s >= warm {
                        lat.push(ms(sync - s, f));
                    }
                }
                last_shown = pc;
                starts.retain(|k, _| *k > pc);
            }
        }
        if on {
            build.push(ms(t1 - t0, f));
            submit.push(ms(info.q_ret - t1, f));
            if prev_start >= warm {
                interval.push(ms(t0 - prev_start, f));
            }
            measured += 1;
        }
        prev_start = t0;
        frame += 1;
    }
    let q_end = qpc::now();
    let cpu1 = cpu_time();
    let late = jit.finish();
    // 가짜 오디오 스레드의 늦음: 측정 구간에 해당하는 것만 (1 ms마다 하나이므로 뒤쪽 secs×1000개)
    let keep = ((secs * 1000.0) as usize).min(late.len());
    let late = &late[late.len() - keep..];
    drop(be);
    close(hwnd);
    let wall = (q_end - q_cpu0) as f64 / f as f64;
    let cpu_pct = (cpu1 - cpu0) as f64 / 1e7 / wall * 100.0;
    let mx = |v: &[f64]| v.iter().cloned().fold(f64::NAN, f64::max);
    let period = pct(&interval, 0.5);
    let phase: Vec<f64> = since_sync.iter().map(|d| d.rem_euclid(period)).collect();
    Ok(format!(
        "{backend},{mode},{scene_name},{measured},{:.1},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.2},{:.2},{},{:.3},{:.3},{:.3},{:.3},{:.1},{},{:.2}",
        measured as f64 / wall,
        pct(&build, 0.5),
        pct(&submit, 0.5),
        pct(&submit, 0.99),
        pct(&interval, 0.5),
        pct(&interval, 0.99),
        mx(&interval),
        pct(&lat, 0.5),
        pct(&lat, 0.99),
        lat.len(),
        pct(late, 0.5),
        pct(late, 0.99),
        pct(late, 0.999),
        mx(late),
        cpu_pct,
        tearing,
        pct(&phase, 0.5)
    ))
}

fn idle(secs: f64) -> String {
    let jit = Jitter::start();
    let c0 = cpu_time();
    let q0 = qpc::now();
    std::thread::sleep(std::time::Duration::from_secs_f64(secs + 1.0));
    let wall = (qpc::now() - q0) as f64 / qpc::frequency() as f64;
    let c1 = cpu_time();
    let late = jit.finish();
    let keep = ((secs * 1000.0) as usize).min(late.len());
    let late = &late[late.len() - keep..];
    let mx = late.iter().cloned().fold(f64::NAN, f64::max);
    format!(
        "idle,-,-,0,0,,,,,,,,,0,{:.3},{:.3},{:.3},{:.3},{:.1},false,",
        pct(late, 0.5),
        pct(late, 0.99),
        pct(late, 0.999),
        mx,
        (c1 - c0) as f64 / 1e7 / wall * 100.0
    )
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("run") => {
            let secs = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(5.0);
            match run(&a[2], &a[3], &a[4], secs) {
                Ok(row) => println!("{row}"),
                Err(e) => {
                    eprintln!("실패: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("idle") => println!("{}", idle(a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5.0))),
        Some("all") => {
            let secs: f64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5.0);
            let out = a.get(3).cloned().unwrap_or_else(|| "gfxbench.csv".into());
            let exe = std::env::current_exe().unwrap();
            let mut file = std::fs::File::create(&out).unwrap();
            writeln!(file, "{HEADER}").unwrap();
            let mut jobs: Vec<Vec<String>> = vec![vec!["idle".into(), secs.to_string()]];
            for mode in ["win", "full", "tear"] {
                for scene in ["gear", "heavy", "stress", "text"] {
                    for be in ["d2d", "d2dtext", "d3d11", "d3d12"] {
                        jobs.push(vec!["run".into(), be.into(), mode.into(), scene.into(), secs.to_string()]);
                    }
                }
            }
            jobs.push(vec!["idle".into(), secs.to_string()]);
            for (i, j) in jobs.iter().enumerate() {
                eprintln!("[{}/{}] {}", i + 1, jobs.len(), j.join(" "));
                let o = std::process::Command::new(&exe).args(j).output().unwrap();
                let s = String::from_utf8_lossy(&o.stdout);
                match s.lines().last() {
                    Some(row) if o.status.success() => {
                        writeln!(file, "{row}").unwrap();
                        file.flush().unwrap();
                    }
                    _ => eprintln!("  실패: {}", String::from_utf8_lossy(&o.stderr)),
                }
            }
        }
        _ => eprintln!("사용법: gfxbench run <d2d|d2dtext|d3d11|d3d12> <win|full|tear> <gear|heavy|stress|text> <secs> | idle <secs> | all <secs> <out.csv>"),
    }
}
