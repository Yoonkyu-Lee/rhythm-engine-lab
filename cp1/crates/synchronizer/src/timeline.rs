//! 기록 시계 (CP1-4): 게임의 현재 = 지금 결정한 소리가 놓이는 가장 이른 샘플.
//!
//! - 시간축 프레임: 곡·채보·예약이 쓰는 프레임 번호. 장치 시계 직선(`SharedClock`)은 이 프레임으로 맞춘다.
//! - 스트림 프레임: 렌더 스트림에 실제로 쓴 순번. 평소에는 시간축 프레임 − `offset`.
//! - 오디오가 멈추면(언더런) 장치는 그동안 우리 스트림을 진행하지 않는다 → 스트림이 통째로 밀린다.
//!   그대로 두면 시간축이 튄다(CP2 1차: 시계 점프를 0.15~0.4초 뒤에야 감지, 화면이 뒤로 튐).
//!   대신 깨어날 때마다 장치 위치를 직선의 예측과 비교해, 뒤처진 만큼 `offset`을 늘린다 = **콘텐츠를 건너뛴다**(빠지기).
//!   그 구간의 소리는 무음으로 빠지지만 시간축은 끊기지 않는다. 직선에는 시간축 프레임으로 관측을 넣으므로 직선도 튀지 않는다.
//! - 기록 시점 C(q) = 장치 시계(q) + D. 키음은 C(누른 시각)에, 음악은 C = 채보 시각에 예약한다 → PERFECT면 같은 샘플.

use crate::shared_clock::ClockSnapshot;

/// 오디오 정지 때 콘텐츠를 건너뛰어 시간축을 잇는다. 오디오 스레드가 소유한다 (잠금·할당 없음, 건너뛴 기록은 미리 잡은 용량 안에서만).
pub struct ContentSkip {
    /// 시간축 프레임 − 스트림 프레임.
    pub offset: i64,
    threshold: f64,
    /// (관측 시각 100 ns, 건너뛴 프레임 수)
    pub skips: Vec<(i64, i64)>,
}

impl ContentSkip {
    /// `threshold_frames`: 이만큼 넘게 뒤처져야 건너뛴다 (위치 보고의 잡음·톱니보다 크게. 공유 모드는 1 ms 톱니가 있다).
    pub fn new(threshold_frames: f64) -> Self {
        ContentSkip { offset: 0, threshold: threshold_frames, skips: Vec::with_capacity(1024) }
    }

    /// 장치 위치 관측 하나(스트림 프레임 `pos`, 그 시각 `t100`)를 시간축 프레임으로 바꾼다.
    /// 직선 `snap`의 예측보다 `threshold` 넘게 뒤처졌으면 그만큼 건너뛴다. 돌려준 값을 시계 관측으로 넣는다.
    pub fn observe(&mut self, pos: u64, t100: i64, snap: &ClockSnapshot) -> u64 {
        let tl = pos as i64 + self.offset;
        if snap.valid {
            let lag = snap.frame_at(t100) - tl as f64;
            if lag > self.threshold {
                let d = lag.round() as i64;
                self.offset += d;
                if self.skips.len() < self.skips.capacity() {
                    self.skips.push((t100, d));
                }
                return (pos as i64 + self.offset).max(0) as u64;
            }
        }
        tl.max(0) as u64
    }

    /// 스트림 프레임 → 시간축 프레임.
    pub fn timeline(&self, stream_frame: u64) -> u64 {
        (stream_frame as i64 + self.offset).max(0) as u64
    }
}

/// 기록 시점 C(t100): 장치 시계 직선 + D (프레임).
pub fn commit_frame(snap: &ClockSnapshot, t100: i64, d_frames: f64) -> f64 {
    snap.frame_at(t100) + d_frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> ClockSnapshot {
        // 48 kHz, t100 0에서 프레임 0
        ClockSnapshot { valid: true, epoch: 0, nominal_rate: 48_000.0, t0: 0, f0: 0.0, a: 0.0, b: 48_000.0 }
    }

    #[test]
    fn skips_when_device_falls_behind() {
        let s = line();
        let mut k = ContentSkip::new(96.0); // 2 ms
        // 1초: 장치 위치가 직선과 같음 → 그대로
        assert_eq!(k.observe(48_000, 10_000_000, &s), 48_000);
        // 1초 뒤 위치 보고가 0.5 ms 뒤처짐 (톱니) → 건너뛰지 않음
        assert_eq!(k.observe(95_976, 20_000_000, &s), 95_976);
        assert_eq!(k.offset, 0);
        // 언더런: 150 ms 동안 스트림이 멈춤 → 3초에 위치가 7200프레임 뒤처짐 → 건너뜀
        assert_eq!(k.observe(144_000 - 7_200, 30_000_000, &s), 144_000);
        assert_eq!(k.offset, 7_200);
        assert_eq!(k.timeline(150_000), 157_200);
        assert_eq!(k.skips, vec![(30_000_000, 7_200)]);
    }
}
