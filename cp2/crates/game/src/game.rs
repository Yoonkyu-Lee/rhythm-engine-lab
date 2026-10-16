//! `cp2game run`: 건반 하나짜리 게임. 창·오디오·입력·판정·봇·녹음·기록을 엮는다.
//!
//!   cp2game run <곡.rtc> [--audio shared|exclusive|asio] [--asio-buffer 64] [--excl-period-ms 5] [--d-ms (자동)]
//!                [--profile ours|iidx|iidx-fine|djmax|bms] [--bot] [--bot-offset-ms 0] [--bot-jitter-ms 0] [--bot-adapt-ms 0]
//!                [--faults audio@30:150,render@45:300,judge@60:300,input@75:300] [--fault-ms 300]
//!                [--display-offset-ms 0] [--no-vsync] [--no-capture] [--tag run] [--out runs] [--quit-at-end] [--stop-at 곡초]
//!                [--late-drop-ms 100] [--resync-judge void|count] [--iidx-ref bgm|device] [--input-mmcss]
//!
//! 시계: 기록 시계 C = 장치 시계 + D (CP1-4). 오디오 정지는 콘텐츠를 건너뛰어 시간축을 잇는다 (우리 방식).
//! 스레드: 화면(이 스레드) · 오디오 · 입력 · 판정 · 봇 · 녹음 · 기록. 실시간 스레드끼리는 SPSC 링과 원자 변수만 쓴다.
//! 고장 주입: 곡 시각 예약(--faults) 또는 단축키 F1 오디오 · F2 화면 · F3 판정 · F4 입력 (길이는 + / − 로 두 배·절반, 50 ms ~ 4초).
//! 사람이 칠 때 키: Space, J, F (봇은 F24). F5: 오토(봇) 켜기·끄기 (--bot이면 켜진 채로 시작). G: 실시간 싱크 그래프 숨기기·보이기.
//! 출력: runs/<tag>-<시각>/ meta.json, audio_blocks.csv, mix_events.csv, judgments.csv, bot.csv, frames.csv, faults.csv, capture_*.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcore::judge::Windows;
use rcore::mixer::{LatePolicy, MixEvent};
use rcore::ring::{ring, Consumer};
use rcore::runtime::NS_PER_S;
use rcore::{rtc, runtime};
use synchronizer::input::KeyEvent;
use synchronizer::qpc;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY,
    WM_KEYDOWN, WM_QUIT, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};

use crate::audio::{self, AudioCfg};
use crate::bot::{self, BotCfg, BotLog, BOT_KEY};
use crate::envlog::EnvLog;
use crate::graph::{LiveGraph, Mark};
use crate::judging::{self, JudgeCfg, JudgeLog};
use crate::monitor::{self, Heartbeats, LagEvent, MonitorShared, LAG_NAMES, STATE_NAMES};
use crate::profiles::{FrameLogic, Profile};
use crate::render::{D3D11Renderer, DrawList, FrameTiming, Renderer, Rgba};
use crate::{arg, capture, flag};

const W: u32 = 1280;
const H: u32 = 720;
const LANE_X: f32 = 540.0;
const LANE_W: f32 = 200.0;
const JUDGE_Y: f32 = 600.0;
/// 노트가 보이는 시간 (CP2 고정, 기준 BPM 1배속에서 판정선까지 내려오는 시간).
const VISIBLE_S: f64 = 0.8;

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

/// 화면 스레드의 프레임 기록.
#[derive(Clone, Copy, Debug, Default)]
struct FrameLog {
    t: FrameTiming,
    /// 화면에 쓴 청취 곡 시각 (ns)과 그 시각을 계산할 때 쓴 표시 예측 QPC.
    song_ns: i64,
    q_display: i64,
    notes_drawn: u16,
    injected_ms: u16,
}

static mut HOTKEY: u32 = 0;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            HOTKEY = wp.0 as u32;
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("cp2_game");
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
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("CP2 시험장"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rc.right - rc.left,
            rc.bottom - rc.top,
            None,
            None,
            hinst,
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
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
            writeln!(fj, "kind,note,grade,err_ns,t_ns,q_in,q_proc,vkey,key_frame,state,q_recv,q_floor")?;
            let mut fl = BufWriter::new(File::create(dir.join("lag_events.csv"))?);
            writeln!(fl, "kind,name,q_start,q_detect,q_end,magnitude_us")?;
            let mut fo = BufWriter::new(File::create(dir.join("bot.csv"))?);
            writeln!(fo, "note,down,target_ns,q_target,q_sent")?;
            let mut ff = BufWriter::new(File::create(dir.join("frames.csv"))?);
            writeln!(ff, "q_wait_start,q_wait_end,q_present_start,q_present_end,present_count,present_refresh,sync_refresh,sync_qpc,song_ns,q_display,notes_drawn,injected_ms")?;
            loop {
                let last = stop.load(Ordering::Acquire);
                while let Some(b) = l.blocks.pop() {
                    writeln!(fb, "{},{},{},{},{},{},{},{},{},{},{}", b.qpc_wake, b.tl, b.offset, b.padding, b.frames, b.dev_frame, b.dev_qpc100, b.need, b.fill_ns, b.voices, b.injected_ms)?;
                }
                while let Some(e) = l.mix.pop() {
                    match e {
                        MixEvent::Start { sound, due, start, block, late, player, gain, t } => writeln!(fm, "start,{sound},{due},{start},{block},{late},{},{gain},{}", player as u8, if t == rcore::mixer::NO_T { String::new() } else { t.to_string() })?,
                        MixEvent::Dropped { sound, due, block, player } => writeln!(fm, "dropped,{sound},{due},,{block},,{},,", player as u8)?,
                        MixEvent::Overflow { sound, due, player } => writeln!(fm, "overflow,{sound},{due},,,,{},,", player as u8)?,
                        MixEvent::Cut { sound, at } => writeln!(fm, "cut,{sound},{at},,,,,,")?,
                    }
                }
                while let Some(j) = l.judge.pop() {
                    writeln!(fj, "{},{},{},{},{},{},{},{},{},{},{},{}", j.kind, j.note, j.grade, j.err_ns, j.t_ns, j.q_in, j.q_proc, j.vkey, j.key_frame, j.state, j.q_recv, j.q_floor)?;
                }
                while let Some(e) = l.lags.pop() {
                    writeln!(fl, "{},{},{},{},{},{}", e.kind, LAG_NAMES[e.kind as usize], e.q_start, e.q_detect, e.q_end, e.magnitude_us)?;
                }
                while let Some(o) = l.bot.pop() {
                    writeln!(fo, "{},{},{},{},{}", o.note, o.down as u8, o.target_ns, o.q_target, o.q_sent)?;
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

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    unsafe { timeBeginPeriod(1) };
    let r = run_inner(args);
    unsafe { timeEndPeriod(1) };
    r
}

fn run_inner(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.get(2).ok_or("곡 파일이 필요함")?;
    let render_name = arg(args, "--render", "Playback 1/2");
    let apath = audio::AudioPath::parse(&arg(args, "--audio", "shared"))?;
    let src = rtc::decode(&fs::read(path)?)?;
    let src_rate = src.bank.rate;
    let dev_rate = audio::device_rate(apath, &render_name)?;
    let ready = if dev_rate != src_rate { rcore::resample::resample_chart(&src, dev_rate)? } else { src };
    let chart = Arc::new(runtime::compile(ready)?);
    let (gain, peak) = crate::master_gain(&chart);
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    // 늦은 키음: 이 ms보다 늦으면 버린다 (0이면 끝없이 민다). CP2 2-5 결정: 100 ms (평소 34~44 ms, 그보다 크게 늦은 키음은 틀린 소리로 들린다)
    let late_drop_ms: f64 = arg(args, "--late-drop-ms", "100").parse().map_err(|_| "--late-drop-ms ms")?;
    // 기록 시계 D: 우리 방식은 기본으로 리드인 동안 잰다 (--d-ms로 고정 가능). 프레임 구동 프로필은 0 ("지금 울려"가 같은 지연을 갖는다)
    let profile0 = Profile::parse(&arg(args, "--profile", "ours"))?;
    let d_ms: Option<f64> = if profile0 != Profile::Ours {
        Some(0.0)
    } else {
        match arg(args, "--d-ms", "").as_str() {
            "" => None,
            v => Some(v.parse().map_err(|_| "--d-ms ms")?),
        }
    };
    let mut fault_ms: u32 = arg(args, "--fault-ms", "300").parse().map_err(|_| "--fault-ms ms")?;
    let late_drop_frames = if late_drop_ms > 0.0 { (late_drop_ms * dev_rate as f64 / 1e3) as i64 } else { i64::MAX };
    let stop_at: f64 = arg(args, "--stop-at", "0").parse().map_err(|_| "--stop-at 곡초")?;
    // 화면 오프셋 (CP2 2-6 결정, 2026-10-16): 노트를 그리는 위치만 옮긴다 (+면 노트를 그만큼 미리 그림 = 화면 지연 보정).
    // 판정·키음·봇은 건드리지 않는다 → "PERFECT면 키음이 반주 위"와 누름 → 키음 지연(D)이 그대로다. --out-offset-ms는 옛 이름.
    let display_offset_ns = (arg(args, "--display-offset-ms", &arg(args, "--out-offset-ms", "0")).parse::<f64>()? * 1e6) as i64;
    let out_offset_ns = 0i64;
    let vsync = !flag(args, "--no-vsync");
    let use_bot = flag(args, "--bot");
    let profile = Profile::parse(&arg(args, "--profile", "ours"))?;
    let keys = vec![BOT_KEY, 0x20, b'J' as u16, b'F' as u16];
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = arg(args, "--tag", "run");
    let dir = PathBuf::from(arg(args, "--out", "runs")).join(format!("{tag}-{unix}"));
    fs::create_dir_all(&dir)?;
    println!("{} → {}", chart.chart.title, dir.display());

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

    // 링
    let (key_tx, key_rx) = ring::<audio::KeyReq>(256);
    let (blk_tx, blk_rx) = ring::<audio::BlockLog>(1 << 14);
    let (mix_tx, mix_rx) = ring::<MixEvent>(1 << 15);
    let (kev_tx, kev_rx) = ring::<KeyEvent>(1024);
    let (jl_tx, jl_rx) = ring::<JudgeLog>(1 << 13);
    let (bot_tx, bot_rx) = ring::<BotLog>(1 << 13);
    let (fr_tx, fr_rx) = ring::<FrameLog>(1 << 14);
    let (lag_tx, lag_rx) = ring::<LagEvent>(4096);
    let (live_tx, mut live_rx) = ring::<audio::SyncSample>(1 << 13);
    let mut fr_tx = fr_tx;
    // 프레임 구동 프로필이 프레임마다 공개하는 자기 시계 (봇과 실시간 싱크 그래프가 읽는다)
    let pclock = Arc::new(bot::ProfileClock::default());

    let a = audio::spawn(
        chart.clone(),
        AudioCfg {
            path: apath,
            render: render_name,
            asio_buffer: arg(args, "--asio-buffer", "64").parse().map_err(|_| "--asio-buffer 샘플")?,
            excl_period_ms: arg(args, "--excl-period-ms", "5").parse().map_err(|_| "--excl-period-ms ms")?,
            lead_in_s: 2.0,
            tail_s: 1.0,
            master_gain: gain,
            late: LatePolicy { drop_after: late_drop_frames, ..LatePolicy::DEFAULT },
            autoplay_notes: false,
            auto_from_chart: profile == Profile::Ours,
            cut_same: profile == Profile::Bms,
            d_ms,
            skip_on_stall: profile == Profile::Ours,
            asio_capture: (apath == audio::AudioPath::Asio && !flag(args, "--no-capture")).then(|| dir.clone()),
            faults: vec![],
            live: Some(audio::LiveSync { tx: live_tx, pclock: (profile != Profile::Ours).then(|| pclock.clone()) }),
        },
        key_rx,
        blk_tx,
        mix_tx,
    )?;
    let (origin, rate) = (a.info.origin, a.info.rate);
    // 입력: BMS식은 1 kHz 폴링, 나머지는 Raw Input
    let input = if profile == Profile::Bms { crate::input::Input::spawn_poll(kev_tx, keys.clone())? } else { crate::input::Input::spawn(kev_tx, flag(args, "--input-mmcss"))? };
    let stop = Arc::new(AtomicBool::new(false));
    let beats = Arc::new(Heartbeats::default());
    let mon = Arc::new(MonitorShared::default());
    let monitor_join = monitor::spawn(mon.clone(), a.monitor.clone(), a.clock.clone(), input.watermark.clone(), beats.clone(), lag_tx, stop.clone());
    let windows_cfg = Windows::CP2;
    // 우리 방식: 판정 스레드. 나머지: 화면 프레임에서 도는 로직 (판정 스레드 없음, 화면용 공유 상태만 둔다)
    let (judge, judge_join, mut logic) = if profile == Profile::Ours {
        let (j, h) = judging::spawn(
            chart.clone(),
            JudgeCfg { origin, rate, out_offset_ns, windows: windows_cfg, keys: keys.clone(), faults: vec![], resync_void: arg(args, "--resync-judge", "void") != "count", audio: a.monitor.clone() },
            a.clock.clone(),
            input.watermark.clone(),
            kev_rx,
            key_tx,
            jl_tx,
            stop.clone(),
            beats.clone(),
            mon.clone(),
        );
        (j, Some(h), None)
    } else {
        // IIDX식 따라잡기 기준: 반주 트랙 위치(실제 IIDX, 기본) 또는 장치 시계(--iidx-ref device, CP2 2-4·2-5 측정 때)
        let bgm_ref = (arg(args, "--iidx-ref", "bgm") != "device").then(|| a.monitor.clone());
        let l = FrameLogic::new(profile, chart.clone(), windows_cfg, keys.clone(), origin, rate, out_offset_ns, kev_rx, key_tx, jl_tx, bgm_ref);
        (judging::JudgeShared::new(1), None, Some(l))
    };
    // 봇은 늘 띄워 두고 F5로 켜고 끈다 (--bot = 켜진 채로 시작)
    let auto_on = Arc::new(AtomicBool::new(use_bot));
    let bot_join = Some({
        let cfg = BotCfg {
            origin,
            rate,
            out_offset_ns,
            offset_ns: (arg(args, "--bot-offset-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            jitter_ns: (arg(args, "--bot-jitter-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            seed: 12345,
            adapt_ns: (arg(args, "--bot-adapt-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            follow: (profile != Profile::Ours).then(|| pclock.clone()),
            enabled: auto_on.clone(),
        };
        bot::spawn(chart.clone(), cfg, a.clock.clone(), bot_tx, stop.clone())
    });
    let log_stop = Arc::new(AtomicBool::new(false));
    let logger = spawn_logger(dir.clone(), Logs { blocks: blk_rx, mix: mix_rx, judge: jl_rx, bot: bot_rx, frames: fr_rx, lags: lag_rx }, log_stop.clone());
    let mut flog = BufWriter::new(File::create(dir.join("faults.csv"))?);
    writeln!(flog, "kind,at_song_s,ms,q,source")?;
    let mut alog = BufWriter::new(File::create(dir.join("auto.csv"))?);
    writeln!(alog, "on,at_song_s,q")?;
    writeln!(alog, "{},start,0", use_bot as u8)?;

    let hwnd = create_window()?;
    let mut rend = D3D11Renderer::new(hwnd, W, H)?;
    let notes = &chart.chart.lanes[0].notes;
    let qf = qpc::frequency();
    let px_per_beat = (JUDGE_Y as f64) / (VISIBLE_S * chart.chart.reference_bpm / 60.0);
    let mut fired = vec![false; faults.len()];
    let mut period_q = qf / 60; // 표시 주기 추정 (DXGI 통계로 갱신)
    let (mut last_sync_q, mut _last_sync_r) = (0i64, 0u32);
    let mut first_sync: Option<(i64, u32)> = None;
    let mut fps_frames = 0u32;
    let mut fps_t0 = qpc::now();
    let (mut fps, mut max_gap_ms, mut cur_max_gap) = (0.0, 0.0, 0.0f64);
    let mut prev_wait_end = qpc::now();
    let mut quit = false;
    let mut msg = MSG::default();
    let mut list = DrawList::default();
    let mut audio_gap_ms = 0.0;
    let mut graph = LiveGraph::new(&dir, qf)?;
    let mut envlog = EnvLog::new(&dir, qf)?;
    let mut env_gap = 0u64;
    let mut prev_skipped = 0i64;
    let mut prev_lags = [0u32; 9];
    let (mut prev_extra, mut prev_hold) = (0u64, 0u64);
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
            audio_gap_ms = a.monitor.max_wake_gap_us.swap(0, Ordering::Relaxed) as f64 / 1e3;
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
        // 지금 곡 시각 (표시 예측: 다음 vblank)
        let now = qpc::now();
        let q_display = if last_sync_q > 0 {
            let k = ((now - last_sync_q) / period_q) + 1;
            last_sync_q + k * period_q
        } else {
            now + period_q
        };
        // 우리 방식: 장치 시계로 본 다음 vblank의 청취 곡 시각. 프레임 구동 방식: 그 프로필의 시계 (표시 예측 없음, 프레임 시작 시각)
        let state_now = mon.state.load(Ordering::Relaxed);
        let song = match logic.as_mut() {
            None => judging::song_ns(&a.clock, q_display, origin, rate, 0).map(|x| x.0),
            Some(l) => {
                beats.judge.store(qpc::now(), Ordering::Release);
                let t = l.frame(we, period_q, &a.clock, state_now);
                if let Some(t) = t {
                    pclock.publish(we, t);
                }
                t
            }
        };
        let song_s = song.unwrap_or(i64::MIN) as f64 / NS_PER_S as f64;

        // 고장 주입: 예약 + 단축키
        let mut injected = 0u16;
        let mut inject = |k: FaultKind, ms: u32, src: &str, flog: &mut BufWriter<File>| -> std::io::Result<()> {
            writeln!(flog, "{k:?},{song_s:.3},{ms},{},{src}", qpc::now())?;
            let (c, name) = match k {
                FaultKind::Audio => (Rgba(1.0, 0.35, 0.35, 0.18), "오디오"),
                FaultKind::Render => (Rgba(0.35, 0.65, 1.0, 0.18), "화면"),
                FaultKind::Judge => (Rgba(1.0, 0.8, 0.3, 0.18), "판정"),
                FaultKind::Input => (Rgba(0.6, 1.0, 0.5, 0.18), "입력"),
            };
            graph.mark(Mark { q: qpc::now(), len_q: ms as i64 * qf / 1000, color: c, label: format!("{name} {ms}") });
            match k {
                FaultKind::Audio => a.monitor.inject_stall_ms.store(ms, Ordering::Release),
                FaultKind::Judge if logic.is_none() => judge.inject_stall_ms.store(ms, Ordering::Release),
                FaultKind::Judge => {
                    // 프레임 구동 방식은 판정이 화면 스레드에 있다: 화면 스레드를 재운다
                    thread::sleep(Duration::from_millis(ms as u64));
                    injected += ms as u16;
                }
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
        let hk = unsafe { std::mem::replace(&mut *std::ptr::addr_of_mut!(HOTKEY), 0) };
        match hk {
            0x70 => inject(FaultKind::Audio, fault_ms, "key", &mut flog)?,
            0x71 => inject(FaultKind::Render, fault_ms, "key", &mut flog)?,
            0x72 => inject(FaultKind::Judge, fault_ms, "key", &mut flog)?,
            0x73 => inject(FaultKind::Input, fault_ms, "key", &mut flog)?,
            // + / − (본 자판·숫자 자판): 고장 길이 두 배 · 절반 (50 ms ~ 4초. 화면이 5초 넘게 멈추면 Windows가 "응답 없음"으로 표시한다)
            0xBB | 0x6B => fault_ms = (fault_ms * 2).min(4000),
            0xBD | 0x6D => fault_ms = (fault_ms / 2).max(50),
            0x74 => {
                let on = !auto_on.load(Ordering::Relaxed);
                auto_on.store(on, Ordering::Relaxed);
                writeln!(alog, "{},{song_s:.3},{}", on as u8, qpc::now())?;
            }
            0x47 => graph.visible = !graph.visible,
            0x1B => quit = true,
            _ => {}
        }
        // 실시간 싱크 그래프: 표본 받기 + 건너뜀·렉 감지·IIDX식 slew 표시
        graph.ingest(&mut live_rx, now)?;
        let sk = a.monitor.skipped_frames.load(Ordering::Relaxed);
        if sk > prev_skipped {
            let ms = (sk - prev_skipped) as f64 * 1e3 / rate as f64;
            graph.skip(now, ms);
            prev_skipped = sk;
        }
        for k in [0usize, 2, 3, 4, 5] {
            let c = mon.counts[k].load(Ordering::Relaxed);
            if c > prev_lags[k] {
                graph.mark(Mark { q: now, len_q: 0, color: Rgba(1.0, 0.6, 0.4, 0.6), label: String::new() });
                prev_lags[k] = c;
            }
        }
        if let Some(l) = logic.as_ref() {
            if l.slew_extra > prev_extra {
                graph.slew(we, 1);
            }
            if l.slew_hold > prev_hold {
                graph.slew(we, -1);
            }
            (prev_extra, prev_hold) = (l.slew_extra, l.slew_hold);
        }

        // 그리기
        list.cmds.clear();
        list.clear = Some(Rgba(0.04, 0.04, 0.06, 1.0));
        list.rect(LANE_X, 0.0, LANE_W, H as f32, Rgba(0.10, 0.10, 0.14, 1.0));
        let view = match logic.as_ref() {
            Some(l) => l.view(),
            None => judge.view.lock().map(|v| v.clone()).unwrap_or_default(),
        };
        if view.pressed {
            list.rect(LANE_X, JUDGE_Y - 240.0, LANE_W, 240.0, Rgba(0.25, 0.35, 0.6, 0.35));
        }
        list.rect(LANE_X - 10.0, JUDGE_Y, LANE_W + 20.0, 4.0, Rgba(0.9, 0.3, 0.3, 1.0));
        let mut drawn = 0u16;
        if let Some(song) = song.map(|s| s + display_offset_ns) {
            let pos_now = chart.chart.scroll_pos(song);
            for n in &notes[view.next_note.first().copied().unwrap_or(0)..] {
                let y_head = JUDGE_Y as f64 - (chart.chart.scroll_pos(n.t) - pos_now) * px_per_beat;
                if y_head < -40.0 {
                    break;
                }
                let y_tail = JUDGE_Y as f64 - (chart.chart.scroll_pos(n.end) - pos_now) * px_per_beat;
                if n.end > n.t {
                    list.rect(LANE_X + 30.0, y_tail as f32, LANE_W - 60.0, (y_head - y_tail) as f32, Rgba(0.4, 0.8, 0.5, 0.6));
                }
                list.rect(LANE_X + 4.0, y_head as f32 - 14.0, LANE_W - 8.0, 14.0, Rgba(0.95, 0.95, 0.95, 1.0));
                drawn += 1;
            }
        }
        if let Some((g, err)) = view.last {
            let name = ["PERFECT", "GREAT", "GOOD", "BAD", "MISS"][g as usize];
            list.text(LANE_X + 20.0, JUDGE_Y - 160.0, 28.0, Rgba(1.0, 0.9, 0.4, 1.0), format!("{name} {:+.1} ms", err as f64 / 1e6));
        }
        let t = &view.tally;
        let hud = Rgba(0.8, 0.85, 0.9, 1.0);
        let lines = [
            match logic.as_ref() {
                Some(l) if matches!(profile, Profile::Iidx | Profile::IidxFine) => format!("프로필: {} | slew 오차 {:+.1} ms, 틱 추가 {} · 쉼 {}", profile.title(), l.slew_err_ns as f64 / 1e6, l.slew_extra, l.slew_hold),
                _ => format!("프로필: {}", profile.title()),
            },
            format!("곡 {:.2} s | 화면 {fps:.1} fps, 최근 1초 최대 프레임 간격 {max_gap_ms:.1} ms | vsync {}", song_s.max(0.0), vsync),
            format!(
                "오디오 {}: D {:.1} ms (필요 최대 {:.1}) · 건너뜀 {}번 {:.0} ms · 최대 깨어남 간격 {audio_gap_ms:.1} ms · 보이스 {} · 늦은 키음 {}",
                a.info.path.name(),
                a.monitor.d_frames.load(Ordering::Relaxed) as f64 * 1e3 / rate as f64,
                a.monitor.need_max.load(Ordering::Relaxed) as f64 * 1e3 / rate as f64,
                a.monitor.skips.load(Ordering::Relaxed),
                a.monitor.skipped_frames.load(Ordering::Relaxed) as f64 * 1e3 / rate as f64,
                a.monitor.voices.load(Ordering::Relaxed),
                a.monitor.late_player.load(Ordering::Relaxed)
            ),
            format!(
                "판정: P {} G {} Good {} Bad {} Miss {} | 콤보 {} (최대 {}) | 빈 누름 {} | LN {}/{}",
                t.perfect, t.great, t.good, t.bad, t.miss, t.combo, t.max_combo, t.empty, t.ln_ok, t.ln_break
            ),
            format!(
                "watermark 지연 {:.1} ms · 계약 위반 {} · 입력 손실 {}",
                if logic.is_none() { song.map(|s| (s - judge.watermark_song_ns.load(Ordering::Relaxed)) as f64 / 1e6).unwrap_or(0.0) } else { f64::NAN },
                judge.violations.load(Ordering::Relaxed),
                input.lost.load(Ordering::Relaxed)
            ),
            format!(
                "감지: 오디오 정지 {} · 콘텐츠 건너뜀 {} · 시계 점프 {} · 화면 정지 {} · 판정 정지 {} · 입력 정지 {}",
                mon.counts[0].load(Ordering::Relaxed),
                mon.counts[8].load(Ordering::Relaxed),
                mon.counts[2].load(Ordering::Relaxed),
                mon.counts[3].load(Ordering::Relaxed),
                mon.counts[4].load(Ordering::Relaxed),
                mon.counts[5].load(Ordering::Relaxed)
            ),
            format!(
                "오토 {} (F5) | 그래프 G | 고장 {fault_ms} ms (+ / − 로 바꿈): F1 오디오 · F2 화면 · F3 판정 · F4 입력 · Esc 끝",
                if auto_on.load(Ordering::Relaxed) { "켜짐" } else { "꺼짐" }
            ),
        ];
        for (i, s) in lines.iter().enumerate() {
            list.text(16.0, 12.0 + i as f32 * 22.0, 15.0, hud, s.as_str());
        }
        // 상태와 최근 렉 사건 (감지까지 걸린 시간, 길이)
        let st_now = mon.state.load(Ordering::Relaxed);
        let st_col = [Rgba(0.4, 0.9, 0.5, 1.0), Rgba(1.0, 0.8, 0.3, 1.0), Rgba(1.0, 0.4, 0.4, 1.0)][st_now as usize];
        let base_y = 12.0 + lines.len() as f32 * 22.0 + 8.0;
        list.text(16.0, base_y, 22.0, st_col, STATE_NAMES[st_now as usize]);
        if let Ok(r) = mon.recent.lock() {
            for (i, e) in r.iter().rev().enumerate() {
                let s = match e.kind {
                    6 => format!("상태 → {}", STATE_NAMES[e.magnitude_us as usize]),
                    2 | 7 | 8 => format!("{} {:+.1} ms", LAG_NAMES[e.kind as usize], e.magnitude_us as f64 / 1e3),
                    1 => LAG_NAMES[1].to_string(),
                    k => format!("{} {:.0} ms (감지 {:.0} ms 뒤)", LAG_NAMES[k as usize], e.magnitude_us as f64 / 1e3, (e.q_detect - e.q_start) as f64 * 1e3 / qf as f64),
                };
                list.text(16.0, base_y + 34.0 + i as f32 * 20.0, 14.0, Rgba(0.9, 0.7, 0.5, 1.0), s);
            }
        }
        graph.draw(&mut list, now, 790.0, 200.0, 430.0, 480.0);
        rend.draw(&list)?;
        let ft = rend.present(vsync);
        envlog.sample(qpc::now(), song_s, hwnd, ft.occluded, input.gap_win_us.swap(0, Ordering::Relaxed), &mut env_gap)?;
        // 화면 주기: 첫 vblank부터의 긴 기준선 (프레임마다 짧게 재면 출렁여 평균이 1% 넘게 길게 치우친다 — IIDX 틱 길이가 틀어짐)
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
        }
        if ft.sync_qpc > 0 {
            last_sync_q = ft.sync_qpc;
            _last_sync_r = ft.sync_refresh;
        }
        let _ = fr_tx.push(FrameLog { t: ft, song_ns: song.unwrap_or(i64::MIN), q_display, notes_drawn: drawn, injected_ms: injected });
        if stop_at > 0.0 && song_s > stop_at {
            quit = true;
        }
        if a.done.load(Ordering::Acquire) && flag(args, "--quit-at-end") {
            quit = true;
        }
        if a.done.load(Ordering::Acquire) && !flag(args, "--quit-at-end") && song_s > chart.chart.end as f64 / 1e9 + 3.0 {
            quit = true;
        }
    }
    flog.flush()?;
    alog.flush()?;
    graph.flush()?;
    envlog.flush()?;
    let es = &envlog.sum;
    let env_json = format!(
        "{{\"not_foreground_s\": {:.2}, \"iconic_s\": {:.2}, \"occluded_s\": {:.2}, \"timer_res_max_us\": {:.1}, \"input_gap_max_us\": {}}}",
        es.not_foreground_s, es.iconic_s, es.occluded_s, es.timer_res_max_us, es.input_gap_max_us
    );
    a.stop.store(true, Ordering::Release);
    stop.store(true, Ordering::Release);
    let st = a.join.join().map_err(|_| "오디오 스레드 패닉")??;
    let judge_lost = match judge_join {
        Some(h) => h.join().map_err(|_| "판정 스레드 패닉")?,
        None => logic.as_ref().map(|l| l.log_lost).unwrap_or(0),
    };
    let _ = monitor_join.join();
    if let Some(b) = bot_join {
        let _ = b.join();
    }
    let input_lost = input.lost.load(Ordering::Relaxed);
    let (input_mmcss, input_gap_max_us, input_gap_over_2ms) = (input.mmcss, input.gap_max_us.load(Ordering::Relaxed), input.gap_over_2ms.load(Ordering::Relaxed));
    input.stop();
    log_stop.store(true, Ordering::Release);
    logger.join().map_err(|_| "기록 스레드 패닉")??;
    cap_stop.store(true, Ordering::Release);
    let capm = match cap {
        Some(h) => Some(h.join().map_err(|_| "녹음 스레드 패닉")??),
        None => None,
    };
    let view = match logic.as_ref() {
        Some(l) => l.view(),
        None => judge.view.lock().map(|v| v.clone()).unwrap_or_default(),
    };
    let t = &view.tally;
    let snap = a.clock.load();
    let meta = format!(
        "{{\n \"chart\": \"{}\", \"chart_hash\": \"{:016x}\", \"title\": \"{}\", \"profile\": \"{}\", \"slew\": [{}, {}],\n \"device\": \"{}\", \"rate\": {}, \"buffer_frames\": {}, \"period_frames\": {}, \"path\": \"{}\", \"origin\": {origin}, \"d_frames\": {}, \"need_max_frames\": {}, \"skips\": {}, \"skipped_frames\": {}, \"late_player\": {}, \"master_gain\": {gain}, \"render_peak\": {peak},\n \"out_offset_ns\": {out_offset_ns}, \"display_offset_ns\": {display_offset_ns}, \"input_mmcss\": {input_mmcss}, \"env\": {env_json}, \"input_gap_max_us\": {input_gap_max_us}, \"input_gap_over_2ms\": {input_gap_over_2ms}, \"vsync\": {vsync}, \"bot\": {use_bot}, \"windows_ns\": [{}, {}, {}, {}], \"render_timing_grade\": \"{}\",\n \"written\": {}, \"blocks\": {}, \"overflows\": {}, \"log_lost\": {}, \"mix_lost\": {}, \"judge_log_lost\": {judge_lost}, \"input_lost\": {input_lost},\n \"lag_counts\": [{}],\n \"tally\": {{\"perfect\": {}, \"great\": {}, \"good\": {}, \"bad\": {}, \"miss\": {}, \"empty\": {}, \"ln_ok\": {}, \"ln_break\": {}, \"void\": {}, \"max_combo\": {}}},\n \"clock\": {{\"valid\": {}, \"epoch\": {}, \"t0\": {}, \"f0\": {}, \"a\": {}, \"b\": {}}}, \"qpc_freq\": {qf},\n \"capture\": {}\n}}\n",
        path.replace('\\', "/"),
        chart.hash,
        chart.chart.title,
        profile.name(),
        logic.as_ref().map(|l| l.slew_extra).unwrap_or(0),
        logic.as_ref().map(|l| l.slew_hold).unwrap_or(0),
        a.info.device,
        rate,
        a.info.buffer_frames,
        a.info.period_frames,
        a.info.path.key(),
        a.monitor.d_frames.load(Ordering::Relaxed),
        a.monitor.need_max.load(Ordering::Relaxed),
        a.monitor.skips.load(Ordering::Relaxed),
        a.monitor.skipped_frames.load(Ordering::Relaxed),
        a.monitor.late_player.load(Ordering::Relaxed),
        windows_cfg.perfect,
        windows_cfg.great,
        windows_cfg.good,
        windows_cfg.bad,
        rend.timing_grade(),
        st.written,
        st.blocks,
        st.overflows,
        st.log_lost,
        st.mix_lost,
        mon.counts.iter().map(|c| c.load(Ordering::Relaxed).to_string()).collect::<Vec<_>>().join(", "),
        t.perfect,
        t.great,
        t.good,
        t.bad,
        t.miss,
        t.empty,
        t.ln_ok,
        t.ln_break,
        t.void,
        t.max_combo,
        snap.valid,
        snap.epoch,
        snap.t0,
        snap.f0,
        snap.a,
        snap.b,
        capm.map(|m| format!("{{\"name\": \"{}\", \"rate\": {}, \"frames\": {}, \"silent_packets\": {}}}", m.name, m.rate, m.frames, m.silent_packets)).unwrap_or_else(|| "null".into())
    );
    fs::write(dir.join("meta.json"), meta)?;
    println!("  판정 P {} G {} Good {} Bad {} Miss {} 빈 {} LN {}/{} 무효 {} 최대 콤보 {}", t.perfect, t.great, t.good, t.bad, t.miss, t.empty, t.ln_ok, t.ln_break, t.void, t.max_combo);
    Ok(())
}
