//! 잠금 없는 단일 생산자·단일 소비자(SPSC) 고정 크기 링.
//!
//! 실시간 스레드(오디오 콜백, 판정)가 잠금·할당 없이 다른 스레드로 값을 넘길 때 쓴다.
//! 가득 차면 넣지 않고 `false`를 돌려준다 (생산자가 버린 개수를 직접 센다). 소비자 인덱스는 생산자가 건드리지 않는다.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Shared<T> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: AtomicUsize, // 다음에 쓸 위치 (생산자만 씀)
    tail: AtomicUsize, // 다음에 읽을 위치 (소비자만 씀)
}

// 한 칸은 생산자가 쓰고 head를 공개한 뒤에만 소비자가 읽고, 소비자가 tail을 공개한 뒤에만 생산자가 다시 쓴다.
unsafe impl<T: Send> Sync for Shared<T> {}
unsafe impl<T: Send> Send for Shared<T> {}

pub struct Producer<T> {
    s: Arc<Shared<T>>,
    head: usize,
    tail_cache: usize,
}

pub struct Consumer<T> {
    s: Arc<Shared<T>>,
    tail: usize,
}

/// 용량은 2의 거듭제곱으로 올림한다.
pub fn ring<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(2).next_power_of_two();
    let buf = (0..cap).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect::<Vec<_>>().into_boxed_slice();
    let s = Arc::new(Shared { buf, mask: cap - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { s: s.clone(), head: 0, tail_cache: 0 }, Consumer { s, tail: 0 })
}

impl<T: Copy + Send> Producer<T> {
    pub fn push(&mut self, v: T) -> bool {
        let cap = self.s.mask + 1;
        if self.head - self.tail_cache == cap {
            self.tail_cache = self.s.tail.load(Ordering::Acquire);
            if self.head - self.tail_cache == cap {
                return false;
            }
        }
        unsafe { (*self.s.buf[self.head & self.s.mask].get()).write(v) };
        self.head += 1;
        self.s.head.store(self.head, Ordering::Release);
        true
    }

    pub fn capacity(&self) -> usize {
        self.s.mask + 1
    }
}

impl<T: Copy + Send> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        if self.tail == self.s.head.load(Ordering::Acquire) {
            return None;
        }
        let v = unsafe { (*self.s.buf[self.tail & self.s.mask].get()).assume_init() };
        self.tail += 1;
        self.s.tail.store(self.tail, Ordering::Release);
        Some(v)
    }

    /// 생산자가 사라졌는가 (남은 값은 아직 읽을 수 있다).
    pub fn producer_gone(&self) -> bool {
        Arc::strong_count(&self.s) == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_and_full() {
        let (mut p, mut c) = ring::<u32>(3); // → 4
        assert!(p.push(1) && p.push(2) && p.push(3) && p.push(4));
        assert!(!p.push(5));
        assert_eq!(c.pop(), Some(1));
        assert!(p.push(5));
        assert_eq!((c.pop(), c.pop(), c.pop(), c.pop(), c.pop()), (Some(2), Some(3), Some(4), Some(5), None));
    }

    #[test]
    fn threads_keep_order() {
        let (mut p, mut c) = ring::<u64>(64);
        let n = 200_000u64;
        let t = std::thread::spawn(move || {
            let mut i = 0;
            while i < n {
                if p.push(i) {
                    i += 1;
                }
            }
        });
        let mut want = 0;
        while want < n {
            if let Some(v) = c.pop() {
                assert_eq!(v, want);
                want += 1;
            }
        }
        t.join().unwrap();
        assert!(c.producer_gone());
    }
}
