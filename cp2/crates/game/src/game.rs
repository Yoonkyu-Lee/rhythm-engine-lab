//! `cp2game run`: 건반 하나짜리 게임 ("우리 방식" 프로필). 창·오디오·입력·판정·봇·녹음·기록을 엮는다.
//!
//!   cp2game run <곡.rtc> [--bot] [--bot-offset-ms 0] [--bot-jitter-ms 0] [--faults audio@30:150,render@45:300,judge@60:300,input@75:300]
//!                [--out-offset-ms 0] [--no-vsync] [--no-capture] [--tag run] [--out runs] [--quit-at-end]
//!
//! 스레드: 화면(이 스레드) · 오디오 · 입력 · 판정 · 봇 · 녹음 · 기록. 실시간 스레드끼리는 SPSC 링과 원자 변수만 쓴다.
//! 고장 주입: 곡 시각 예약(--faults) 또는 단축키 F1 오디오 150 ms, F2 화면 300 ms, F3 판정 300 ms, F4 입력 300 ms.
//! 사람이 칠 때 키: Space, J, F (봇은 F24).
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
use crate::judging::{self, JudgeCfg, JudgeLog};
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
            w!("CP2 시험장 — 우리 방식"),
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
}

/// 기록 스레드: 링을 비워 CSV로 쓴다. 실시간 스레드는 파일을 만지지 않는다.
fn spawn_logger(dir: PathBuf, mut l: Logs, stop: Arc<AtomicBool>) -> thread::JoinHandle<std::io::Result<()>> {
    thread::Builder::new()
        .name("logger".into())
        .spawn(move || {
            let mut fb = BufWriter::new(File::create(dir.join("audio_blocks.csv"))?);
            writeln!(fb, "qpc_wake,written,padding,frames,dev_frame,dev_qpc100,fill_ns,voices,injected_ms")?;
            let mut fm = BufWriter::new(File::create(dir.join("mix_events.csv"))?);
            writeln!(fm, "kind,sound,due,start,block,late,player,gain")?;
            let mut fj = BufWriter::new(File::create(dir.join("judgments.csv"))?);
            writeln!(fj, "kind,note,grade,err_ns,t_ns,q_in,q_proc,vkey,key_frame")?;
            let mut fo = BufWriter::new(File::create(dir.join("bot.csv"))?);
            writeln!(fo, "note,down,target_ns,q_target,q_sent")?;
            let mut ff = BufWriter::new(File::create(dir.join("frames.csv"))?);
            writeln!(ff, "q_wait_start,q_wait_end,q_present_start,q_present_end,present_count,present_refresh,sync_refresh,sync_qpc,song_ns,q_display,notes_drawn,injected_ms")?;
            loop {
                let last = stop.load(Ordering::Acquire);
                while let Some(b) = l.blocks.pop() {
                    writeln!(fb, "{},{},{},{},{},{},{},{},{}", b.qpc_wake, b.written, b.padding, b.frames, b.dev_frame, b.dev_qpc100, b.fill_ns, b.voices, b.injected_ms)?;
                }
                while let Some(e) = l.mix.pop() {
                    match e {
                        MixEvent::Start { sound, due, start, block, late, player, gain } => writeln!(fm, "start,{sound},{due},{start},{block},{late},{},{gain}", player as u8)?,
                        MixEvent::Dropped { sound, due, block, player } => writeln!(fm, "dropped,{sound},{due},,{block},,{},", player as u8)?,
                        MixEvent::Overflow { sound, due, player } => writeln!(fm, "overflow,{sound},{due},,,,{},", player as u8)?,
                    }
                }
                while let Some(j) = l.judge.pop() {
                    writeln!(fj, "{},{},{},{},{},{},{},{},{}", j.kind, j.note, j.grade, j.err_ns, j.t_ns, j.q_in, j.q_proc, j.vkey, j.key_frame)?;
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
            for f in [&mut fb, &mut fm, &mut fj, &mut fo, &mut ff] {
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
    let src = rtc::decode(&fs::read(path)?)?;
    let src_rate = src.bank.rate;
    let dev_rate = audio::device_rate(&render_name)?;
    let ready = if dev_rate != src_rate { rcore::resample::resample_chart(&src, dev_rate)? } else { src };
    let chart = Arc::new(runtime::compile(ready)?);
    let (gain, peak) = crate::master_gain(&chart);
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    let out_offset_ns = (arg(args, "--out-offset-ms", "0").parse::<f64>()? * 1e6) as i64;
    let vsync = !flag(args, "--no-vsync");
    let use_bot = flag(args, "--bot");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = arg(args, "--tag", "run");
    let dir = PathBuf::from(arg(args, "--out", "runs")).join(format!("{tag}-{unix}"));
    fs::create_dir_all(&dir)?;
    println!("{} → {}", chart.chart.title, dir.display());

    // 녹음
    let cap_stop = Arc::new(AtomicBool::new(false));
    let cap_started = Arc::new(AtomicBool::new(false));
    let cap = if flag(args, "--no-capture") {
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
    let mut fr_tx = fr_tx;

    let a = audio::spawn(
        chart.clone(),
        AudioCfg { render: render_name, lead_in_s: 2.0, tail_s: 1.0, master_gain: gain, late: LatePolicy::DEFAULT, autoplay_notes: false, faults: vec![] },
        key_rx,
        blk_tx,
        mix_tx,
    )?;
    let (origin, rate) = (a.info.origin, a.info.rate);
    let input = crate::input::Input::spawn(kev_tx)?;
    let stop = Arc::new(AtomicBool::new(false));
    let keys = vec![BOT_KEY, 0x20, b'J' as u16, b'F' as u16];
    let windows_cfg = Windows::CP2;
    let (judge, judge_join) = judging::spawn(
        chart.clone(),
        JudgeCfg { origin, rate, out_offset_ns, windows: windows_cfg, keys, faults: vec![] },
        a.clock.clone(),
        input.watermark.clone(),
        kev_rx,
        key_tx,
        jl_tx,
        stop.clone(),
    );
    let bot_join = use_bot.then(|| {
        let cfg = BotCfg {
            origin,
            rate,
            out_offset_ns,
            offset_ns: (arg(args, "--bot-offset-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            jitter_ns: (arg(args, "--bot-jitter-ms", "0").parse::<f64>().unwrap_or(0.0) * 1e6) as i64,
            seed: 12345,
        };
        bot::spawn(chart.clone(), cfg, a.clock.clone(), bot_tx, stop.clone())
    });
    let log_stop = Arc::new(AtomicBool::new(false));
    let logger = spawn_logger(dir.clone(), Logs { blocks: blk_rx, mix: mix_rx, judge: jl_rx, bot: bot_rx, frames: fr_rx }, log_stop.clone());
    let mut flog = BufWriter::new(File::create(dir.join("faults.csv"))?);
    writeln!(flog, "kind,at_song_s,ms,q,source")?;

    let hwnd = create_window()?;
    let mut rend = D3D11Renderer::new(hwnd, W, H)?;
    let notes = &chart.chart.lanes[0].notes;
    let qf = qpc::frequency();
    let px_per_beat = (JUDGE_Y as f64) / (VISIBLE_S * chart.chart.reference_bpm / 60.0);
    let mut fired = vec![false; faults.len()];
    let mut period_q = qf / 60; // 표시 주기 추정 (DXGI 통계로 갱신)
    let (mut last_sync_q, mut last_sync_r) = (0i64, 0u32);
    let mut fps_frames = 0u32;
    let mut fps_t0 = qpc::now();
    let (mut fps, mut max_gap_ms, mut cur_max_gap) = (0.0, 0.0, 0.0f64);
    let mut prev_wait_end = qpc::now();
    let mut quit = false;
    let mut msg = MSG::default();
    let mut list = DrawList::default();
    let mut audio_gap_ms = 0.0;
    while !quit {
        let (_ws, we) = rend.wait();
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
        let song = judging::song_ns(&a.clock, q_display, origin, rate, out_offset_ns).map(|x| x.0);
        let song_s = song.unwrap_or(i64::MIN) as f64 / NS_PER_S as f64;

        // 고장 주입: 예약 + 단축키
        let mut injected = 0u16;
        let mut inject = |k: FaultKind, ms: u32, src: &str, flog: &mut BufWriter<File>| -> std::io::Result<()> {
            writeln!(flog, "{k:?},{song_s:.3},{ms},{},{src}", qpc::now())?;
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
        let hk = unsafe { std::mem::replace(&mut *std::ptr::addr_of_mut!(HOTKEY), 0) };
        match hk {
            0x70 => inject(FaultKind::Audio, 150, "key", &mut flog)?,
            0x71 => inject(FaultKind::Render, 300, "key", &mut flog)?,
            0x72 => inject(FaultKind::Judge, 300, "key", &mut flog)?,
            0x73 => inject(FaultKind::Input, 300, "key", &mut flog)?,
            0x1B => quit = true,
            _ => {}
        }

        // 그리기
        list.cmds.clear();
        list.clear = Some(Rgba(0.04, 0.04, 0.06, 1.0));
        list.rect(LANE_X, 0.0, LANE_W, H as f32, Rgba(0.10, 0.10, 0.14, 1.0));
        let view = judge.view.lock().map(|v| v.clone()).unwrap_or_default();
        if view.pressed {
            list.rect(LANE_X, JUDGE_Y - 240.0, LANE_W, 240.0, Rgba(0.25, 0.35, 0.6, 0.35));
        }
        list.rect(LANE_X - 10.0, JUDGE_Y, LANE_W + 20.0, 4.0, Rgba(0.9, 0.3, 0.3, 1.0));
        let mut drawn = 0u16;
        if let Some(song) = song {
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
            format!("프로필: 우리 방식 (오디오 장치 시계 · 샘플 예약 · 입력 도장+watermark)"),
            format!("곡 {:.2} s | 화면 {fps:.1} fps, 최근 1초 최대 프레임 간격 {max_gap_ms:.1} ms | vsync {}", song_s.max(0.0), vsync),
            format!(
                "오디오: 블록 {} · 언더런 의심 {} · 최대 깨어남 간격 {audio_gap_ms:.1} ms · 보이스 {} · 늦은 키음 {}",
                a.monitor.blocks.load(Ordering::Relaxed),
                a.monitor.underruns.load(Ordering::Relaxed),
                a.monitor.voices.load(Ordering::Relaxed),
                a.monitor.late_player.load(Ordering::Relaxed)
            ),
            format!(
                "판정: P {} G {} Good {} Bad {} Miss {} | 콤보 {} (최대 {}) | 빈 누름 {} | LN {}/{}",
                t.perfect, t.great, t.good, t.bad, t.miss, t.combo, t.max_combo, t.empty, t.ln_ok, t.ln_break
            ),
            format!(
                "watermark 지연 {:.1} ms · 계약 위반 {} · 입력 손실 {}",
                song.map(|s| (s - judge.watermark_song_ns.load(Ordering::Relaxed)) as f64 / 1e6).unwrap_or(0.0),
                judge.violations.load(Ordering::Relaxed),
                input.lost.load(Ordering::Relaxed)
            ),
            format!("고장: F1 오디오 150 ms · F2 화면 300 ms · F3 판정 300 ms · F4 입력 300 ms · Esc 끝"),
        ];
        for (i, s) in lines.iter().enumerate() {
            list.text(16.0, 12.0 + i as f32 * 22.0, 15.0, hud, s.as_str());
        }
        rend.draw(&list)?;
        let ft = rend.present(vsync);
        if ft.sync_qpc > 0 && ft.sync_refresh > last_sync_r && last_sync_q > 0 {
            let p = (ft.sync_qpc - last_sync_q) / (ft.sync_refresh - last_sync_r) as i64;
            if p > qf / 1000 && p < qf / 20 {
                period_q = p;
            }
        }
        if ft.sync_qpc > 0 {
            last_sync_q = ft.sync_qpc;
            last_sync_r = ft.sync_refresh;
        }
        let _ = fr_tx.push(FrameLog { t: ft, song_ns: song.unwrap_or(i64::MIN), q_display, notes_drawn: drawn, injected_ms: injected });
        if a.done.load(Ordering::Acquire) && flag(args, "--quit-at-end") {
            quit = true;
        }
        if a.done.load(Ordering::Acquire) && !flag(args, "--quit-at-end") && song_s > chart.chart.end as f64 / 1e9 + 3.0 {
            quit = true;
        }
    }
    flog.flush()?;
    a.stop.store(true, Ordering::Release);
    stop.store(true, Ordering::Release);
    let st = a.join.join().map_err(|_| "오디오 스레드 패닉")??;
    let judge_lost = judge_join.join().map_err(|_| "판정 스레드 패닉")?;
    if let Some(b) = bot_join {
        let _ = b.join();
    }
    let input_lost = input.lost.load(Ordering::Relaxed);
    input.stop();
    log_stop.store(true, Ordering::Release);
    logger.join().map_err(|_| "기록 스레드 패닉")??;
    cap_stop.store(true, Ordering::Release);
    let capm = match cap {
        Some(h) => Some(h.join().map_err(|_| "녹음 스레드 패닉")??),
        None => None,
    };
    let view = judge.view.lock().map(|v| v.clone()).unwrap_or_default();
    let t = &view.tally;
    let snap = a.clock.load();
    let meta = format!(
        "{{\n \"chart\": \"{}\", \"chart_hash\": \"{:016x}\", \"title\": \"{}\", \"profile\": \"ours\",\n \"device\": \"{}\", \"rate\": {}, \"buffer_frames\": {}, \"origin\": {origin}, \"master_gain\": {gain}, \"render_peak\": {peak},\n \"out_offset_ns\": {out_offset_ns}, \"vsync\": {vsync}, \"bot\": {use_bot}, \"windows_ns\": [{}, {}, {}, {}], \"render_timing_grade\": \"{}\",\n \"written\": {}, \"blocks\": {}, \"overflows\": {}, \"log_lost\": {}, \"mix_lost\": {}, \"judge_log_lost\": {judge_lost}, \"input_lost\": {input_lost},\n \"tally\": {{\"perfect\": {}, \"great\": {}, \"good\": {}, \"bad\": {}, \"miss\": {}, \"empty\": {}, \"ln_ok\": {}, \"ln_break\": {}, \"max_combo\": {}}},\n \"clock\": {{\"valid\": {}, \"epoch\": {}, \"t0\": {}, \"f0\": {}, \"a\": {}, \"b\": {}}}, \"qpc_freq\": {qf},\n \"capture\": {}\n}}\n",
        path.replace('\\', "/"),
        chart.hash,
        chart.chart.title,
        a.info.device,
        rate,
        a.info.buffer_frames,
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
        t.perfect,
        t.great,
        t.good,
        t.bad,
        t.miss,
        t.empty,
        t.ln_ok,
        t.ln_break,
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
    println!("  판정 P {} G {} Good {} Bad {} Miss {} 빈 {} LN {}/{} 최대 콤보 {}", t.perfect, t.great, t.good, t.bad, t.miss, t.empty, t.ln_ok, t.ln_break, t.max_combo);
    Ok(())
}
