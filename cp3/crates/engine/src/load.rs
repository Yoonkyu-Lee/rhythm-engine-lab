//! Track 묶음 → 실행 데이터 (`Compiled`). 곡을 고른 뒤, 제어 경로에서.
//!
//! 두 단계로 나눈다 (README 4-3: 플레이와 미리보기가 같은 코드).
//! 1. **계획** (`plan_chart`): `CHART` 구역을 읽고 의미 검사(`pack::decode_chart`), 변형 선택으로 거르고, 걸러진 차분이 쓰는 소리만
//!    데이터 슬롯으로 묶는다(같은 (소리 데이터, start, length)는 한 슬롯 — 논리 ID는 따로, 끊기는 논리 소리 단위).
//!    슬롯의 프레임 수는 디코딩 전에 소리 표로 계산한다(메모리 예산·준비 순서용). 슬롯은 비어 있다.
//! 2. **디코딩** (`decode_slot`): 슬롯 하나 = 소리 데이터 디코딩 → start·length로 자르기 → 스테레오(모노 복제) →
//!    메모리 형식(16비트 정수이고 리샘플링이 필요 없으면 i16, 그 밖은 f32) → 다른 속도면 장치 속도로 리샘플링(f32).
//! `load_chart` = 계획 + 모든 슬롯을 스레드 여럿으로 채움 (플레이는 전부 준비돼야 시작, 4-3 3).

use crate::resample::{out_frames, Filters};
use crate::runtime::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use track::audio::{self, Codec, Packed, Pcm};
use track::pack::{self, Bundle};

pub struct LoadOpts {
    /// 장치 출력 속도 (Hz)
    pub device_rate: u32,
    /// 변형마다 선택지 번호 (0부터). 길이가 맞지 않으면 `random_seed`로 고르고, 그것도 없으면 모두 0.
    pub choice: Vec<u16>,
    /// 변형을 가중치대로 무작위로 고를 씨앗 (플레이: 원작처럼 매번 다르게. 고른 결과는 `Chart::variant_choice`에 남아 리플레이에 쓴다)
    pub random_seed: Option<u64>,
    /// 디코딩 스레드 수
    pub threads: usize,
    /// 메모리 예산 (바이트, 0 = 검사 안 함). 계획의 최고 사용량 추정이 넘으면 시작하지 않는다 (README 4-3 9, 리뷰 3-3 #11). ⚑ 값은 3-5 측정으로
    pub budget: u64,
}

/// 슬롯 열쇠: (소리 데이터 해시, start, length, 장치 속도). 같은 열쇠면 같은 PCM (README 4-3 2: 저장소 키에 장치 속도).
pub type SlotKey = ([u8; 32], u32, u32, u32);
/// Track 하나의 소리 저장소: 차분을 바꾸거나 미리보기에서 플레이로 들어갈 때 슬롯을 그대로 다시 쓴다. 곡을 바꾸면 버린다(곡 사이 캐시 없음).
pub type SlotCache = HashMap<SlotKey, Slot>;

/// 슬롯 하나를 채우는 작업.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotJob {
    pub slot: u32,
    pub hash: [u8; 32],
    pub start: u32,
    pub length: u32,
}

/// 계획: 빈 슬롯을 가진 실행 데이터 + 작업 목록 + 소리 데이터(묶음 안 압축 형태).
pub struct Plan {
    /// 오디오 스레드·작업 스레드가 함께 본다 (슬롯은 `OnceLock`이라 공유한 채로 채운다)
    pub compiled: std::sync::Arc<Compiled>,
    /// 아직 준비되지 않은 슬롯만 (재사용한 준비된 슬롯은 빠짐)
    pub jobs: Vec<SlotJob>,
    pub datas: HashMap<[u8; 32], Packed>,
    /// 최고 메모리 추정 (바이트): 이 차분의 모든 슬롯 PCM + 동시 디코딩 임시 버퍼(가장 큰 디코딩 결과들을 스레드 수만큼)
    pub peak_bytes: u64,
}

/// 묶음 안 차분 목록 (id, 이름, 레벨).
pub fn chart_list(b: &Bundle) -> Result<Vec<(String, String, String)>, String> {
    Ok(pack::read_charts(b)?.into_iter().map(|c| (c.id, c.name, c.level)).collect())
}

fn int_bits(p: &Packed) -> u32 {
    match p.codec {
        Codec::MsAdpcm => 16,
        Codec::Flac => ((((p.codec_table[12] & 1) as u32) << 4) | (p.codec_table[13] >> 4) as u32) + 1,
        Codec::Vorbis => 0,
    }
}

/// 자른 뒤 원래 속도의 프레임 수
fn cut_frames(p: &Packed, start: u32, length: u32) -> u64 {
    let total = p.frames;
    let s = (start as u64).min(total);
    if length == 0 {
        total - s
    } else {
        (length as u64).min(total - s)
    }
}

/// 슬롯 하나 디코딩 (작업 스레드에서). `pcm`은 같은 소리 데이터의 디코딩 결과를 재사용할 때 넘긴다.
pub fn decode_slot(p: &Packed, pcm: &Pcm, start: u32, length: u32, rate: u32, filters: &Filters) -> Result<Pcm2, String> {
    decode_slot_with(p, pcm, start, length, rate, filters, &|| false)
}

/// `decode_slot` + 리샘플링 조각 사이 취소 확인 (`stop()`이 참이면 `Err(audio::CANCELLED)`).
pub fn decode_slot_with(p: &Packed, pcm: &Pcm, start: u32, length: u32, rate: u32, filters: &Filters, stop: &dyn Fn() -> bool) -> Result<Pcm2, String> {
    let ch = p.channels as usize;
    if ch == 0 || ch > 2 {
        return Err(format!("채널 {ch}개는 지원하지 않음"));
    }
    let s = (start as usize).min(p.frames as usize);
    let e = s + cut_frames(p, start, length) as usize;
    let bits = int_bits(p);
    let need_resample = p.rate != rate;
    // 형식 바꾸기도 긴 소리면 오래 걸린다 → 65,536프레임마다 취소 확인 (리뷰 3-5 #7)
    let cancelled = |f: usize| (f - s) % 65_536 == 65_535 && stop();
    let stereo_f32 = |get: &dyn Fn(usize) -> f32| -> Result<Vec<f32>, String> {
        let mut v = Vec::with_capacity((e - s) * 2);
        for f in s..e {
            if cancelled(f) {
                return Err(track::audio::CANCELLED.into());
            }
            let l = get(f * ch);
            v.push(l);
            v.push(if ch == 2 { get(f * ch + 1) } else { l });
        }
        Ok(v)
    };
    let v = match pcm {
        Pcm::Int(x) if bits == 16 && !need_resample => {
            let mut v = Vec::with_capacity((e - s) * 2);
            for f in s..e {
                if cancelled(f) {
                    return Err(track::audio::CANCELLED.into());
                }
                let l = x[f * ch] as i16;
                v.push(l);
                v.push(if ch == 2 { x[f * ch + 1] as i16 } else { l });
            }
            return Ok(Pcm2::I16(v.into_boxed_slice()));
        }
        Pcm::Int(x) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            stereo_f32(&|i| x[i] as f32 * scale)?
        }
        Pcm::Float(x) => stereo_f32(&|i| x[i])?,
    };
    if !need_resample {
        return Ok(Pcm2::F32(v.into_boxed_slice()));
    }
    match filters.get(p.rate, rate)?.run_stereo_with(&v, stop) {
        Some(o) => Ok(Pcm2::F32(o.into_boxed_slice())),
        None => Err(track::audio::CANCELLED.into()),
    }
}

/// 부분 디코딩 (README 4-3 3, 3-5): 슬롯 출력 프레임 [from, to)만 `decode_slot`과 비트까지 같게 만든다. 지금은 Vorbis만
/// (첫 소리를 좌우하는 긴 반주 stem). 원래 소리의 절대 위치로 디코딩·리샘플링하고, 필터에 필요한 앞뒤 입력을 같이 디코딩한다.
#[allow(clippy::too_many_arguments)]
pub fn decode_slot_partial(p: &Packed, start: u32, length: u32, rate: u32, filters: &Filters, from: i64, to: i64, stop: &dyn Fn() -> bool) -> Result<crate::runtime::Partial, String> {
    use track::audio::Codec;
    if p.codec != Codec::Vorbis {
        return Err("부분 디코딩은 Vorbis만".into());
    }
    let ch = p.channels as usize;
    if ch == 0 || ch > 2 {
        return Err(format!("채널 {ch}개는 지원하지 않음"));
    }
    let s = (start as u64).min(p.frames) as i64;
    let n_src = cut_frames(p, start, length) as i64;
    let filter = if p.rate != rate { Some(filters.get(p.rate, rate)?) } else { None };
    let total = filter.as_ref().map_or(n_src, |f| f.out_len(n_src as usize) as i64);
    let (from, to) = (from.clamp(0, total), to.clamp(0, total));
    if from >= to {
        return Err("빈 구간".into());
    }
    let (ia, ib) = filter.as_ref().map_or((from, to), |f| f.src_span(from as usize, to as usize));
    let (ia, ib) = (ia.max(0), ib.min(n_src));
    let (f0, pcm) = track::audio::decode_vorbis_range(p, (s + ia) as u64, (s + ib) as u64, stop)?;
    let mut v = Vec::with_capacity((ib - ia) as usize * 2);
    for f in ia..ib {
        let k = (s + f - f0 as i64) as usize * ch;
        let l = pcm[k];
        v.push(l);
        v.push(if ch == 2 { pcm[k + 1] } else { l });
    }
    let out = match &filter {
        Some(f) => f.run_stereo_range(&v, ia, n_src as usize, from as usize, to as usize, stop)?,
        None => v,
    };
    Ok(crate::runtime::Partial { from, pcm: Pcm2::F32(out.into_boxed_slice()) })
}

pub fn plan_chart(b: &Bundle, index: usize, o: &LoadOpts) -> Result<Plan, String> {
    plan_chart_reuse(b, index, o, &mut SlotCache::new())
}

/// 계획 (Track 저장소 `cache`의 슬롯을 다시 쓰고, 새 슬롯은 넣어 둔다).
pub fn plan_chart_reuse(b: &Bundle, index: usize, o: &LoadOpts, cache: &mut SlotCache) -> Result<Plan, String> {
    let entries = pack::read_charts(b)?;
    let e = entries.get(index).ok_or("차분 번호 범위 밖")?;
    let sec = pack::decode_chart(b.section(pack::K_CHART, e.section as usize).ok_or("CHART 없음")?)?;
    let choice: Vec<u16> = if o.choice.len() == sec.variants.len() && o.choice.iter().zip(&sec.variants).all(|(c, v)| (*c as usize) < v.1.len()) {
        o.choice.clone()
    } else if let Some(seed) = o.random_seed {
        let mut x = seed | 1;
        sec.variants
            .iter()
            .map(|(_, w)| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let total: u64 = w.iter().map(|&v| v as u64).sum::<u64>().max(1);
                let mut r = x % total;
                w.iter().position(|&v| {
                    if r < v as u64 {
                        true
                    } else {
                        r -= v as u64;
                        false
                    }
                })
                .unwrap_or(0) as u16
            })
            .collect()
    } else {
        vec![0; sec.variants.len()]
    };
    let on = |c: u32| sec.conds[c as usize].iter().all(|&(v, ch)| choice[v as usize] == ch);
    // 걸러진 차분이 쓰는 소리 (자동재생·노트 소리·빈 레인 소리)
    let mut used = vec![false; sec.sounds.len()];
    for a in sec.auto.iter().filter(|a| on(a.3)) {
        used[a.1 as usize] = true;
    }
    for l in &sec.lanes {
        for n in l.notes.iter().filter(|n| on(n.cond)) {
            for s in &sec.note_sounds[n.ns_start as usize..n.ns_start as usize + n.ns_len as usize] {
                used[s.1 as usize] = true;
            }
        }
        for x in l.lsnds.iter().filter(|x| on(x.2)) {
            used[x.1 as usize] = true;
        }
    }
    let datas: HashMap<[u8; 32], Packed> = pack::read_sounds(b)?.into_iter().collect();
    let rate = o.device_rate;
    let mut jobs: Vec<SlotJob> = Vec::new();
    let mut keys: Vec<(SlotKey, Slot)> = Vec::new();
    let mut slot_frames: Vec<u32> = Vec::new();
    let mut pcm_bytes = 0u64;
    let mut temps: Vec<u64> = Vec::new();
    let mut sounds = Vec::with_capacity(sec.sounds.len());
    for (i, s) in sec.sounds.iter().enumerate() {
        let (slot, frames) = if used[i] {
            let p = datas.get(&s.hash).ok_or_else(|| format!("소리 데이터 없음 (차분 안 소리 {i})"))?;
            let key: SlotKey = (s.hash, s.start, s.length, rate);
            let k = keys.iter().position(|x| x.0 == key).unwrap_or_else(|| {
                let slot = cache.entry(key).or_insert_with(|| Arc::new(OnceLock::new())).clone();
                if slot.get().is_none() {
                    jobs.push(SlotJob { slot: keys.len() as u32, hash: s.hash, start: s.start, length: s.length });
                }
                keys.push((key, slot));
                let n = cut_frames(p, s.start, s.length);
                let f = if p.rate == rate { n } else { out_frames(n, p.rate, rate) };
                slot_frames.push(f as u32);
                pcm_bytes += f * 2 * if p.int16 && p.rate == rate { 2 } else { 4 };
                temps.push(p.frames * p.channels as u64 * 4 * if p.rate == rate { 1 } else { 3 });
                keys.len() - 1
            });
            (k as u32, slot_frames[k])
        } else {
            (u32::MAX, 0)
        };
        let dur = (frames as i128 * NS_PER_S as i128 + rate as i128 - 1) / rate as i128;
        sounds.push(SoundRef {
            data: slot,
            frames,
            kind: if dur as i64 > LONG_SOUND_NS { SoundKind::Long } else { SoundKind::Short },
            gain: s.gain,
            cut: s.retrigger == 1,
            choke: s.choke,
        });
    }
    // 쓰지 않는 소리는 빈 슬롯 하나를 가리키게 (울리지 않는다)
    let empty_slot = keys.len() as u32;
    for s in sounds.iter_mut().filter(|s| s.data == u32::MAX) {
        s.data = empty_slot;
    }
    let mut data: Vec<Slot> = keys.into_iter().map(|x| x.1).collect();
    let empty = OnceLock::new();
    let _ = empty.set(Pcm2::F32(Box::new([])));
    data.push(Arc::new(empty));
    // 동시 디코딩 임시 버퍼: 스레드마다 다른 소리를 풀므로 가장 큰 것들을 스레드 수만큼 더한다
    temps.sort_unstable_by(|a, b| b.cmp(a));
    let peak_bytes = pcm_bytes + temps.iter().take(o.threads.max(1)).sum::<u64>();
    if o.budget > 0 && peak_bytes > o.budget {
        return Err(format!("메모리 예산 초과: 최고 {:.0} MB 추정 > 예산 {:.0} MB (시작하지 않음)", peak_bytes as f64 / 1e6, o.budget as f64 / 1e6));
    }
    let partial = (0..data.len()).map(|_| std::sync::Arc::new(std::sync::OnceLock::new())).collect();
    let bank = SoundBank { rate, sounds, data, partial };
    let lanes = sec
        .lanes
        .iter()
        .map(|l| Lane {
            id: l.id.clone(),
            role: Role::from_code(l.role),
            notes: l
                .notes
                .iter()
                .filter(|n| on(n.cond))
                .map(|n| Note {
                    t: n.t,
                    end: n.end,
                    sounds_start: n.ns_start,
                    sounds_len: n.ns_len,
                    kind: match n.kind {
                        1 => NoteKind::Long,
                        2 => NoteKind::Mine,
                        _ => NoteKind::Tap,
                    },
                    damage: n.damage,
                })
                .collect(),
            lane_sounds: l.lsnds.iter().filter(|x| on(x.2)).map(|x| LaneSound { t: x.0, sound: x.1 }).collect(),
        })
        .collect();
    let chart = Chart {
        title: e.name.clone(),
        chart_id: e.id.clone(),
        layout: String::new(),
        bank,
        auto: sec.auto.iter().filter(|a| on(a.3)).map(|a| SoundEvent { t: a.0, sound: a.1, gain: a.2 }).collect(),
        note_sounds: sec.note_sounds.iter().map(|x| NoteSound { offset: x.0, sound: x.1, gain: x.2 }).collect(),
        lanes,
        groups: sec.groups.clone(),
        scroll: sec.scroll.iter().map(|x| ScrollPoint { t: x.0, pos: x.1, speed: x.2 }).collect(),
        bars: sec.bars.clone(),
        reference_scroll: sec.reference_scroll,
        long_rule: match sec.long_note {
            1 => LongRule::Cn,
            2 => LongRule::Hcn,
            _ => LongRule::Ln,
        },
        empty_lane: match sec.empty_lane {
            1 => EmptyLane::NextNote,
            2 => EmptyLane::None,
            _ => EmptyLane::PreviousNote,
        },
        end: sec.end,
        chart_hash: e.hash,
        variant_choice: choice,
    };
    Ok(Plan { compiled: std::sync::Arc::new(Compiled { chart, voice_budget: sec.voice_budget.max(16) }), jobs, datas, peak_bytes })
}

/// 계획 + 모든 슬롯 채우기 (플레이). 슬롯을 소리 데이터별로 묶어 스레드 여럿에 나눈다(같은 데이터는 한 번 디코딩).
pub fn load_chart(b: &Bundle, index: usize, o: &LoadOpts) -> Result<Compiled, String> {
    let plan = plan_chart(b, index, o)?;
    fill_all(&plan, o.threads)?;
    Ok(std::sync::Arc::try_unwrap(plan.compiled).unwrap_or_else(|a| (*a).clone()))
}

/// 계획의 빈 슬롯을 모두 채운다.
pub fn fill_all(plan: &Plan, threads: usize) -> Result<(), String> {
    let filters = Filters::default();
    let rate = plan.compiled.chart.bank.rate;
    let mut groups: Vec<Vec<SlotJob>> = Vec::new();
    for j in &plan.jobs {
        match groups.iter_mut().find(|g| g[0].hash == j.hash) {
            Some(g) => g.push(*j),
            None => groups.push(vec![*j]),
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let err: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    std::thread::scope(|sc| {
        for _ in 0..threads.max(1).min(groups.len().max(1)) {
            sc.spawn(|| loop {
                let gi = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(g) = groups.get(gi) else { break };
                let p = &plan.datas[&g[0].hash];
                let r = audio::decode(p).and_then(|pcm| {
                    for j in g {
                        let x = decode_slot(p, &pcm, j.start, j.length, rate, &filters)?;
                        let _ = plan.compiled.chart.bank.data[j.slot as usize].set(x);
                    }
                    Ok(())
                });
                if let Err(e) = r {
                    *err.lock().unwrap() = Some(e);
                }
            });
        }
    });
    match err.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
