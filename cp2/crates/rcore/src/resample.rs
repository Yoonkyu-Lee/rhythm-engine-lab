//! 소리 뱅크를 장치 속도로 바꾼다 (곡을 불러올 때 한 번, 제어 경로에서). 설계안 8-1.
//!
//! 윈도 sinc 다상 필터, 정확한 유리수 비율 L/M (예: 44.1 → 48 kHz = 160/147).
//! 소리마다 따로 변환하고(소리 경계 밖은 0), 뱅크를 다시 이어 붙인다. 결정적이다 (같은 입력 → 같은 비트).

use crate::runtime::{Chart, SoundBank, SoundKind, SoundRef, LONG_SOUND_NS, NS_PER_S};

/// 필터 반폭(탭 수 = 2 × HALF). 차단은 나이퀴스트의 95% (낮은 쪽 속도 기준).
const HALF: usize = 32;
const CUTOFF: f64 = 0.95;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

struct Filter {
    l: usize,
    m: usize,
    /// taps[p][k], k = 0..2*HALF, 입력 위치 i + k − (HALF − 1)에 곱한다.
    taps: Vec<f32>,
}

impl Filter {
    fn new(from: u32, to: u32) -> Result<Filter, String> {
        let g = gcd(from as u64, to as u64);
        let (l, m) = ((to as u64 / g) as usize, (from as u64 / g) as usize);
        if l > 8192 {
            return Err(format!("{from} → {to} Hz: 위상 수 {l}이 너무 큼"));
        }
        // 차단 주파수 (입력 샘플 단위의 정규화 주파수, 1 = 입력 나이퀴스트 2배)
        let fc = 0.5 * CUTOFF * (l as f64 / m as f64).min(1.0);
        let n = 2 * HALF;
        let mut taps = vec![0f32; l * n];
        for p in 0..l {
            let frac = p as f64 / l as f64; // 출력 위치가 입력 i에서 frac만큼 뒤
            let mut sum = 0.0;
            let row = &mut taps[p * n..(p + 1) * n];
            for (k, t) in row.iter_mut().enumerate() {
                let x = k as f64 - (HALF as f64 - 1.0) - frac; // 입력 샘플과 출력 위치의 거리
                let s = if x.abs() < 1e-12 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                // Blackman-Harris 창 (반폭 HALF)
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
            // 위상마다 DC 이득을 정확히 1로
            for t in row.iter_mut() {
                *t = (*t as f64 / sum) as f32;
            }
        }
        Ok(Filter { l, m, taps })
    }

    /// 한 채널(간격 `stride`)을 변환해 `out`(간격 `stride`)에 쓴다.
    fn run(&self, src: &[f32], frames: usize, stride: usize, ch: usize, out: &mut [f32], out_frames: usize) {
        let n = 2 * HALF;
        for o in 0..out_frames {
            let pos = o * self.m; // 입력 위치 × L
            let i = pos / self.l;
            let p = pos % self.l;
            let row = &self.taps[p * n..(p + 1) * n];
            let base = i as isize - (HALF as isize - 1);
            let mut acc = 0f32;
            for (k, &t) in row.iter().enumerate() {
                let j = base + k as isize;
                if j >= 0 && (j as usize) < frames {
                    acc += src[j as usize * stride + ch] * t;
                }
            }
            out[o * stride + ch] = acc;
        }
    }
}

/// 출력 프레임 수 = 올림(frames × L / M).
fn out_len(frames: usize, l: usize, m: usize) -> usize {
    (frames * l + m - 1) / m
}

/// 뱅크 전체를 `to` Hz로. 이미 같으면 그대로 돌려준다.
pub fn resample_bank(bank: &SoundBank, to: u32) -> Result<SoundBank, String> {
    if bank.rate == to {
        return Ok(bank.clone());
    }
    let f = Filter::new(bank.rate, to)?;
    let ch = bank.channels as usize;
    let lens: Vec<usize> = bank.sounds.iter().map(|s| out_len(s.frames as usize, f.l, f.m)).collect();
    let total: usize = lens.iter().sum();
    let mut pcm = vec![0f32; total * ch];
    let mut sounds = Vec::with_capacity(bank.sounds.len());
    let mut off = 0usize;
    for (s, &n) in bank.sounds.iter().zip(&lens) {
        let src = &bank.pcm[s.offset as usize * ch..(s.offset as usize + s.frames as usize) * ch];
        let dst = &mut pcm[off * ch..(off + n) * ch];
        for c in 0..ch {
            f.run(src, s.frames as usize, ch, c, dst, n);
        }
        let dur = ((n as i128 * NS_PER_S as i128 + to as i128 - 1) / to as i128) as i64;
        sounds.push(SoundRef { offset: off as u64, frames: n as u32, kind: if dur > LONG_SOUND_NS { SoundKind::Long } else { SoundKind::Short } });
        off += n;
    }
    Ok(SoundBank { rate: to, channels: bank.channels, pcm, sounds })
}

/// 곡의 뱅크를 `to` Hz로 바꾼 사본 (이벤트 시각은 ns라 그대로).
pub fn resample_chart(c: &Chart, to: u32) -> Result<Chart, String> {
    Ok(Chart { bank: resample_bank(&c.bank, to)?, ..c.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank_of(x: Vec<f32>, rate: u32) -> SoundBank {
        let n = x.len() as u32;
        let pcm = x.iter().flat_map(|&v| [v, -v]).collect();
        SoundBank { rate, channels: 2, pcm, sounds: vec![SoundRef { offset: 0, frames: n, kind: SoundKind::Short }] }
    }

    #[test]
    fn sine_keeps_amplitude_and_length() {
        let (from, to) = (44_100, 48_000);
        let f0 = 1000.0;
        let x: Vec<f32> = (0..44_100).map(|i| (2.0 * std::f64::consts::PI * f0 * i as f64 / from as f64).sin() as f32).collect();
        let b = resample_bank(&bank_of(x, from), to).unwrap();
        assert_eq!(b.sounds[0].frames, 48_000);
        // 가운데에서 이상적인 사인과 비교
        let mut err = 0f64;
        for o in 1000..47_000 {
            let want = (2.0 * std::f64::consts::PI * f0 * o as f64 / to as f64).sin();
            err = err.max((b.pcm[o * 2] as f64 - want).abs());
            assert!((b.pcm[o * 2 + 1] + b.pcm[o * 2]).abs() < 1e-6); // 오른쪽 = −왼쪽
        }
        assert!(err < 1e-3, "최대 오차 {err}");
    }

    #[test]
    fn deterministic_and_identity() {
        let x: Vec<f32> = (0..999).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
        let b0 = bank_of(x, 44_100);
        assert_eq!(resample_bank(&b0, 44_100).unwrap(), b0);
        assert_eq!(resample_bank(&b0, 48_000).unwrap(), resample_bank(&b0, 48_000).unwrap());
    }
}
