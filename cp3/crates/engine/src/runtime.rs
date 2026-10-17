//! 실행 데이터: 엔진이 플레이할 때 읽는 유일한 곡 표현 (Track 차분 하나 + 그 차분의 소리 뱅크).
//!
//! Track 묶음의 `CHART` 구역(명세 4장)을 읽어 변형 선택으로 거른 것이다. 시간은 모두 **차분 자기 시각** i64 ns.
//! 소리는 플레이 전에 디코딩·리샘플링해 둔다(명세 README 4-3: 소리마다 16비트 정수 또는 f32, 손실 없는 쪽).
//! 소리 번호는 차분 안 소리 번호(명세 3-5)라, 같은 내용이라도 논리 ID가 다르면 다른 소리다(같은 소리 끊기는 이 번호로 판단).

pub const NS_PER_S: i64 = 1_000_000_000;
/// 세션 끝 여유 (모든 소리·판정이 끝난 뒤)
pub const SESSION_MARGIN_NS: i64 = 500_000_000;

/// 늦게 시작할 때의 처리를 가르는 기준: 이보다 긴 소리는 "긴 소리"(늦으면 앞부분을 잘라 원래 자리에 맞춤, CP1 3-3).
pub const LONG_SOUND_NS: i64 = 2 * NS_PER_S;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Short,
    Long,
}

/// 데이터 슬롯 하나의 PCM: 스테레오 끼움, 장치 속도. 메모리 형식 (README 4-3 결정): 디코딩 결과가 16비트 정수이고
/// 리샘플링이 필요 없으면 i16, 그 밖은 f32.
#[derive(Clone, Debug, PartialEq)]
pub enum Pcm2 {
    I16(Box<[i16]>),
    F32(Box<[f32]>),
}

impl Pcm2 {
    pub fn bytes(&self) -> usize {
        match self {
            Pcm2::I16(v) => v.len() * 2,
            Pcm2::F32(v) => v.len() * 4,
        }
    }
}

/// 차분 안 소리 하나 (논리 소리). PCM은 데이터 슬롯 `data`에 있고, 같은 (소리 데이터, start, length)인 논리 소리끼리 공유한다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundRef {
    pub data: u32,
    /// 장치 속도 프레임 수 (디코딩 전에 소리 표로 계산해 둔다)
    pub frames: u32,
    pub kind: SoundKind,
    /// 소리 표 이득 (이벤트 이득과 곱한다)
    pub gain: f32,
    /// 같은 소리를 다시 울리면 앞 소리를 끊는가 (retrigger = cut)
    pub cut: bool,
    /// choke 묶음 (0 = 없음): 같은 묶음의 다른 소리가 나면 이 소리를 끊는다
    pub choke: u32,
}

/// 소리 저장소 (README 4-3 2): 슬롯은 작업 스레드가 다 만든 뒤 한 번만 게시하고(`OnceLock`), 믹서는 잠금 없이 준비된 슬롯만 읽는다.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundBank {
    pub rate: u32,
    pub sounds: Vec<SoundRef>,
    pub data: Vec<Slot>,
    /// 데이터 슬롯마다 부분 슬롯 (미리보기, 3-5). 비어 있으면 없음.
    pub partial: Vec<PSlot>,
}

/// 데이터 슬롯 하나: 한 번 게시되는 PCM. `Arc`라 같은 Track의 차분 사이·미리보기 → 플레이에서 공유한다 (README 4-3 2·7).
pub type Slot = std::sync::Arc<std::sync::OnceLock<Pcm2>>;

/// 부분 슬롯 (README 4-3 3 부분 디코딩, 3-5): 긴 소리에서 들을 곳 근처만 먼저 디코딩한 것. `from` = 슬롯 안 첫 프레임.
/// 전체 슬롯과 그 구간이 비트까지 같다. 전체 슬롯이 준비되면 믹서는 그쪽을 읽는다. 한 번만 게시된다.
#[derive(Clone, Debug, PartialEq)]
pub struct Partial {
    pub from: i64,
    pub pcm: Pcm2,
}

pub type PSlot = std::sync::Arc<std::sync::OnceLock<Partial>>;

impl Pcm2 {
    /// 스테레오 프레임 수
    pub fn frames(&self) -> i64 {
        (match self {
            Pcm2::F32(x) => x.len(),
            Pcm2::I16(x) => x.len(),
        } / 2) as i64
    }
}

impl SoundBank {
    pub fn duration_ns(&self, sound: u32) -> i64 {
        let f = self.sounds[sound as usize].frames as i128;
        ((f * NS_PER_S as i128 + self.rate as i128 - 1) / self.rate as i128) as i64
    }
    pub fn ready(&self, sound: u32) -> bool {
        self.data[self.sounds[sound as usize].data as usize].get().is_some()
    }

    /// 데이터 슬롯 `d`의 프레임 `a`를 읽을 PCM: (PCM, 그 PCM의 슬롯 안 첫 프레임, 덮는 끝 프레임). 전체 우선, 없으면 `a`를 덮는 부분 슬롯.
    pub fn view(&self, d: usize, a: i64) -> Option<(&Pcm2, i64, i64)> {
        if let Some(p) = self.data[d].get() {
            return Some((p, 0, p.frames()));
        }
        let q = self.partial.get(d)?.get()?;
        let end = q.from + q.pcm.frames();
        (q.from <= a && a < end).then_some((&q.pcm, q.from, end))
    }

    /// 소리 `sound`의 슬롯 프레임 [a, b)를 지금 읽을 수 있나 (전체 슬롯, 또는 그 구간을 덮는 부분 슬롯). 소리 길이 밖은 필요 없다.
    pub fn covers(&self, sound: u32, a: i64, b: i64) -> bool {
        let s = self.sounds[sound as usize];
        let (a, b) = (a.max(0), b.min(s.frames as i64));
        if a >= b {
            return true;
        }
        let d = s.data as usize;
        if self.data[d].get().is_some() {
            return true;
        }
        self.partial.get(d).and_then(|p| p.get()).is_some_and(|q| q.from <= a && q.from + q.pcm.frames() >= b)
    }
    pub fn all_ready(&self) -> bool {
        self.data.iter().all(|d| d.get().is_some())
    }
    /// 상주 메모리 (준비된 슬롯, 바이트)
    pub fn bytes(&self) -> usize {
        self.data.iter().filter_map(|d| d.get()).map(|p| p.bytes()).sum::<usize>() + self.partial.iter().filter_map(|d| d.get()).map(|p| p.pcm.bytes()).sum::<usize>()
    }
}

/// 정해진 시각에 저절로 울리는 소리 (자동재생).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundEvent {
    pub t: i64,
    pub sound: u32,
    pub gain: f32,
}

/// 노트를 쳤을 때 울릴 소리 하나. `offset` = round(소리) − round(노트) (명세 3-1의 6, 아르페지오).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteSound {
    pub offset: i64,
    pub sound: u32,
    pub gain: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Tap,
    Long,
    Mine,
}

/// 노트 하나. 단노트·지뢰는 `end == t`, 롱노트는 `end > t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub t: i64,
    pub end: i64,
    /// `note_sounds[sounds_start .. sounds_start + sounds_len]`
    pub sounds_start: u32,
    pub sounds_len: u16,
    pub kind: NoteKind,
    /// 지뢰 피해 (게이지 %의 1/1000, −1 = 규칙 기본)
    pub damage: i32,
}

/// 빈 레인 소리 바꾸기: 이 시각부터 이 레인을 빈 손으로 치면 이 소리 (명세 2-3, G5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneSound {
    pub t: i64,
    pub sound: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Key,
    Scratch,
    Side,
    Trigger,
    Pedal,
    Other,
}

impl Role {
    pub fn from_code(c: u8) -> Role {
        match c {
            0 => Role::Key,
            1 => Role::Scratch,
            2 => Role::Side,
            3 => Role::Trigger,
            4 => Role::Pedal,
            _ => Role::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub id: String,
    pub role: Role,
    pub notes: Vec<Note>,
    pub lane_sounds: Vec<LaneSound>,
}

/// 스크롤 지도의 꺾이는 점. `t` 이후로 위치는 `pos + speed × (시각 − t)`. 위치 단위는 박.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollPoint {
    pub t: i64,
    pub pos: f64,
    /// 박/초
    pub speed: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongRule {
    /// BMS LN: 끝 판정 없음 (끝 전에 떼면 끊김)
    Ln,
    /// 떼기 판정
    Cn,
    /// HCN: CP3은 CN으로 처리 (⚑)
    Hcn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyLane {
    PreviousNote,
    NextNote,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: String,
    pub chart_id: String,
    pub layout: String,
    pub bank: SoundBank,
    /// 시간순 (같은 시각이면 작성 순서) — 정규 발음 순서의 자동재생 부분
    pub auto: Vec<SoundEvent>,
    pub note_sounds: Vec<NoteSound>,
    pub lanes: Vec<Lane>,
    pub groups: Vec<(String, u64)>,
    /// 시간순, 첫 점은 t ≤ 0
    pub scroll: Vec<ScrollPoint>,
    pub bars: Vec<i64>,
    /// 기준 스크롤 속도 (박/분)
    pub reference_scroll: f64,
    pub long_rule: LongRule,
    pub empty_lane: EmptyLane,
    /// 곡 끝 (이 시각 이후에는 이벤트가 없다)
    pub end: i64,
    /// 차분 해시 (기록·리플레이가 묶이는 값, 명세 3-5)
    pub chart_hash: [u8; 32],
    /// 고른 변형 선택 (변형마다 선택지 번호) — 리플레이에 기록
    pub variant_choice: Vec<u16>,
}

impl Chart {
    /// 시각 `t`의 스크롤 위치 (박).
    pub fn scroll_pos(&self, t: i64) -> f64 {
        let i = self.scroll.partition_point(|p| p.t <= t).saturating_sub(1);
        let p = &self.scroll[i];
        p.pos + p.speed * ((t - p.t) as f64 / NS_PER_S as f64)
    }

    /// 기준 스크롤 고르기 (3-4 설정 `scroll_base`, 박/분). 노출 시간은 이 속도에서 노트가 나타나 판정선에 닿기까지다.
    /// `chart` = 차분 필드 `reference_scroll`, `start` = 첫 노트 때 속도, `mode` = 첫·끝 노트 사이에서 가장 오래 머문 속도,
    /// `max`·`min` = 그 사이 0.5초 넘게 머문 속도 중 최고·최저 (순간 이동 기믹·정지를 빼려고 ⚑). 고를 것이 없으면 차분 필드.
    pub fn scroll_base(&self, kind: &str) -> f64 {
        let notes = self.lanes.iter().flat_map(|l| l.notes.iter());
        let (t0, t1) = notes.fold((i64::MAX, i64::MIN), |(a, b), n| (a.min(n.t), b.max(n.end)));
        if t0 > t1 || self.scroll.is_empty() {
            return self.reference_scroll;
        }
        let speed_at = |t: i64| self.scroll[self.scroll.partition_point(|p| p.t <= t).saturating_sub(1)].speed.abs();
        // 속도(박/초를 1e-9 단위로 묶음) → 머문 시간
        let mut dur: Vec<(i64, i64)> = Vec::new();
        for (i, p) in self.scroll.iter().enumerate() {
            let a = p.t.max(t0);
            let b = self.scroll.get(i + 1).map_or(t1, |q| q.t).min(t1);
            if b <= a || p.speed == 0.0 {
                continue;
            }
            let k = (p.speed.abs() * 1e9).round() as i64;
            match dur.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += b - a,
                None => dur.push((k, b - a)),
            }
        }
        let long: Vec<i64> = dur.iter().filter(|x| x.1 > NS_PER_S / 2).map(|x| x.0).collect();
        let bps = match kind {
            "start" => Some(speed_at(t0)).filter(|v| *v > 0.0),
            "mode" => dur.iter().max_by_key(|x| (x.1, x.0)).map(|x| x.0 as f64 / 1e9),
            "max" => long.iter().max().map(|&k| k as f64 / 1e9),
            "min" => long.iter().min().map(|&k| k as f64 / 1e9),
            _ => None,
        };
        bps.map_or(self.reference_scroll, |v| v * 60.0)
    }

    /// 플레이 세션 끝 (곡 시각 ns). **채보만으로** 정하고 플레이어 입력과 무관하다 — 끝난 뒤 계속 눌러도 늦춰지지 않는다.
    /// - 판정 끝 = 마지막 노트(롱노트는 끝) + `late_ns`(가장 늦은 판정 창). 노트가 없으면 곡 끝(`end`).
    /// - 소리 끝 = 채보가 정한 소리가 다 울리는 시각: 자동재생(반주 포함)은 시작 + 길이, 노트 키음은 가장 늦게 쳤을 때(노트 + offset + `late_ns`) + 길이.
    ///   빈 누름 소리처럼 입력이 만드는 소리는 넣지 않는다. 소리 길이는 실제 길이 그대로 쓴다(상한 없음 — CP3 마감 리뷰 #4: 곡 끝 + 30초
    ///   상한이 정상적인 긴 반주를 자르던 것). 길이는 소리 표에서 정해진 유한한 값이라 입력으로 늘어나지 않는다.
    /// - 세션 끝 = max(곡 끝, 판정 끝, 소리 끝) + `SESSION_MARGIN_NS`. 돌려주는 값 = (판정 끝, 소리 끝, 세션 끝).
    pub fn session_end(&self, late_ns: i64) -> (i64, i64, i64) {
        let notes = self.lanes.iter().flat_map(|l| l.notes.iter());
        let judge_end = notes.clone().map(|n| n.end.max(n.t)).max().map_or(self.end, |e| e + late_ns);
        let bank = &self.bank;
        let auto_end = self.auto.iter().map(|e| e.t + bank.duration_ns(e.sound)).max().unwrap_or(0);
        let note_end = notes
            .flat_map(|n| self.note_sounds[n.sounds_start as usize..n.sounds_start as usize + n.sounds_len as usize].iter().map(move |x| n.t + x.offset + late_ns + bank.duration_ns(x.sound)))
            .max()
            .unwrap_or(0);
        let sound_end = auto_end.max(note_end);
        (judge_end, sound_end, self.end.max(judge_end).max(sound_end) + SESSION_MARGIN_NS)
    }

    /// 판정 대상 수 (게이지 회복량 계산의 N): 탭 1, 롱노트 1(LN) 또는 2(CN·HCN: 머리 + 끝), 지뢰 제외.
    pub fn judge_count(&self) -> u32 {
        let per_long = if self.long_rule == LongRule::Ln { 1 } else { 2 };
        self.lanes
            .iter()
            .flat_map(|l| l.notes.iter())
            .map(|n| match n.kind {
                NoteKind::Tap => 1,
                NoteKind::Long => per_long,
                NoteKind::Mine => 0,
            })
            .sum()
    }
}

/// 정규 시간 → 이 뱅크 속도의 프레임 (가장 가까운 정수, 반올림 반 프레임 이하, 음수 대칭).
pub fn ns_to_frame(t: i64, rate: u32) -> i64 {
    let x = t as i128 * rate as i128;
    let half = NS_PER_S as i128 / 2;
    (if x >= 0 { (x + half) / NS_PER_S as i128 } else { -((-x + half) / NS_PER_S as i128) }) as i64
}

#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub chart: Chart,
    /// 보이스 풀 크기 (최대 동시 발음 + 여유). 플레이 중 할당하지 않도록 미리 잡는다.
    pub voice_budget: u32,
}
