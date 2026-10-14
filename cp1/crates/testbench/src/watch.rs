//! CP1-3: 노트북 멈춤 상주 감시. 관리자 권한도 오디오 장치도 쓰지 않으므로 몇 시간씩, 게임을 하면서도 켜 둘 수 있다.
//!
//!   testbench watch [--hours 0] [--out runs] [--gap-ms 4] [--every-s 5] [--alert-ms 50]
//!
//! 1. 탐침: 최고 우선순위 스레드가 1 ms마다 깬다. 못 깬 공백(--gap-ms 초과)을 시각과 함께 gaps.csv에 남긴다.
//!    ACPI 87 ms DPC 같은 드라이버의 CPU 독점이 있으면 여기에 그 길이로 찍힌다.
//! 2. 상황: --every-s마다 CPU 사용률, 온도 센서(Thermal Zone), 충전기·배터리, CPU를 많이 쓴 프로세스 5개를 context.csv에 남긴다.
//!    멈춤이 다시 나타난 때와 사라진 때의 조건을 비교하려는 것.
//! 끝내기: --hours가 지나거나, 출력 폴더에 stop 파일을 만들거나, Ctrl+C (기록은 줄마다 바로 쓴다).

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use synchronizer::qpc;
use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::arg;

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
            if let Ok((lt, q, g, c)) = grx.recv_timeout(Duration::from_millis(200)) {
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
            println!("[{lt}] {:.0}분 경과, 공백 {n_gaps}회 (그중 {alert_ms} ms 이상 {n_alert}회)", el / 60.0);
        }
        if (hours > 0.0 && el >= hours * 3600.0) || out.join("stop").exists() {
            break;
        }
    }
    stop.store(true, Ordering::Release);
    let wakes = probe.join().map_err(|_| "탐침 스레드 패닉")?;
    println!("끝: {}  (탐침 깨어남 {wakes}회, 공백 {n_gaps}회, {alert_ms} ms 이상 {n_alert}회)", out.display());
    Ok(())
}
