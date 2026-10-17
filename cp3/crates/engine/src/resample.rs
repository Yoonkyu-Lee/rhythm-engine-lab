//! 소리를 장치 속도로 바꾼다 (곡을 불러올 때, 제어 경로에서). CP2 rcore/resample.rs에서 가져와 소리 하나 단위로 바꿨다.
//!
//! 윈도 sinc 다상 필터, 정확한 유리수 비율 L/M (예: 44.1 → 48 kHz = 160/147). 소리 경계 밖은 0. 결정적이다 (같은 입력 → 같은 비트).
//! 위상 수 상한을 8192 → 196,608로 올렸다: 30,697 Hz 같은 비표준 속도(예시 Track)는 48 kHz로 L = 48,000이 된다.
//! 표 크기는 L × 64탭 × 4 B (L = 48,000이면 약 12 MB)이고, 같은 (원래 속도, 장치 속도)는 한 번만 만든다.

use std::collections::HashMap;
use std::sync::Arc;

/// 필터 반폭(탭 수 = 2 × HALF). 차단은 나이퀴스트의 95% (낮은 쪽 속도 기준).
const HALF: usize = 32;
const CUTOFF: f64 = 0.95;
pub const MAX_PHASES: usize = 196_608;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// 리샘플링 뒤 프레임 수 (필터를 만들지 않고): 올림(frames × L / M).
pub fn out_frames(frames: u64, from: u32, to: u32) -> u64 {
    let g = gcd(from as u64, to as u64);
    let (l, m) = (to as u64 / g, from as u64 / g);
    (frames * l + m - 1) / m
}

pub struct Filter {
    l: usize,
    m: usize,
    /// taps[p][k], k = 0..2*HALF, 입력 위치 i + k − (HALF − 1)에 곱한다.
    taps: Vec<f32>,
}

impl Filter {
    pub fn new(from: u32, to: u32) -> Result<Filter, String> {
        let g = gcd(from as u64, to as u64);
        let (l, m) = ((to as u64 / g) as usize, (from as u64 / g) as usize);
        if l > MAX_PHASES {
            return Err(format!("{from} → {to} Hz: 위상 수 {l}이 너무 큼"));
        }
        let fc = 0.5 * CUTOFF * (l as f64 / m as f64).min(1.0);
        let n = 2 * HALF;
        let mut taps = vec![0f32; l * n];
        for p in 0..l {
            let frac = p as f64 / l as f64;
            let mut sum = 0.0;
            let row = &mut taps[p * n..(p + 1) * n];
            for (k, t) in row.iter_mut().enumerate() {
                let x = k as f64 - (HALF as f64 - 1.0) - frac;
                let s = if x.abs() < 1e-12 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                let u = (x / HALF as f64 + 1.0) / 2.0;
                let w = if (0.0..=1.0).contains(&u) {
                    let a = 2.0 * std::f64::consts::PI * u;
                    0.35875 - 0.48829 * a.cos() + 0.14128 * (2.0 * a).cos() - 0.01168 * (3.0 * a).cos()
                } else {
                    0.0
                };
                let v = s * w;
                *t = v as f32;
                sum += v;
            }
            for t in row.iter_mut() {
                *t = (*t as f64 / sum) as f32;
            }
        }
        Ok(Filter { l, m, taps })
    }

    /// 출력 프레임 수 = 올림(frames × L / M).
    pub fn out_len(&self, frames: usize) -> usize {
        (frames * self.l + self.m - 1) / self.m
    }

    /// 스테레오 끼움 `src`(frames 프레임) → 스테레오 끼움 출력.
    pub fn run_stereo(&self, src: &[f32]) -> Vec<f32> {
        let frames = src.len() / 2;
        let out_frames = self.out_len(frames);
        let mut out = vec![0f32; out_frames * 2];
        let n = 2 * HALF;
        for o in 0..out_frames {
            let pos = o * self.m;
            let i = pos / self.l;
            let p = pos % self.l;
            let row = &self.taps[p * n..(p + 1) * n];
            let base = i as isize - (HALF as isize - 1);
            let (mut a0, mut a1) = (0f32, 0f32);
            for (k, &t) in row.iter().enumerate() {
                let j = base + k as isize;
                if j >= 0 && (j as usize) < frames {
                    a0 += src[j as usize * 2] * t;
                    a1 += src[j as usize * 2 + 1] * t;
                }
            }
            out[o * 2] = a0;
            out[o * 2 + 1] = a1;
        }
        out
    }
}

/// (원래 속도, 장치 속도) → 필터. 곡 하나를 불러오는 동안 공유한다.
#[derive(Default)]
pub struct Filters(std::sync::Mutex<HashMap<(u32, u32), Arc<Filter>>>);

impl Filters {
    pub fn get(&self, from: u32, to: u32) -> Result<Arc<Filter>, String> {
        if let Some(f) = self.0.lock().unwrap().get(&(from, to)) {
            return Ok(f.clone());
        }
        let f = Arc::new(Filter::new(from, to)?);
        self.0.lock().unwrap().insert((from, to), f.clone());
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_keeps_amplitude_and_length() {
        let (from, to) = (44_100, 48_000);
        let f0 = 1000.0;
        let x: Vec<f32> = (0..44_100)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * f0 * i as f64 / from as f64).sin() as f32;
                [v, -v]
            })
            .collect();
        let y = Filter::new(from, to).unwrap().run_stereo(&x);
        assert_eq!(y.len(), 96_000);
        let mut err = 0f64;
        for o in 1000..47_000 {
            let want = (2.0 * std::f64::consts::PI * f0 * o as f64 / to as f64).sin();
            err = err.max((y[o * 2] as f64 - want).abs());
            assert!((y[o * 2 + 1] + y[o * 2]).abs() < 1e-6);
        }
        assert!(err < 1e-3, "최대 오차 {err}");
    }

    #[test]
    fn odd_rate_is_accepted() {
        // 예시 Track의 30,697 Hz (CP2 상한 8192에서는 거부되던 것)
        let f = Filter::new(30_697, 48_000).unwrap();
        assert_eq!(f.out_len(30_697), 48_000);
    }
}
