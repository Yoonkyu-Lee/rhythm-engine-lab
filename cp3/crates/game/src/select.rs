//! `cp3game select [<라이브러리 폴더>]` (없으면 설정의 라이브러리 폴더): CP3 단순 선곡 화면 + 미리보기 바탕 검증 (README 4-3 범위: 화면 모양은 CP4).
//!
//! 키: ↑↓ 곡 · PgUp/PgDn 10곡 · ←→ 차분 · Enter 플레이 · Space 미리보기 켜기/끄기 · [ ] 10초 탐색 · Home 하이라이트로 · F1 설정 · Esc 끝.
//! - F1 = 설정 화면(settings_ui.rs, `--settings settings.toml`). 보정은 설정 화면에서 실행하고, 끝나면 결과를 들고 설정 화면으로 돌아온다.
//! - 곡 목록은 `index.bin`만 읽는다(곡 묶음을 열지 않음). 구간 통계(250 ms 칸 노트 수)를 막대로 그린다.
//! - 커서가 150 ms 머물면(디바운스 ⚑ 측정으로 확정) 백그라운드에서 묶음 열기 → 계획 → 소리 저장소(들을 곳부터) 시작.
//!   준비 집합(하이라이트 지점에 울리는 소리 + 앞으로 W = 2초 ⚑)이 차면 그 지점부터 자동 연주한다. 재생 중 준비 부족은 버퍼링.
//! - 곡·차분을 바꾸면 세대를 올려 이전 작업을 취소하고 저장소를 버린다(곡 사이 캐시 없음). 탐색은 같은 저장소에서 순서만 바꾼다.
//! - "첫 소리까지" = 커서가 머문 순간(디바운스 끝)부터, 오디오 스레드가 시작 지점의 첫 콘텐츠 프레임을 장치 버퍼에 쓴 순간까지
//!   (장치 출력 지연은 빠짐, 3-5에서 p50·p95로 잰다).
//! - `--bench N`: 사람 없이 곡 N개를 차례로(씨앗 고정 무작위) 고르고, 미리보기가 1초 재생되면(또는 20초 안에 못 하면) 다음 곡.
//!   끝나면 첫 소리까지 시간 분포를 출력하고 닫는다.
//! - `--keys 70,09,28,…`: 사람 없이 화면 경로를 시험할 때, 16진 가상 키를 10프레임마다 하나씩 창 키처럼 넣는다(설정 화면·보정 포함).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::load::{plan_chart_reuse, LoadOpts, SlotCache};
use engine::mixer::{LatePolicy, MixEvent};
use engine::preview::Store;
use engine::ring::{ring, Consumer};
use synchronizer::qpc;
use track::library::{read_index, IndexEntry};
use track::pack::Bundle;
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT};

use crate::audio::{self, AudioCfg, AudioHandles};
use crate::render::{D3D11Renderer, DrawList, Renderer, Rgba};
use crate::settings::{Global, Settings};
use crate::settings_ui::{Action, Ui};
use crate::{arg, game, keys};

const DEBOUNCE: Duration = Duration::from_millis(150);
const W_NS: i64 = 2_000_000_000;

struct Loading {
    gen: u64,
    rx: Receiver<Result<(Store, Vec<(String, String, String)>, Arc<Bundle>), String>>,
}

struct Playing {
    a: AudioHandles,
    mix: Consumer<MixEvent>,
    blk: Consumer<audio::BlockLog>,
    t0: i64,
    started: Instant,
}

struct Preview {
    store: Store,
    names: Vec<(String, String, String)>,
    t0: i64,
    play: Option<Playing>,
    /// 커서가 머문 시각 (첫 소리까지 시간을 재는 기준)
    settled: Instant,
    first_ms: Option<f64>,
}

fn start_audio(store: &Store, t0: i64, render: &str, g: &Global) -> Result<Playing, String> {
    let (_key_tx, key_rx) = ring::<audio::KeyReq>(16);
    let (blk_tx, blk) = ring::<audio::BlockLog>(1 << 12);
    let (mix_tx, mix) = ring::<MixEvent>(1 << 14);
    let a = audio::spawn(
        store.compiled(),
        AudioCfg {
            path: audio::AudioPath::Shared,
            render: render.to_string(),
            asio_buffer: 64,
            excl_period_ms: 5.0,
            lead_in_s: 0.03,
            tail_s: 0.0,
            master_gain: 0.5 * g.volume_master as f32, // 미리보기는 정답 렌더 최댓값을 아직 모른다 (전체 준비 전) — 고정 −6 dB
            late: LatePolicy::DEFAULT,
            autoplay_notes: false,
            auto_from_chart: true,
            d_ms: Some(20.0),
            d_margin_ms: 2.0,
            volumes: (g.volume_bgm as f32, g.volume_keys as f32),
            skip_on_stall: true,
            asio_capture: None,
            faults: vec![],
            live: None,
            preview: Some((t0, W_NS as f64 / 1e6)),
        },
        key_rx,
        blk_tx,
        mix_tx,
    )?;
    Ok(Playing { a, mix, blk, t0, started: Instant::now() })
}

fn stop_audio(p: Playing) {
    p.a.stop.store(true, Ordering::Release);
    let _ = p.a.join.join();
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let settings_path = PathBuf::from(arg(args, "--settings", "settings.toml"));
    let mut g = Settings::load(&settings_path).global();
    // 라이브러리: 명령줄 위치 인자가 있으면 그것(이번 실행만), 없으면 설정의 라이브러리 폴더 (설정 화면에서 바꾸면 다시 읽음)
    let lib_arg = args.get(2).filter(|a| !a.starts_with("--")).cloned();
    let mut lib = PathBuf::from(lib_arg.clone().unwrap_or_else(|| g.library.clone()));
    let pass_from = if lib_arg.is_some() { 3 } else { 2 };
    let render_name = arg(args, "--render", &g.render);
    let mut ui: Option<Ui> = None;
    let mut pending_lib: Option<(Vec<IndexEntry>, PathBuf)> = None;
    let mut script: std::collections::VecDeque<u32> = arg(args, "--keys", "").split(',').filter_map(|k| u32::from_str_radix(k.trim(), 16).ok()).collect();
    let mut frame_no = 0u64;
    let rate = audio::device_rate(audio::AudioPath::Shared, &render_name)?;
    let t_idx = Instant::now();
    let mut entries: Vec<IndexEntry> = read_index(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    let idx_ms = t_idx.elapsed().as_secs_f64() * 1e3;
    if entries.is_empty() {
        return Err("색인에 곡이 없음".into());
    }
    let hwnd = game::create_window(w!("CP3 — 선곡"))?;
    let mut rend = D3D11Renderer::new(hwnd, 1280, 720)?;
    let gen = Arc::new(AtomicU64::new(0));
    let (mut sel, mut chart) = (0usize, 0usize);
    let mut last_change = Instant::now();
    let mut want_preview = true;
    let mut loading: Option<Loading> = None;
    let mut preview: Option<Preview> = None;
    let mut last_result = String::new();
    let mut firsts: Vec<f64> = Vec::new();
    // 지금 곡(Track)의 소리 저장소: 차분을 바꿔도 준비된 슬롯을 다시 쓰고, 플레이에 넘긴다. 곡을 바꾸면 버린다 (README 4-3 2·5·7)
    let track_cache: Arc<std::sync::Mutex<(usize, SlotCache)>> = Arc::new(std::sync::Mutex::new((usize::MAX, SlotCache::new())));
    let mut msg = MSG::default();
    let mut list = DrawList::default();
    let quit = Arc::new(AtomicBool::new(false));
    let bench: usize = arg(args, "--bench", "0").parse().unwrap_or(0);
    let mut bench_rng: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut bench_done = 0usize;
    let mut bench_fail = 0usize;
    let mut bench_t = Instant::now();
    let mut bench_played = false;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).max(2);
    let reset = |preview: &mut Option<Preview>, loading: &mut Option<Loading>, gen: &AtomicU64| {
        gen.fetch_add(1, Ordering::AcqRel); // 이전 작업 취소
        if let Some(p) = preview.take() {
            if let Some(pl) = p.play {
                stop_audio(pl);
            }
            p.store.cancel();
            std::thread::spawn(move || p.store.join()); // 해제는 작업 스레드가 멈춘 뒤 (믹서는 이미 멈춤)
        }
        *loading = None;
    };
    while !quit.load(Ordering::Relaxed) {
        let _ = rend.wait();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    quit.store(true, Ordering::Relaxed);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if game::take_close() {
            quit.store(true, Ordering::Relaxed);
        }
        if let Some((v, nl)) = pending_lib.take() {
            *track_cache.lock().unwrap() = (usize::MAX, SlotCache::new());
            (entries, lib, sel, chart) = (v, nl, 0, 0);
        }
        let e = &entries[sel];
        let n_charts = e.charts.len().max(1);
        let mut hk = game::take_hotkey();
        frame_no += 1;
        if hk == 0 && frame_no % 10 == 0 {
            hk = script.pop_front().unwrap_or(0);
        }
        if bench > 0 {
            let played = preview.as_ref().and_then(|p| p.play.as_ref()).map_or(false, |pl| pl.started.elapsed() >= Duration::from_secs(1));
            let stuck = bench_t.elapsed() >= Duration::from_secs(20);
            if played || stuck || bench_done == 0 {
                if bench_done > 0 && !played {
                    bench_fail += 1;
                }
                if bench_done == bench {
                    if !bench_played && played {
                        hk = 0x0D; // 마지막 곡: 미리보기 저장소를 넘겨 플레이 (인계 확인, 플레이 인자는 select 뒤 인자에서)
                        bench_played = true;
                    } else {
                        quit.store(true, Ordering::Relaxed);
                    }
                } else {
                    bench_rng ^= bench_rng << 13;
                    bench_rng ^= bench_rng >> 7;
                    bench_rng ^= bench_rng << 17;
                    sel = (bench_rng % entries.len() as u64) as usize;
                    hk = 0xFFFF; // 곡을 바꿈
                    bench_done += 1;
                    bench_t = Instant::now();
                }
            }
        }
        let mut changed = false;
        // 설정 화면이 열려 있으면 키는 모두 그쪽으로 (선곡은 멈춤)
        if let Some(u) = ui.as_mut() {
            let shift = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) } < 0;
            match if hk != 0 { u.key(hk, shift) } else { Action::None } {
                Action::Close => {
                    ui = None;
                    g = Settings::load(&settings_path).global(); // 저장한 값으로 (미리보기 음량·라이브러리)
                    if lib_arg.is_none() && PathBuf::from(&g.library) != lib {
                        let nl = PathBuf::from(&g.library);
                        match read_index(&nl) {
                            Ok(v) if !v.is_empty() => {
                                reset(&mut preview, &mut loading, &gen);
                                last_result = format!("라이브러리 {} ({}곡)", nl.display(), v.len());
                                pending_lib = Some((v, nl)); // 다음 프레임 처음에 바꾼다 (지금 곡 참조가 살아 있음)
                            }
                            Ok(_) => last_result = format!("라이브러리 {}: 색인에 곡이 없음 (그대로 둠)", nl.display()),
                            Err(err) => last_result = format!("라이브러리 {}: {err} (그대로 둠)", nl.display()),
                        }
                    }
                    changed = true;
                }
                Action::PickLibrary => {
                    if let Some(f) = crate::settings_ui::pick_folder() {
                        u.global.library = f.replace('\\', "/");
                        u.dirty = true;
                        u.msg = if lib_arg.is_some() { "명령줄의 라이브러리 폴더가 이번 실행에서는 우선함".into() } else { "저장(S)하고 닫으면 다시 읽음".into() };
                    }
                }
                Action::Calib(sound) => {
                    reset(&mut preview, &mut loading, &gen);
                    // 보정에 넘길 설정은 임시 폴더의 고유한 새 파일 (사용자 설정 파일과 겹칠 수 없음, 리뷰 3-4 #1). 만든 것만 지운다
                    let tmp = crate::settings::unique_sibling(&std::env::temp_dir().join("cp3-calib-settings.toml"), "run");
                    if tmp == settings_path {
                        u.msg = "보정 준비 실패: 임시 파일 경로가 설정 파일과 같음".into();
                    } else {
                        match u.snapshot_to(&tmp) {
                            Err(err) => u.msg = format!("보정 준비 실패: {err}"),
                            Ok(()) => {
                                let mut a: Vec<String> = [args[0].as_str(), "run", "calib", "--calib", if sound { "sound" } else { "screen" }, "--settings"].iter().map(|s| s.to_string()).collect();
                                a.push(tmp.to_string_lossy().to_string());
                                a.extend(["--quit-at-end", "--tag", "calib"].iter().map(|s| s.to_string()));
                                a.extend(args.iter().skip(pass_from).cloned());
                                match game::run(&a) {
                                    Ok(r) => match r.calib {
                                        Some(o) => u.calib_done(&o),
                                        None => u.msg = r.summary,
                                    },
                                    Err(err) => u.msg = format!("보정 실패: {err}"),
                                }
                                let _ = std::fs::remove_file(&tmp);
                            }
                        }
                    }
                }
                Action::None => {}
            }
            hk = 0;
        }
        match hk {
            0x70 => {
                let layout = preview.as_ref().map(|p| keys::layout_key(&p.store.compiled().chart.lanes));
                ui = Some(Ui::open(&settings_path, layout));
            }
            0x26 => (sel, changed) = ((sel + entries.len() - 1) % entries.len(), true),
            0x28 => (sel, changed) = ((sel + 1) % entries.len(), true),
            0x21 => (sel, changed) = (sel.saturating_sub(10), true),
            0x22 => (sel, changed) = ((sel + 10).min(entries.len() - 1), true),
            0x25 => (chart, changed) = ((chart + n_charts - 1) % n_charts, true),
            0x27 => (chart, changed) = ((chart + 1) % n_charts, true),
            0x20 => {
                want_preview = !want_preview;
                changed = true;
            }
            0xDB | 0xDD | 0x24 => {
                // 탐색: 같은 저장소, 순서만 다시
                if let Some(p) = preview.as_mut() {
                    let t = match hk {
                        0x24 => e.charts.get(chart).map(|c| c.3).unwrap_or(0),
                        k => (p.t0 + if k == 0xDD { 10 } else { -10 } * 1_000_000_000).max(0),
                    };
                    if let Some(pl) = p.play.take() {
                        stop_audio(pl);
                    }
                    p.t0 = t;
                    p.store.reorder(t);
                    p.settled = Instant::now();
                    p.first_ms = None;
                }
            }
            0x0D => {
                let path = lib.join(&e.path).to_string_lossy().to_string();
                let cid = e.charts.get(chart).map(|c| c.0.clone()).unwrap_or_default();
                reset(&mut preview, &mut loading, &gen);
                let mut a: Vec<String> = vec![args[0].clone(), "run".into(), path, "--chart".into(), cid, "--quit-at-end".into()];
                a.extend(args.iter().skip(pass_from).cloned());
                let handoff = {
                    let mut tc = track_cache.lock().unwrap();
                    if tc.0 == sel { std::mem::take(&mut tc.1) } else { SlotCache::new() }
                };
                last_result = match game::run_with(&a, handoff) {
                    Ok(r) => r.summary,
                    Err(err) => format!("플레이 오류: {err}"),
                };
                changed = true;
            }
            0x1B => quit.store(true, Ordering::Relaxed),
            0xFFFF => (chart, changed) = (0, true),
            _ => {}
        }
        if changed {
            if hk == 0x26 || hk == 0x28 || hk == 0x21 || hk == 0x22 {
                chart = 0;
            }
            last_change = Instant::now();
            reset(&mut preview, &mut loading, &gen);
        }
        let e = &entries[sel];
        // 디바운스가 지나면 백그라운드로 열기·계획·저장소 시작
        if want_preview && ui.is_none() && preview.is_none() && loading.is_none() && last_change.elapsed() >= DEBOUNCE {
            let path = lib.join(&e.path);
            let ci = chart;
            let t0 = e.charts.get(chart).map(|c| c.3.max(0)).unwrap_or(0);
            let g = gen.load(Ordering::Acquire);
            let (tx, rx) = channel();
            let gen2 = gen.clone();
            let tc = track_cache.clone();
            let ti = sel;
            std::thread::spawn(move || {
                let r = (|| {
                    let b = Arc::new(Bundle::open(std::fs::read(&path).map_err(|e| e.to_string())?)?);
                    if gen2.load(Ordering::Acquire) != g {
                        return Err("취소".to_string());
                    }
                    let names = engine::load::chart_list(&b)?;
                    let mut c = tc.lock().unwrap();
                    // 곡이 바뀌었거나 합집합이 예산을 넘으면 저장소를 비운다 (곡 사이 캐시 없음, 차분 합집합에도 예산)
                    let ready: u64 = c.1.values().filter_map(|s| s.get()).map(|p| p.bytes() as u64).sum();
                    if c.0 != ti || ready > crate::BUDGET / 2 {
                        *c = (ti, SlotCache::new());
                    }
                    let plan = plan_chart_reuse(&b, ci, &LoadOpts { device_rate: rate, choice: vec![], threads, random_seed: None, budget: crate::BUDGET }, &mut c.1)?;
                    drop(c);
                    Ok((Store::start(plan, t0, threads, gen2, g), names, b))
                })();
                let _ = tx.send(r);
            });
            loading = Some(Loading { gen: g, rx });
        }
        if let Some(l) = loading.as_ref() {
            if let Ok(r) = l.rx.try_recv() {
                if l.gen == gen.load(Ordering::Acquire) {
                    if let Ok((store, names, _b)) = r {
                        let t0 = e.charts.get(chart).map(|c| c.3.max(0)).unwrap_or(0);
                        preview = Some(Preview { store, names, t0, play: None, settled: last_change + DEBOUNCE, first_ms: None });
                    } else if let Err(err) = r {
                        last_result = format!("미리보기 실패: {err}");
                    }
                }
                loading = None;
            }
        }
        if let Some(p) = preview.as_mut() {
            if p.play.is_none() && p.store.ready_at(p.t0, W_NS) {
                match start_audio(&p.store, p.t0, &render_name, &g) {
                    Ok(pl) => p.play = Some(pl),
                    Err(err) => last_result = format!("미리보기 오디오 실패: {err}"),
                }
            }
            if let Some(pl) = p.play.as_mut() {
                while pl.mix.pop().is_some() {}
                while pl.blk.pop().is_some() {}
                // 첫 소리까지 = 미리보기 시작 지점의 첫 콘텐츠 프레임을 (버퍼링 없이 섞어) 오디오 스레드가 장치 버퍼에 쓴 순간
                // (세션 생성 반환도, 버퍼링 무음도 아니라 — 리뷰 3-5 #4)
                let fc = pl.a.monitor.first_content.load(Ordering::Relaxed);
                if p.first_ms.is_none() && fc >= 0 && pl.a.monitor.written.load(Ordering::Relaxed) > fc {
                    let ms = p.settled.elapsed().as_secs_f64() * 1e3;
                    p.first_ms = Some(ms);
                    firsts.push(ms);
                }
            }
        }

        // 그리기
        list.cmds.clear();
        list.clear = Some(Rgba(0.05, 0.05, 0.07, 1.0));
        let hud = Rgba(0.8, 0.85, 0.9, 1.0);
        list.text(16.0, 10.0, 14.0, hud, format!("라이브러리 {} 곡 (색인 {idx_ms:.0} ms) · 장치 {rate} Hz · ↑↓ 곡 ←→ 차분 Enter 플레이 Space 미리보기 [ ] 탐색 Home 하이라이트 F1 설정 Esc", entries.len()));
        for k in -9i64..=9 {
            let i = (sel as i64 + k).rem_euclid(entries.len() as i64) as usize;
            let y = 340.0 + k as f32 * 30.0;
            let c = if k == 0 { Rgba(1.0, 0.9, 0.5, 1.0) } else { Rgba(0.65, 0.68, 0.72, 1.0) };
            let t = &entries[i];
            list.text(30.0, y, if k == 0 { 18.0 } else { 15.0 }, c, format!("{}  —  {}", t.title, t.artist));
        }
        // 차분 목록 + 구간 통계
        let e = &entries[sel];
        let names = preview.as_ref().map(|p| &p.names);
        for (j, c) in e.charts.iter().enumerate() {
            let y = 60.0 + j as f32 * 20.0;
            if y > 330.0 {
                break;
            }
            let label = names.and_then(|n| n.iter().find(|x| x.0 == c.0)).map(|x| format!("{} lv {}", x.1, x.2)).unwrap_or_default();
            let col = if j == chart { Rgba(0.5, 0.9, 1.0, 1.0) } else { Rgba(0.6, 0.6, 0.65, 1.0) };
            list.text(700.0, y, 14.0, col, format!("{:28} 노트 {:5} 최대 {:3}/s  {label}", c.0, c.1, c.2));
        }
        if let Some(c) = e.charts.get(chart) {
            let bins = &c.4;
            let (gx, gy, gw, gh) = (700.0, 680.0, 560.0, 120.0);
            let maxb = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
            let k = (bins.len() as f32 / gw).max(1.0);
            for x in 0..(gw as usize) {
                let i0 = (x as f32 * k) as usize;
                let v = bins.get(i0..((x as f32 + 1.0) * k) as usize).map(|s| s.iter().copied().max().unwrap_or(0)).unwrap_or(0) as f32;
                list.rect(gx + x as f32, gy - gh * v / maxb, 1.0, gh * v / maxb, Rgba(0.35, 0.6, 1.0, 0.8));
            }
            // 미리보기 위치
            if let Some(p) = preview.as_ref() {
                let pos_s = p.t0 as f64 / 1e9 + p.play.as_ref().map(|pl| pl.started.elapsed().as_secs_f64()).unwrap_or(0.0);
                let x = gx + (pos_s / 0.25 / k as f64) as f32;
                list.rect(x.min(gx + gw), gy - gh, 2.0, gh, Rgba(1.0, 0.5, 0.4, 1.0));
            }
        }
        let status = match (&preview, &loading) {
            (Some(p), _) => {
                let (done, total) = (p.store.done.load(Ordering::Relaxed), p.store.total());
                let head = match &p.play {
                    Some(pl) => format!(
                        "미리보기 재생 {:.1} s부터 · 첫 소리까지 {:.0} ms · 소리 {done}/{total} · 버퍼링 {}번 · 메모리 {:.0} MB",
                        pl.t0 as f64 / 1e9,
                        p.first_ms.unwrap_or(0.0),
                        pl.a.monitor.pauses.load(Ordering::Relaxed),
                        p.store.compiled().chart.bank.bytes() as f64 / 1e6
                    ),
                    None => format!("미리보기 준비 중 · 소리 {done}/{total}"),
                };
                head + &match p.store.failed.load(Ordering::Relaxed) {
                    0 => String::new(),
                    n => format!(" · 디코딩 실패 {n} (무음)"),
                }
            }
            (None, Some(_)) => "묶음 여는 중".into(),
            _ if !want_preview => "미리보기 꺼짐 (Space)".into(),
            _ => String::new(),
        };
        list.text(700.0, 530.0, 15.0, Rgba(1.0, 0.8, 0.5, 1.0), status);
        if !firsts.is_empty() {
            let mut v = firsts.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            list.text(700.0, 552.0, 13.0, hud, format!("첫 소리까지: {}번, 중앙 {:.0} ms, 최대 {:.0} ms", v.len(), v[v.len() / 2], v[v.len() - 1]));
        }
        list.text(16.0, 690.0, 14.0, Rgba(0.6, 0.95, 0.6, 1.0), last_result.as_str());
        if let Some(u) = ui.as_ref() {
            u.draw(&mut list);
        }
        rend.draw(&list)?;
        let _ = rend.present(true);
        let _ = qpc::now();
        std::thread::sleep(Duration::from_millis(1));
    }
    reset(&mut preview, &mut loading, &gen);
    drop(rend);
    game::close_window(hwnd);
    if !firsts.is_empty() {
        let mut v = firsts.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        println!("미리보기 첫 소리까지 (디바운스 150 ms 뒤부터): {}곡, p50 {:.0} ms, p95 {:.0} ms, 최대 {:.0} ms, 실패 {bench_fail}", v.len(), q(0.5), q(0.95), q(1.0));
    }
    Ok(())
}
