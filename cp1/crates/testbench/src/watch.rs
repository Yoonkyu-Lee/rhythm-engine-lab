//! CP1-3: 노트북 멈춤 상주 감시. 관리자 권한이 필요 없으므로 몇 시간씩 켜 둘 수 있다.
//!
//!   testbench watch [--hours 0] [--out runs] [--gap-ms 4] [--every-s 5] [--alert-ms 50]
//!                   [--audio none|wasapi|asio] [--render "Playback 1/2"] [--driver TOPPING] [--buffer 64]
//!
//! 1. 탐침: 최고 우선순위 스레드가 1 ms마다 깬다. 못 깬 공백(--gap-ms 초과)을 시각과 함께 gaps.csv에 남긴다.
//!    ACPI 87 ms DPC 같은 드라이버의 CPU 독점이 있으면 여기에 그 길이로 찍힌다.
//! 2. 상황: --every-s마다 CPU 사용률, 온도 센서(Thermal Zone), 충전기·배터리, CPU를 많이 쓴 프로세스 5개를 context.csv에 남긴다.
//!    멈춤이 다시 나타난 때와 사라진 때의 조건을 비교하려는 것.
//! 3. 오디오 유지 (--audio): 87 ms 멈춤은 오디오가 돌 때만 관찰됐다 (CP1-3). 무음을 계속 내보내며 오디오 주기
//!    (ASIO 콜백, WASAPI 이벤트) 간격이 벌어진 때를 audio_gaps.csv에 남긴다. 탐침 공백과 같은 시각축.
//!    wasapi는 공유 모드라 다른 프로그램(유튜브 등)과 장치를 같이 쓴다. asio는 장치를 잡으므로 ASIO 게임과 충돌할 수 있다.
//! 끝내기: --hours가 지나거나, 출력 폴더에 stop 파일을 만들거나, Ctrl+C (기록은 줄마다 바로 쓴다).

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use synchronizer::asio::{self, AsioBufferInfo, AsioCallbacks, Driver};
use synchronizer::qpc;
use synchronizer::shared_clock::{obs_ring, ObsConsumer, ObsProducer};
use synchronizer::wasapi::{self, IAudioRenderClient};
use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::arg;
use crate::asiorun::pump_messages;

/// ASIO 콜백 상태. 콜백은 무음을 쓰고, 간격이 문턱을 넘으면 링에 (간격 µs, 이전 QPC)를 넣기만 한다.
struct AsioWatch {
    bufs: [AsioBufferInfo; 2],
    frames: usize,
    limit_qpc: i64,
    last: i64,
    callbacks: u64,
    ring: ObsProducer,
}

static ASIO_WATCH: std::sync::atomic::AtomicPtr<AsioWatch> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

extern "C" fn watch_switch(index: i32, _direct: i32) {
    let q = qpc::now();
    let p = ASIO_WATCH.load(Ordering::Acquire);
    if p.is_null() {
        return;
    }
    let st = unsafe { &mut *p };
    for b in &st.bufs {
        unsafe { std::ptr::write_bytes(b.buffers[index as usize] as *mut i32, 0, st.frames) };
    }
    if st.last != 0 && q - st.last > st.limit_qpc {
        let us = ((q - st.last) as i128 * 1_000_000 / qpc::frequency() as i128) as u64;
        st.ring.push(us, st.last);
    }
    st.last = q;
    st.callbacks += 1;
}

extern "C" fn watch_rate(_r: f64) {}

extern "C" fn watch_message(selector: i32, value: i32, _m: *mut std::ffi::c_void, _o: *mut f64) -> i32 {
    match selector {
        1 => matches!(value, 2 | 3 | 5 | 6) as i32,
        2 => 2,
        3 | 5 | 6 => 1,
        _ => 0,
    }
}

extern "C" fn watch_switch_ti(params: *mut std::ffi::c_void, index: i32, direct: i32) -> *mut std::ffi::c_void {
    watch_switch(index, direct);
    params
}

static WATCH_CALLBACKS: AsioCallbacks = AsioCallbacks {
    buffer_switch: watch_switch,
    sample_rate_did_change: watch_rate,
    asio_message: watch_message,
    buffer_switch_time_info: watch_switch_ti,
};

type AudioGap = (String, &'static str, i64, f64);

/// 켜 둔 오디오 경로. 끝낼 때 정리한다.
enum Audio {
    None,
    Asio { drv: Driver, st: Box<AsioWatch>, rx: ObsConsumer, period_ms: f64 },
    Wasapi { stop: Arc<AtomicBool>, th: thread::JoinHandle<Result<u64, String>>, period_ms: f64 },
}

/// ASIO: 출력 0/1에 무음, 콜백 간격이 주기 + 4 ms를 넘으면 기록.
fn start_asio(driver: &str, buffer: i32) -> Result<Audio, Box<dyn std::error::Error>> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let entry = asio::drivers().into_iter().find(|d| d.name.contains(driver)).ok_or(format!("ASIO 드라이버 없음: {driver}"))?;
    let drv = Driver::open(&entry)?;
    let rate = drv.sample_rate().map_err(|e| format!("샘플레이트 조회 실패 {e}"))?;
    let period_ms = buffer as f64 / rate * 1e3;
    let (ring, rx) = obs_ring(4096);
    let null = std::ptr::null_mut();
    let mut st = Box::new(AsioWatch {
        bufs: [
            AsioBufferInfo { is_input: 0, channel_num: 0, buffers: [null; 2] },
            AsioBufferInfo { is_input: 0, channel_num: 1, buffers: [null; 2] },
        ],
        frames: buffer as usize,
        limit_qpc: ((period_ms + 4.0) * 1e-3 * qpc::frequency() as f64) as i64,
        last: 0,
        callbacks: 0,
        ring,
    });
    unsafe { drv.create_buffers(&mut st.bufs, buffer, &WATCH_CALLBACKS).map_err(|e| format!("createBuffers 실패 {e} ({})", asio::error_name(e)))? };
    ASIO_WATCH.store(&mut *st as *mut AsioWatch, Ordering::Release);
    drv.start().map_err(|e| format!("start 실패 {e}"))?;
    println!("오디오 유지: ASIO {} 버퍼 {buffer} ({period_ms:.2} ms), 콜백 간격 > {:.2} ms 기록", entry.name, period_ms + 4.0);
    Ok(Audio::Asio { drv, st, rx, period_ms })
}

/// WASAPI 공유: 무음 패킷을 계속 채우고, 이벤트 간격이 주기 × 2 + 5 ms를 넘거나 버퍼가 비어 있던(언더런) 때를 기록.
fn start_wasapi(render: &str, tx: std::sync::mpsc::Sender<AudioGap>) -> Result<Audio, Box<dyn std::error::Error>> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ptx, prx) = channel::<Result<f64, String>>();
    let (stop2, render) = (stop.clone(), render.to_string());
    let name = render.clone();
    let th = thread::spawn(move || -> Result<u64, String> {
        let r = (|| -> Result<u64, Box<dyn std::error::Error>> {
            wasapi::com_init()?;
            wasapi::mmcss_pro_audio();
            let ep = wasapi::find(false, &render)?;
            let s = wasapi::Stream::open(&ep.device)?;
            let rc: IAudioRenderClient = s.service()?;
            let period_ms = s.default_period_100ns as f64 * 1e-4;
            let _ = ptx.send(Ok(period_ms));
            let qf = qpc::frequency() as f64;
            let limit = (period_ms * 2.0 + 5.0) * 1e-3 * qf;
            unsafe {
                let _ = rc.GetBuffer(s.buffer_frames)?;
                rc.ReleaseBuffer(s.buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
                s.client.Start()?;
            }
            let (mut last, mut wakes) = (qpc::now(), 0u64);
            while !stop2.load(Ordering::Relaxed) {
                unsafe { WaitForSingleObject(s.event, 2000) };
                let now = qpc::now();
                let pad = unsafe { s.client.GetCurrentPadding()? };
                if (now - last) as f64 > limit {
                    let _ = tx.send((local_time(), "wasapi", last, (now - last) as f64 * 1e3 / qf));
                }
                if pad == 0 && wakes > 10 {
                    let _ = tx.send((local_time(), "wasapi_underrun", last, (now - last) as f64 * 1e3 / qf));
                }
                let n = s.buffer_frames - pad;
                if n > 0 {
                    unsafe {
                        let _ = rc.GetBuffer(n)?;
                        rc.ReleaseBuffer(n, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
                    }
                }
                last = now;
                wakes += 1;
            }
            unsafe { s.client.Stop()? };
            Ok(wakes)
        })();
        r.map_err(|e| {
            let m = e.to_string();
            let _ = ptx.send(Err(m.clone()));
            m
        })
    });
    let period_ms = prx.recv_timeout(Duration::from_secs(5)).map_err(|_| "WASAPI 시작 응답 없음")??;
    println!("오디오 유지: WASAPI 공유 {name}, 주기 {period_ms:.2} ms, 이벤트 간격 > {:.1} ms 또는 언더런 기록", period_ms * 2.0 + 5.0);
    Ok(Audio::Wasapi { stop, th, period_ms })
}

fn local_time() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

/// 성능 카운터 하나 (와일드카드면 인스턴스 여러 개).
struct Counter(isize);

impl Counter {
    fn add(query: isize, path: PCWSTR) -> Option<Counter> {
        let mut c = 0isize;
        let r = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) };
        (r == 0).then_some(Counter(c))
    }

    fn read(&self) -> Vec<(String, f64)> {
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000); // PDH_FMT_NOCAP100: 프로세스 사용률은 코어 수만큼 100%를 넘을 수 있다
        let (mut size, mut count) = (0u32, 0u32);
        let r = unsafe { PdhGetFormattedCounterArrayW(self.0, fmt, &mut size, &mut count, None) };
        if r != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        // 버퍼 앞쪽은 항목 배열, 뒤쪽은 인스턴스 이름 문자열.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        let r = unsafe { PdhGetFormattedCounterArrayW(self.0, fmt, &mut size, &mut count, Some(items)) };
        if r != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .filter_map(|i| unsafe {
                let it = &*items.add(i);
                if it.FmtValue.CStatus != 0 {
                    return None;
                }
                Some((it.szName.to_string().unwrap_or_default(), it.FmtValue.Anonymous.doubleValue))
            })
            .collect()
    }
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let hours: f64 = arg(args, "--hours", "0").parse()?;
    let gap_ms: f64 = arg(args, "--gap-ms", "4").parse()?;
    let every_s: f64 = arg(args, "--every-s", "5").parse()?;
    let alert_ms: f64 = arg(args, "--alert-ms", "50").parse()?;
    let unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let audio_kind = arg(args, "--audio", "none");
    let out = PathBuf::from(arg(args, "--out", "runs")).join(format!("watch-{unix}"));
    fs::create_dir_all(&out)?;

    let mut query = 0isize;
    if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
        return Err("성능 카운터 쿼리 열기 실패".into());
    }
    let cpu = Counter::add(query, w!("\\Processor Information(_Total)\\% Processor Utility"));
    let temp = Counter::add(query, w!("\\Thermal Zone Information(*)\\Temperature"));
    let procs = Counter::add(query, w!("\\Process(*)\\% Processor Time"));
    let ncpu = thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    unsafe { PdhCollectQueryData(query) };

    let stop = Arc::new(AtomicBool::new(false));
    let (gtx, grx) = channel::<(String, i64, f64, u32)>();
    let probe = {
        let stop = stop.clone();
        thread::spawn(move || {
            use windows::Win32::System::Threading::{GetCurrentProcessorNumber, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
                windows::Win32::Media::timeBeginPeriod(1);
            }
            let qf = qpc::frequency() as f64;
            let (mut wakes, mut last) = (0u64, qpc::now());
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(1));
                let now = qpc::now();
                let g = (now - last) as f64 * 1e3 / qf;
                if g > gap_ms {
                    let _ = gtx.send((local_time(), last, g, unsafe { GetCurrentProcessorNumber() }));
                }
                last = now;
                wakes += 1;
            }
            unsafe { windows::Win32::Media::timeEndPeriod(1) };
            wakes
        })
    };

    let (atx, arx) = channel::<AudioGap>();
    let mut audio = match audio_kind.as_str() {
        "none" => Audio::None,
        "asio" => start_asio(&arg(args, "--driver", "TOPPING"), arg(args, "--buffer", "64").parse()?)?,
        "wasapi" => start_wasapi(&arg(args, "--render", "Playback 1/2"), atx.clone())?,
        k => return Err(format!("--audio 는 none, wasapi, asio: {k}").into()),
    };
    let mut aw = BufWriter::new(File::create(out.join("audio_gaps.csv"))?);
    writeln!(aw, "local_time,kind,qpc_before,gap_ms")?;
    aw.flush()?;
    let mut n_audio = 0u64;

    let mut gw = BufWriter::new(File::create(out.join("gaps.csv"))?);
    writeln!(gw, "local_time,qpc_before,gap_ms,cpu_after")?;
    gw.flush()?;
    let mut cw = BufWriter::new(File::create(out.join("context.csv"))?);
    writeln!(cw, "local_time,cpu_pct,temps_c,ac,battery_pct,gaps_in_interval,max_gap_ms,top_procs")?;
    cw.flush()?;
    println!("감시 시작: {}  (공백 > {gap_ms} ms 기록, {alert_ms} ms 이상은 화면에 표시, 상황은 {every_s}초마다)", out.display());
    println!("끝내기: Ctrl+C 또는 {} 파일 만들기", out.join("stop").display());

    let start = Instant::now();
    let (mut n_gaps, mut n_alert) = (0u64, 0u64);
    let mut last_ctx = String::new();
    loop {
        let t0 = Instant::now();
        let (mut k, mut mx) = (0u32, 0f64);
        while t0.elapsed().as_secs_f64() < every_s {
            // ASIO 드라이버를 만든 이 스레드(STA)는 메시지를 처리해야 콜백이 계속 온다.
            pump_messages();
            if let Audio::Asio { rx, .. } = &mut audio {
                while let Some((us, q)) = rx.pop() {
                    let _ = atx.send((local_time(), "asio", q, us as f64 * 1e-3));
                }
            }
            while let Ok((lt, kind, q, g)) = arx.try_recv() {
                writeln!(aw, "{lt},{kind},{q},{g:.3}")?;
                aw.flush()?;
                n_audio += 1;
                if g >= 20.0 || kind == "wasapi_underrun" {
                    println!("[{lt}] 오디오 {kind} 간격 {g:.1} ms  직전 상황: {last_ctx}");
                }
            }
            if let Ok((lt, q, g, c)) = grx.recv_timeout(Duration::from_millis(20)) {
                writeln!(gw, "{lt},{q},{g:.3},{c}")?;
                gw.flush()?;
                n_gaps += 1;
                k += 1;
                mx = mx.max(g);
                if g >= alert_ms {
                    n_alert += 1;
                    println!("[{lt}] 멈춤 {g:.1} ms (CPU {c})  직전 상황: {last_ctx}");
                }
            }
        }
        unsafe { PdhCollectQueryData(query) };
        let cpu_pct = cpu.as_ref().and_then(|c| c.read().first().map(|v| v.1)).unwrap_or(f64::NAN);
        let temps: Vec<String> = temp.as_ref().map(|c| c.read()).unwrap_or_default().iter().map(|(n, k)| format!("{n}={:.1}", k - 273.15)).collect();
        let mut top: Vec<(String, f64)> = procs
            .as_ref()
            .map(|c| c.read())
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| n != "_Total" && n != "Idle")
            .collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top: Vec<String> = top.iter().take(5).map(|(n, v)| format!("{n}={:.1}", v / ncpu)).collect();
        let mut ps = SYSTEM_POWER_STATUS::default();
        let _ = unsafe { GetSystemPowerStatus(&mut ps) };
        let lt = local_time();
        writeln!(cw, "{lt},{cpu_pct:.1},{},{},{},{k},{mx:.1},{}", temps.join(";"), ps.ACLineStatus, ps.BatteryLifePercent, top.join(";"))?;
        cw.flush()?;
        last_ctx = format!("CPU {cpu_pct:.0}%, 온도 {}, 상위 {}", temps.join(" "), top.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
        let el = start.elapsed().as_secs_f64();
        if el as u64 % 600 < every_s as u64 {
            println!("[{lt}] {:.0}분 경과, 공백 {n_gaps}회 (그중 {alert_ms} ms 이상 {n_alert}회), 오디오 간격 이상 {n_audio}회", el / 60.0);
        }
        if (hours > 0.0 && el >= hours * 3600.0) || out.join("stop").exists() {
            break;
        }
    }
    stop.store(true, Ordering::Release);
    let wakes = probe.join().map_err(|_| "탐침 스레드 패닉")?;
    let audio_note = match audio {
        Audio::None => String::new(),
        Audio::Asio { drv, st, period_ms, .. } => {
            let _ = drv.stop();
            ASIO_WATCH.store(std::ptr::null_mut(), Ordering::Release);
            thread::sleep(Duration::from_millis(20));
            let _ = drv.dispose_buffers();
            format!(", ASIO 콜백 {}회 (주기 {period_ms:.2} ms)", st.callbacks)
        }
        Audio::Wasapi { stop, th, period_ms } => {
            stop.store(true, Ordering::Release);
            let w = th.join().map_err(|_| "WASAPI 스레드 패닉")??;
            format!(", WASAPI 이벤트 {w}회 (주기 {period_ms:.2} ms)")
        }
    };
    println!("끝: {}  (탐침 깨어남 {wakes}회, 공백 {n_gaps}회, {alert_ms} ms 이상 {n_alert}회, 오디오 간격 이상 {n_audio}회{audio_note})", out.display());
    Ok(())
}
