//! 3-5 측정 명령 (창·소리 없음). README 4-3과 7장 3-5.
//!
//!   cp3game loadbench <라이브러리> [--rate 48000] [--threads N] [--limit N] [--csv 출력.csv]
//!       곡마다 첫 차분을 플레이처럼 불러온다(열기 → 계획 → 전부 디코딩). 열기·계획·디코딩 ms, PCM 크기, 최고 사적 메모리.
//!   cp3game prevsweep <라이브러리> [--w 0.5,1,2,4] [--debounce 75,150,250] [--songs 30] [--threads N] [--seed 1]
//!       미리보기 "누락 없는 첫 출력까지" (커서가 머문 순간 → 준비 집합 완료; 오디오 열기는 빠짐)를 시나리오별로:
//!       settle(곡에 바로 머묾) · late(곡 70% 지점) · scroll(곡 5개를 간격 I로 지나 6번째에 머묾, I = 100·200·300 ms) · drag(같은 곡에서 2초 동안 100 ms마다 탐색 뒤 멈춤).
//!       취소된 작업: 시작된 저장소 수, 버린 슬롯 수, 취소 → 작업자 모두 멈춤까지(회수 시간), 최고 사적 메모리.
//!   cp3game memtest <라이브러리> [--switches 20] [--interval-ms 50] [--rate 48000]
//!       빠른 곡 전환(미리보기 저장소를 띄우고 곧 취소), 한 곡 안 차분 순회(저장소 공유·합집합 예산), 장치 레이트 바꾸기(44.1k ↔ 48k).
//!
//!   cp3game partcheck <라이브러리> [--limit 60] [--rate 48000]
//!       긴 Vorbis 슬롯마다 부분 디코딩(여러 구간) = 전체 디코딩의 같은 구간인지 비트까지 비교 (README 4-3 3 채택 조건).
//!
//! 메모리 = 프로세스 사적 커밋(PrivateUsage)을 5 ms마다 재서 최댓값. 결과는 표준 출력(사람용)과 CSV.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::load::{fill_all, plan_chart_reuse, LoadOpts, SlotCache};
use engine::preview::Store;
use track::library::{read_index, IndexEntry};
use track::pack::Bundle;

use crate::arg;

/// 프로세스 사적 커밋 (바이트)
pub fn private_bytes() -> u64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
    }
    c.PrivateUsage as u64
}

/// 5 ms마다 사적 커밋을 재서 최댓값을 남기는 감시 스레드. `reset`하면 지금 값부터 다시.
pub struct PeakMem {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    h: Option<std::thread::JoinHandle<()>>,
}

impl PeakMem {
    pub fn start() -> PeakMem {
        let peak = Arc::new(AtomicU64::new(private_bytes()));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let h = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(private_bytes(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        PeakMem { peak, stop, h: Some(h) }
    }
    pub fn reset(&self) {
        self.peak.store(private_bytes(), Ordering::Relaxed);
    }
    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed).max(private_bytes())
    }
}

impl Drop for PeakMem {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.h.take() {
            let _ = h.join();
        }
    }
}

fn mb(b: u64) -> f64 {
    b as f64 / 1e6
}

fn q(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f64 * p).round() as usize]
}

fn dist(v: &[f64]) -> String {
    format!("p50 {:.0} · p95 {:.0} · p99 {:.0} · 최대 {:.0} ms (n={})", q(v, 0.5), q(v, 0.95), q(v, 0.99), q(v, 1.0), v.len())
}

fn threads_of(args: &[String]) -> usize {
    arg(args, "--threads", &std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(2).max(2).to_string()).parse().unwrap_or(4)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

pub fn loadbench(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let lib = PathBuf::from(args.get(2).ok_or("라이브러리 폴더")?);
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let threads: usize = threads_of(args);
    let limit: usize = arg(args, "--limit", "100000").parse()?;
    let t = Instant::now();
    let entries = read_index(&lib)?;
    let idx_ms = t.elapsed().as_secs_f64() * 1e3;
    println!("색인 {}곡 읽기 {idx_ms:.1} ms (index.bin {:.1} MB)", entries.len(), mb(std::fs::metadata(lib.join("index.bin"))?.len()));
    let pm = PeakMem::start();
    let mut rows = Vec::new();
    let (mut open_v, mut plan_v, mut fill_v, mut all_v) = (vec![], vec![], vec![], vec![]);
    for e in entries.iter().take(limit) {
        let path = lib.join(&e.path);
        pm.reset();
        let base = private_bytes();
        let t0 = Instant::now();
        let b = Bundle::open(std::fs::read(&path)?)?;
        let t_open = t0.elapsed().as_secs_f64() * 1e3;
        let mut cache = SlotCache::new();
        let plan = match plan_chart_reuse(&b, 0, &LoadOpts { device_rate: rate, choice: vec![], threads, random_seed: Some(1), budget: crate::BUDGET }, &mut cache) {
            Ok(p) => p,
            Err(err) => {
                println!("  {}: 계획 실패 {err}", e.path);
                continue;
            }
        };
        let t_plan = t0.elapsed().as_secs_f64() * 1e3;
        fill_all(&plan, threads)?;
        let t_all = t0.elapsed().as_secs_f64() * 1e3;
        let bytes = plan.compiled.chart.bank.bytes() as u64;
        let peak = pm.peak().saturating_sub(base);
        open_v.push(t_open);
        plan_v.push(t_plan - t_open);
        fill_v.push(t_all - t_plan);
        all_v.push(t_all);
        rows.push(format!("{},{},{},{t_open:.1},{:.1},{:.1},{t_all:.1},{},{},{},{}", e.path, e.size, e.charts.first().map(|c| c.0.as_str()).unwrap_or(""), t_plan - t_open, t_all - t_plan, plan.jobs.len(), bytes, plan.peak_bytes, peak));
        drop(plan);
        drop(cache);
    }
    println!("곡 {}개, 장치 {rate} Hz, 스레드 {threads}", all_v.len());
    println!("  열기   {}", dist(&open_v));
    println!("  계획   {}", dist(&plan_v));
    println!("  디코딩 {}", dist(&fill_v));
    println!("  전체   {}", dist(&all_v));
    if let Some(out) = args.iter().position(|a| a == "--csv").and_then(|i| args.get(i + 1)) {
        std::fs::write(out, format!("path,size,chart,open_ms,plan_ms,fill_ms,all_ms,slots,pcm_bytes,plan_peak,peak_private\n{}\n", rows.join("\n")))?;
    }
    Ok(())
}

/// 미리보기 저장소 하나를 띄운다 (선곡 화면과 같은 경로: 열기 → 계획(합집합 캐시 없음) → 저장소)
fn open_store(lib: &Path, e: &IndexEntry, t0: i64, rate: u32, threads: usize, gen: &Arc<AtomicU64>) -> Result<Store, String> {
    let g = gen.load(Ordering::Acquire);
    let b = Bundle::open(std::fs::read(lib.join(&e.path)).map_err(|x| x.to_string())?)?;
    let plan = plan_chart_reuse(&b, 0, &LoadOpts { device_rate: rate, choice: vec![], threads, random_seed: None, budget: crate::BUDGET }, &mut SlotCache::new())?;
    Ok(Store::start(plan, t0, threads, gen.clone(), g))
}

/// 선곡 화면처럼 회수를 다른 스레드에 맡긴다 (다음 곡은 바로 연다). 결과는 `finish`에서 모은다.
fn cancel_async(s: Store, pending: &mut Vec<std::thread::JoinHandle<(f64, usize)>>) {
    pending.push(std::thread::spawn(move || cancel_join(s)));
}

fn finish(pending: &mut Vec<std::thread::JoinHandle<(f64, usize)>>) -> (Vec<f64>, usize) {
    let (mut ms, mut done) = (vec![], 0);
    for h in pending.drain(..) {
        if let Ok((m, d)) = h.join() {
            ms.push(m);
            done += d;
        }
    }
    (ms, done)
}

/// 취소하고 작업자가 모두 멈출 때까지 (회수 시간 ms, 그때까지 게시된 슬롯 수)
fn cancel_join(s: Store) -> (f64, usize) {
    let t = Instant::now();
    let done = s.done.load(Ordering::Acquire);
    s.cancel();
    s.join();
    (t.elapsed().as_secs_f64() * 1e3, done)
}

fn wait_ready(s: &Store, t0: i64, w: i64, limit: Duration) -> Option<f64> {
    let t = Instant::now();
    loop {
        if s.ready_at(t0, w) {
            return Some(t.elapsed().as_secs_f64() * 1e3);
        }
        if t.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
}

pub fn prevsweep(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let lib = PathBuf::from(args.get(2).ok_or("라이브러리 폴더")?);
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let threads = threads_of(args);
    let ws: Vec<f64> = arg(args, "--w", "0.5,1,2,4").split(',').filter_map(|x| x.parse().ok()).collect();
    let debs: Vec<u64> = arg(args, "--debounce", "75,150,250").split(',').filter_map(|x| x.parse().ok()).collect();
    let n: usize = arg(args, "--songs", "30").parse()?;
    let filter = arg(args, "--only", "");
    let mut rng = Rng(arg(args, "--seed", "1").parse::<u64>()?.max(1) * 0x9E37_79B9_7F4A_7C15);
    let entries: Vec<IndexEntry> = read_index(&lib)?.into_iter().filter(|e| filter.is_empty() || e.path.contains(&filter)).collect();
    let picks: Vec<usize> = (0..n).map(|_| (rng.next() % entries.len() as u64) as usize).collect();
    let gen = Arc::new(AtomicU64::new(0));
    let pm = PeakMem::start();
    println!("미리보기 첫 출력 (머문 순간 → 준비 집합 완료, 오디오 열기 제외), 곡 {n}개 무작위(씨앗 고정), 스레드 {threads}");
    // settle·late: W마다 곡을 처음부터 다시 연다 (W는 준비 판정에만 쓰인다 → 같은 저장소로 여러 W를 함께 잰다)
    for (name, frac) in [("settle", -1.0), ("late", 0.7)] {
        let mut per_w: Vec<Vec<f64>> = vec![vec![]; ws.len()];
        let mut open_ms = vec![];
        pm.reset();
        let base = private_bytes();
        for &i in &picks {
            let e = &entries[i];
            let hl = e.charts.first().map(|c| c.3.max(0)).unwrap_or(0);
            let len = e.charts.first().map(|c| c.4.len() as i64 * 250_000_000).unwrap_or(0);
            let t0 = if frac < 0.0 { hl } else { (len as f64 * frac) as i64 };
            let t = Instant::now();
            let s = open_store(&lib, e, t0, rate, threads, &gen)?;
            open_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let mut pending: Vec<usize> = (0..ws.len()).collect();
            let start = Instant::now();
            while !pending.is_empty() && start.elapsed() < Duration::from_secs(30) {
                pending.retain(|&k| {
                    if s.ready_at(t0, (ws[k] * 1e9) as i64) {
                        per_w[k].push(t.elapsed().as_secs_f64() * 1e3);
                        false
                    } else {
                        true
                    }
                });
                std::thread::sleep(Duration::from_micros(500));
            }
            cancel_join(s);
            gen.fetch_add(1, Ordering::AcqRel);
        }
        println!("[{name}] 열기+계획 {}", dist(&open_ms));
        for (k, w) in ws.iter().enumerate() {
            for &d in &debs {
                let v: Vec<f64> = per_w[k].iter().map(|x| x + d as f64).collect();
                println!("  W {w:>3} s · 디바운스 {d:>3} ms: {}", dist(&v));
            }
        }
        println!("  최고 사적 메모리 +{:.0} MB", mb(pm.peak().saturating_sub(base)));
    }
    // scroll: 곡 5개를 간격 I로 지나 6번째에 머문다. 디바운스보다 오래 머문 곡은 저장소를 띄웠다가 다음 이동 때 취소.
    for &deb in &debs {
        for interval in [100u64, 200, 300] {
            let (mut first, mut started, mut timeouts) = (vec![], 0usize, 0usize);
            let mut pending = vec![];
            pm.reset();
            let base = private_bytes();
            for chunk in picks.chunks(6).filter(|c| c.len() == 6) {
                let mut live: Option<Store> = None;
                for (k, &i) in chunk.iter().enumerate() {
                    let e = &entries[i];
                    let t0 = e.charts.first().map(|c| c.3.max(0)).unwrap_or(0);
                    if let Some(s) = live.take() {
                        gen.fetch_add(1, Ordering::AcqRel);
                        cancel_async(s, &mut pending);
                    }
                    let last = k == 5;
                    let dwell = Duration::from_millis(if last { deb } else { interval });
                    if !last && interval < deb {
                        std::thread::sleep(dwell); // 디바운스 전에 떠남 → 작업 없음
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(deb));
                    let t = Instant::now();
                    let s = open_store(&lib, e, t0, rate, threads, &gen)?;
                    let open_ms = t.elapsed().as_secs_f64() * 1e3;
                    started += 1;
                    if last {
                        // 첫 출력 = 디바운스 + 열기·계획 + 준비 집합까지
                        match wait_ready(&s, t0, 2_000_000_000, Duration::from_secs(30)) {
                            Some(ms) => first.push(deb as f64 + open_ms + ms),
                            None => timeouts += 1,
                        }
                        gen.fetch_add(1, Ordering::AcqRel);
                        cancel_async(s, &mut pending);
                    } else {
                        std::thread::sleep(Duration::from_millis(interval - deb));
                        live = Some(s);
                    }
                }
            }
            let peak = pm.peak().saturating_sub(base);
            let (reclaim, wasted) = finish(&mut pending);
            println!(
                "[scroll I {interval} ms · 디바운스 {deb} ms, W 2 s, 회수 겹침] 첫 출력 {} · 시간 초과 {timeouts} · 띄운 저장소 {started} · 버린 슬롯 {wasted} · 취소 회수 {} · 최고 메모리 +{:.0} MB",
                dist(&first),
                dist(&reclaim),
                mb(peak)
            );
        }
    }
    // drag: 같은 곡에서 2초 동안 100 ms마다 탐색(순서만 다시) 뒤 멈춤 → 멈춘 지점 준비까지
    let mut drag = vec![];
    for &i in picks.iter().take(10) {
        let e = &entries[i];
        let len = e.charts.first().map(|c| c.4.len() as i64 * 250_000_000).unwrap_or(0).max(1);
        let s = open_store(&lib, e, 0, rate, threads, &gen)?;
        let mut t0 = 0;
        for k in 1..=20 {
            t0 = len * k / 25;
            s.reorder(t0);
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(ms) = wait_ready(&s, t0, 2_000_000_000, Duration::from_secs(30)) {
            drag.push(ms);
        }
        gen.fetch_add(1, Ordering::AcqRel);
        cancel_join(s);
    }
    println!("[drag 2초, 100 ms마다 탐색 → 멈춘 지점 준비까지, W 2 s] {}", dist(&drag));
    Ok(())
}

pub fn memtest(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let lib = PathBuf::from(args.get(2).ok_or("라이브러리 폴더")?);
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let threads = threads_of(args);
    let switches: usize = arg(args, "--switches", "20").parse()?;
    let interval: u64 = arg(args, "--interval-ms", "50").parse()?;
    let entries = read_index(&lib)?;
    let gen = Arc::new(AtomicU64::new(0));
    let pm = PeakMem::start();
    let base = private_bytes();
    println!("시작 사적 메모리 {:.0} MB, 예산 {:.0} MB", mb(base), mb(crate::BUDGET));
    // 1) 빠른 곡 전환: 큰 곡 위주로(파일 크기 상위) 저장소를 띄우고 interval 뒤 취소 → 다음 곡
    let mut big: Vec<&IndexEntry> = entries.iter().collect();
    big.sort_by_key(|e| std::cmp::Reverse(e.size));
    // 선곡 화면처럼: 선택 사건은 절대 시각(interval 간격)으로, 이전 저장소 회수는 다른 스레드에 맡기고 바로 다음 곡을 연다 (리뷰 3-5 #8)
    let mut pending = vec![];
    let mut live: Option<Store> = None;
    let t_start = Instant::now();
    for (k, e) in big.iter().cycle().take(switches).enumerate() {
        let at = t_start + Duration::from_millis(interval * k as u64);
        if let Some(d) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        if let Some(s) = live.take() {
            gen.fetch_add(1, Ordering::AcqRel);
            cancel_async(s, &mut pending);
        }
        live = Some(open_store(&lib, e, 0, rate, threads, &gen)?);
    }
    std::thread::sleep(Duration::from_millis(interval));
    if let Some(s) = live.take() {
        gen.fetch_add(1, Ordering::AcqRel);
        cancel_async(s, &mut pending);
    }
    let (reclaim, _) = finish(&mut pending);
    std::thread::sleep(Duration::from_millis(200));
    println!(
        "[빠른 곡 전환 {switches}번, {interval} ms 간격(절대 시각), 큰 곡부터, 회수 겹침] 취소 회수 {} · 최고 +{:.0} MB · 끝난 뒤 +{:.0} MB",
        dist(&reclaim),
        mb(pm.peak().saturating_sub(base)),
        mb(private_bytes().saturating_sub(base))
    );
    // 2) 한 곡 안 차분 순회: 차분이 가장 많은 곡, 같은 저장소(합집합)로 모든 차분을 차례로 다 채운다
    let many = entries.iter().max_by_key(|e| (e.charts.len(), e.size)).unwrap();
    pm.reset();
    let b = Bundle::open(std::fs::read(lib.join(&many.path))?)?;
    let n = engine::load::chart_list(&b)?.len();
    let mut cache = SlotCache::new();
    let mut union = vec![];
    for ci in 0..n {
        let plan = plan_chart_reuse(&b, ci, &LoadOpts { device_rate: rate, choice: vec![], threads, random_seed: Some(1), budget: crate::BUDGET }, &mut cache)?;
        fill_all(&plan, threads)?;
        let u: u64 = cache.values().filter_map(|s| s.get()).map(|p| p.bytes() as u64).sum();
        union.push(u);
        // 선곡 화면과 같은 규칙: 합집합이 예산 절반을 넘으면 비운다
        if u > crate::BUDGET / 2 {
            cache = SlotCache::new();
        }
    }
    println!(
        "[차분 순회] {} 차분 {n}개: 합집합 {} MB · 최고 +{:.0} MB",
        many.title,
        union.iter().map(|u| format!("{:.0}", mb(*u))).collect::<Vec<_>>().join("→"),
        mb(pm.peak().saturating_sub(base))
    );
    drop(cache);
    // 3) 장치 레이트 바꾸기: 같은 곡을 44.1k로 채운 저장소와 48k로 채운 저장소 (캐시 열쇠에 레이트 → 다시 디코딩)
    pm.reset();
    let e = big[0];
    let b = Bundle::open(std::fs::read(lib.join(&e.path))?)?;
    let mut cache = SlotCache::new();
    let mut out = vec![];
    for r in [44_100u32, 48_000] {
        let t = Instant::now();
        let plan = plan_chart_reuse(&b, 0, &LoadOpts { device_rate: r, choice: vec![], threads, random_seed: Some(1), budget: crate::BUDGET }, &mut cache)?;
        let fresh = plan.jobs.len();
        fill_all(&plan, threads)?;
        out.push(format!("{r} Hz: 새로 디코딩 {fresh}슬롯 {:.0} ms, PCM {:.0} MB", t.elapsed().as_secs_f64() * 1e3, mb(plan.compiled.chart.bank.bytes() as u64)));
    }
    println!("[장치 레이트 바꾸기] {} — {} · 최고 +{:.0} MB", e.title, out.join(" / "), mb(pm.peak().saturating_sub(base)));
    Ok(())
}

pub fn partcheck(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use engine::load::{decode_slot, decode_slot_partial};
    use engine::resample::Filters;
    use engine::runtime::Pcm2;
    let lib = PathBuf::from(args.get(2).ok_or("라이브러리 폴더")?);
    let rate: u32 = arg(args, "--rate", "48000").parse()?;
    let limit: usize = arg(args, "--limit", "60").parse()?;
    let entries = read_index(&lib)?;
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let filters = Filters::default();
    let (mut slots, mut ranges, mut bad, mut part_ms, mut full_ms) = (0usize, 0usize, 0usize, vec![], vec![]);
    let step = (entries.len() / limit.max(1)).max(1);
    for e in entries.iter().step_by(step).take(limit) {
        let b = Bundle::open(std::fs::read(lib.join(&e.path))?)?;
        let plan = engine::load::plan_chart(&b, 0, &LoadOpts { device_rate: rate, choice: vec![], threads: 1, random_seed: Some(1), budget: crate::BUDGET })?;
        for j in &plan.jobs {
            let p = &plan.datas[&j.hash];
            if p.codec != track::audio::Codec::Vorbis || p.frames < 4 * p.rate as u64 {
                continue;
            }
            let t = Instant::now();
            let pcm = track::audio::decode(p)?;
            let full = decode_slot(p, &pcm, j.start, j.length, rate, &filters)?;
            full_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let Pcm2::F32(fx) = &full else { continue };
            let n = (fx.len() / 2) as i64;
            slots += 1;
            for k in 0..10 {
                let from = match k {
                    0 => 0,
                    1 => n - 1000.min(n),
                    2 => n - 1,
                    3 => 1,
                    _ => (rng.next() % n as u64) as i64,
                };
                let to = if k == 3 { 2 } else { (from + (rate as i64 * 8)).min(n) };
                let t = Instant::now();
                let pt = decode_slot_partial(p, j.start, j.length, rate, &filters, from, to, &|| false)?;
                part_ms.push(t.elapsed().as_secs_f64() * 1e3);
                let Pcm2::F32(px) = &pt.pcm else { bad += 1; continue };
                ranges += 1;
                let want = &fx[from as usize * 2..to as usize * 2];
                if pt.from != from || px.len() != want.len() || px.iter().zip(want).any(|(a, b)| a.to_bits() != b.to_bits()) {
                    bad += 1;
                    println!("  다름: {} 슬롯 {} [{from}, {to})", e.path, j.slot);
                }
            }
        }
    }
    println!("긴 Vorbis 슬롯 {slots}개, 구간 {ranges}개 (8초씩, 장치 {rate} Hz): 다름 {bad}");
    println!("  전체 디코딩 {}", dist(&full_ms));
    println!("  부분 디코딩 {}", dist(&part_ms));
    if bad > 0 {
        return Err(format!("부분 디코딩 ≠ 전체 디코딩: {bad}구간").into());
    }
    Ok(())
}

/// 실행 폴더의 차분 노트 소리 목록 (싱크 분석기가 키음을 노트·소리별로 일대일로 짝짓는 데 쓴다, 리뷰 3-5 #2)
pub fn notes(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (l, _) = crate::load_run(std::path::Path::new(args.get(2).ok_or("실행 폴더")?))?;
    let c = &l.compiled.chart;
    println!("lane,note,t_ns,offset_ns,sound");
    for (li, lane) in c.lanes.iter().enumerate() {
        for (ni, n) in lane.notes.iter().enumerate() {
            for x in &c.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
                println!("{li},{ni},{},{},{}", n.t, x.offset, x.sound);
            }
        }
    }
    Ok(())
}
