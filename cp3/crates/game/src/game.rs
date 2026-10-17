//! `cp3game run`: 여러 레인 게임. CP2 game.rs에서 갈라져 나왔다 — Track 묶음, 레이아웃이 정하는 레인, IIDX식 판정·게이지.
//!
//!   cp3game run <묶음.trk> [--chart <차분 id>] [--gauge normal|easy|assist|hard|exhard] [--choice 0,1,…]
//!                [--audio shared|exclusive|asio] [--asio-buffer 64] [--excl-period-ms 5] [--d-ms (자동)]
//!                [--visible-ms 600] [--offset-a-ms 0] [--display-offset-ms 0] [--bot] [--bot-direct] [--bot-offset-ms 0] [--bot-jitter-ms 0]
//!                [--faults audio@30:150,render@45:300,judge@60:300,input@75:300] [--fault-ms 300]
//!                [--no-vsync] [--no-capture] [--tag run] [--out runs] [--quit-at-end] [--stop-at 곡초] [--late-drop-ms 100]
//!                [--settings settings.toml] [--seed -1] [--empty-sound chart|previous|next|none] [--scroll-base chart|start|mode|max|min]
//!                [--bot-ignore-a]  (봇이 A를 모르는 척: 청취 시각 기준으로 누른다 — A 효과 시험용)
//!   cp3game run calib --calib sound|screen [...]   보정 (calib_ui.rs): 합성 차분을 같은 플레이 경로로 돌리고 오프셋 제안
//!
//! 값의 출처: 명령줄 > `settings.toml`(3-4, settings.rs의 범위별 표) > 기본값. 설정 파일이 없으면 3-3과 같은 기본값.
//! 오프셋 A(`--offset-a-ms`, 장치별 `offset_a_ms`) = 판정 + 키음 (청취 곡 시각에 더함), 오프셋 B(`--display-offset-ms`, `offset_b_ms`) = 화면만.

//! 시계·스레드·고장 주입·기록은 CP2와 같다(우리 방식만; CP2의 프레임 구동 프로필은 비교 실험용이라 가져오지 않았다).
//! 키: 레이아웃마다 설정의 배치 또는 기본 배치(`keys::keymap`), 봇은 레인마다 F13~ 키. F5 오토, G 그래프, F1~F4 고장, Esc 끝.
//! 노출 시간: 기준 스크롤(차분의 reference_scroll, 박/분)일 때 노트가 나타나 판정선에 닿기까지 `--visible-ms`.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use engine::judge::{Grade, Rules};
use engine::runtime::LongRule;
use engine::mixer::{LatePolicy, MixEvent};
use engine::ring::{ring, Consumer};
use engine::runtime::{Compiled, NoteKind, Role, NS_PER_S};
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};

use crate::audio::{self, AudioCfg};
use crate::bot::{self, BotCfg, BotLog};
use crate::envlog::EnvLog;
use crate::graph::{LiveGraph, Mark};
use crate::judging::{self, JudgeCfg, JudgeLog};
use crate::keys;
use crate::monitor::{self, Heartbeats, LagEvent, MonitorShared, LAG_NAMES, STATE_NAMES};
use crate::render::{D3D11Renderer, DrawList, FrameTiming, Renderer, Rgba};
use crate::settings::Settings;
use crate::{arg, calib_ui, capture, flag};

const W: u32 = 1280;
const H: u32 = 720;
const JUDGE_Y: f32 = 620.0;

#[derive(Clone, Copy, Debug)]
enum FaultKind {
    Audio,
    Render,
    Judge,
    Input,
}

#[derive(Clone, Copy, Debug)]
struct Fault {
    kind: FaultKind,
    at_s: f64,
    ms: u32,
}

fn parse_faults(s: &str) -> Result<Vec<Fault>, String> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|f| {
            let (k, rest) = f.split_once('@').ok_or(format!("고장 형식 종류@곡초:ms — {f}"))?;
            let (t, ms) = rest.split_once(':').ok_or(format!("고장 형식 — {f}"))?;
            let kind = match k {
                "audio" => FaultKind::Audio,
                "render" => FaultKind::Render,
                "judge" => FaultKind::Judge,
                "input" => FaultKind::Input,
                _ => return Err(format!("고장 종류: {k}")),
            };
            Ok(Fault { kind, at_s: t.parse().map_err(|_| f.to_string())?, ms: ms.parse().map_err(|_| f.to_string())? })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameLog {
    t: FrameTiming,
    song_ns: i64,
    q_display: i64,
    notes_drawn: u16,
    injected_ms: u16,
}

thread_local! {
    /// 창이 받은 키 (창 스레드 = 메시지 루프 스레드에서만 쓴다). 한 프레임에 여러 키가 와도 잃지 않는다.
    static HOTKEYS: std::cell::RefCell<std::collections::VecDeque<u32>> = std::cell::RefCell::new(std::collections::VecDeque::with_capacity(16));
}

/// 창이 받은 키 (WM_KEYDOWN, 들어온 순서) 하나를 꺼낸다. 없으면 0.
pub fn take_hotkey() -> u32 {
    HOTKEYS.with(|q| q.borrow_mut().pop_front().unwrap_or(0))
}

/// WM_KEYDOWN/WM_SYSKEYDOWN의 (wParam, lParam) → 가상 키. Shift·Ctrl·Alt는 왼쪽·오른쪽을 가른다
/// (입력 스레드의 Raw Input 가상 키 0xA0~0xA5와 같게: 키 묶기 화면과 플레이 입력이 같은 번호).
pub fn window_vkey(wp: usize, lp: isize) -> u32 {
    let ext = (lp >> 24) & 1 == 1;
    match wp {
        0x10 => if ((lp >> 16) & 0xFF) == 0x36 { 0xA1 } else { 0xA0 },
        0x11 => if ext { 0xA3 } else { 0xA2 },
        0x12 => if ext { 0xA5 } else { 0xA4 },
        v => v as u32,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if msg == WM_KEYDOWN || wp.0 == 0x12 => {
            let v = window_vkey(wp.0, lp.0);
            HOTKEYS.with(|q| {
                let mut q = q.borrow_mut();
                if q.len() < 64 {
                    q.push_back(v);
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 창을 앞으로 (3-5): 뒤에서 띄운 프로세스는 Windows의 포커스 빼앗기 방지에 막히므로, 지금 앞 창의 입력 스레드에 잠깐 붙어서 올린다.
/// 실패해도 진행한다 (봇은 앞 창이 아니면 누르지 않는다).
fn bring_to_front(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
    unsafe {
        let fg = GetForegroundWindow();
        let other = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = other != 0 && other != me && AttachThreadInput(me, other, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(me, other, false);
        }
    }
}

pub fn create_window(title: windows::core::PCWSTR) -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("cp3_game");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
        let mut rc = windows::Win32::Foundation::RECT { left: 0, top: 0, right: W as i32, bottom: H as i32 };
        AdjustWindowRect(&mut rc, style, false)?;
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, title, style, CW_USEDEFAULT, CW_USEDEFAULT, rc.right - rc.left, rc.bottom - rc.top, None, None, hinst, None)?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        bring_to_front(hwnd);
        Ok(hwnd)
    }
}

struct Logs {
    blocks: Consumer<audio::BlockLog>,
    mix: Consumer<MixEvent>,
    judge: Consumer<JudgeLog>,
    bot: Consumer<BotLog>,
    frames: Consumer<FrameLog>,
    lags: Consumer<LagEvent>,
}

/// 기록 스레드: 링을 비워 CSV로 쓴다. 실시간 스레드는 파일을 만지지 않는다.
fn spawn_logger(dir: PathBuf, mut l: Logs, stop: Arc<AtomicBool>) -> thread::JoinHandle<std::io::Result<()>> {
    thread::Builder::new()
        .name("logger".into())
        .spawn(move || {
            let mut fb = BufWriter::new(File::create(dir.join("audio_blocks.csv"))?);
            writeln!(fb, "qpc_wake,tl,offset,padding,frames,dev_frame,dev_qpc100,need,fill_ns,voices,injected_ms")?;
            let mut fm = BufWriter::new(File::create(dir.join("mix_events.csv"))?);
            writeln!(fm, "kind,sound,due,start,block,late,player,gain,t_ns")?;
            let mut fj = BufWriter::new(File::create(dir.join("judgments.csv"))?);
            writeln!(fj, "kind,lane,note,grade,err_ns,t_ns,q_in,q_proc,vkey,key_frame,state,q_recv,q_floor")?;
            let mut fl = BufWriter::new(File::create(dir.join("lag_events.csv"))?);
            writeln!(fl, "kind,name,q_start,q_detect,q_end,magnitude_us")?;
            let mut fo = BufWriter::new(File::create(dir.join("bot.csv"))?);
            writeln!(fo, "lane,note,down,target_ns,q_target,q_sent")?;
            let mut ff = BufWriter::new(File::create(dir.join("frames.csv"))?);
            writeln!(ff, "q_wait_start,q_wait_end,q_present_start,q_present_end,present_count,present_refresh,sync_refresh,sync_qpc,song_ns,q_display,notes_drawn,injected_ms")?;
            loop {
                let last = stop.load(Ordering::Acquire);
                while let Some(b) = l.blocks.pop() {
                    writeln!(fb, "{},{},{},{},{},{},{},{},{},{},{}", b.qpc_wake, b.tl, b.offset, b.padding, b.frames, b.dev_frame, b.dev_qpc100, b.need, b.fill_ns, b.voices, b.injected_ms)?;
                }
                while let Some(e) = l.mix.pop() {
                    match e {
                        MixEvent::Start { sound, due, start, block, late, player, gain, t } => writeln!(fm, "start,{sound},{due},{start},{block},{late},{},{gain},{}", player as u8, if t == engine::mixer::NO_T { String::new() } else { t.to_string() })?,
                        MixEvent::Dropped { sound, due, block, player } => writeln!(fm, "dropped,{sound},{due},,{block},,{},,", player as u8)?,
                        MixEvent::Overflow { sound, due, player } => writeln!(fm, "overflow,{sound},{due},,,,{},,", player as u8)?,
                        MixEvent::Cut { sound, at } => writeln!(fm, "cut,{sound},{at},,,,,,")?,
                    }
                }
                while let Some(j) = l.judge.pop() {
                    writeln!(fj, "{},{},{},{},{},{},{},{},{},{},{},{},{}", j.kind, j.lane, j.note, j.grade, j.err_ns, j.t_ns, j.q_in, j.q_proc, j.vkey, j.key_frame, j.state, j.q_recv, j.q_floor)?;
                }
                while let Some(e) = l.lags.pop() {
                    writeln!(fl, "{},{},{},{},{},{}", e.kind, LAG_NAMES[e.kind as usize], e.q_start, e.q_detect, e.q_end, e.magnitude_us)?;
                }
                while let Some(o) = l.bot.pop() {
                    writeln!(fo, "{},{},{},{},{},{}", o.lane, o.note, o.down as u8, o.target_ns, o.q_target, o.q_sent)?;
                }
                while let Some(f) = l.frames.pop() {
                    let t = f.t;
                    writeln!(
                        ff,
                        "{},{},{},{},{},{},{},{},{},{},{},{}",
                        t.q_wait_start, t.q_wait_end, t.q_present_start, t.q_present_end, t.present_count, t.present_refresh, t.sync_refresh, t.sync_qpc, f.song_ns, f.q_display, f.notes_drawn, f.injected_ms
                    )?;
                }
                if last {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            for f in [&mut fb, &mut fm, &mut fj, &mut fo, &mut ff, &mut fl] {
                f.flush()?;
            }
            Ok(())
        })
        .expect("기록 스레드")
}

/// 레인 그리기 배치: (x, 폭, 노트 색)
fn lane_geometry(c: &Compiled) -> Vec<(f32, f32, Rgba)> {
    let widths: Vec<f32> = c.chart.lanes.iter().map(|l| match l.role {
        Role::Scratch => 84.0,
        Role::Side => 72.0,
        _ => 50.0,
    }).collect();
    let gap = 2.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
    let mut x = (W as f32 - total) / 2.0 - 120.0;
    // 건반 색은 양 끝에서 대칭으로: k번째 건반은 min(k, n−1−k)가 짝수면 흰색 (6건반 백청백백청백, 7건반 백청백청백청백, DP 14건반은 양쪽 7개씩)
    let n_keys = c.chart.lanes.iter().filter(|l| !matches!(l.role, Role::Scratch | Role::Side | Role::Trigger)).count();
    let mut key_i = 0;
    let mut out = Vec::new();
    for (l, w) in c.chart.lanes.iter().zip(&widths) {
        let col = match l.role {
            Role::Scratch => Rgba(0.95, 0.35, 0.35, 1.0),
            Role::Side => Rgba(0.95, 0.75, 0.3, 1.0),
            Role::Trigger => Rgba(0.6, 0.9, 0.5, 1.0),
            _ => {
                let mirror = key_i.min(n_keys - 1 - key_i);
                key_i += 1;
                if mirror % 2 == 0 { Rgba(0.95, 0.95, 0.95, 1.0) } else { Rgba(0.35, 0.6, 1.0, 1.0) }
            }
        };
        out.push((x, *w, col));
        x += w + gap;
    }
    out
}

pub fn gauge_of(s: &str) -> Result<engine::judge::GaugeKind, String> {
    use engine::judge::GaugeKind::*;
    Ok(match s {
        "normal" => Normal,
        "easy" => Easy,
        "assist" => AssistEasy,
        "hard" => Hard,
        "exhard" => ExHard,
        _ => return Err(format!("--gauge normal|easy|assist|hard|exhard: {s}")),
    })
}

/// 규칙 해시 (3-4, `cp3/설정창_계획.md` 3장): 판정에 영향을 주는 값의 정규 바이트열 → BLAKE3 (16진 64자). 점수 비교 단위.
/// 넣는 것: `Rules`(창·게이지 표·빈 POOR)·롱노트 규칙·재동기화 중 판정 정책(무효/집계 — 점수를 바꾼다, 리뷰 3-4 #7).
/// 오프셋 A·B·음량·D는 판정 규칙이 아니므로 넣지 않는다.
pub fn rules_hash(rules: &Rules, long_rule: LongRule, resync_void: bool) -> String {
    let mut b = b"cp3-rules/2\0".to_vec();
    b.extend_from_slice(&rules.canonical_bytes(long_rule));
    b.push(resync_void as u8);
    blake3::hash(&b).to_hex().to_string()
}

pub struct RunResult {
    /// 결과 요약 한 줄 (선곡 화면이 보여 준다)
    pub summary: String,
    /// 보정 실행이면 분석 결과 (측정 장치·B·문제 포함)
    pub calib: Option<calib_ui::CalibOut>,
}

/// 플레이 한 번.
pub fn run(args: &[String]) -> Result<RunResult, Box<dyn std::error::Error>> {
    run_with(args, engine::load::SlotCache::new())
}

/// 플레이 (선곡 화면의 소리 저장소를 넘겨받아: 준비된 PCM은 다시 쓰고 모자란 것만 디코딩, README 4-3 7).
pub fn run_with(args: &[String], mut cache: engine::load::SlotCache) -> Result<RunResult, Box<dyn std::error::Error>> {
    unsafe { timeBeginPeriod(1) };
    let r = run_inner(args, &mut cache);
    unsafe { timeEndPeriod(1) };
    r
}

fn run_inner(args: &[String], cache: &mut engine::load::SlotCache) -> Result<RunResult, Box<dyn std::error::Error>> {
    let path = args.get(2).ok_or("Track 묶음(.trk)이 필요함")?;
    let mut set = Settings::load(std::path::Path::new(&arg(args, "--settings", "settings.toml")));
    let g = set.global();
    let render_name = arg(args, "--render", &g.render);
    let render_key = render_name.clone();
    let apath = audio::AudioPath::parse(&arg(args, "--audio", &g.audio))?;
    let dev = set.device(&format!("{}:{render_name}", apath.key()));
    let calib = match arg(args, "--calib", "").as_str() {
        "" => None,
        "sound" => Some(true),
        "screen" => Some(false),
        v => return Err(format!("--calib sound|screen: {v}").into()),
    };
    let dev_rate = audio::device_rate(apath, &render_name)?;
    let t_load = qpc::now();
    // 변형: --choice가 없으면 원작처럼 매번 무작위 (고른 결과는 meta.json variant_choice)
    // 변형 시드: 명령줄 --seed > 설정(프리셋, −1 = 무작위) > 지금 시각
    let seed_set = arg(args, "--seed", &set.variant_seed().to_string()).parse::<i64>().map_err(|_| "--seed 정수")?;
    let seed = if seed_set >= 0 { seed_set as u64 } else { SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64 };
    let mut loaded = match calib {
        Some(with_click) => crate::Loaded { compiled: calib_ui::chart(dev_rate, with_click), title: "보정".into(), losses: vec![] },
        None => crate::load_track(path, &arg(args, "--chart", ""), &arg(args, "--choice", ""), dev_rate, Some(seed), cache)?,
    };
    let load_ms = (qpc::now() - t_load) as f64 * 1e3 / qpc::frequency() as f64;
    // 빈 누름 소리 (소리만 바뀌고 판정은 같다 → 규칙 해시에 넣지 않음). none이면 빈 레인 소리도 끈다
    let empty_sound = arg(args, "--empty-sound", &g.empty_sound);
    {
        use engine::runtime::EmptyLane;
        let c = &mut loaded.compiled.chart;
        match empty_sound.as_str() {
            "chart" => {}
            "previous" => c.empty_lane = EmptyLane::PreviousNote,
            "next" => c.empty_lane = EmptyLane::NextNote,
            "none" => {
                c.empty_lane = EmptyLane::None;
                c.lanes.iter_mut().for_each(|l| l.lane_sounds.clear());
            }
            v => return Err(format!("--empty-sound chart|previous|next|none: {v}").into()),
        }
    }
    let chart = Arc::new(loaded.compiled);
    let (gain0, peak) = crate::master_gain(&chart);
    let gain = gain0 * g.volume_master as f32;
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    let late_drop_ms: f64 = arg(args, "--late-drop-ms", &g.late_drop_ms.to_string()).parse().map_err(|_| "--late-drop-ms ms")?;
    let d_manual = if dev.d_auto { String::new() } else { dev.d_manual_ms.to_string() };
    let d_ms: Option<f64> = match arg(args, "--d-ms", &d_manual).as_str() {
        "" => None,
        v => Some(v.parse().map_err(|_| "--d-ms ms")?),
    };
    let mut fault_ms: u32 = arg(args, "--fault-ms", "300").parse().map_err(|_| "--fault-ms ms")?;
    let late_drop_frames = if late_drop_ms > 0.0 { (late_drop_ms * dev_rate as f64 / 1e3) as i64 } else { i64::MAX };
    let stop_at: f64 = arg(args, "--stop-at", "0").parse().map_err(|_| "--stop-at 곡초")?;
    let display_offset_ns = (arg(args, "--display-offset-ms", &dev.offset_b_ms.to_string()).parse::<f64>()? * 1e6) as i64;
    let layout = keys::layout_key(&chart.chart.lanes);
    let km = set.keymode(&layout, chart.chart.lanes.len());
    let visible_s: f64 = arg(args, "--visible-ms", &km.visible_ms.to_string()).parse::<f64>().map_err(|_| "--visible-ms ms")? / 1e3;
    let scroll_base = arg(args, "--scroll-base", &km.scroll_base);
    let base_bpm = chart.chart.scroll_base(&scroll_base);
    // 오프셋 A: 소리로 맞추기(보정)는 지금 A를 빼고 재야 사람의 늦음이 그대로 나온다
    let out_offset_ns = if calib == Some(true) { 0 } else { (arg(args, "--offset-a-ms", &dev.offset_a_ms.to_string()).parse::<f64>().map_err(|_| "--offset-a-ms ms")? * 1e6).round() as i64 };
    let vsync = !flag(args, "--no-vsync");
    let use_bot = flag(args, "--bot");
    let gauge_name = set.gauge();
    let rules = Rules::iidx(gauge_of(&arg(args, "--gauge", &gauge_name))?);
    let resync_void = arg(args, "--resync-judge", "void") != "count";
    let rhash = rules_hash(&rules, chart.chart.long_rule, resync_void);
    let keymap = keys::keymap(&chart.chart.lanes, &km.keys);
    for w in &set.warnings {
        println!("  설정: {w}");
    }
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = arg(args, "--tag", "run");
    let dir = PathBuf::from(arg(args, "--out", "runs")).join(format!("{tag}-{unix}"));
    fs::create_dir_all(&dir)?;
    println!("{} [{}] → {}", loaded.title, chart.chart.chart_id, dir.display());
    println!("  불러오기 {load_ms:.0} ms (소리 {} 개, {:.1} MB, 장치 {dev_rate} Hz)", chart.chart.bank.sounds.len(), chart.chart.bank.bytes() as f64 / 1e6);
    if !loaded.losses.is_empty() {
        println!("  불완전 변환: {}", loaded.losses.join(", "));
    }

    // 녹음
    let cap_stop = Arc::new(AtomicBool::new(false));
    let cap_started = Arc::new(AtomicBool::new(false));
    let cap = if flag(args, "--no-capture") || apath == audio::AudioPath::Asio {
        None
    } else {
        let (stop, started, d, name) = (cap_stop.clone(), cap_started.clone(), dir.clone(), arg(args, "--capture", "Loopback 1/2"));
        let h = thread::Builder::new().name("capture".into()).spawn(move || capture::capture_loop(&name, &d, stop, started))?;
        while !cap_started.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        Some(h)
    };

    let (key_tx, key_rx) = ring::<audio::KeyReq>(1024);
    let (blk_tx, blk_rx) = ring::<audio::BlockLog>(1 << 14);
    let (mix_tx, mix_rx) = ring::<MixEvent>(1 << 16);
    let (kev_tx, kev_rx) = ring::<KeyEvent>(4096);
    let (jl_tx, jl_rx) = ring::<JudgeLog>(1 << 15);
    let (bot_tx, bot_rx) = ring::<BotLog>(1 << 15);
    let (fr_tx, fr_rx) = ring::<FrameLog>(1 << 14);
    let (lag_tx, lag_rx) = ring::<LagEvent>(4096);
    let (live_tx, mut live_rx) = ring::<audio::SyncSample>(1 << 13);
    let mut fr_tx = fr_tx;

    let a = audio::spawn(
        chart.clone(),
        AudioCfg {
            path: apath,
            render: render_name,
            asio_buffer: arg(args, "--asio-buffer", &g.asio_buffer.to_string()).parse().map_err(|_| "--asio-buffer 샘플")?,
            excl_period_ms: arg(args, "--excl-period-ms", &g.excl_period_ms.to_string()).parse().map_err(|_| "--excl-period-ms ms")?,
            lead_in_s: 2.0,
            tail_s: 1.0,
            master_gain: gain,
            late: LatePolicy { drop_after: late_drop_frames, ..LatePolicy::DEFAULT },
            autoplay_notes: false,
            auto_from_chart: true,
            d_ms,
            d_margin_ms: dev.d_margin_ms,
            volumes: (g.volume_bgm as f32, g.volume_keys as f32),
            skip_on_stall: true,
            asio_capture: (apath == audio::AudioPath::Asio && !flag(args, "--no-capture")).then(|| dir.clone()),
            faults: vec![],
            live: Some(audio::LiveSync { tx: live_tx, pclock: None }),
            preview: None,
        },
        key_rx,
        blk_tx,
        mix_tx,
    )?;
    let (origin, rate) = (a.info.origin, a.info.rate);
    let direct = flag(args, "--bot-direct");
    let (input, mut bot_keys) = if direct { (crate::input::Input::direct(), Some(kev_tx)) } else { (crate::input::Input::spawn(kev_tx, flag(args, "--input-mmcss"))?, None) };
    let stop = Arc::new(AtomicBool::new(false));
    let beats = Arc::new(Heartbeats::default());
    let mon = Arc::new(MonitorShared::default());
    let monitor_join = monitor::spawn(mon.clone(), a.monitor.clone(), a.clock.clone(), input.watermark.clone(), beats.clone(), lag_tx, stop.clone());
    let (judge, judge_join) = judging::spawn(
        chart.clone(),
        JudgeCfg { origin, rate, out_offset_ns, rules, keymap: keymap.clone(), resync_void, audio: a.monitor.clone() },
        a.clock.clone(),
        input.watermark.clone(),
        kev_rx,
        key_tx,
        jl_tx,
        stop.clone(),
        beats.clone(),
        mon.clone(),
    );
    let auto_on = Arc::new(AtomicBool::new(use_bot || direct));
    let bot_join = {
        let cfg = BotCfg {
            origin,
            rate,
            out_offset_ns: if flag(args, "--bot-ignore-a") { 0 } else { out_offset_ns },
            offset_ns: (arg(args, "--bot-offset-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            jitter_ns: (arg(args, "--bot-jitter-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            seed: 12345,
            adapt_ns: (arg(args, "--bot-adapt-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            follow: None,
            enabled: auto_on.clone(),
            direct: direct.then(|| input.watermark.clone()),
        };
        bot::spawn(chart.clone(), cfg, a.clock.clone(), bot_tx, stop.clone(), bot_keys.take())
    };
    let log_stop = Arc::new(AtomicBool::new(false));
    let logger = spawn_logger(dir.clone(), Logs { blocks: blk_rx, mix: mix_rx, judge: jl_rx, bot: bot_rx, frames: fr_rx, lags: lag_rx }, log_stop.clone());
    let mut flog = BufWriter::new(File::create(dir.join("faults.csv"))?);
    writeln!(flog, "kind,at_song_s,ms,q,source")?;

    let hwnd = create_window(w!("CP3 — 여러 레인"))?;
    let mut rend = D3D11Renderer::new(hwnd, W, H)?;
    let geo = lane_geometry(&chart);
    let key_names = keys::keymap_names(&keymap, chart.chart.lanes.len());
    let qf = qpc::frequency();
    // 기준 스크롤(박/분)에서 노출 시간 visible_s → 1박의 픽셀
    let px_per_beat = (JUDGE_Y as f64) / (visible_s * base_bpm / 60.0);
    let mut fired = vec![false; faults.len()];
    let mut period_q = qf / 60;
    let mut last_sync_q = 0i64;
    let mut first_sync: Option<(i64, u32)> = None;
    let mut fps_frames = 0u32;
    let mut fps_t0 = qpc::now();
    let (mut fps, mut max_gap_ms, mut cur_max_gap) = (0.0, 0.0, 0.0f64);
    let mut prev_wait_end = qpc::now();
    let mut quit = false;
    let mut msg = MSG::default();
    let mut list = DrawList::default();
    let mut graph = LiveGraph::new(&dir, qf)?;
    graph.visible = false;
    let mut envlog = EnvLog::new(&dir, qf)?;
    let mut env_gap = 0u64;
    let mut prev_skipped = 0i64;
    while !quit {
        let (_ws, we) = rend.wait();
        beats.render.store(we, Ordering::Release);
        let gap = (we - prev_wait_end) as f64 * 1e3 / qf as f64;
        prev_wait_end = we;
        cur_max_gap = cur_max_gap.max(gap);
        fps_frames += 1;
        if we - fps_t0 >= qf {
            fps = fps_frames as f64 * qf as f64 / (we - fps_t0) as f64;
            max_gap_ms = cur_max_gap;
            cur_max_gap = 0.0;
            fps_frames = 0;
            fps_t0 = we;
        }
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    quit = true;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let now = qpc::now();
        let q_display = if last_sync_q > 0 { last_sync_q + ((now - last_sync_q) / period_q + 1) * period_q } else { now + period_q };
        let song = judging::song_ns(&a.clock, q_display, origin, rate, 0).map(|x| x.0);
        let song_s = song.unwrap_or(i64::MIN) as f64 / NS_PER_S as f64;

        // 고장 주입: 예약 + 단축키
        let mut injected = 0u16;
        let mut inject = |k: FaultKind, ms: u32, src: &str, flog: &mut BufWriter<File>| -> std::io::Result<()> {
            writeln!(flog, "{k:?},{song_s:.3},{ms},{},{src}", qpc::now())?;
            graph.mark(Mark { q: qpc::now(), len_q: ms as i64 * qf / 1000, color: Rgba(1.0, 0.5, 0.4, 0.18), label: format!("{k:?} {ms}") });
            match k {
                FaultKind::Audio => a.monitor.inject_stall_ms.store(ms, Ordering::Release),
                FaultKind::Judge => judge.inject_stall_ms.store(ms, Ordering::Release),
                FaultKind::Input => input.stall(ms),
                FaultKind::Render => {
                    thread::sleep(Duration::from_millis(ms as u64));
                    injected += ms as u16;
                }
            }
            Ok(())
        };
        for (i, f) in faults.iter().enumerate() {
            if !fired[i] && song.is_some() && song_s >= f.at_s {
                fired[i] = true;
                inject(f.kind, f.ms, "script", &mut flog)?;
            }
        }
        let hk = take_hotkey();
        match hk {
            0x70 => inject(FaultKind::Audio, fault_ms, "key", &mut flog)?,
            0x71 => inject(FaultKind::Render, fault_ms, "key", &mut flog)?,
            0x72 => inject(FaultKind::Judge, fault_ms, "key", &mut flog)?,
            0x73 => inject(FaultKind::Input, fault_ms, "key", &mut flog)?,
            0xBB | 0x6B => fault_ms = (fault_ms * 2).min(4000),
            0xBD | 0x6D => fault_ms = (fault_ms / 2).max(50),
            0x74 => auto_on.store(!auto_on.load(Ordering::Relaxed), Ordering::Relaxed),
            0x47 => graph.visible = !graph.visible,
            0x1B => quit = true,
            _ => {}
        }
        graph.ingest(&mut live_rx, now)?;
        let sk = a.monitor.skipped_frames.load(Ordering::Relaxed);
        if sk > prev_skipped {
            graph.skip(now, (sk - prev_skipped) as f64 * 1e3 / rate as f64);
            prev_skipped = sk;
        }

        // 그리기
        list.cmds.clear();
        list.clear = Some(Rgba(0.04, 0.04, 0.06, 1.0));
        let view = judge.view.lock().map(|v| v.clone()).unwrap_or_else(|_| judging::new_view(&chart, &rules));
        let (lx0, lx1) = (geo.first().map_or(0.0, |g| g.0), geo.last().map_or(0.0, |g| g.0 + g.1));
        for (li, &(x, w, _)) in geo.iter().enumerate() {
            list.rect(x, 0.0, w, JUDGE_Y, Rgba(0.09, 0.09, 0.12, 1.0));
            if view.pressed.get(li).copied().unwrap_or(false) {
                list.rect(x, JUDGE_Y - 200.0, w, 200.0, Rgba(0.25, 0.35, 0.6, 0.35));
            }
            list.text(x + 4.0, JUDGE_Y + 14.0, 12.0, Rgba(0.6, 0.65, 0.7, 1.0), key_names[li].as_str());
        }
        list.rect(lx0 - 6.0, JUDGE_Y, lx1 - lx0 + 12.0, 4.0, Rgba(0.9, 0.3, 0.3, 1.0));
        let mut drawn = 0u16;
        if let Some(song) = song.map(|s| s + display_offset_ns) {
            let pos_now = chart.chart.scroll_pos(song);
            for b in &chart.chart.bars {
                let y = JUDGE_Y as f64 - (chart.chart.scroll_pos(*b) - pos_now) * px_per_beat;
                if (0.0..JUDGE_Y as f64).contains(&y) {
                    list.rect(lx0, y as f32, lx1 - lx0, 1.0, Rgba(0.3, 0.3, 0.35, 1.0));
                }
            }
            for (li, lane) in chart.chart.lanes.iter().enumerate() {
                let (x, w, col) = geo[li];
                let start = view.first_open.get(li).copied().unwrap_or(0);
                for (i, n) in lane.notes.iter().enumerate().skip(start) {
                    if view.done.get(li).and_then(|d| d.get(i)).copied().unwrap_or(false) {
                        continue;
                    }
                    let y_head = JUDGE_Y as f64 - (chart.chart.scroll_pos(n.t) - pos_now) * px_per_beat;
                    if y_head < -40.0 && chart.chart.scroll_pos(n.t) >= pos_now {
                        break;
                    }
                    let y_tail = JUDGE_Y as f64 - (chart.chart.scroll_pos(n.end) - pos_now) * px_per_beat;
                    match n.kind {
                        NoteKind::Long => list.rect(x + w * 0.2, y_tail as f32, w * 0.6, (y_head - y_tail) as f32, Rgba(col.0 * 0.6, col.1 * 0.6, col.2 * 0.6, 0.8)),
                        NoteKind::Mine => {
                            list.rect(x + 4.0, y_head as f32 - 8.0, w - 8.0, 8.0, Rgba(0.9, 0.2, 0.2, 0.9));
                            drawn += 1;
                            continue;
                        }
                        NoteKind::Tap => {}
                    }
                    list.rect(x + 2.0, y_head as f32 - 12.0, w - 4.0, 12.0, col);
                    drawn += 1;
                }
            }
        }
        if let Some((g, err)) = view.last {
            let name = ["PGREAT", "GREAT", "GOOD", "BAD", "POOR"][g as usize];
            let c = if g == Grade::PGreat { Rgba(0.6, 0.95, 1.0, 1.0) } else { Rgba(1.0, 0.9, 0.4, 1.0) };
            list.text((lx0 + lx1) / 2.0 - 70.0, JUDGE_Y - 220.0, 28.0, c, format!("{name} {:+.1}", err as f64 / 1e6));
        }
        let s = &view.score;
        // 게이지 막대 (2% 눈금)
        let gx = lx1 + 24.0;
        let gpct = s.gauge.percent() as f32 / 100.0;
        list.rect(gx, 60.0, 16.0, JUDGE_Y - 60.0, Rgba(0.15, 0.15, 0.18, 1.0));
        let gcol = if s.gauge.survival() { Rgba(0.95, 0.4, 0.3, 1.0) } else if gpct >= 0.8 { Rgba(0.95, 0.5, 0.8, 1.0) } else { Rgba(0.4, 0.75, 1.0, 1.0) };
        list.rect(gx, JUDGE_Y - (JUDGE_Y - 60.0) * gpct, 16.0, (JUDGE_Y - 60.0) * gpct, gcol);
        let hud = Rgba(0.8, 0.85, 0.9, 1.0);
        let lines = [
            format!("{} [{}] {} | 규칙 {} | A {:+.1} ms B {:+.1} ms", loaded.title, chart.chart.chart_id, if loaded.losses.is_empty() { "" } else { "(불완전 변환)" }, &rhash[..8], out_offset_ns as f64 / 1e6, display_offset_ns as f64 / 1e6),
            format!("EX {} | PG {} GR {} GD {} BD {} PR {} 빈PR {} | 콤보 {} (최대 {}) | 게이지 {}% {}", s.ex(), s.pgreat, s.great, s.good, s.bad, s.poor, s.empty_poor, s.combo, s.max_combo, s.gauge.percent(), if s.gauge.failed { "탈락" } else { "" }),
            format!("FAST {} SLOW {} | 곡 {:.2} s | 화면 {fps:.1} fps (최대 간격 {max_gap_ms:.1} ms)", s.fast, s.slow, song_s.max(0.0)),
            format!(
                "오디오 {}: D {:.1} ms · 건너뜀 {} · 보이스 {} · 늦은 키음 {} | 입력 손실 {}",
                a.info.path.name(),
                a.monitor.d_frames.load(Ordering::Relaxed) as f64 * 1e3 / rate as f64,
                a.monitor.skips.load(Ordering::Relaxed),
                a.monitor.voices.load(Ordering::Relaxed),
                a.monitor.late_player.load(Ordering::Relaxed),
                input.lost.load(Ordering::Relaxed)
            ),
            format!("오토 {} (F5) | 그래프 G | 고장 {fault_ms} ms: F1 오디오 F2 화면 F3 판정 F4 입력 | Esc", if auto_on.load(Ordering::Relaxed) { "켜짐" } else { "꺼짐" }),
        ];
        for (i, s) in lines.iter().enumerate() {
            list.text(16.0, 10.0 + i as f32 * 20.0, 14.0, hud, s.as_str());
        }
        let st_now = mon.state.load(Ordering::Relaxed);
        let st_col = [Rgba(0.4, 0.9, 0.5, 1.0), Rgba(1.0, 0.8, 0.3, 1.0), Rgba(1.0, 0.4, 0.4, 1.0)][st_now as usize];
        list.text(16.0, 120.0, 18.0, st_col, STATE_NAMES[st_now as usize]);
        graph.draw(&mut list, now, 840.0, 200.0, 420.0, 400.0);
        rend.draw(&list)?;
        let ft = rend.present(vsync);
        envlog.sample(qpc::now(), song_s, hwnd, ft.occluded, input.gap_win_us.swap(0, Ordering::Relaxed), &mut env_gap)?;
        if ft.sync_qpc > 0 {
            if first_sync.is_none() {
                first_sync = Some((ft.sync_qpc, ft.sync_refresh));
            }
            if let Some((q0, r0)) = first_sync {
                if ft.sync_refresh >= r0 + 60 {
                    let p = (ft.sync_qpc - q0) / (ft.sync_refresh - r0) as i64;
                    if p > qf / 1000 && p < qf / 20 {
                        period_q = p;
                    }
                }
            }
            last_sync_q = ft.sync_qpc;
        }
        let _ = fr_tx.push(FrameLog { t: ft, song_ns: song.unwrap_or(i64::MIN), q_display, notes_drawn: drawn, injected_ms: injected });
        if stop_at > 0.0 && song_s > stop_at {
            quit = true;
        }
        if a.done.load(Ordering::Acquire) && (flag(args, "--quit-at-end") || song_s > chart.chart.end as f64 / 1e9 + 3.0) {
            quit = true;
        }
    }
    flog.flush()?;
    graph.flush()?;
    envlog.flush()?;
    a.stop.store(true, Ordering::Release);
    stop.store(true, Ordering::Release);
    let st = a.join.join().map_err(|_| "오디오 스레드 패닉")??;
    let judge_lost = judge_join.join().map_err(|_| "판정 스레드 패닉")?;
    let _ = monitor_join.join();
    let _ = bot_join.join();
    let input_lost = input.lost.load(Ordering::Relaxed);
    input.stop();
    log_stop.store(true, Ordering::Release);
    logger.join().map_err(|_| "기록 스레드 패닉")??;
    cap_stop.store(true, Ordering::Release);
    let capm = match cap {
        Some(h) => Some(h.join().map_err(|_| "녹음 스레드 패닉")??),
        None => None,
    };
    let view = judge.view.lock().map(|v| v.clone()).unwrap_or_else(|_| judging::new_view(&chart, &rules));
    let s = &view.score;
    let snap = a.clock.load();
    let hex: String = chart.chart.chart_hash.iter().map(|b| format!("{b:02x}")).collect();
    let meta = format!(
        "{{\n \"track\": \"{}\", \"chart\": \"{}\", \"chart_hash\": \"{hex}\", \"title\": \"{}\", \"variant_choice\": {:?}, \"lanes\": {}, \"load_ms\": {load_ms:.1},\n \"device\": \"{}\", \"rate\": {rate}, \"buffer_frames\": {}, \"period_frames\": {}, \"path\": \"{}\", \"origin\": {origin}, \"d_frames\": {}, \"skips\": {}, \"skipped_frames\": {}, \"late_player\": {}, \"master_gain\": {gain}, \"render_peak\": {peak},\n \"display_offset_ns\": {display_offset_ns}, \"offset_a_ns\": {out_offset_ns}, \"rules_hash\": \"{rhash}\", \"volumes\": [{}, {}, {}], \"d_margin_ms\": {}, \"calib\": \"{}\", \"seed\": {seed_set}, \"empty_sound\": \"{empty_sound}\", \"scroll_base\": \"{scroll_base}\", \"base_bpm\": {base_bpm}, \"visible_ms\": {}, \"vsync\": {vsync}, \"bot\": {use_bot}, \"bot_direct\": {direct}, \"bot_skipped_not_front\": {}, \"resync_void\": {}, \"gauge\": \"{:?}\", \"windows_ns\": [{}, {}, {}, {}], \"scratch_windows_ns\": [{}, {}, {}, {}],\n \"written\": {}, \"blocks\": {}, \"overflows\": {}, \"log_lost\": {}, \"mix_lost\": {}, \"judge_log_lost\": {judge_lost}, \"input_lost\": {input_lost},\n \"score\": {{\"ex\": {}, \"pgreat\": {}, \"great\": {}, \"good\": {}, \"bad\": {}, \"poor\": {}, \"empty_poor\": {}, \"mines\": {}, \"max_combo\": {}, \"fast\": {}, \"slow\": {}, \"gauge\": {}, \"cleared\": {}}},\n \"clock\": {{\"valid\": {}, \"epoch\": {}, \"t0\": {}, \"f0\": {}, \"a\": {}, \"b\": {}}}, \"qpc_freq\": {qf},\n \"capture\": {}\n}}\n",
        if calib.is_some() { "calib".to_string() } else { path.replace('\\', "/") },
        chart.chart.chart_id,
        loaded.title.replace('"', "'"),
        chart.chart.variant_choice,
        chart.chart.lanes.len(),
        a.info.device,
        a.info.buffer_frames,
        a.info.period_frames,
        a.info.path.key(),
        a.monitor.d_frames.load(Ordering::Relaxed),
        a.monitor.skips.load(Ordering::Relaxed),
        a.monitor.skipped_frames.load(Ordering::Relaxed),
        a.monitor.late_player.load(Ordering::Relaxed),
        g.volume_master,
        g.volume_bgm,
        g.volume_keys,
        dev.d_margin_ms,
        match calib {
            Some(true) => "sound",
            Some(false) => "screen",
            None => "",
        },
        visible_s * 1e3,
        bot::SKIPPED_NOT_FRONT.load(Ordering::Relaxed),
        resync_void,
        rules.gauge_kind,
        rules.key.late[0],
        rules.key.late[1],
        rules.key.late[2],
        rules.key.late[3],
        rules.scratch.late[0],
        rules.scratch.late[1],
        rules.scratch.late[2],
        rules.scratch.late[3],
        st.written,
        st.blocks,
        st.overflows,
        st.log_lost,
        st.mix_lost,
        s.ex(),
        s.pgreat,
        s.great,
        s.good,
        s.bad,
        s.poor,
        s.empty_poor,
        s.mines,
        s.max_combo,
        s.fast,
        s.slow,
        s.gauge.value,
        s.gauge.cleared(),
        snap.valid,
        snap.epoch,
        snap.t0,
        snap.f0,
        snap.a,
        snap.b,
        capm.map(|m| format!("{{\"name\": \"{}\", \"rate\": {}, \"frames\": {}, \"silent_packets\": {}}}", m.name, m.rate, m.frames, m.silent_packets)).unwrap_or_else(|| "null".into())
    );
    fs::write(dir.join("meta.json"), meta)?;
    let summary = format!(
        "  EX {} | PG {} GR {} GD {} BD {} PR {} 빈PR {} | 최대 콤보 {} | 게이지 {}% {}",
        s.ex(),
        s.pgreat,
        s.great,
        s.good,
        s.bad,
        s.poor,
        s.empty_poor,
        s.max_combo,
        s.gauge.percent(),
        if s.gauge.cleared() { "클리어" } else { "실패" }
    );
    println!("  {summary} | 규칙 {}", &rhash[..8]);
    if calib.is_some() {
        let o = calib_ui::result(&dir, calib == Some(true), &format!("{}:{render_key}", apath.key()), display_offset_ns as f64 / 1e6)?;
        let line = calib_ui::describe(&o);
        println!("  {line}");
        return Ok(RunResult { summary: line, calib: Some(o) });
    }
    let hexs: String = chart.chart.chart_hash[..4].iter().map(|b| format!("{b:02x}")).collect();
    Ok(RunResult { summary: format!("{} [{}] {hexs}: {summary} | 규칙 {}", loaded.title, chart.chart.chart_id, &rhash[..8]), calib: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keys_split_left_right_and_queue() {
        // 창 프로시저를 직접 불러 확인: 왼쪽 Shift(스캔 0x2A)·오른쪽 Shift(0x36)·오른쪽 Ctrl(확장)·왼쪽 Alt(WM_SYSKEYDOWN)·A
        let msgs = [
            (WM_KEYDOWN, 0x10usize, 0x2Aisize << 16),
            (WM_KEYDOWN, 0x10, 0x36 << 16),
            (WM_KEYDOWN, 0x11, (0x1D << 16) | (1 << 24)),
            (WM_SYSKEYDOWN, 0x12, 0x38 << 16),
            (WM_KEYDOWN, 0x41, 0x1E << 16),
        ];
        for (m, wp, lp) in msgs {
            unsafe { wndproc(HWND::default(), m, WPARAM(wp), LPARAM(lp)) };
        }
        let got: Vec<u32> = (0..6).map(|_| take_hotkey()).collect();
        assert_eq!(got, vec![0xA0, 0xA1, 0xA3, 0xA4, 0x41, 0]); // 한 프레임에 다섯 개가 와도 순서대로 다 꺼낸다
    }
}
