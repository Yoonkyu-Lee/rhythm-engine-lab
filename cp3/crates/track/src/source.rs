//! 작성 형식(TOML)의 구조 (명세 2장). 알 수 없는 키는 오류(`deny_unknown_fields`).

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackToml {
    pub format: String,
    pub id: String,
    pub revision: u32,
    pub info: Info,
    pub origin: Origin,
    pub layout: Vec<LayoutDef>,
    pub sound: Vec<SoundDef>,
    #[serde(default)]
    pub media: Vec<MediaDef>,
    pub source: Option<SourceDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub title: String,
    pub title_sort: Option<String>,
    pub artist: Option<String>,
    pub genre: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub highlight: Option<String>,
    #[serde(default)]
    pub lang: BTreeMap<String, LangInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LangInfo {
    pub title: Option<String>,
    pub title_sort: Option<String>,
    pub artist: Option<String>,
    pub genre: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub anchor_chart: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutDef {
    pub id: String,
    pub lanes: Vec<LaneDef>,
    #[serde(default)]
    pub groups: Vec<GroupDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneDef {
    pub id: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupDef {
    pub id: String,
    pub lanes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDef {
    pub id: String,
    pub file: String,
    pub gain: Option<String>,
    pub start: Option<String>,
    pub retrigger: Option<String>,
    pub choke: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaDef {
    pub hash: String,
    pub kind: String,
    pub role: Option<String>,
    pub start: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDef {
    pub format: String,
    pub converter: Option<String>,
    #[serde(default)]
    pub files: Vec<SourceFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    pub path: String,
    pub hash: String,
}

/// 차분 파일. 일반 차분과 배속 변형(`base`가 있음)이 같은 구조를 쓰고, 종류별 허용 키는 compile에서 검사한다.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartToml {
    pub id: String,
    pub name: Option<String>,
    pub level: Option<String>,
    pub layout: Option<String>,
    pub shift: Option<String>,
    pub reference_scroll: Option<String>,
    pub base: Option<String>,
    pub time_scale: Option<String>,
    pub rules: Option<Rules>,
    pub timing: Option<TimingDef>,
    pub scroll: Option<ScrollDef>,
    #[serde(default)]
    pub event: Vec<EventDef>,
    #[serde(default)]
    pub note: Vec<NoteDef>,
    #[serde(default)]
    pub lane_sound: Vec<LaneSoundDef>,
    #[serde(default)]
    pub variant: Vec<VariantDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub long_note: Option<String>,
    pub empty_lane: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingDef {
    pub unit: String,
    pub ticks_per_beat: Option<u32>,
    #[serde(default)]
    pub tempo: Vec<TempoDef>,
    #[serde(default)]
    pub meter: Vec<MeterDef>,
    #[serde(default)]
    pub stops: Vec<StopDef>,
    #[serde(default)]
    pub bars: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempoDef {
    pub at: String,
    pub bpm: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterDef {
    pub measure: u32,
    pub length: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopDef {
    pub at: String,
    pub beats: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollDef {
    pub points: Vec<ScrollPointDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollPointDef {
    pub at: String,
    pub pos: Option<String>,
    pub speed: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum When {
    One(String),
    All(Vec<String>),
}

impl When {
    pub fn terms(&self) -> Vec<&str> {
        match self {
            When::One(s) => vec![s.as_str()],
            When::All(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    pub at: String,
    pub sound: String,
    pub gain: Option<String>,
    pub when: Option<When>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteDef {
    pub at: String,
    pub lane: String,
    pub kind: String,
    pub end: Option<String>,
    #[serde(default)]
    pub sounds: Vec<NoteSoundDef>,
    pub damage: Option<String>,
    pub when: Option<When>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NoteSoundDef {
    Id(String),
    Full(NoteSoundFull),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteSoundFull {
    pub sound: String,
    pub at: Option<String>,
    pub gain: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSoundDef {
    pub at: String,
    pub lane: String,
    pub sound: String,
    pub when: Option<When>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantDef {
    pub id: String,
    pub choices: Vec<String>,
    pub weights: Option<Vec<String>>,
}
