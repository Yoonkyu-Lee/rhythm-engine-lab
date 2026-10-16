//! CP1-2: ASIO 경로에서 예약기와 고장 A·B를 다시 잰다. 기록 형식은 sched와 같아서 analyze_sched.py / analyze_faults.py를 그대로 쓴다.
//!
//!   testbench asiorun --mode sample|immediate [--fps 144] [--secs 50] [--lookahead-ms 100] [--late cut|whole]
//!                     [--sound click|noise|long] [--event-period-s 0.5007] [--faults A@10:50,B@30:300]
//!                     [--index position|callback] [--buffer 64]
//!
//! 출력 0/1 = Playback 1/2 (왼쪽 시험 이벤트, 오른쪽 기준 클릭), 입력 4/5 = Loopback 1/2 (같은 콜백에서 녹음).
//! --index: 우리 소리의 프레임 번호를 무엇으로 셀지.
//!   position (장치 고정): 콜백마다 드라이버의 재생 위치(getSamplePosition). 콜백이 늦어 주기를 건너뛰면 그만큼 소리도 건너뜀.
//!   callback (내용 연속): 콜백 수 × 버퍼 크기. 늦어도 소리는 이어지고, 장치 시간에서는 밀린다.
//! 고장 A@t:ms = 오디오 콜백 안에서 ms 동안 잠듦 (처리 과부하).

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use synchronizer::asio::{self, AsioBufferInfo, AsioCallbacks, Driver, ASIOST_INT32_LSB};
use synchronizer::clock::ClockPolicy;
use synchronizer::shared_clock::{obs_ring, spawn_clock_thread, ObsProducer, SharedClock};
use synchronizer::qpc;
use synchronizer::scheduler::{Cmd, LatePolicy, Mixer};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

use crate::arg;
use crate::sched::{game_loop, noise_burst, spawn_req_probe, parse_faults, Fault, Mode, CLICK, EVENT_PERIOD_S, PILOT_PERIOD_S, PILOT_START_S};

const OUT_L: i32 = 0;
const OUT_R: i32 = 1;
const IN_L: i32 = 4; // Loopback 1
const IN_R: i32 = 5; // Loopback 2

#[derive(Clone, Copy)]
struct CbLog {
    qpc_in: i64,
    qpc_out: i64,
    spos: u64,
    systime_ns: u64,
    frame: u64,
    index: i32,
}

struct State {
    driver: *mut c_void,
    bufs: [AsioBufferInfo; 4],
    buf_frames: usize,
    by_position: bool,
    output_ready: bool,
    rx: Receiver<Cmd>,
    mixer: Mixer,
    scratch: Vec<f32>,
    pilot_start: u64,
    pilot_period: u64,
    total: u64,
    faults: Vec<Fault>,
    fired: Vec<bool>,
    rate: f64,
    callbacks: u64,
    capture: Vec<f32>,
    cblog: Vec<CbLog>,
    fault_log: Vec<(f64, u64, i64, i64, u64)>,
    messages: Vec<(i32, i32, i64)>,
    /// 시계 관측은 링에 넣기만 한다 (잠금·할당 없음). 다시 맞추는 계산은 시계 스레드가 한다.
    obs: ObsProducer,
    done: Arc<AtomicBool>,
}

static STATE: AtomicPtr<State> = AtomicPtr::new(std::ptr::null_mut());
/// 진단용: 콜백 진입·종료 수와 마지막으로 지난 단계.
static CB_IN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CB_OUT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CB_STAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn sample_position(drv: *mut c_void) -> (u64, u64) {
    // Driver 래퍼는 Drop에서 Release를 부르므로, 콜백에서는 함수표를 직접 부른다.
    #[repr(C)]
    struct V {
        _p: [usize; 17],
        get_sample_position: extern "system" fn(*mut c_void, *mut asio::AsioSamples, *mut asio::AsioSamples) -> i32,
    }
    unsafe {
        let vt = &**(drv as *const *const V);
        let (mut s, mut t) = (asio::AsioSamples::default(), asio::AsioSamples::default());
        (vt.get_sample_position)(drv, &mut s, &mut t);
        (s.get(), t.get())
    }
}

fn output_ready(drv: *mut c_void) {
    #[repr(C)]
    struct V {
        _p: [usize; 23],
        output_ready: extern "system" fn(*mut c_void) -> i32,
    }
    unsafe {
        let vt = &**(drv as *const *const V);
        (vt.output_ready)(drv);
    }
}

extern "C" fn buffer_switch(index: i32, _direct: i32) {
    let q_in = qpc::now();
    CB_IN.fetch_add(1, Ordering::Relaxed);
    CB_STAGE.store(1, Ordering::Relaxed);
    let st = unsafe { &mut *STATE.load(Ordering::Acquire) };
    if st.done.load(Ordering::Relaxed) {
        return;
    }
    let (spos, sys) = sample_position(st.driver);
    let n = st.buf_frames;
    let frame = if st.by_position { spos } else { st.callbacks * n as u64 };
    st.callbacks += 1;

    for (i, f) in st.faults.iter().enumerate() {
        if !st.fired[i] && frame as f64 >= f.at_s * st.rate {
            st.fired[i] = true;
            let q0 = qpc::now();
            thread::sleep(Duration::from_millis(f.ms)); // 고장 A: 콜백 처리 과부하
            st.fault_log.push((f.at_s, f.ms, q0, qpc::now(), frame));
        }
    }

    CB_STAGE.store(2, Ordering::Relaxed);
    // 녹음: 루프백 L/R (Int32 → f32)
    let idx = index as usize;
    unsafe {
        let l = st.bufs[2].buffers[idx] as *const i32;
        let r = st.bufs[3].buffers[idx] as *const i32;
        for i in 0..n {
            if st.capture.len() + 2 <= st.capture.capacity() {
                st.capture.push(*l.add(i) as f32 / 2147483648.0);
                st.capture.push(*r.add(i) as f32 / 2147483648.0);
            }
        }
    }
    CB_STAGE.store(3, Ordering::Relaxed);
    // 재생: 예약기 + 오른쪽 기준 클릭
    let buf = &mut st.scratch[..n * 2];
    buf.fill(0.0);
    st.mixer.fill(buf, 2, frame, n, &st.rx);
    for i in 0..n as u64 {
        let f = frame + i;
        if f >= st.pilot_start && (f - st.pilot_start) % st.pilot_period < CLICK.len() as u64 && f < st.total {
            buf[i as usize * 2 + 1] += CLICK[0];
        }
    }
    unsafe {
        let l = st.bufs[0].buffers[idx] as *mut i32;
        let r = st.bufs[1].buffers[idx] as *mut i32;
        for i in 0..n {
            *l.add(i) = (buf[i * 2].clamp(-1.0, 1.0) * 2147483647.0) as i32;
            *r.add(i) = (buf[i * 2 + 1].clamp(-1.0, 1.0) * 2147483647.0) as i32;
        }
    }
    CB_STAGE.store(4, Ordering::Relaxed);
    if st.output_ready {
        output_ready(st.driver);
    }
    CB_STAGE.store(5, Ordering::Relaxed);
    st.obs.push(frame, to_100ns(q_in));
    if st.cblog.len() < st.cblog.capacity() {
        st.cblog.push(CbLog { qpc_in: q_in, qpc_out: qpc::now(), spos, systime_ns: sys, frame, index });
    }
    CB_STAGE.store(6, Ordering::Relaxed);
    CB_OUT.fetch_add(1, Ordering::Relaxed);
}

extern "C" fn sample_rate_did_change(_r: f64) {}

extern "C" fn asio_message(selector: i32, value: i32, _msg: *mut c_void, _opt: *mut f64) -> i32 {
    let st = STATE.load(Ordering::Acquire);
    if !st.is_null() {
        let st = unsafe { &mut *st };
        if st.messages.len() < st.messages.capacity() {
            st.messages.push((selector, value, qpc::now()));
        }
    }
    match selector {
        1 => matches!(value, 2 | 3 | 5 | 6) as i32, // selector supported?
        2 => 2,                                      // engine version
        3 | 5 | 6 => 1,                              // reset / resync / latencies changed: 기록만
        _ => 0,                                      // time info 등 미지원 → bufferSwitch 사용
    }
}

extern "C" fn buffer_switch_time_info(params: *mut c_void, index: i32, direct: i32) -> *mut c_void {
    buffer_switch(index, direct);
    params
}

static CALLBACKS: AsioCallbacks = AsioCallbacks {
    buffer_switch,
    sample_rate_did_change,
    asio_message,
    buffer_switch_time_info,
};

/// QPC 틱 → 100 ns. 곱셈이 i64를 넘지 않게 128비트로 계산한다 (QPC 약 2×10¹² × 10⁷).
fn to_100ns(q: i64) -> i64 {
    (q as i128 * 10_000_000 / qpc::frequency() as i128) as i64
}

pub(crate) fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mode = match arg(args, "--mode", "sample").as_str() {
        "sample" => Mode::Sample,
        "immediate" => Mode::Immediate,
        "perfect" => Mode::Perfect,
        m => return Err(format!("--mode 는 sample · immediate · perfect: {m}").into()),
    };
    let d_ms: f64 = arg(args, "--d-ms", "0").parse()?;
    if mode == Mode::Perfect && d_ms <= 0.0 {
        return Err("--mode perfect 에는 --d-ms (기록 시계 D) 가 필요".into());
    }
    let fps: f64 = arg(args, "--fps", "144").parse()?;
    let secs: f64 = arg(args, "--secs", "50").parse()?;
    let lookahead_ms: f64 = arg(args, "--lookahead-ms", "100").parse()?;
    let late = if arg(args, "--late", "cut") == "whole" { LatePolicy::PlayWhole } else { LatePolicy::CutFront };
    let sound_kind = arg(args, "--sound", "noise");
    let noise_s = match sound_kind.as_str() { "noise" => 0.05, "long" => 1.0, _ => 0.0 };
    let event_period: f64 = arg(args, "--event-period-s", &EVENT_PERIOD_S.to_string()).parse()?;
    let faults = parse_faults(&arg(args, "--faults", ""))?;
    let by_position = arg(args, "--index", "position") != "callback";
    let buffer: i32 = arg(args, "--buffer", "64").parse()?;
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let tag = format!(
        "{}-{}-{}",
        match mode { Mode::Sample => format!("sample-{}", if late == LatePolicy::CutFront { "cut" } else { "whole" }), Mode::Immediate => format!("immediate-{fps:.0}fps"), Mode::Perfect => format!("perfect-d{d_ms}") },
        if by_position { "pos" } else { "cb" },
        buffer
    );
    let out: PathBuf = Path::new(&arg(args, "--out", "runs")).join(format!("asio-{tag}-{unix}"));
    fs::create_dir_all(&out)?;

    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let entry = asio::drivers().into_iter().find(|d| d.name.contains("TOPPING")).ok_or("TOPPING ASIO 드라이버 없음")?;
    let drv = Driver::open(&entry)?;
    let rate = drv.sample_rate().map_err(|e| format!("샘플레이트 조회 실패 {e}"))?;
    for (c, is_in) in [(OUT_L, false), (OUT_R, false), (IN_L, true), (IN_R, true)] {
        let (_, t, _) = drv.channel_info(c, is_in).map_err(|e| format!("채널 정보 실패 {e}"))?;
        if t != ASIOST_INT32_LSB {
            return Err(format!("채널 {c}가 Int32LSB가 아님 (형식 {t})").into());
        }
    }
    let (in_lat, out_lat) = drv.latencies().unwrap_or((0, 0));

    let sound0 = if noise_s > 0.0 { noise_burst(rate, noise_s) } else { CLICK.to_vec() };
    {
        let mut f = BufWriter::new(File::create(out.join("sound0.f32"))?);
        for v in &sound0 {
            f.write_all(&v.to_le_bytes())?;
        }
    }
    let (tx, rx) = channel::<Cmd>();
    // 시계 스레드: 콜백이 링에 넣은 관측으로 모델을 갱신하고 스냅샷을 공개한다. 게임 스레드는 잠금 없이 읽는다.
    let clock = Arc::new(SharedClock::new());
    let (obs, obs_rx) = obs_ring(8192);
    let clock_thread = spawn_clock_thread(rate, ClockPolicy::default(), obs_rx, clock.clone());
    // 노트북 맥박 탐침: 오디오와 무관한 최고 우선순위 스레드가 1 ms마다 깬다. ASIO가 멈춘 순간에 이것도 못 깼으면
    // 노트북 전체가 멈춘 것, 멀쩡했으면 USB 오디오 경로 쪽이다.
    let probe_stop = Arc::new(AtomicBool::new(false));
    let probe = {
        let stop = probe_stop.clone();
        thread::spawn(move || {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
                windows::Win32::Media::timeBeginPeriod(1);
            }
            let qf = qpc::frequency() as f64;
            let mut gaps: Vec<(i64, f64)> = Vec::with_capacity(100_000);
            let (mut wakes, mut last) = (0u64, qpc::now());
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(1));
                let now = qpc::now();
                let g = (now - last) as f64 * 1e3 / qf;
                if g > 4.0 && gaps.len() < gaps.capacity() {
                    gaps.push((last, g));
                }
                last = now;
                wakes += 1;
            }
            unsafe { windows::Win32::Media::timeEndPeriod(1) };
            (wakes, gaps)
        })
    };
    let done = Arc::new(AtomicBool::new(false));
    let total = (secs * rate).round() as u64;
    let null = std::ptr::null_mut();
    let mut st = Box::new(State {
        driver: drv.raw(),
        bufs: [
            AsioBufferInfo { is_input: 0, channel_num: OUT_L, buffers: [null; 2] },
            AsioBufferInfo { is_input: 0, channel_num: OUT_R, buffers: [null; 2] },
            AsioBufferInfo { is_input: 1, channel_num: IN_L, buffers: [null; 2] },
            AsioBufferInfo { is_input: 1, channel_num: IN_R, buffers: [null; 2] },
        ],
        buf_frames: buffer as usize,
        by_position,
        output_ready: false,
        rx,
        mixer: Mixer::new(vec![sound0, Vec::new(), CLICK.to_vec()], late), // 1번 = 요청 탐침용 무음, 2번 = 키음 (CP1-4 4-3)
        scratch: vec![0.0; 4096 * 2],
        // 기준 클릭(오른쪽)은 PERFECT 누름 시험에서 끈다 (오른쪽을 키음이 쓴다)
        pilot_start: if mode == Mode::Perfect { u64::MAX / 2 } else { (PILOT_START_S * rate).round() as u64 },
        pilot_period: (PILOT_PERIOD_S * rate).round() as u64,
        total,
        faults: faults.iter().copied().filter(|f| f.kind == 'A').collect(),
        fired: vec![false; faults.len()],
        rate,
        callbacks: 0,
        capture: Vec::with_capacity(((secs + 2.0) * rate) as usize * 2),
        cblog: Vec::with_capacity(((secs + 2.0) * rate / buffer as f64) as usize + 1024),
        fault_log: Vec::new(),
        messages: Vec::with_capacity(1024),
        obs,
        done: done.clone(),
    });
    unsafe {
        drv.create_buffers(&mut st.bufs, buffer, &CALLBACKS).map_err(|e| format!("createBuffers 실패 {e} ({})", asio::error_name(e)))?;
    }
    st.output_ready = drv.output_ready().is_ok();
    // 지연은 버퍼 크기에 따라 달라질 수 있어서, 버퍼를 만든 뒤에 다시 묻는다 (만들기 전 값과 둘 다 기록).
    let (in_lat_buf, out_lat_buf) = drv.latencies().unwrap_or((0, 0));
    STATE.store(&mut *st as *mut State, Ordering::Release);
    drv.start().map_err(|e| format!("start 실패 {e}"))?;

    // 게임 스레드 (sched와 같은 코드). 시계 모델은 콜백이 채운다.
    // 드라이버를 만든 이 스레드(STA)는 메시지를 계속 처리한다: 처리하지 않으면 약 0.7초 뒤 콜백이 멈췄다.
    let fb: Vec<Fault> = faults.iter().copied().filter(|f| f.kind == 'B').collect();
    let probe_hz: f64 = arg(args, "--probe-hz", "0").parse()?;
    let req_probe = (probe_hz > 0.0).then(|| spawn_req_probe(clock.clone(), tx.clone(), probe_hz, done.clone()));
    let game_thread = {
        let (out, done) = (out.clone(), done.clone());
        let fbc: Vec<Fault> = faults.iter().copied().filter(|f| f.kind == 'B' || f.kind == 'C').collect();
        thread::spawn(move || {
            if mode == Mode::Perfect {
                crate::perfect::perfect_loop(&out, secs, event_period, d_ms, fbc, tx, clock, done).map_err(|e| e.to_string())
            } else {
                game_loop(&out, mode, fps, secs, lookahead_ms, event_period, fb, tx, clock, done).map_err(|e| e.to_string())
            }
        })
    };
    // 콜백이 2초 넘게 멈추면 어느 단계에서 멈췄는지 알린다 (진단).
    let (mut last_report, mut last_in) = (std::time::Instant::now(), 0u64);
    while !game_thread.is_finished() {
        pump_messages();
        thread::sleep(Duration::from_millis(1));
        if last_report.elapsed() > Duration::from_secs(2) {
            last_report = std::time::Instant::now();
            let now_in = CB_IN.load(Ordering::Relaxed);
            if now_in == last_in {
                println!("  [진단] 콜백 멈춤: 진입 {now_in} 종료 {} 마지막 단계 {}", CB_OUT.load(Ordering::Relaxed), CB_STAGE.load(Ordering::Relaxed));
            }
            last_in = now_in;
        }
    }
    let game = game_thread.join().map_err(|_| "게임 스레드 패닉")?;
    // 마지막 소리가 루프백으로 돌아올 때까지
    for _ in 0..700 {
        pump_messages();
        thread::sleep(Duration::from_millis(1));
    }
    done.store(true, Ordering::Release);
    thread::sleep(Duration::from_millis(50));
    let _ = drv.stop();
    let _ = drv.dispose_buffers();
    STATE.store(std::ptr::null_mut(), Ordering::Release);
    probe_stop.store(true, Ordering::Release);
    if let Some(p) = req_probe {
        println!("  요청 탐침: {}개 보냄", p.join().map_err(|_| "요청 탐침 스레드 패닉")?);
    }
    let (wakes, gaps) = probe.join().map_err(|_| "탐침 스레드 패닉")?;
    game.map_err(|e| e.to_string())?;

    // 기록 (sched와 같은 형식)
    let qf = qpc::frequency();
    let mut cap = BufWriter::new(File::create(out.join("capture_2ch.f32"))?);
    for v in &st.capture {
        cap.write_all(&v.to_le_bytes())?;
    }
    cap.flush()?;
    let mut cp = BufWriter::new(File::create(out.join("capture_packets.csv"))?);
    writeln!(cp, "qpc,device_pos,qpc100ns,frames,flags")?;
    let mut cbl = BufWriter::new(File::create(out.join("asio_callbacks.csv"))?);
    writeln!(cbl, "qpc_in,qpc_out,spos,systime_ns,frame,index")?;
    for (i, c) in st.cblog.iter().enumerate() {
        // 녹음 위치 = 장치 재생 위치 (콜백이 멈춘 동안 장치는 흘러가고 그 구간의 입력은 받지 못한다 → 위치로 적어야 구멍이 보인다)
        let _ = i;
        writeln!(cp, "{},{},{},{},0", c.qpc_in, c.spos, to_100ns(c.qpc_in), buffer)?;
        writeln!(cbl, "{},{},{},{},{},{}", c.qpc_in, c.qpc_out, c.spos, c.systime_ns, c.frame, c.index)?;
    }
    cp.flush()?;
    cbl.flush()?;
    let mut pl = BufWriter::new(File::create(out.join("pilots.csv"))?);
    writeln!(pl, "frame")?;
    let mut k = st.pilot_start;
    while k < total {
        writeln!(pl, "{k}")?;
        k += st.pilot_period;
    }
    pl.flush()?;
    let mut sl = BufWriter::new(File::create(out.join("starts.csv"))?);
    writeln!(sl, "sound,requested,start,block")?;
    for l in &st.mixer.log {
        writeln!(sl, "{},{},{},{}", l.sound, l.requested.map(|r| r as i64).unwrap_or(-1), l.start, l.block)?;
    }
    sl.flush()?;
    let mut fl = BufWriter::new(File::create(out.join("faults_render.csv"))?);
    writeln!(fl, "kind,at_s,ms,qpc_before,qpc_after,written")?;
    for (at, ms, q0, q1, w) in &st.fault_log {
        writeln!(fl, "A,{at},{ms},{q0},{q1},{w}")?;
    }
    fl.flush()?;
    let mut pg = BufWriter::new(File::create(out.join("probe_gaps.csv"))?);
    writeln!(pg, "qpc_before,gap_ms")?;
    for (q, g) in &gaps {
        writeln!(pg, "{q},{g:.3}")?;
    }
    pg.flush()?;
    let mut ml = BufWriter::new(File::create(out.join("asio_messages.csv"))?);
    writeln!(ml, "selector,value,qpc")?;
    for (s, v, q) in &st.messages {
        writeln!(ml, "{s},{v},{q}")?;
    }
    ml.flush()?;
    let m = format!(
        "{{\"name\": \"{}\", \"rate\": {}, \"channels\": 2, \"buffer_frames\": {buffer}, \"default_period_100ns\": {}, \"clock_freq\": {}, \"mmcss\": false}}",
        entry.name, rate as u32, (buffer as f64 / rate * 1e7) as i64, rate as u64
    );
    fs::write(out.join("meta.json"), format!(
        "{{\"path\": \"ASIO\", \"mode\": \"{}\", \"fps\": {fps}, \"lookahead_ms\": {lookahead_ms}, \"late\": \"{}\", \"sound\": \"{sound_kind}\", \"faults\": \"{}\", \"secs\": {secs}, \"d_ms\": {d_ms}, \"qpc_freq\": {qf}, \
         \"index\": \"{}\", \"asio_latency\": {{\"input\": {in_lat}, \"output\": {out_lat}}}, \"asio_latency_after_create\": {{\"input\": {in_lat_buf}, \"output\": {out_lat_buf}}}, \"output_ready\": {}, \"callbacks\": {}, \
         \"event\": {{\"start_s\": 1.0, \"period_s\": {event_period}}}, \"pilot\": {{\"start_s\": {PILOT_START_S}, \"period_s\": {PILOT_PERIOD_S}}}, \"render\": {m}, \"capture\": {m}}}\n",
        match mode { Mode::Sample => "sample", Mode::Immediate => "immediate", Mode::Perfect => "perfect" },
        if late == LatePolicy::CutFront { "cut" } else { "whole" },
        arg(args, "--faults", ""),
        if by_position { "position" } else { "callback" },
        st.output_ready, st.callbacks
    ))?;
    println!("기록: {}  (콜백 {}회, outputReady {}, 메시지 {}개, 탐침 깨어남 {}회 중 4 ms 넘는 공백 {}회)", out.display(), st.callbacks, st.output_ready, st.messages.len(), wakes, gaps.len());
    // 시계 스레드는 콜백 쪽 생산자가 사라지고 링이 비면 끝난다.
    drop(st);
    let cs = clock_thread.join().map_err(|_| "시계 스레드 패닉")?;
    println!("  시계: 관측 {}개, 공개 {}회, 새 기준점 {}회, 버린 관측 {}개, 지연(만들기 전 → 후) 입력 {in_lat} → {in_lat_buf}, 출력 {out_lat} → {out_lat_buf}", cs.observed, cs.published, cs.new_epochs, cs.dropped);
    Ok(())
}
