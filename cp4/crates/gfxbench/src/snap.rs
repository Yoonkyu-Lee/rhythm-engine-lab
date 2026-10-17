//! CP4 실험 D1: 엔진(오디오·판정) 스레드가 쓰는 상태를 화면 스레드가 어떻게 읽어야 쓰는 쪽이 절대 기다리지 않나.
//!
//! snapbench <secs>
//!   쓰는 쪽 = 1 ms마다 상태(판정 사건·점수·레인 불빛 등, 4 KiB)를 고쳐 내놓는 스레드 (오디오 콜백 흉내, MMCSS Pro Audio)
//!   읽는 쪽 = 화면 스레드 흉내: 쉬지 않고 최신 상태를 통째로 복사 (최악의 경우: 프레임 상한 없음)
//!   방식 세 가지: (1) Mutex (2) 삼중 버퍼(원자 교환, 무잠금) (3) seqlock(쓰는 쪽 무잠금, 읽는 쪽 재시도)
//!   잼: 쓰는 쪽 한 번에 걸린 시간 p50·p99·p99.9·최대, 읽는 쪽 초당 복사 수, 찢어진(일관되지 않은) 읽기 수

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use synchronizer::qpc;

const WORDS: usize = 512; // 4 KiB

#[derive(Clone, Copy)]
struct State {
    w: [u64; WORDS],
}

impl State {
    fn new() -> State {
        let mut s = State { w: [0; WORDS] };
        s.fill(0);
        s
    }
    fn fill(&mut self, gen: u64) {
        for (i, v) in self.w.iter_mut().enumerate() {
            *v = gen.wrapping_mul(0x9E37_79B9).wrapping_add(i as u64);
        }
    }
    /// 일관성 검사: 모든 칸이 같은 세대에서 나왔나
    fn ok(&self) -> bool {
        let g = self.w[0];
        self.w.iter().enumerate().all(|(i, v)| *v == g.wrapping_add(i as u64))
    }
}

trait Chan: Send + Sync {
    /// 게시. 건너뛰었으면(try_lock 실패) false
    fn publish(&self, gen: u64) -> bool;
    fn read(&self, out: &mut State) -> bool;
}

struct Locked(Mutex<State>);
impl Chan for Locked {
    fn publish(&self, gen: u64) -> bool {
        self.0.lock().unwrap().fill(gen);
        true
    }
    fn read(&self, out: &mut State) -> bool {
        *out = *self.0.lock().unwrap();
        out.ok()
    }
}

/// CP3 방식: 쓰는 쪽은 try_lock — 기다리지 않는 대신, 화면이 잡고 있으면 그 게시를 건너뛴다
struct TryLocked(Mutex<State>);
impl Chan for TryLocked {
    fn publish(&self, gen: u64) -> bool {
        match self.0.try_lock() {
            Ok(mut g) => {
                g.fill(gen);
                true
            }
            Err(_) => false,
        }
    }
    fn read(&self, out: &mut State) -> bool {
        *out = *self.0.lock().unwrap();
        out.ok()
    }
}

/// 삼중 버퍼: 쓰는 쪽은 자기 칸에 쓰고 '가운데' 칸과 맞바꾼다. 읽는 쪽은 새 것이 있으면 가운데와 자기 칸을 맞바꾼다. 둘 다 기다리지 않는다
struct Triple {
    buf: [UnsafeCell<State>; 3],
    mid: AtomicUsize, // 칸 번호 | 새것 표시(4)
    w_idx: UnsafeCell<usize>,
    r_idx: UnsafeCell<usize>,
}
unsafe impl Sync for Triple {}
impl Chan for Triple {
    fn publish(&self, gen: u64) -> bool {
        unsafe {
            let wi = *self.w_idx.get();
            (*self.buf[wi].get()).fill(gen);
            let old = self.mid.swap(wi | 4, Ordering::AcqRel);
            *self.w_idx.get() = old & 3;
        }
        true
    }
    fn read(&self, out: &mut State) -> bool {
        unsafe {
            if self.mid.load(Ordering::Relaxed) & 4 != 0 {
                let ri = *self.r_idx.get();
                let old = self.mid.swap(ri, Ordering::AcqRel);
                *self.r_idx.get() = old & 3;
            }
            *out = *self.buf[*self.r_idx.get()].get();
            out.ok()
        }
    }
}

/// seqlock: 쓰는 쪽은 번호를 홀수로 → 쓰기 → 짝수로. 읽는 쪽은 번호가 바뀌었으면 다시 읽는다
struct Seq {
    seq: AtomicU64,
    data: UnsafeCell<State>,
}
unsafe impl Sync for Seq {}
impl Chan for Seq {
    fn publish(&self, gen: u64) -> bool {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s + 1, Ordering::Relaxed);
        fence(Ordering::Release);
        unsafe {
            let d = self.data.get();
            for (i, v) in (*d).w.iter_mut().enumerate() {
                std::ptr::write_volatile(v, gen.wrapping_mul(0x9E37_79B9).wrapping_add(i as u64));
            }
        }
        self.seq.store(s + 2, Ordering::Release);
        true
    }
    fn read(&self, out: &mut State) -> bool {
        loop {
            let a = self.seq.load(Ordering::Acquire);
            if a & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            unsafe {
                let d = self.data.get();
                for (i, v) in out.w.iter_mut().enumerate() {
                    *v = std::ptr::read_volatile(&(*d).w[i]);
                }
            }
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == a {
                return out.ok();
            }
        }
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// `paced` = 읽는 쪽이 240 Hz로 한 번씩만 복사 (실제 화면), 아니면 쉬지 않고 (최악)
fn run(name: &str, ch: Arc<dyn Chan>, secs: f64, readers: usize, paced: bool) -> String {
    let stop = Arc::new(AtomicBool::new(false));
    let f = qpc::frequency();
    let mut rs = vec![];
    for _ in 0..readers {
        let (ch, stop) = (ch.clone(), stop.clone());
        rs.push(std::thread::spawn(move || {
            let mut st = State { w: [0; WORDS] };
            let (mut n, mut torn) = (0u64, 0u64);
            let mut next = qpc::now();
            while !stop.load(Ordering::Relaxed) {
                if paced {
                    next += qpc::frequency() * 10 / 2400;
                    while qpc::now() < next {
                        std::thread::yield_now();
                    }
                }
                if !ch.read(&mut st) {
                    torn += 1;
                }
                n += 1;
            }
            (n, torn)
        }));
    }
    let wch = ch.clone();
    let w = std::thread::spawn(move || {
        unsafe {
            let mut task = 0u32;
            let _ = windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task);
        }
        let mut cost = Vec::with_capacity(100_000);
        let end = qpc::now() + (secs * f as f64) as i64;
        let period = f / 1000;
        let mut next = qpc::now();
        let mut gen = 1u64;
        let mut skipped = 0u64;
        while qpc::now() < end {
            while qpc::now() < next {
                std::hint::spin_loop();
            }
            let a = qpc::now();
            if !wch.publish(gen) {
                skipped += 1;
            }
            cost.push((qpc::now() - a) as f64 * 1e6 / f as f64);
            gen += 1;
            next += period;
        }
        (cost, skipped)
    });
    let (mut cost, skipped) = w.join().unwrap();
    stop.store(true, Ordering::Relaxed);
    let (mut n, mut torn) = (0, 0);
    for r in rs {
        let (a, b) = r.join().unwrap();
        n += a;
        torn += b;
    }
    let mx = cost.iter().cloned().fold(0.0, f64::max);
    format!(
        "{name},{readers},{paced},{},{:.2},{:.2},{:.2},{:.1},{:.0},{torn},{skipped}",
        cost.len(),
        pct(&mut cost, 0.5),
        pct(&mut cost, 0.99),
        pct(&mut cost, 0.999),
        mx,
        n as f64 / secs
    )
}

fn main() {
    let secs: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    println!("method,readers,paced,writes,write_us_p50,write_us_p99,write_us_p999,write_us_max,reads_per_s,torn,skipped");
    for paced in [true, false] {
        for readers in [1, 3] {
            if paced && readers == 3 {
                continue;
            }
            println!("{}", run("mutex", Arc::new(Locked(Mutex::new(State::new()))), secs, readers, paced));
            println!("{}", run("trylock", Arc::new(TryLocked(Mutex::new(State::new()))), secs, readers, paced));
            if readers == 1 {
                let t = Triple {
                    buf: std::array::from_fn(|_| UnsafeCell::new(State::new())),
                    mid: AtomicUsize::new(1),
                    w_idx: UnsafeCell::new(0),
                    r_idx: UnsafeCell::new(2),
                };
                println!("{}", run("triple", Arc::new(t), secs, 1, paced));
            }
            println!(
                "{}",
                run(
                    "seqlock",
                    Arc::new(Seq {
                        seq: AtomicU64::new(0),
                        data: UnsafeCell::new(State::new())
                    }),
                    secs,
                    readers,
                    paced
                )
            );
        }
    }
}
