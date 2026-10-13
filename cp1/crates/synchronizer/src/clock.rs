//! 시계 부품: 오디오 장치 샘플 위치 ↔ QPC 대응 모델.
//!
//! 진실은 장치가 내보낸 샘플 위치다. QPC는 입력 시각과 화면 시각을 그 위에 올리기 위한 다리다.
//! Windows가 주는 (위치, QPC) 보고값은 σ ≈ 0.3 ms로 흔들리므로 (0번 단계 측정), 하나하나를 믿지 않고
//! 구간 안의 관측으로 직선 `frame = a + b·t` 를 맞춘다.
//!
//! 정책 (`ClockPolicy`):
//! - `window_s`: 맞춤에 쓰는 최근 구간 길이.
//! - `min_slope_span_s`: 관측이 이 길이보다 짧으면 기울기는 명목값(샘플레이트)으로 두고 절편만 맞춘다.
//!   짧은 구간에서 기울기를 맞추면 ±50 ppm까지 틀리기 때문이다 (0번 단계, 15초 창).
//! - `jump_ms`: 새 관측이 모델에서 이만큼 벗어나는 일이 `jump_count`번 연속되면 대응이 깨졌다고 보고
//!   기준점(epoch)을 새로 잡는다. 장치 재시작, 위치 점프, 긴 언더런 등.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
pub struct ClockPolicy {
    pub window_s: f64,
    pub min_slope_span_s: f64,
    pub jump_ms: f64,
    pub jump_count: u32,
    /// 기울기를 명목값으로 둘 때 절편을 평균 대신 중앙값으로 잡는다. 혼자 튀는 보고값(약 1%, 0.1–0.3 ms)을 무시한다.
    pub median_intercept: bool,
    /// 관측을 이 간격(초, 프레임 수로 환산)보다 촘촘하면 솎는다 (점프 감지는 모든 관측으로 한다). 창 안 관측 수에 상한을 두어
    /// 다시 맞추는 계산량을 묶는다. ASIO 버퍼 64는 초당 689개가 오는데, 솎지 않으면 600초 창에 40만 개가 쌓여
    /// 콜백 처리 시간이 분마다 늘었다 (CP1-3). WASAPI(10 ms마다)는 그대로.
    pub keep_interval_s: f64,
}

impl Default for ClockPolicy {
    /// 0번 단계 측정으로 정한 기본값 (TOPPING E2x2 OTG, WASAPI 공유 모드):
    /// 드라이버는 명목 샘플레이트로 위치를 보고하다가, 실제 장치 시계(QPC 대비 −7.4 ppm)와의 차이가
    /// USB 프레임 하나(1 ms)만큼 쌓이면 1 ms를 한 번에 고친다 (약 135초마다, 재생·녹음 동시에).
    /// 보고값은 실제 시계를 중심으로 0–1 ms 톱니(σ 0.29 ms)를 그리므로, 톱니 여러 주기를 덮는 긴 창의 직선이
    /// 실제 시계에 가장 가깝다. 한 주기(약 135초)가 쌓이기 전에는 기울기를 명목값으로 둔다.
    /// 점프 감지 2 ms × 2회: 언더런 계단을 0.14초 만에 새 기준점으로 잡는다 (3-3 단계). 이 장치의 정상 흔들림
    /// (톱니 계단 1 ms, 혼자 튀는 값 0.3 ms)보다 커서 오탐이 없었다. 다른 장치에서는 톱니 크기를 먼저 재고 정한다.
    fn default() -> Self {
        ClockPolicy { window_s: 600.0, min_slope_span_s: 150.0, jump_ms: 2.0, jump_count: 2, median_intercept: false, keep_interval_s: 0.01 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Observe {
    /// 모델에 반영함.
    Accepted,
    /// 모델에서 크게 벗어나 보류함 (연속 횟수).
    Suspect(u32),
    /// 대응이 깨져 기준점을 새로 잡음.
    NewEpoch(u32),
}

/// 시간 단위: 입력은 100 ns 단위 정수(WASAPI qpcPosition과 같은 단위). 내부는 기준점에서 뺀 f64 초.
pub struct ClockModel {
    pub policy: ClockPolicy,
    nominal_rate: f64,
    epoch: u32,
    t0: i64,  // 기준 시각 (100 ns)
    f0: f64,  // 기준 위치 (프레임)
    obs: VecDeque<(f64, f64)>, // (t − t0 초, frame − f0)
    // 현재 직선: y = a + b·x
    a: f64,
    b: f64,
    resid_std_s: f64,
    suspects: u32,
}

impl ClockModel {
    pub fn new(nominal_rate: f64, policy: ClockPolicy) -> Self {
        ClockModel { policy, nominal_rate, epoch: 0, t0: 0, f0: 0.0, obs: VecDeque::new(), a: 0.0, b: nominal_rate,
                     resid_std_s: f64::NAN, suspects: 0 }
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// 관측이 하나라도 있어야 쓸 수 있다.
    pub fn is_valid(&self) -> bool {
        !self.obs.is_empty()
    }

    /// 맞춘 기울기 (QPC 1초당 장치 프레임 수).
    pub fn rate(&self) -> f64 {
        self.b
    }

    pub fn rate_ppm(&self) -> f64 {
        (self.b / self.nominal_rate - 1.0) * 1e6
    }

    /// 맞춤 잔차의 표준편차 (ms). 보고값 흔들림의 크기.
    pub fn residual_std_ms(&self) -> f64 {
        self.resid_std_s * 1e3
    }

    pub fn span_s(&self) -> f64 {
        match (self.obs.front(), self.obs.back()) {
            (Some(f), Some(l)) => l.0 - f.0,
            _ => 0.0,
        }
    }

    pub fn observe(&mut self, frame: u64, t100: i64) -> Observe {
        if self.obs.is_empty() {
            self.t0 = t100;
            self.f0 = frame as f64;
        }
        let x = (t100 - self.t0) as f64 * 1e-7;
        let y = frame as f64 - self.f0;
        if self.obs.len() >= 2 {
            let err_s = (y - (self.a + self.b * x)) / self.b;
            if err_s.abs() * 1e3 > self.policy.jump_ms {
                self.suspects += 1;
                if self.suspects < self.policy.jump_count {
                    return Observe::Suspect(self.suspects);
                }
                // 대응이 깨졌다: 이 관측부터 새로 시작한다.
                self.epoch += 1;
                self.obs.clear();
                self.suspects = 0;
                self.t0 = t100;
                self.f0 = frame as f64;
                self.obs.push_back((0.0, 0.0));
                self.a = 0.0;
                self.b = self.nominal_rate;
                self.resid_std_s = f64::NAN;
                return Observe::NewEpoch(self.epoch);
            }
        }
        self.suspects = 0;
        // 솎기는 프레임 간격으로 한다: 시각에는 흔들림이 섞여 있어서, 시각 간격으로 고르면 늦게 찍힌 관측이 더 자주 남아 치우친다.
        if let Some(&(_, ly)) = self.obs.back() {
            if y - ly < self.policy.keep_interval_s * self.nominal_rate * 0.95 {
                return Observe::Accepted;
            }
        }
        self.obs.push_back((x, y));
        while let Some(&(fx, _)) = self.obs.front() {
            if x - fx > self.policy.window_s {
                self.obs.pop_front();
            } else {
                break;
            }
        }
        self.refit();
        Observe::Accepted
    }

    fn refit(&mut self) {
        let n = self.obs.len() as f64;
        let (mx, my) = self.obs.iter().fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
        let (mx, my) = (mx / n, my / n);
        let slope_fit = self.span_s() >= self.policy.min_slope_span_s && self.obs.len() >= 3;
        let b = if slope_fit {
            let (sxy, sxx) = self.obs.iter().fold((0.0, 0.0), |(sxy, sxx), &(x, y)| {
                (sxy + (x - mx) * (y - my), sxx + (x - mx) * (x - mx))
            });
            sxy / sxx
        } else {
            self.nominal_rate
        };
        self.b = b;
        self.a = if !slope_fit && self.policy.median_intercept {
            let mut d: Vec<f64> = self.obs.iter().map(|&(x, y)| y - b * x).collect();
            let mid = d.len() / 2;
            *d.select_nth_unstable_by(mid, |p, q| p.partial_cmp(q).unwrap()).1
        } else {
            my - b * mx
        };
        let ss = self.obs.iter().map(|&(x, y)| {
            let r = (y - (self.a + b * x)) / b;
            r * r
        }).sum::<f64>();
        self.resid_std_s = if self.obs.len() > 1 { (ss / (n - 1.0)).sqrt() } else { f64::NAN };
    }

    /// QPC 시각(100 ns)에 장치가 내보내고 있는 프레임 위치.
    pub fn frame_at(&self, t100: i64) -> f64 {
        let x = (t100 - self.t0) as f64 * 1e-7;
        self.f0 + self.a + self.b * x
    }

    /// 장치가 프레임 위치 `frame`을 내보내는 QPC 시각(100 ns).
    pub fn t100_at(&self, frame: f64) -> f64 {
        let x = (frame - self.f0 - self.a) / self.b;
        self.t0 as f64 + x * 1e7
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_rate_and_offset() {
        let mut m = ClockModel::new(44100.0, ClockPolicy { window_s: 30.0, min_slope_span_s: 1.0, median_intercept: false, ..Default::default() });
        // 장치가 명목보다 10 ppm 빠르고, 관측 시각이 ±0.3 ms 흔들린다.
        let true_rate = 44100.0 * (1.0 + 10e-6);
        for i in 0..2000 {
            let t_s = i as f64 * 0.01;
            let jitter = if i % 2 == 0 { 0.0003 } else { -0.0003 };
            let frame = (t_s * true_rate) as u64;
            let t100 = ((t_s + jitter) * 1e7) as i64 + 5_000_000_000;
            assert_ne!(m.observe(frame, t100), Observe::NewEpoch(1));
        }
        assert!((m.rate_ppm() - 10.0).abs() < 1.0, "ppm {}", m.rate_ppm());
        let t100 = (20.0 * 1e7) as i64 + 5_000_000_000;
        let err_frames = m.frame_at(t100) - 20.0 * true_rate;
        assert!(err_frames.abs() < 2.0, "err {err_frames}");
    }

    #[test]
    fn detects_jump() {
        let mut m = ClockModel::new(44100.0, ClockPolicy::default());
        for i in 0..100 {
            m.observe(i * 441, i as i64 * 100_000);
        }
        // 위치가 100 ms 건너뜀
        let mut last = Observe::Accepted;
        for i in 100..104 {
            last = m.observe(i * 441 + 4410, i as i64 * 100_000);
        }
        assert_eq!(m.epoch(), 1);
        assert_eq!(last, Observe::Accepted);
    }
}
