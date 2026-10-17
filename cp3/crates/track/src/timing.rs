//! 차분 하나의 위치 → 차분 자기 시각(초, 유리수) (명세 3-1).

use crate::rational::{int, parse, Q};
use crate::source::TimingDef;
use num_traits::{Signed, Zero};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Beat,
    Ms,
    Tick,
}

#[derive(Clone, Debug)]
pub struct Timing {
    pub unit: Unit,
    tpb: u32,
    /// (마디 번호, 이 마디부터의 마디 길이 박)
    meter: Vec<(u32, Q)>,
    /// (위치: beat/tick은 박, ms는 ms, bpm)
    pub tempo: Vec<(Q, Q)>,
    /// (박, 정지 박 수) — beat/tick만
    pub stops: Vec<(Q, Q)>,
    /// 명시 마디선 (그 단위의 위치 문자열을 읽은 값)
    pub bars: Vec<String>,
}

impl Timing {
    pub fn new(t: &TimingDef) -> Result<Timing, String> {
        let unit = match t.unit.as_str() {
            "beat" => Unit::Beat,
            "ms" => Unit::Ms,
            "tick" => Unit::Tick,
            u => return Err(format!("unit 오류: {u}")),
        };
        let tpb = match (unit, t.ticks_per_beat) {
            (Unit::Tick, Some(n)) if n > 0 => n,
            (Unit::Tick, _) => return Err("unit = tick에는 ticks_per_beat(> 0)가 필요".into()),
            (_, Some(_)) => return Err("ticks_per_beat는 unit = tick에서만".into()),
            _ => 0,
        };
        let mut meter = Vec::new();
        for m in &t.meter {
            let l = parse(&m.length)?;
            if !l.is_positive() {
                return Err(format!("마디 길이는 양수: {}", m.length));
            }
            meter.push((m.measure, l));
        }
        meter.sort_by_key(|m| m.0);
        if meter.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err("meter에 같은 마디가 두 번".into());
        }
        if !meter.is_empty() && meter[0].0 != 0 {
            return Err("meter는 마디 0에서 시작해야 함".into());
        }
        let mut tm = Timing { unit, tpb, meter, tempo: vec![], stops: vec![], bars: t.bars.clone() };
        let mut tempo = Vec::new();
        for x in &t.tempo {
            let at = match unit {
                Unit::Ms => parse(&x.at)?,
                _ => tm.beat(&x.at)?,
            };
            tempo.push((at, parse(&x.bpm)?));
        }
        tempo.sort_by(|a, b| a.0.cmp(&b.0));
        if tempo.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err("tempo에 같은 위치가 두 번".into());
        }
        if unit != Unit::Ms {
            match tempo.first() {
                Some((p, _)) if p.is_zero() => {}
                _ => return Err("unit이 beat·tick이면 tempo는 위치 0에서 시작해야 함 (G10)".into()),
            }
            if !tempo[0].1.is_positive() {
                return Err("첫 tempo는 양수여야 함".into());
            }
        }
        let mut stops = Vec::new();
        for s in &t.stops {
            if unit == Unit::Ms {
                return Err("unit = ms에는 stops를 쓸 수 없음(시각에 이미 반영)".into());
            }
            let b = parse(&s.beats)?;
            if b.is_negative() {
                return Err("정지 박 수는 0 이상".into());
            }
            stops.push((tm.beat(&s.at)?, b));
        }
        stops.sort_by(|a, b| a.0.cmp(&b.0));
        tm.tempo = tempo;
        tm.stops = stops;
        Ok(tm)
    }

    pub fn has_meter(&self) -> bool {
        !self.meter.is_empty()
    }

    /// 마디 m의 시작 박.
    pub fn measure_start(&self, m: u32) -> Q {
        let mut total = Q::zero();
        let mut cur = 0u32;
        let mut len = self.meter[0].1.clone();
        for (mm, l) in &self.meter {
            if *mm > m {
                break;
            }
            total += &len * int((*mm - cur) as i64);
            cur = *mm;
            len = l.clone();
        }
        total + len * int((m - cur) as i64)
    }

    /// 위치 문자열 → 박 (beat·tick). ms 단위에는 쓰지 않는다.
    pub fn beat(&self, pos: &str) -> Result<Q, String> {
        match self.unit {
            Unit::Tick => {
                let t: i64 = pos.trim().parse().map_err(|_| format!("tick은 정수: {pos:?}"))?;
                Ok(int(t) / int(self.tpb as i64))
            }
            Unit::Beat => {
                if let Some((m, b)) = pos.split_once(':') {
                    if self.meter.is_empty() {
                        return Err(format!("'마디:박' 표기에 meter가 없음: {pos:?}"));
                    }
                    let m: u32 = m.trim().parse().map_err(|_| format!("마디 번호: {pos:?}"))?;
                    Ok(self.measure_start(m) + parse(b)?)
                } else {
                    parse(pos)
                }
            }
            Unit::Ms => Err("ms 단위에서 박을 물음".into()),
        }
    }

    pub fn bpm_at_beat(&self, b: &Q) -> Q {
        let mut cur = self.tempo[0].1.clone();
        for (tb, bpm) in &self.tempo {
            if tb <= b {
                cur = bpm.clone();
            }
        }
        cur
    }

    /// 박 → 초 (정지 포함). 0보다 앞은 첫 tempo를 뒤로 늘인다.
    pub fn sec_of_beat(&self, b: &Q) -> Q {
        if b.is_negative() {
            return b * int(60) / &self.tempo[0].1;
        }
        let mut sec = Q::zero();
        for (i, (tb, bpm)) in self.tempo.iter().enumerate() {
            if b <= tb {
                break;
            }
            if !bpm.is_positive() {
                continue; // bpm ≤ 0은 시간 계산에 쓰지 않음 (명세 3-1)
            }
            let end = self.tempo.get(i + 1).map(|x| &x.0);
            let hi = match end {
                Some(e) if e < b => e.clone(),
                _ => b.clone(),
            };
            sec += (hi - tb) * int(60) / bpm;
        }
        for (sb, beats) in &self.stops {
            if b > sb {
                sec += beats * int(60) / self.bpm_at_beat(sb);
            }
        }
        sec
    }

    /// 정지 시작 시각과 길이(초) 목록.
    pub fn stop_spans(&self) -> Vec<(Q, Q)> {
        self.stops
            .iter()
            .map(|(sb, beats)| (self.sec_of_beat(sb), beats * int(60) / self.bpm_at_beat(sb)))
            .collect()
    }

    /// 위치 문자열 → 초.
    pub fn seconds(&self, pos: &str) -> Result<Q, String> {
        match self.unit {
            Unit::Ms => Ok(parse(pos)? / int(1000)),
            _ => Ok(self.sec_of_beat(&self.beat(pos)?)),
        }
    }

    /// 노트 위치 + 상대 위치(`"+x"`) → 위치 문자열과 같은 의미의 초.
    pub fn seconds_rel(&self, base: &str, rel: &str) -> Result<Q, String> {
        let r = rel.trim().strip_prefix('+').ok_or_else(|| format!("상대 위치는 '+'로 시작: {rel:?}"))?;
        match self.unit {
            Unit::Ms => Ok((parse(base)? + parse(r)?) / int(1000)),
            Unit::Tick => {
                let a: i64 = base.trim().parse().map_err(|_| format!("tick은 정수: {base:?}"))?;
                let b: i64 = r.trim().parse().map_err(|_| format!("tick은 정수: {rel:?}"))?;
                Ok(self.sec_of_beat(&(int(a + b) / int(self.tpb as i64))))
            }
            Unit::Beat => Ok(self.sec_of_beat(&(self.beat(base)? + parse(r)?))),
        }
    }

    /// ms 단위의 tempo에서 그 시각의 bpm (스크롤 기본값용).
    pub fn bpm_at_ms(&self, ms: &Q) -> Option<Q> {
        let mut cur = self.tempo.first()?.1.clone();
        for (t, bpm) in &self.tempo {
            if t <= ms {
                cur = bpm.clone();
            }
        }
        Some(cur)
    }
}
