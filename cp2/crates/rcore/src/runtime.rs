//! 실행 데이터: 코어가 플레이할 때 읽는 유일한 곡 표현 (난이도 하나).
//!
//! 모양은 소비자가 정한다.
//! - 오디오 콜백: `auto`를 시간순으로 앞에서부터 한 번 훑는다 (커서 하나). 소리는 `bank`의 연속 PCM에서 바로 읽는다.
//! - 판정 스레드: 레인마다 `notes`를 시간순으로 훑는다. 노트가 울릴 소리는 `note_sounds`의 구간 (화음 = 소리 여러 개).
//! - 화면: `scroll`로 시각 → 스크롤 위치.
//!
//! 시간은 모두 정규 곡 시간 i64 ns (곡 원점 0). 장치 프레임으로는 예약할 때만 바꾼다.
//! 볼륨은 이벤트마다 선형 이득 (기존 형식처럼 볼륨별 소리 사본을 만들지 않는다).
//! 같은 소리를 다시 울리면 겹친다 (끊는 것은 정책이지 데이터가 아님).

use std::fmt;

pub const NS_PER_S: i64 = 1_000_000_000;

/// 늦게 시작할 때의 처리를 가르는 기준: 이보다 긴 소리는 "긴 소리"(늦으면 앞부분을 잘라 원래 자리에 맞춤, CP1 3-3).
pub const LONG_SOUND_NS: i64 = 2 * NS_PER_S;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Short,
    Long,
}

/// 뱅크 안의 소리 하나. `offset`·`frames`는 프레임 단위 (1프레임 = 채널 수만큼의 샘플).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundRef {
    pub offset: u64,
    pub frames: u32,
    pub kind: SoundKind,
}

/// 미리 디코딩한 PCM 한 덩어리. 곡의 모든 소리가 이어 붙어 있다 (할당·파일 접근 없이 콜백에서 읽기 위함).
#[derive(Clone, Debug, PartialEq)]
pub struct SoundBank {
    pub rate: u32,
    pub channels: u16,
    /// 채널을 끼워 넣은 f32 샘플. 길이 = 총 프레임 × channels.
    pub pcm: Vec<f32>,
    pub sounds: Vec<SoundRef>,
}

impl SoundBank {
    pub fn frames(&self) -> u64 {
        self.pcm.len() as u64 / self.channels.max(1) as u64
    }

    /// 소리 길이(ns, 올림).
    pub fn duration_ns(&self, sound: u32) -> i64 {
        let f = self.sounds[sound as usize].frames as i128;
        ((f * NS_PER_S as i128 + self.rate as i128 - 1) / self.rate as i128) as i64
    }
}

/// 정해진 시각에 저절로 울리는 소리 (자동재생).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundEvent {
    pub t: i64,
    pub sound: u32,
    pub gain: f32,
}

/// 노트를 쳤을 때 울릴 소리 하나.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteSound {
    pub sound: u32,
    pub gain: f32,
}

/// 노트 하나. 단노트는 `end == t`, 롱노트는 `end > t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub t: i64,
    pub end: i64,
    /// `note_sounds[sounds_start .. sounds_start + sounds_len]`
    pub sounds_start: u32,
    pub sounds_len: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lane {
    pub notes: Vec<Note>,
}

/// 스크롤 지도의 꺾이는 점. `t` 이후로 위치는 `pos + speed × (시각 − t)`. 위치 단위는 "기준 박"이다.
/// 기준 박은 곡이 명시한 `reference_bpm`의 한 박이다 (배속은 이 기준에 곱한다. 구동기가 추정하지 않는다).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollPoint {
    pub t: i64,
    pub pos: f64,
    /// 초당 기준 박 수.
    pub speed: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: String,
    pub bank: SoundBank,
    /// 시간순 (같은 시각이면 입력 순서 유지).
    pub auto: Vec<SoundEvent>,
    pub note_sounds: Vec<NoteSound>,
    pub lanes: Vec<Lane>,
    /// 시간순, 첫 점은 t ≤ 0.
    pub scroll: Vec<ScrollPoint>,
    pub reference_bpm: f64,
    /// 곡 끝 (이 시각 이후에는 이벤트가 없다).
    pub end: i64,
}

impl Chart {
    /// 시각 `t`의 스크롤 위치 (기준 박).
    pub fn scroll_pos(&self, t: i64) -> f64 {
        let i = self.scroll.partition_point(|p| p.t <= t).saturating_sub(1);
        let p = &self.scroll[i];
        p.pos + p.speed * ((t - p.t) as f64 / NS_PER_S as f64)
    }
}

/// 정규 시간 → 이 뱅크 속도의 프레임 (가장 가까운 정수, 반올림 반 프레임 이하).
pub fn ns_to_frame(t: i64, rate: u32) -> i64 {
    let x = t as i128 * rate as i128;
    let half = NS_PER_S as i128 / 2;
    (if x >= 0 { (x + half) / NS_PER_S as i128 } else { -((-x + half) / NS_PER_S as i128) }) as i64
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompileStats {
    pub sounds: usize,
    pub bank_frames: u64,
    pub auto_events: usize,
    pub notes: usize,
    pub long_notes: usize,
    /// 모든 소리가 끝까지 울리고 모든 노트를 제때 친다고 할 때 동시에 울리는 소리 수의 최댓값.
    pub peak_voices: u32,
}

/// 검증을 통과한 실행 데이터. 코어는 이것만 받는다.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub chart: Chart,
    /// 보이스 풀 크기 (최댓값 + 여유). 플레이 중 할당하지 않도록 미리 잡는다.
    pub voice_budget: u32,
    /// 실행 데이터 전체(바이너리 표현)의 해시. 리플레이·로그가 같은 곡인지 확인하는 데 쓴다.
    pub hash: u64,
    pub stats: CompileStats,
}

#[derive(Debug, PartialEq)]
pub enum CompileError {
    BadBank(String),
    BadSound { index: usize, reason: &'static str },
    BadEvent { index: usize, reason: &'static str },
    BadNote { lane: usize, index: usize, reason: &'static str },
    BadScroll(&'static str),
    BadEnd { end: i64, last: i64 },
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CompileError {}

/// 실행 데이터를 검증하고 보이스 예산과 해시를 계산한다.
pub fn compile(chart: Chart) -> Result<Compiled, CompileError> {
    let b = &chart.bank;
    if b.rate == 0 || b.channels == 0 || b.pcm.len() % b.channels as usize != 0 {
        return Err(CompileError::BadBank(format!("rate {} channels {} samples {}", b.rate, b.channels, b.pcm.len())));
    }
    let total = b.frames();
    for (i, s) in b.sounds.iter().enumerate() {
        if s.frames == 0 {
            return Err(CompileError::BadSound { index: i, reason: "길이 0" });
        }
        if s.offset + s.frames as u64 > total {
            return Err(CompileError::BadSound { index: i, reason: "뱅크 밖" });
        }
        let long = b.duration_ns(i as u32) > LONG_SOUND_NS;
        if long != (s.kind == SoundKind::Long) {
            return Err(CompileError::BadSound { index: i, reason: "길이 구분(kind)이 LONG_SOUND_NS 기준과 다름" });
        }
    }
    let n_sounds = b.sounds.len() as u32;
    let gain_ok = |g: f32| g.is_finite() && g >= 0.0;
    let mut last = i64::MIN;
    for (i, e) in chart.auto.iter().enumerate() {
        if e.sound >= n_sounds {
            return Err(CompileError::BadEvent { index: i, reason: "없는 소리" });
        }
        if !gain_ok(e.gain) {
            return Err(CompileError::BadEvent { index: i, reason: "이득이 음수거나 유한하지 않음" });
        }
        if e.t < last {
            return Err(CompileError::BadEvent { index: i, reason: "시간순이 아님" });
        }
        last = e.t;
    }
    for (i, s) in chart.note_sounds.iter().enumerate() {
        if s.sound >= n_sounds || !gain_ok(s.gain) {
            return Err(CompileError::BadEvent { index: i, reason: "노트 소리: 없는 소리 또는 잘못된 이득" });
        }
    }
    let mut notes = 0;
    let mut long_notes = 0;
    for (l, lane) in chart.lanes.iter().enumerate() {
        let mut prev_end = i64::MIN;
        for (i, n) in lane.notes.iter().enumerate() {
            if n.end < n.t {
                return Err(CompileError::BadNote { lane: l, index: i, reason: "끝이 시작보다 앞" });
            }
            if n.t <= prev_end {
                return Err(CompileError::BadNote { lane: l, index: i, reason: "같은 레인의 앞 노트와 겹치거나 시간순이 아님" });
            }
            if n.sounds_start as usize + n.sounds_len as usize > chart.note_sounds.len() {
                return Err(CompileError::BadNote { lane: l, index: i, reason: "소리 구간이 note_sounds 밖" });
            }
            prev_end = n.end;
            last = last.max(n.end);
            notes += 1;
            long_notes += (n.end > n.t) as usize;
        }
    }
    if chart.scroll.is_empty() || chart.scroll[0].t > 0 {
        return Err(CompileError::BadScroll("첫 점이 없거나 0보다 늦음"));
    }
    if chart.scroll.windows(2).any(|w| w[1].t < w[0].t) || chart.scroll.iter().any(|p| !p.pos.is_finite() || !p.speed.is_finite()) {
        return Err(CompileError::BadScroll("시간순이 아니거나 유한하지 않은 값"));
    }
    if !(chart.reference_bpm.is_finite() && chart.reference_bpm > 0.0) {
        return Err(CompileError::BadScroll("기준 BPM이 양수가 아님"));
    }
    if chart.end < last {
        return Err(CompileError::BadEnd { end: chart.end, last });
    }

    let peak = peak_voices(&chart);
    let stats = CompileStats {
        sounds: b.sounds.len(),
        bank_frames: total,
        auto_events: chart.auto.len(),
        notes,
        long_notes,
        peak_voices: peak,
    };
    let voice_budget = peak + peak / 4 + 8;
    let hash = fnv1a64(&crate::rtc::encode(&chart));
    Ok(Compiled { chart, voice_budget, hash, stats })
}

/// 동시에 울리는 소리 수의 최댓값 (자동재생 + 제때 친 노트, 모두 끝까지 울린다고 가정).
fn peak_voices(chart: &Chart) -> u32 {
    let mut edges: Vec<(i64, i32)> = Vec::with_capacity(2 * (chart.auto.len() + chart.note_sounds.len()));
    for e in &chart.auto {
        edges.push((e.t, 1));
        edges.push((e.t + chart.bank.duration_ns(e.sound), -1));
    }
    for lane in &chart.lanes {
        for n in &lane.notes {
            for s in &chart.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize] {
                edges.push((n.t, 1));
                edges.push((n.t + chart.bank.duration_ns(s.sound), -1));
            }
        }
    }
    edges.sort_by_key(|&(t, d)| (t, d)); // 같은 시각이면 끝나는 것(-1)을 먼저
    let (mut cur, mut peak) = (0i32, 0i32);
    for (_, d) in edges {
        cur += d;
        peak = peak.max(cur);
    }
    peak as u32
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in bytes {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 시험용 작은 곡: 44.1 kHz 스테레오, 짧은 소리 2개 + 긴 소리 1개, 자동재생 3개, 레인 1개에 노트 2개(하나는 화음).
    pub fn tiny() -> Chart {
        let rate = 44_100;
        let (a, b, c) = (100u32, 200u32, 3 * rate);
        let total = (a + b + c) as usize;
        let pcm: Vec<f32> = (0..total * 2).map(|i| (i % 7) as f32 * 0.01).collect();
        Chart {
            title: "tiny".into(),
            bank: SoundBank {
                rate,
                channels: 2,
                pcm,
                sounds: vec![
                    SoundRef { offset: 0, frames: a, kind: SoundKind::Short },
                    SoundRef { offset: a as u64, frames: b, kind: SoundKind::Short },
                    SoundRef { offset: (a + b) as u64, frames: c, kind: SoundKind::Long },
                ],
            },
            auto: vec![
                SoundEvent { t: 0, sound: 2, gain: 1.0 },
                SoundEvent { t: 500_000_000, sound: 0, gain: 0.5 },
                SoundEvent { t: 500_000_000, sound: 0, gain: 0.5 },
            ],
            note_sounds: vec![NoteSound { sound: 1, gain: 1.0 }, NoteSound { sound: 0, gain: 0.8 }, NoteSound { sound: 1, gain: 0.8 }],
            lanes: vec![Lane {
                notes: vec![
                    Note { t: 250_000_000, end: 250_000_000, sounds_start: 0, sounds_len: 1 },
                    Note { t: 750_000_000, end: 1_000_000_000, sounds_start: 1, sounds_len: 2 },
                ],
            }],
            scroll: vec![ScrollPoint { t: 0, pos: 0.0, speed: 2.0 }],
            reference_bpm: 120.0,
            end: 3_000_000_000,
        }
    }

    #[test]
    fn compiles_and_counts_voices() {
        let c = compile(tiny()).unwrap();
        assert_eq!(c.stats.notes, 2);
        assert_eq!(c.stats.long_notes, 1);
        // 0.5초에 긴 소리 + 같은 소리 두 번 겹침 = 3
        assert_eq!(c.stats.peak_voices, 3);
        assert_eq!(c.voice_budget, 3 + 0 + 8);
    }

    #[test]
    fn rejects_overlapping_notes_and_unsorted_events() {
        let mut ch = tiny();
        ch.lanes[0].notes[1].t = 250_000_000;
        assert!(matches!(compile(ch), Err(CompileError::BadNote { .. })));
        let mut ch = tiny();
        ch.auto.swap(0, 1);
        assert!(matches!(compile(ch), Err(CompileError::BadEvent { .. })));
        let mut ch = tiny();
        ch.bank.sounds[2].kind = SoundKind::Short;
        assert!(matches!(compile(ch), Err(CompileError::BadSound { .. })));
    }

    #[test]
    fn ns_to_frame_rounds_to_nearest() {
        assert_eq!(ns_to_frame(0, 44_100), 0);
        assert_eq!(ns_to_frame(NS_PER_S, 44_100), 44_100);
        // 반 프레임 = 11,337.8 ns
        assert_eq!(ns_to_frame(11_337, 44_100), 0);
        assert_eq!(ns_to_frame(11_339, 44_100), 1);
        assert_eq!(ns_to_frame(-11_339, 44_100), -1);
    }

    #[test]
    fn scroll_position() {
        let ch = tiny();
        assert_eq!(ch.scroll_pos(0), 0.0);
        assert!((ch.scroll_pos(1_500_000_000) - 3.0).abs() < 1e-12);
    }
}
