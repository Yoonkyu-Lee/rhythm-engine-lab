//! 작성 형식 → 컴파일된 Track (명세 3장). 묶음 바이트로 쓰는 것은 `pack`.
//!
//! 차분은 먼저 유리수 시각의 중간 형태(`QChart`)로 만들고, 배속 변형은 그 유리수 시각을 나눈 뒤 **한 번** 반올림한다
//! (반올림된 ns를 다시 나누지 않는다).

use crate::audio::{self, Packed};
use crate::rational::{self, int, parse, sec_to_ns, to_f32, to_f64, Q};
use crate::source::*;
use crate::timing::{Timing, Unit};
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub const FORMAT_MAJOR: u16 = 1;
pub const FORMAT_MINOR: u16 = 3;
pub const BIN_NS: i64 = 250_000_000;

#[derive(Clone, Debug)]
pub struct LayoutOut {
    pub id: String,
    pub lanes: Vec<(String, u8)>,
    pub groups: Vec<(String, u64)>,
}

#[derive(Clone, Debug)]
pub struct SoundIdOut {
    pub id: String,
    pub data: u32,
    pub start: u32,
    pub gain: Q,
    pub retrigger: u8,
    /// 0 = 없음, 그 밖 = Track choke 이름 번호 + 1
    pub choke: u32,
}

#[derive(Clone, Debug)]
pub struct SoundDataOut {
    pub hash: [u8; 32],
    pub packed: Packed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalSound {
    pub hash: [u8; 32],
    pub start: u32,
    pub gain: f32,
    pub retrigger: u8,
    pub choke: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteRec {
    pub t: i64,
    pub end: i64,
    pub ns_start: u32,
    pub ns_len: u16,
    pub kind: u8,
    pub cond: u32,
    pub damage: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaneRec {
    pub id: String,
    pub role: u8,
    pub notes: Vec<NoteRec>,
    pub lsnds: Vec<(i64, u32, u32)>,
}

/// 실행 형식 `CHART` 구역의 내용 (명세 4장).
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSec {
    pub shift: i64,
    pub end: i64,
    pub reference_scroll: f64,
    pub long_note: u8,
    pub empty_lane: u8,
    pub voice_budget: u32,
    pub sounds: Vec<LocalSound>,
    pub lanes: Vec<LaneRec>,
    pub groups: Vec<(String, u64)>,
    /// (t, sound, gain, cond)
    pub auto: Vec<(i64, u32, f32, u32)>,
    /// (offset, sound, gain)
    pub note_sounds: Vec<(i64, u32, f32)>,
    pub scroll: Vec<(i64, f64, f64)>,
    pub bars: Vec<i64>,
    /// (id, 선택지 수, 가중치)
    pub variants: Vec<(String, Vec<u32>)>,
    /// 조건: 0번은 빈 조건
    pub conds: Vec<Vec<(u16, u16)>>,
}

#[derive(Clone, Debug)]
pub struct ChartStats {
    pub notes: u32,
    pub bins: Vec<u16>,
    pub highlight: i64,
    pub max_nps: u32,
}

#[derive(Clone, Debug)]
pub struct ChartOut {
    pub id: String,
    pub name: String,
    pub level: String,
    pub base: Option<u32>,
    pub scale: (i64, i64),
    pub sec: ChartSec,
    /// 차분 안 소리 번호 → Track 논리 소리 번호
    pub local_to_track: Vec<u32>,
    pub stats: ChartStats,
}

#[derive(Clone, Debug)]
pub struct MediaOut {
    pub hash: String,
    pub kind: String,
    pub role: String,
    pub start: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct TrackOut {
    pub id: [u8; 16],
    pub revision: u32,
    pub info: InfoOut,
    pub layouts: Vec<LayoutOut>,
    pub sound_ids: Vec<SoundIdOut>,
    pub chokes: Vec<String>,
    pub datas: Vec<SoundDataOut>,
    pub charts: Vec<ChartOut>,
    pub media: Vec<MediaOut>,
    /// SOURCE 구역: (상대 경로, 바이트)
    pub source: Vec<(String, Vec<u8>)>,
    /// 경고 (보고서용)
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct InfoOut {
    pub title: String,
    pub title_sort: String,
    pub artist: String,
    pub genre: String,
    pub tags: Vec<String>,
    /// Track 시각 ns
    pub highlight: Option<i64>,
    pub lang: Vec<(String, [String; 4])>,
    pub source_format: String,
    pub converter: String,
    pub source_files: Vec<(String, String)>,
}

fn role_code(r: &str) -> Result<u8, String> {
    Ok(match r {
        "key" => 0,
        "scratch" => 1,
        "side" => 2,
        "trigger" => 3,
        "pedal" => 4,
        _ => return Err(format!("알 수 없는 레인 역할: {r}")),
    })
}

fn parse_uuid(s: &str) -> Result<[u8; 16], String> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || s.len() != 36 {
        return Err(format!("Track id는 UUID 형식: {s}"));
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| format!("Track id는 UUID 형식: {s}"))?;
    }
    Ok(out)
}

fn uniq<'a>(what: &str, it: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for x in it {
        if !seen.insert(x) {
            return Err(format!("{what}: 중복 ID '{x}'"));
        }
    }
    Ok(())
}

/// 작성 형식 폴더 → 컴파일된 Track.
pub fn compile_dir(dir: &Path) -> Result<TrackOut, String> {
    let track_text = std::fs::read_to_string(dir.join("track.toml")).map_err(|e| format!("track.toml: {e}"))?;
    let tr: TrackToml = toml::from_str(&track_text).map_err(|e| format!("track.toml: {e}"))?;
    match tr.format.split_once('.') {
        Some(("1", _)) => {}
        _ => return Err(format!("형식 버전 {}는 읽을 수 없음 (주 버전 1)", tr.format)),
    }
    let id = parse_uuid(&tr.id)?;
    let mut warnings = Vec::new();

    // 레이아웃
    uniq("layout", tr.layout.iter().map(|l| l.id.as_str()))?;
    let mut layouts = Vec::new();
    for l in &tr.layout {
        uniq(&format!("layout {} 레인", l.id), l.lanes.iter().map(|x| x.id.as_str()))?;
        uniq(&format!("layout {} 묶음", l.id), l.groups.iter().map(|x| x.id.as_str()))?;
        if l.lanes.is_empty() || l.lanes.len() > 64 {
            return Err(format!("layout {}: 레인 수 1~64", l.id));
        }
        let lanes: Vec<(String, u8)> = l.lanes.iter().map(|x| Ok((x.id.clone(), role_code(&x.role)?))).collect::<Result<_, String>>()?;
        let mut groups = Vec::new();
        for g in &l.groups {
            let mut mask = 0u64;
            for ln in &g.lanes {
                let i = lanes.iter().position(|x| &x.0 == ln).ok_or_else(|| format!("layout {} 묶음 {}: 없는 레인 {ln}", l.id, g.id))?;
                mask |= 1 << i;
            }
            groups.push((g.id.clone(), mask));
        }
        layouts.push(LayoutOut { id: l.id.clone(), lanes, groups });
    }

    // 소리
    uniq("sound", tr.sound.iter().map(|s| s.id.as_str()))?;
    let mut datas: Vec<SoundDataOut> = Vec::new();
    let mut data_by_hash: HashMap<[u8; 32], u32> = HashMap::new();
    let mut sound_ids = Vec::new();
    let mut chokes: Vec<String> = Vec::new();
    let mut sound_dur: Vec<(u64, u32)> = Vec::new(); // (start 뒤 샘플 수, rate) — 보이스 예산용
    for s in &tr.sound {
        let bytes = std::fs::read(dir.join(&s.file)).map_err(|e| format!("소리 {} ({}): {e}", s.id, s.file))?;
        let packed = audio::pack_file(&bytes).map_err(|e| format!("소리 {} ({}): {e}", s.id, s.file))?;
        let pcm = audio::decode(&packed).map_err(|e| format!("소리 {}: {e}", s.id))?;
        if packed.frames == 0 {
            return Err(format!("소리 {}: 샘플 수 0", s.id));
        }
        let hash = audio::sound_hash(packed.rate, packed.channels, packed.frames, &pcm);
        let start = match &s.start {
            None => 0,
            Some(x) => {
                let q = parse(x)?;
                if !q.is_integer() || q.is_negative() {
                    return Err(format!("소리 {}: start는 0 이상의 정수 샘플 수", s.id));
                }
                q.to_integer().to_u32().ok_or("start가 너무 큼")?
            }
        };
        if start as u64 >= packed.frames {
            return Err(format!("소리 {}: start {start}가 길이 {} 이상", s.id, packed.frames));
        }
        let gain = parse(s.gain.as_deref().unwrap_or("1"))?;
        if gain.is_negative() {
            return Err(format!("소리 {}: 이득이 음수", s.id));
        }
        let retrigger = match s.retrigger.as_deref().unwrap_or("overlap") {
            "overlap" => 0,
            "cut" => 1,
            r => return Err(format!("소리 {}: retrigger 값 오류 {r}", s.id)),
        };
        let choke = match &s.choke {
            None => 0,
            Some(c) => {
                let i = match chokes.iter().position(|x| x == c) {
                    Some(i) => i,
                    None => {
                        chokes.push(c.clone());
                        chokes.len() - 1
                    }
                };
                i as u32 + 1
            }
        };
        sound_dur.push((packed.frames - start as u64, packed.rate));
        let data = *data_by_hash.entry(hash).or_insert_with(|| {
            datas.push(SoundDataOut { hash, packed });
            datas.len() as u32 - 1
        });
        sound_ids.push(SoundIdOut { id: s.id.clone(), data, start, gain, retrigger, choke });
    }

    // 정보
    let ms_to_ns = |s: &str| -> Result<i64, String> { sec_to_ns(&(parse(s)? / int(1000))) };
    let mut info = InfoOut {
        title: tr.info.title.clone(),
        title_sort: tr.info.title_sort.clone().unwrap_or_default(),
        artist: tr.info.artist.clone().unwrap_or_default(),
        genre: tr.info.genre.clone().unwrap_or_default(),
        tags: tr.info.tags.clone(),
        highlight: tr.info.highlight.as_deref().map(ms_to_ns).transpose()?,
        ..Default::default()
    };
    for (k, v) in &tr.info.lang {
        info.lang.push((
            k.clone(),
            [
                v.title.clone().unwrap_or_default(),
                v.title_sort.clone().unwrap_or_default(),
                v.artist.clone().unwrap_or_default(),
                v.genre.clone().unwrap_or_default(),
            ],
        ));
    }
    if let Some(src) = &tr.source {
        info.source_format = src.format.clone();
        info.converter = src.converter.clone().unwrap_or_default();
        info.source_files = src.files.iter().map(|f| (f.path.clone(), f.hash.clone())).collect();
    }
    let media = tr
        .media
        .iter()
        .map(|m| {
            if !matches!(m.kind.as_str(), "video" | "image_pack" | "thumbnail" | "cover") {
                return Err(format!("media kind 오류: {}", m.kind));
            }
            Ok(MediaOut { hash: m.hash.clone(), kind: m.kind.clone(), role: m.role.clone().unwrap_or_default(), start: m.start.as_deref().map(ms_to_ns).transpose()? })
        })
        .collect::<Result<Vec<_>, String>>()?;

    // 차분
    let mut source = vec![("track.toml".to_string(), track_text.into_bytes())];
    let mut files: Vec<_> = std::fs::read_dir(dir.join("charts"))
        .map_err(|e| format!("charts/: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
        .collect();
    files.sort();
    let mut raw: Vec<(String, ChartToml)> = Vec::new();
    for p in &files {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let c: ChartToml = toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        if c.id != stem {
            return Err(format!("차분 id ≠ 파일 이름: {} / {stem}", c.id));
        }
        source.push((format!("charts/{stem}.toml"), text.into_bytes()));
        raw.push((stem, c));
    }
    if let Ok(r) = std::fs::read(dir.join("report.md")) {
        source.push(("report.md".into(), r));
    }
    let ids: Vec<&str> = raw.iter().map(|(s, _)| s.as_str()).collect();
    if !ids.contains(&tr.origin.anchor_chart.as_str()) {
        return Err(format!("원점 차분 없음: {}", tr.origin.anchor_chart));
    }
    let ctx = Ctx { layouts: &layouts, sound_ids: &sound_ids, sound_dur: &sound_dur, track_sounds: &tr.sound };

    let mut qcharts: HashMap<String, QChart> = HashMap::new();
    for (cid, c) in &raw {
        if c.base.is_none() {
            qcharts.insert(cid.clone(), compile_q(cid, c, &ctx)?);
        }
    }
    let mut charts = Vec::new();
    for (cid, c) in &raw {
        let (q, scale, shift, refs, base) = match &c.base {
            None => {
                let q = qcharts[cid].clone();
                let shift = parse(c.shift.as_deref().unwrap_or("0"))? / int(1000);
                let refs = parse(c.reference_scroll.as_deref().ok_or_else(|| format!("{cid}: reference_scroll 필요"))?)?;
                (q, Q::one(), shift, refs, None)
            }
            Some(b) => {
                for (k, present) in [
                    ("layout", c.layout.is_some()),
                    ("rules", c.rules.is_some()),
                    ("timing", c.timing.is_some()),
                    ("scroll", c.scroll.is_some()),
                    ("event", !c.event.is_empty()),
                    ("note", !c.note.is_empty()),
                    ("lane_sound", !c.lane_sound.is_empty()),
                    ("variant", !c.variant.is_empty()),
                ] {
                    if present {
                        return Err(format!("{cid}: 배속 변형은 '{k}'를 물려받으므로 적을 수 없음 (G6)"));
                    }
                }
                let bq = qcharts.get(b).ok_or_else(|| format!("{cid}: base '{b}'가 없거나 또 배속 변형"))?;
                let sc = parse(c.time_scale.as_deref().ok_or_else(|| format!("{cid}: time_scale 필요"))?)?;
                if !sc.is_positive() {
                    return Err(format!("{cid}: time_scale ≤ 0"));
                }
                let bc = &raw.iter().find(|(s, _)| s == b).unwrap().1;
                let bshift = parse(bc.shift.as_deref().unwrap_or("0"))? / int(1000);
                let brefs = parse(bc.reference_scroll.as_deref().unwrap_or("0"))?;
                let shift = match &c.shift {
                    Some(s) => parse(s)? / int(1000),
                    None => &bshift / &sc,
                };
                let refs = match &c.reference_scroll {
                    Some(s) => parse(s)?,
                    None => &brefs * &sc,
                };
                let bi = ids.iter().position(|x| x == b).unwrap() as u32;
                (bq.clone(), sc, shift, refs, Some(bi))
            }
        };
        let (sec, local_to_track) = finalize(cid, &q, &scale, &shift, &refs, &ctx)?;
        let stats = chart_stats(&q, &sec, &scale, info.highlight);
        let scale_i = (scale.numer().to_i64().ok_or("time_scale 분자 범위")?, scale.denom().to_i64().ok_or("time_scale 분모 범위")?);
        charts.push(ChartOut {
            id: cid.clone(),
            name: c.name.clone().unwrap_or_default(),
            level: c.level.clone().unwrap_or_default(),
            base,
            scale: scale_i,
            sec,
            local_to_track,
            stats,
        });
    }
    warnings.extend(layer_warnings(&charts, &sound_ids, &datas));
    let mut t = TrackOut { id, revision: tr.revision, info, layouts, sound_ids, chokes, datas, charts, media, source, warnings };
    fill_local_hashes(&mut t);
    Ok(t)
}

struct Ctx<'a> {
    layouts: &'a [LayoutOut],
    sound_ids: &'a [SoundIdOut],
    sound_dur: &'a [(u64, u32)],
    track_sounds: &'a [SoundDef],
}

impl Ctx<'_> {
    fn sound(&self, cid: &str, id: &str) -> Result<u32, String> {
        self.track_sounds.iter().position(|s| s.id == id).map(|i| i as u32).ok_or_else(|| format!("{cid}: 없는 소리 {id}"))
    }
}

/// 유리수 시각의 차분 (배속 변형 전).
#[derive(Clone, Debug)]
struct QChart {
    layout: usize,
    long_note: u8,
    empty_lane: u8,
    /// (초, Track 소리, 이득, 조건, 작성 순번)
    auto: Vec<(Q, u32, Q, Vec<(u16, u16)>, usize)>,
    /// 노트: (레인, 초, 끝 초, 종류, 조건, damage(1/1000 %, -1), 소리들 (초, Track 소리, 이득))
    notes: Vec<(usize, Q, Q, u8, Vec<(u16, u16)>, i32, Vec<(Q, u32, Q)>)>,
    /// (레인, 초, Track 소리, 조건)
    lsnds: Vec<(usize, Q, u32, Vec<(u16, u16)>)>,
    /// (초, 위치 박, 속도 박/초)
    scroll: Vec<(Q, Q, Q)>,
    bars: Vec<Q>,
    variants: Vec<(String, Vec<String>, Vec<u32>)>,
}

fn compile_q(cid: &str, c: &ChartToml, ctx: &Ctx) -> Result<QChart, String> {
    if c.time_scale.is_some() {
        return Err(format!("{cid}: time_scale은 배속 변형(base)에만"));
    }
    let lid = c.layout.as_deref().ok_or_else(|| format!("{cid}: layout 필요"))?;
    let layout = ctx.layouts.iter().position(|l| l.id == lid).ok_or_else(|| format!("{cid}: 없는 레이아웃 {lid}"))?;
    let lanes = &ctx.layouts[layout].lanes;
    let lane_idx = |id: &str| lanes.iter().position(|x| x.0 == id).ok_or_else(|| format!("{cid}: 레이아웃에 없는 레인 {id}"));
    let rules = c.rules.as_ref();
    let long_note = match rules.and_then(|r| r.long_note.as_deref()).unwrap_or("cn") {
        "ln" => 0,
        "cn" => 1,
        "hcn" => 2,
        x => return Err(format!("{cid}: long_note 값 오류 {x}")),
    };
    let empty_lane = match rules.and_then(|r| r.empty_lane.as_deref()).unwrap_or("previous_note") {
        "previous_note" => 0,
        "next_note" => 1,
        "none" => 2,
        x => return Err(format!("{cid}: empty_lane 값 오류 {x}")),
    };
    let tm = Timing::new(c.timing.as_ref().ok_or_else(|| format!("{cid}: timing 필요"))?).map_err(|e| format!("{cid} timing: {e}"))?;
    uniq(&format!("{cid} 변형"), c.variant.iter().map(|v| v.id.as_str()))?;
    let mut variants = Vec::new();
    for v in &c.variant {
        uniq(&format!("{cid} 변형 {} 선택", v.id), v.choices.iter().map(|s| s.as_str()))?;
        if v.choices.is_empty() {
            return Err(format!("{cid} 변형 {}: 선택지 없음", v.id));
        }
        let w = match &v.weights {
            None => vec![1; v.choices.len()],
            Some(ws) => {
                if ws.len() != v.choices.len() {
                    return Err(format!("{cid} 변형 {}: weights 수 ≠ choices 수", v.id));
                }
                ws.iter()
                    .map(|x| {
                        let q = parse(x)?;
                        q.is_integer().then(|| q.to_integer().to_u32()).flatten().filter(|n| *n > 0).ok_or_else(|| format!("{cid}: 가중치는 양의 정수 {x}"))
                    })
                    .collect::<Result<Vec<_>, String>>()?
            }
        };
        variants.push((v.id.clone(), v.choices.clone(), w));
    }
    let cond = |w: &Option<When>| -> Result<Vec<(u16, u16)>, String> {
        let mut out = Vec::new();
        if let Some(w) = w {
            for t in w.terms() {
                let (vid, ch) = t.split_once('=').ok_or_else(|| format!("{cid}: when 형식은 '변형=선택': {t}"))?;
                let vi = variants.iter().position(|v| v.0 == vid).ok_or_else(|| format!("{cid}: 없는 변형 {vid}"))?;
                let ci = variants[vi].1.iter().position(|x| x == ch).ok_or_else(|| format!("{cid}: 없는 변형 선택 {t}"))?;
                if out.iter().any(|(v, _): &(u16, u16)| *v == vi as u16) {
                    return Err(format!("{cid}: when에 같은 변형이 두 번 {t}"));
                }
                out.push((vi as u16, ci as u16));
            }
        }
        out.sort();
        Ok(out)
    };
    let gain_of = |g: &Option<String>| -> Result<Q, String> {
        let q = parse(g.as_deref().unwrap_or("1"))?;
        if q.is_negative() {
            return Err(format!("{cid}: 이득이 음수"));
        }
        Ok(q)
    };
    let mut auto = Vec::new();
    for (i, e) in c.event.iter().enumerate() {
        auto.push((tm.seconds(&e.at)?, ctx.sound(cid, &e.sound)?, gain_of(&e.gain)?, cond(&e.when)?, i));
    }
    let mut notes = Vec::new();
    for n in &c.note {
        let lane = lane_idx(&n.lane)?;
        let kind = match n.kind.as_str() {
            "tap" => 0,
            "long" => 1,
            "mine" => 2,
            k => return Err(format!("{cid}: 노트 종류 오류 {k}")),
        };
        let t = tm.seconds(&n.at)?;
        let end = match (&n.end, kind) {
            (Some(e), 1) => tm.seconds(e)?,
            (None, 1) => return Err(format!("{cid}: 롱노트에 end 없음 ({})", n.at)),
            (Some(_), _) => return Err(format!("{cid}: end는 롱노트에만 ({})", n.at)),
            (None, _) => t.clone(),
        };
        let damage = match (&n.damage, kind) {
            (None, _) => -1,
            (Some(d), 2) => {
                let q = parse(d)?;
                if q.is_negative() || q > int(100) {
                    return Err(format!("{cid}: damage는 0~100 ({d})"));
                }
                rational::round_half_even(&(q * int(1000)))? as i32
            }
            (Some(_), _) => return Err(format!("{cid}: damage는 지뢰에만")),
        };
        let mut snds = Vec::new();
        for s in &n.sounds {
            let (sid, at, gain) = match s {
                NoteSoundDef::Id(id) => (id.as_str(), None, None),
                NoteSoundDef::Full(f) => (f.sound.as_str(), f.at.as_deref(), f.gain.clone()),
            };
            let ts = match at {
                None => t.clone(),
                Some(r) => tm.seconds_rel(&n.at, r)?,
            };
            snds.push((ts, ctx.sound(cid, sid)?, gain_of(&gain)?));
        }
        notes.push((lane, t, end, kind, cond(&n.when)?, damage, snds));
    }
    let mut lsnds = Vec::new();
    for ls in &c.lane_sound {
        lsnds.push((lane_idx(&ls.lane)?, tm.seconds(&ls.at)?, ctx.sound(cid, &ls.sound)?, cond(&ls.when)?));
    }
    // 스크롤 (명세 3-2)
    let sixty = int(60);
    let mut scroll: Vec<(Q, Q, Q)> = Vec::new();
    if let Some(sd) = &c.scroll {
        let mut prev: Option<(Q, Q, Q)> = None;
        for p in &sd.points {
            let t = tm.seconds(&p.at)?;
            let spd = parse(&p.speed)? / &sixty;
            let pos = match (&prev, &p.pos) {
                (None, pos) => {
                    if !t.is_zero() {
                        return Err(format!("{cid}: 첫 스크롤 점은 at = 0"));
                    }
                    pos.as_deref().map(parse).transpose()?.unwrap_or_else(Q::zero)
                }
                (Some(_), Some(p)) => parse(p)?,
                (Some((pt, pp, ps)), None) => pp + ps * (&t - pt),
            };
            if let Some((pt, _, _)) = &prev {
                if &t < pt {
                    return Err(format!("{cid}: 스크롤 점이 시간순이 아님"));
                }
            }
            prev = Some((t.clone(), pos.clone(), spd.clone()));
            scroll.push((t, pos, spd));
        }
    } else if tm.unit == Unit::Ms {
        if tm.tempo.is_empty() {
            return Err(format!("{cid}: unit = ms에 tempo도 scroll도 없음"));
        }
        let mut pos = Q::zero();
        let mut prev: Option<(Q, Q)> = None;
        for (ms, bpm) in &tm.tempo {
            let t = ms / int(1000);
            if let Some((pt, pb)) = &prev {
                pos += (&t - pt) * pb / &sixty;
            }
            scroll.push((t.clone(), pos.clone(), bpm / &sixty));
            prev = Some((t, bpm.clone()));
        }
        if !scroll[0].0.is_zero() {
            let s0 = scroll[0].2.clone();
            scroll.insert(0, (Q::zero(), Q::zero(), s0));
        }
    } else {
        for (b, bpm) in &tm.tempo {
            scroll.push((tm.sec_of_beat(b), b.clone(), bpm / &sixty));
        }
    }
    // STOP 구간 삽입
    for (t0, d) in tm.stop_spans() {
        let i = scroll.iter().rposition(|p| p.0 <= t0).unwrap_or(0);
        let (pt, pp, ps) = scroll[i].clone();
        let pos = &pp + &ps * (&t0 - &pt);
        let mut later: Vec<_> = scroll.split_off(i + 1);
        scroll.push((t0.clone(), pos.clone(), Q::zero()));
        scroll.push((&t0 + &d, pos, ps));
        scroll.append(&mut later);
    }
    // 마디선
    let mut bars = Vec::new();
    if !tm.bars.is_empty() {
        for b in &tm.bars {
            bars.push(tm.seconds(b)?);
        }
    } else if tm.has_meter() && tm.unit != Unit::Ms {
        let last = auto
            .iter()
            .map(|a| a.0.clone())
            .chain(notes.iter().map(|n| n.2.clone()))
            .chain(lsnds.iter().map(|l| l.1.clone()))
            .max()
            .unwrap_or_else(Q::zero);
        let mut m = 0u32;
        loop {
            let t = tm.sec_of_beat(&tm.measure_start(m));
            if t > last {
                break;
            }
            bars.push(t);
            m += 1;
        }
    }
    Ok(QChart { layout, long_note, empty_lane, auto, notes, lsnds, scroll, bars, variants })
}

fn ns(q: &Q) -> Result<i64, String> {
    sec_to_ns(q)
}

/// 유리수 차분 + 배속 → 실행 형식 차분 구역.
fn finalize(cid: &str, q: &QChart, scale: &Q, shift: &Q, refs: &Q, ctx: &Ctx) -> Result<(ChartSec, Vec<u32>), String> {
    let at = |x: &Q| ns(&(x / scale));
    let layout = &ctx.layouts[q.layout];
    // 조건 표
    let mut conds: Vec<Vec<(u16, u16)>> = vec![vec![]];
    let mut cond_of = |c: &Vec<(u16, u16)>| -> u32 {
        if c.is_empty() {
            return 0;
        }
        match conds.iter().position(|x| x == c) {
            Some(i) => i as u32,
            None => {
                conds.push(c.clone());
                conds.len() as u32 - 1
            }
        }
    };
    // 소리 시작 (정규 발음 순서, 명세 3-4): (ns, 종류, 순번…, Track 소리)
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Start {
        t: i64,
        kind: u8,
        seq: (usize, usize, usize),
        sound: u32,
    }
    let mut starts = Vec::new();
    for a in &q.auto {
        starts.push(Start { t: at(&a.0)?, kind: 0, seq: (a.4, 0, 0), sound: a.1 });
    }
    for (ni, n) in q.notes.iter().enumerate() {
        for (j, s) in n.6.iter().enumerate() {
            starts.push(Start { t: at(&s.0)?, kind: 1, seq: (n.0, ni, j), sound: s.1 });
        }
    }
    starts.sort();
    // 차분 안 소리 표: 정규 순서에서 처음 나오는 순서, 그다음 lane_sound에만 쓰인 소리(레인 선언·시각 순)
    let mut local_to_track: Vec<u32> = Vec::new();
    let mut local_of: HashMap<u32, u32> = HashMap::new();
    let mut add = |s: u32, l2t: &mut Vec<u32>| -> u32 {
        *local_of.entry(s).or_insert_with(|| {
            l2t.push(s);
            l2t.len() as u32 - 1
        })
    };
    for s in &starts {
        add(s.sound, &mut local_to_track);
    }
    let mut ls_sorted: Vec<_> = q.lsnds.iter().enumerate().collect();
    ls_sorted.sort_by(|a, b| (a.1 .0, &a.1 .1, a.0).cmp(&(b.1 .0, &b.1 .1, b.0)));
    for (_, l) in &ls_sorted {
        add(l.2, &mut local_to_track);
    }
    // 차분 안 choke 번호: 차분 안 소리 순서에서 처음 나오는 순서
    let mut local_chokes: Vec<u32> = Vec::new();
    let sounds: Vec<LocalSound> = local_to_track
        .iter()
        .map(|&ts| {
            let sid = &ctx.sound_ids[ts as usize];
            let choke = if sid.choke == 0 {
                0
            } else {
                match local_chokes.iter().position(|c| *c == sid.choke) {
                    Some(i) => i as u32 + 1,
                    None => {
                        local_chokes.push(sid.choke);
                        local_chokes.len() as u32
                    }
                }
            };
            LocalSound { hash: [0; 32], start: sid.start, gain: to_f32(&sid.gain), retrigger: sid.retrigger, choke }
        })
        .collect();
    // 자동재생
    let mut auto: Vec<(i64, usize, u32, f32, u32)> = Vec::new();
    for a in &q.auto {
        auto.push((at(&a.0)?, a.4, local_of[&a.1], to_f32(&a.2), cond_of(&a.3)));
    }
    auto.sort_by_key(|x| (x.0, x.1));
    // 노트·노트 소리 (레인마다 시각순, 같은 시각이면 작성 순서)
    let mut lanes: Vec<LaneRec> = layout.lanes.iter().map(|(id, role)| LaneRec { id: id.clone(), role: *role, notes: vec![], lsnds: vec![] }).collect();
    let mut order: Vec<usize> = (0..q.notes.len()).collect();
    order.sort_by_key(|&i| (q.notes[i].0, at(&q.notes[i].1).unwrap_or(i64::MAX), i));
    let mut note_sounds = Vec::new();
    for i in order {
        let n = &q.notes[i];
        let t = at(&n.1)?;
        let end = at(&n.2)?;
        if n.3 == 1 && end <= t {
            return Err(format!("{cid}: 롱노트 끝 ≤ 시작 (ns 반올림 뒤, 레인 {})", layout.lanes[n.0].0));
        }
        let ns_start = note_sounds.len() as u32;
        for s in &n.6 {
            note_sounds.push((at(&s.0)? - t, local_of[&s.1], to_f32(&s.2))); // round(소리) − round(노트) (명세 3-1의 6)
        }
        lanes[n.0].notes.push(NoteRec {
            t,
            end,
            ns_start,
            ns_len: u16::try_from(n.6.len()).map_err(|_| format!("{cid}: 노트 소리가 너무 많음"))?,
            kind: n.3,
            cond: cond_of(&n.4),
            damage: n.5,
        });
    }
    for (_, l) in &ls_sorted {
        lanes[l.0].lsnds.push((at(&l.1)?, local_of[&l.2], cond_of(&l.3)));
    }
    // 같은 레인 겹침: 변형 선택 조합마다 (ns 반올림 뒤)
    let combos: Vec<Vec<u16>> = q.variants.iter().fold(vec![vec![]], |acc, v| {
        acc.iter().flat_map(|c| (0..v.1.len() as u16).map(move |i| [c.clone(), vec![i]].concat())).collect()
    });
    for combo in &combos {
        let on = |ci: u32| conds[ci as usize].iter().all(|(v, ch)| combo[*v as usize] == *ch);
        for lane in &lanes {
            let mut prev_end: Option<i64> = None;
            for n in lane.notes.iter().filter(|n| on(n.cond)) {
                if let Some(pe) = prev_end {
                    if n.t <= pe {
                        return Err(format!("{cid}: 레인 {} 노트 겹침 (변형 {combo:?}, ns {})", lane.id, n.t));
                    }
                }
                prev_end = Some(n.end);
            }
        }
    }
    // 스크롤·마디선
    let scroll: Vec<(i64, f64, f64)> =
        q.scroll.iter().map(|(t, p, s)| Ok((at(t)?, to_f64(p), to_f64(&(s * scale))))).collect::<Result<_, String>>()?;
    let bars: Vec<i64> = q.bars.iter().map(at).collect::<Result<_, String>>()?;
    let end = auto
        .iter()
        .map(|a| a.0)
        .chain(lanes.iter().flat_map(|l| l.notes.iter().map(|n| n.end)))
        .chain(starts.iter().map(|s| s.t))
        .chain(lanes.iter().flat_map(|l| l.lsnds.iter().map(|x| x.0)))
        .max()
        .unwrap_or(0);
    // 보이스 예산: 모든 소리가 끝까지 울린다고 할 때(끊기·choke 무시, 모든 조건 포함) 최대 동시 발음 + 여유
    let mut ev: Vec<(i64, i32)> = Vec::new();
    for s in &starts {
        let (frames, rate) = ctx.sound_dur[s.sound as usize];
        let dur = (frames as i128 * 1_000_000_000 / rate as i128) as i64;
        ev.push((s.t, 1));
        ev.push((s.t + dur.max(1), -1));
    }
    ev.sort();
    let (mut cur, mut peak) = (0i32, 0i32);
    for (_, d) in ev {
        cur += d;
        peak = peak.max(cur);
    }
    let peak = peak as u32;
    let sec = ChartSec {
        shift: ns(shift)?,
        end,
        reference_scroll: to_f64(refs),
        long_note: q.long_note,
        empty_lane: q.empty_lane,
        voice_budget: peak + peak / 4 + 8,
        sounds,
        lanes,
        groups: layout.groups.clone(),
        auto: auto.into_iter().map(|a| (a.0, a.2, a.3, a.4)).collect(),
        note_sounds,
        scroll,
        bars,
        variants: q.variants.iter().map(|v| (v.0.clone(), v.2.clone())).collect(),
        conds,
    };
    // 차분 안 소리 해시는 데이터 표를 아는 compile_dir가 채운다 (fill_local_hashes)
    Ok((sec, local_to_track))
}

/// 구간 통계 (250 ms 칸, 첫 선택지 기준), 하이라이트, 최대 NPS (이동 1초 창).
fn chart_stats(_q: &QChart, sec: &ChartSec, _scale: &Q, track_highlight: Option<i64>) -> ChartStats {
    let on = |ci: u32| sec.conds[ci as usize].iter().all(|(_, ch)| *ch == 0);
    let mut ts: Vec<i64> = sec.lanes.iter().flat_map(|l| l.notes.iter().filter(|n| n.kind != 2 && on(n.cond)).map(|n| n.t)).collect();
    ts.sort();
    let nb = ((sec.end.max(0) / BIN_NS) + 1) as usize;
    let mut bins = vec![0u16; nb];
    for &t in &ts {
        let b = (t.max(0) / BIN_NS) as usize;
        bins[b.min(nb - 1)] = bins[b.min(nb - 1)].saturating_add(1);
    }
    let mut max_nps = 0u32;
    let mut j = 0usize;
    for i in 0..ts.len() {
        while ts[i] - ts[j] >= 1_000_000_000 {
            j += 1;
        }
        max_nps = max_nps.max((i - j + 1) as u32);
    }
    let highlight = match track_highlight {
        Some(h) => h - sec.shift,
        None => {
            let w = 40.min(nb); // 10초 창
            let mut best = (0u32, 0usize);
            let mut s: u32 = bins[..w].iter().map(|&x| x as u32).sum();
            if s > best.0 {
                best = (s, 0);
            }
            for i in 1..=nb - w {
                s = s + bins[i + w - 1] as u32 - bins[i - 1] as u32;
                if s > best.0 {
                    best = (s, i);
                }
            }
            best.1 as i64 * BIN_NS
        }
    };
    ChartStats { notes: ts.len() as u32, bins, highlight, max_nps }
}

/// 음악 레이어 경고: 차분 사이에 같지 않은 이벤트 수 (명세 3-6, README 2장).
fn layer_warnings(charts: &[ChartOut], sids: &[SoundIdOut], datas: &[SoundDataOut]) -> Vec<String> {
    let mut layer: BTreeMap<(i64, [u8; 32], u32, u32), BTreeSet<&str>> = BTreeMap::new();
    for c in charts {
        let s = &c.sec;
        let key = |t: i64, local: u32, g: f32| {
            let sid = &sids[c.local_to_track[local as usize] as usize];
            (t + s.shift, datas[sid.data as usize].hash, sid.start, (to_f32(&sid.gain) * g).to_bits())
        };
        for a in &s.auto {
            layer.entry(key(a.0, a.1, a.2)).or_default().insert(&c.id);
        }
        for l in &s.lanes {
            for n in &l.notes {
                for ns_ in &s.note_sounds[n.ns_start as usize..n.ns_start as usize + n.ns_len as usize] {
                    layer.entry(key(n.t + ns_.0, ns_.1, ns_.2)).or_default().insert(&c.id);
                }
            }
        }
    }
    let n = charts.len();
    let partial = layer.values().filter(|v| v.len() < n).count();
    if n > 1 && partial > 0 {
        vec![format!("음악 레이어 {}개 중 {partial}개는 모든 차분에 있지 않음 (차분마다 소리가 다름)", layer.len())]
    } else {
        vec![]
    }
}

/// 차분 안 소리 표의 해시를 데이터 해시로 채운다.
pub fn fill_local_hashes(t: &mut TrackOut) {
    for c in &mut t.charts {
        for (i, ts) in c.local_to_track.iter().enumerate() {
            let sid = &t.sound_ids[*ts as usize];
            c.sec.sounds[i].hash = t.datas[sid.data as usize].hash;
        }
    }
}
