//! 시계 모델을 스레드 사이에 잠금 없이 나눈다.
//!
//! 오디오 스레드 → (관측 링) → 시계 스레드 → (스냅샷) → 게임·입력 스레드
//!
//! - 관측 링 (`ObsProducer`/`ObsConsumer`): 크기를 미리 정한 단일 생산자-단일 소비자 링. 오디오 스레드는
//!   잠금도 할당도 없이 (프레임, 시각) 관측을 넣는다. 가득 차면 버리고 센다 (시계 스레드가 멈춘 경우뿐).
//! - 시계 스레드 (`spawn_clock_thread`): 링에서 꺼내 `ClockModel`을 갱신한다. 다시 맞추는 계산은 여기서만 한다.
//! - 스냅샷 (`SharedClock`): 맞춘 직선 (기준점, 절편, 기울기)을 seqlock으로 공개한다. 쓰는 쪽은 기다리지 않고,
//!   읽는 쪽은 쓰는 도중이면 다시 읽는다. 읽는 쪽은 직선만 받아 `frame_at`/`t100_at`을 스스로 계산한다.

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::clock::{ClockModel, ClockPolicy, Observe};

/// 맞춘 직선 하나. `ClockModel`의 계산 결과만 담으므로 복사해서 어디서든 쓸 수 있다.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockSnapshot {
    /// 관측이 하나라도 있어 쓸 수 있는가.
    pub valid: bool,
    pub epoch: u32,
    pub nominal_rate: f64,
    pub t0: i64,
    pub f0: f64,
    pub a: f64,
    pub b: f64,
}

impl ClockSnapshot {
    /// QPC 시각(100 ns)에 장치가 내보내고 있는 프레임 위치. `ClockModel::frame_at`과 같은 식.
    pub fn frame_at(&self, t100: i64) -> f64 {
        let x = (t100 - self.t0) as f64 * 1e-7;
        self.f0 + self.a + self.b * x
    }

    /// 장치가 프레임 위치 `frame`을 내보내는 QPC 시각(100 ns). `ClockModel::t100_at`과 같은 식.
    pub fn t100_at(&self, frame: f64) -> f64 {
        let x = (frame - self.f0 - self.a) / self.b;
        self.t0 as f64 + x * 1e7
    }
}

/// seqlock으로 공개하는 스냅샷. 쓰는 쪽은 하나(시계 스레드)뿐이어야 한다.
pub struct SharedClock {
    seq: AtomicU64,
    valid_epoch: AtomicU64, // 상위 32비트 valid, 하위 32비트 epoch
    nominal_rate: AtomicU64,
    t0: AtomicU64,
    f0: AtomicU64,
    a: AtomicU64,
    b: AtomicU64,
}

impl Default for SharedClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedClock {
    pub fn new() -> Self {
        SharedClock {
            seq: AtomicU64::new(0),
            valid_epoch: AtomicU64::new(0),
            nominal_rate: AtomicU64::new(0),
            t0: AtomicU64::new(0),
            f0: AtomicU64::new(0),
            a: AtomicU64::new(0),
            b: AtomicU64::new(0),
        }
    }

    /// 새 스냅샷을 공개한다. 쓰는 쪽이 둘 이상이면 안 된다 (시계 스레드 전용).
    pub fn publish(&self, s: &ClockSnapshot) {
        let n = self.seq.load(Ordering::Relaxed);
        self.seq.store(n + 1, Ordering::Relaxed); // 홀수: 쓰는 중
        fence(Ordering::Release);
        self.valid_epoch.store(((s.valid as u64) << 32) | s.epoch as u64, Ordering::Relaxed);
        self.nominal_rate.store(s.nominal_rate.to_bits(), Ordering::Relaxed);
        self.t0.store(s.t0 as u64, Ordering::Relaxed);
        self.f0.store(s.f0.to_bits(), Ordering::Relaxed);
        self.a.store(s.a.to_bits(), Ordering::Relaxed);
        self.b.store(s.b.to_bits(), Ordering::Relaxed);
        self.seq.store(n + 2, Ordering::Release); // 짝수: 다 씀
    }

    /// 일관된 스냅샷을 읽는다. 쓰는 도중이면 다시 읽는다 (쓰기는 수십 ns라 바로 끝난다).
    pub fn load(&self) -> ClockSnapshot {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let ve = self.valid_epoch.load(Ordering::Relaxed);
            let snap = ClockSnapshot {
                valid: ve >> 32 != 0,
                epoch: ve as u32,
                nominal_rate: f64::from_bits(self.nominal_rate.load(Ordering::Relaxed)),
                t0: self.t0.load(Ordering::Relaxed) as i64,
                f0: f64::from_bits(self.f0.load(Ordering::Relaxed)),
                a: f64::from_bits(self.a.load(Ordering::Relaxed)),
                b: f64::from_bits(self.b.load(Ordering::Relaxed)),
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return snap;
            }
        }
    }
}

struct Ring {
    slots: Box<[UnsafeCell<(u64, i64)>]>,
    head: AtomicUsize, // 다음에 꺼낼 위치 (소비자만 씀)
    tail: AtomicUsize, // 다음에 넣을 위치 (생산자만 씀)
    dropped: AtomicU64,
    producer_alive: AtomicBool,
}

// 슬롯 하나는 생산자가 쓴 뒤(tail Release) 소비자가 읽고(tail Acquire), 소비자가 비운 뒤(head Release) 생산자가 다시 쓴다.
unsafe impl Sync for Ring {}

/// 관측 링의 넣는 쪽. 오디오 스레드가 하나만 가진다.
pub struct ObsProducer {
    ring: Arc<Ring>,
}

/// 관측 링의 꺼내는 쪽. 시계 스레드가 하나만 가진다.
pub struct ObsConsumer {
    ring: Arc<Ring>,
}

/// 용량 `capacity`(2의 거듭제곱으로 올림)인 관측 링을 만든다.
pub fn obs_ring(capacity: usize) -> (ObsProducer, ObsConsumer) {
    let cap = capacity.max(2).next_power_of_two();
    let ring = Arc::new(Ring {
        slots: (0..cap).map(|_| UnsafeCell::new((0, 0))).collect(),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
        producer_alive: AtomicBool::new(true),
    });
    (ObsProducer { ring: ring.clone() }, ObsConsumer { ring })
}

impl ObsProducer {
    /// 관측 하나를 넣는다. 가득 찼으면 버리고 false. 잠금·할당·시스템 호출 없음.
    pub fn push(&mut self, frame: u64, t100: i64) -> bool {
        let r = &*self.ring;
        let tail = r.tail.load(Ordering::Relaxed);
        let head = r.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) == r.slots.len() {
            r.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        unsafe { *r.slots[tail & (r.slots.len() - 1)].get() = (frame, t100) };
        r.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }
}

impl Drop for ObsProducer {
    fn drop(&mut self) {
        self.ring.producer_alive.store(false, Ordering::Release);
    }
}

impl ObsConsumer {
    pub fn pop(&mut self) -> Option<(u64, i64)> {
        let r = &*self.ring;
        let head = r.head.load(Ordering::Relaxed);
        if head == r.tail.load(Ordering::Acquire) {
            return None;
        }
        let v = unsafe { *r.slots[head & (r.slots.len() - 1)].get() };
        r.head.store(head.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    /// 생산자가 사라졌고 링이 비었는가.
    pub fn finished(&self) -> bool {
        let r = &*self.ring;
        !r.producer_alive.load(Ordering::Acquire) && r.head.load(Ordering::Relaxed) == r.tail.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> u64 {
        self.ring.dropped.load(Ordering::Relaxed)
    }
}

/// 시계 스레드가 끝날 때 돌려주는 집계.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockStats {
    pub observed: u64,
    pub published: u64,
    pub new_epochs: u32,
    /// 링이 가득 차서 버린 관측 수 (0이어야 정상).
    pub dropped: u64,
}

/// 시계 스레드를 띄운다. 관측을 링에서 꺼내 모델을 갱신하고, 바뀔 때마다 `shared`에 공개한다.
/// 생산자(`ObsProducer`)가 사라지고 링이 비면 끝난다.
pub fn spawn_clock_thread(nominal_rate: f64, policy: ClockPolicy, mut rx: ObsConsumer, shared: Arc<SharedClock>) -> JoinHandle<ClockStats> {
    thread::spawn(move || {
        let mut m = ClockModel::new(nominal_rate, policy);
        let mut st = ClockStats::default();
        shared.publish(&m.snapshot());
        loop {
            let mut changed = false;
            while let Some((f, t)) = rx.pop() {
                st.observed += 1;
                match m.observe(f, t) {
                    Observe::Accepted => changed = true,
                    Observe::NewEpoch(_) => {
                        st.new_epochs += 1;
                        changed = true;
                    }
                    Observe::Suspect(_) => {}
                }
            }
            if changed {
                shared.publish(&m.snapshot());
                st.published += 1;
            }
            if rx.finished() {
                break;
            }
            // 관측은 버퍼 주기(1.45–10 ms)마다 온다. 1 ms 잠들어도 모델 갱신이 1 ms 늦을 뿐이다.
            thread::sleep(Duration::from_millis(1));
        }
        st.dropped = rx.dropped();
        st
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_order_across_threads() {
        let (mut p, mut c) = obs_ring(64);
        let h = thread::spawn(move || {
            let mut i = 0u64;
            while i < 100_000 {
                if p.push(i, i as i64 * 2) {
                    i += 1;
                }
            }
        });
        let mut next = 0u64;
        while next < 100_000 {
            if let Some((f, t)) = c.pop() {
                assert_eq!((f, t), (next, next as i64 * 2));
                next += 1;
            }
        }
        h.join().unwrap();
        assert!(c.finished());
    }

    #[test]
    fn ring_drops_when_full() {
        let (mut p, c) = obs_ring(4);
        for i in 0..4 {
            assert!(p.push(i, 0));
        }
        assert!(!p.push(9, 0));
        assert_eq!(c.dropped(), 1);
    }

    #[test]
    fn snapshot_matches_model() {
        let mut m = ClockModel::new(44100.0, ClockPolicy::default());
        for i in 0..500u64 {
            m.observe(i * 441 + 1000, 7_000_000_000 + i as i64 * 100_000);
        }
        let shared = SharedClock::new();
        shared.publish(&m.snapshot());
        let s = shared.load();
        assert!(s.valid);
        for t in [7_000_000_000i64, 7_003_000_000, 7_100_000_000] {
            assert_eq!(s.frame_at(t), m.frame_at(t));
        }
        assert_eq!(s.t100_at(30_000.0), m.t100_at(30_000.0));
    }

    #[test]
    fn readers_never_see_torn_snapshots() {
        // 쓰는 쪽은 a = b = f0 = k 인 스냅샷만 공개한다. 읽은 값이 서로 다르면 찢어진 것.
        let shared = Arc::new(SharedClock::new());
        let stop = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (s, stop) = (shared.clone(), stop.clone());
                thread::spawn(move || {
                    let mut n = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let x = s.load();
                        assert!(x.a == x.b && x.b == x.f0 && x.t0 as f64 == x.a, "torn: {x:?}");
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        for k in 0..200_000u64 {
            let v = k as f64;
            shared.publish(&ClockSnapshot { valid: true, epoch: 0, nominal_rate: 1.0, t0: k as i64, f0: v, a: v, b: v });
        }
        stop.store(true, Ordering::Relaxed);
        for r in readers {
            assert!(r.join().unwrap() > 0);
        }
    }

    #[test]
    fn clock_thread_publishes_and_stops() {
        let (mut p, c) = obs_ring(1024);
        let shared = Arc::new(SharedClock::new());
        let h = spawn_clock_thread(44100.0, ClockPolicy::default(), c, shared.clone());
        for i in 0..300u64 {
            while !p.push(i * 441, i as i64 * 100_000) {}
        }
        drop(p);
        let st = h.join().unwrap();
        assert_eq!(st.observed, 300);
        assert_eq!(st.dropped, 0);
        let s = shared.load();
        assert!(s.valid);
        assert!((s.frame_at(299 * 100_000) - 299.0 * 441.0).abs() < 1.0);
    }
}
