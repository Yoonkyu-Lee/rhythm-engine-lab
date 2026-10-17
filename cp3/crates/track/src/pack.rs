//! 묶음 파일 쓰기·읽기 (명세 4장 `CHART` 구역, 5장 묶음).
//!
//! 모든 수는 little-endian, 패딩은 0. 같은 `TrackOut`은 항상 같은 바이트가 된다.

use crate::audio::{Codec, Packed};
use crate::compile::*;

pub const MAGIC: &[u8; 4] = b"TRK\0";
pub const HEADER: usize = 128;
pub const INDEX_ENTRY: usize = 56;

pub const K_INFO: u32 = 1;
pub const K_LAYOUTS: u32 = 2;
pub const K_CHARTS: u32 = 3;
pub const K_CHART: u32 = 4;
pub const K_SOUND_IDS: u32 = 5;
pub const K_SOUNDS: u32 = 6;
pub const K_CODEC_TABLES: u32 = 7;
pub const K_SOUND_DATA: u32 = 8;
pub const K_GRAPHICS: u32 = 9;
pub const K_STATS: u32 = 10;
pub const K_SOURCE: u32 = 11;

pub const DECODERS: &str = "vorbis=lewton-0.10 flac=claxon-0.4 msadpcm=track-1(ms-spec)";

// ------------------------------------------------------------------ 쓰기 도구

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v)
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes())
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b)
    }
    /// u32 길이 + UTF-8
    pub fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.bytes(s.as_bytes());
    }
    pub fn pad_to(&mut self, align: usize) {
        while self.0.len() % align != 0 {
            self.0.push(0);
        }
    }
    pub fn zeros(&mut self, n: usize) {
        self.0.extend(std::iter::repeat(0).take(n));
    }
}

// ------------------------------------------------------------------ CHART 구역 (명세 4장)

pub const CHART_HEADER: usize = 64;
pub const TABLES: usize = 14;
pub const STRIDES: [u32; TABLES] = [56, 32, 16, 24, 32, 16, 16, 24, 8, 16, 4, 8, 4, 1];

pub fn encode_chart(c: &ChartSec) -> Vec<u8> {
    // 문자열 표를 먼저 모은다
    let mut strings = Vec::new();
    let mut put_str = |s: &str| -> (u32, u32) {
        let off = strings.len() as u32;
        strings.extend_from_slice(s.as_bytes());
        (off, s.len() as u32)
    };
    let lane_names: Vec<(u32, u32)> = c.lanes.iter().map(|l| put_str(&l.id)).collect();
    let group_names: Vec<(u32, u32)> = c.groups.iter().map(|g| put_str(&g.0)).collect();
    let var_names: Vec<(u32, u32)> = c.variants.iter().map(|v| put_str(&v.0)).collect();

    let mut tabs: Vec<(Vec<u8>, u32)> = Vec::new(); // (바이트, 개수)
    // 0 차분 안 소리
    let mut w = W::default();
    for s in &c.sounds {
        w.bytes(&s.hash);
        w.u32(s.start);
        w.f32(s.gain);
        w.u8(s.retrigger);
        w.zeros(3);
        w.u32(s.choke);
        w.zeros(8);
    }
    tabs.push((w.0, c.sounds.len() as u32));
    // 1 레인 (노트·빈 레인 소리 구간은 아래에서 계산)
    let mut w = W::default();
    let (mut ni, mut li) = (0u32, 0u32);
    for (l, nm) in c.lanes.iter().zip(&lane_names) {
        w.u32(nm.0);
        w.u32(nm.1);
        w.u8(l.role);
        w.zeros(3);
        w.u32(ni);
        w.u32(l.notes.len() as u32);
        w.u32(li);
        w.u32(l.lsnds.len() as u32);
        w.u32(0);
        ni += l.notes.len() as u32;
        li += l.lsnds.len() as u32;
    }
    tabs.push((w.0, c.lanes.len() as u32));
    // 2 묶음
    let mut w = W::default();
    for (g, nm) in c.groups.iter().zip(&group_names) {
        w.u32(nm.0);
        w.u32(nm.1);
        w.u64(g.1);
    }
    tabs.push((w.0, c.groups.len() as u32));
    // 3 자동재생
    let mut w = W::default();
    for a in &c.auto {
        w.i64(a.0);
        w.u32(a.1);
        w.f32(a.2);
        w.u32(a.3);
        w.u32(0);
    }
    tabs.push((w.0, c.auto.len() as u32));
    // 4 노트
    let mut w = W::default();
    let mut n_notes = 0;
    for l in &c.lanes {
        for n in &l.notes {
            w.i64(n.t);
            w.i64(n.end);
            w.u32(n.ns_start);
            w.u16(n.ns_len);
            w.u8(n.kind);
            w.u8(0);
            w.u32(n.cond);
            w.i32(n.damage);
            n_notes += 1;
        }
    }
    tabs.push((w.0, n_notes));
    // 5 노트 소리
    let mut w = W::default();
    for s in &c.note_sounds {
        w.i64(s.0);
        w.u32(s.1);
        w.f32(s.2);
    }
    tabs.push((w.0, c.note_sounds.len() as u32));
    // 6 빈 레인 소리
    let mut w = W::default();
    let mut n_ls = 0;
    for l in &c.lanes {
        for x in &l.lsnds {
            w.i64(x.0);
            w.u32(x.1);
            w.u32(x.2);
            n_ls += 1;
        }
    }
    tabs.push((w.0, n_ls));
    // 7 스크롤
    let mut w = W::default();
    for s in &c.scroll {
        w.i64(s.0);
        w.f64(s.1);
        w.f64(s.2);
    }
    tabs.push((w.0, c.scroll.len() as u32));
    // 8 마디선
    let mut w = W::default();
    for b in &c.bars {
        w.i64(*b);
    }
    tabs.push((w.0, c.bars.len() as u32));
    // 9 변형, 10 가중치
    let mut w = W::default();
    let mut wt = W::default();
    let mut nw = 0u32;
    for (v, nm) in c.variants.iter().zip(&var_names) {
        w.u32(nm.0);
        w.u32(nm.1);
        w.u16(v.1.len() as u16);
        w.u16(0);
        w.u32(nw);
        for x in &v.1 {
            wt.u32(*x);
            nw += 1;
        }
    }
    tabs.push((w.0, c.variants.len() as u32));
    tabs.push((wt.0, nw));
    // 11 조건, 12 조건 항
    let mut w = W::default();
    let mut wt = W::default();
    let mut nt = 0u32;
    for cd in &c.conds {
        w.u32(nt);
        w.u32(cd.len() as u32);
        for (v, ch) in cd {
            wt.u16(*v);
            wt.u16(*ch);
            nt += 1;
        }
    }
    tabs.push((w.0, c.conds.len() as u32));
    tabs.push((wt.0, nt));
    // 13 문자열
    let n_str = strings.len() as u32;
    tabs.push((strings, n_str));
    assert_eq!(tabs.len(), TABLES);
    for (i, (b, n)) in tabs.iter().enumerate() {
        assert_eq!(b.len(), *n as usize * STRIDES[i] as usize, "표 {i}: 바이트 수 ≠ 개수 × stride");
    }

    let mut out = W::default();
    out.bytes(b"CHRT");
    out.u16(FORMAT_MINOR);
    out.u16(CHART_HEADER as u16);
    out.u16(TABLES as u16);
    out.u16(0);
    out.u32(0);
    out.i64(c.shift);
    out.i64(c.end);
    out.f64(c.reference_scroll);
    out.u16(c.lanes.len() as u16);
    out.u8(c.long_note);
    out.u8(c.empty_lane);
    out.u32(c.voice_budget);
    out.zeros(16);
    debug_assert_eq!(out.0.len(), CHART_HEADER);
    let dir_at = out.0.len();
    out.zeros(12 * TABLES);
    out.pad_to(8);
    let mut dir = Vec::new();
    for (i, (b, n)) in tabs.iter().enumerate() {
        out.pad_to(8);
        dir.push((out.0.len() as u32, *n, STRIDES[i]));
        out.bytes(b);
    }
    out.pad_to(8);
    for (i, (off, n, st)) in dir.iter().enumerate() {
        let o = dir_at + 12 * i;
        out.0[o..o + 4].copy_from_slice(&off.to_le_bytes());
        out.0[o + 4..o + 8].copy_from_slice(&n.to_le_bytes());
        out.0[o + 8..o + 12].copy_from_slice(&st.to_le_bytes());
    }
    out.0
}

/// 차분 해시 = BLAKE3(CHART 구역 바이트, shift 칸은 0) (명세 3-5).
pub fn chart_hash(section: &[u8]) -> [u8; 32] {
    let mut b = section.to_vec();
    b[16..24].fill(0);
    *blake3::hash(&b).as_bytes()
}

// ------------------------------------------------------------------ 묶음 쓰기 (명세 5장)

pub fn write_bundle(t: &TrackOut) -> Vec<u8> {
    let mut sections: Vec<(u32, Vec<u8>)> = Vec::new();

    // INFO
    let mut w = W::default();
    let i = &t.info;
    w.str(&i.title);
    w.str(&i.title_sort);
    w.str(&i.artist);
    w.str(&i.genre);
    w.u32(i.tags.len() as u32);
    i.tags.iter().for_each(|s| w.str(s));
    w.i64(i.highlight.unwrap_or(i64::MIN));
    w.u32(i.lang.len() as u32);
    for (k, v) in &i.lang {
        w.str(k);
        v.iter().for_each(|s| w.str(s));
    }
    w.str(&i.source_format);
    w.str(&i.converter);
    w.u32(i.source_files.len() as u32);
    for (p, h) in &i.source_files {
        w.str(p);
        w.str(h);
    }
    sections.push((K_INFO, w.0));

    // LAYOUTS
    let mut w = W::default();
    w.u32(t.layouts.len() as u32);
    for l in &t.layouts {
        w.str(&l.id);
        w.u16(l.lanes.len() as u16);
        for (id, role) in &l.lanes {
            w.str(id);
            w.u8(*role);
        }
        w.u16(l.groups.len() as u16);
        for (id, mask) in &l.groups {
            w.str(id);
            w.u64(*mask);
        }
    }
    sections.push((K_LAYOUTS, w.0));

    // CHART × n (먼저 만들어 해시를 CHARTS에 넣는다)
    let chart_secs: Vec<Vec<u8>> = t.charts.iter().map(|c| encode_chart(&c.sec)).collect();
    let mut w = W::default();
    w.u32(t.charts.len() as u32);
    for (k, c) in t.charts.iter().enumerate() {
        w.str(&c.id);
        w.str(&c.name);
        w.str(&c.level);
        w.bytes(&chart_hash(&chart_secs[k]));
        w.u32(k as u32);
        w.u32(c.base.unwrap_or(u32::MAX));
        w.i64(c.scale.0);
        w.i64(c.scale.1);
        w.i64(c.sec.shift);
        w.u32(c.local_to_track.len() as u32);
        c.local_to_track.iter().for_each(|x| w.u32(*x));
    }
    sections.push((K_CHARTS, w.0));
    for s in chart_secs {
        sections.push((K_CHART, s));
    }

    // SOUND_IDS
    let mut w = W::default();
    w.u32(t.sound_ids.len() as u32);
    for s in &t.sound_ids {
        w.u32(s.data);
        w.u32(s.start);
        w.f32(crate::rational::to_f32(&s.gain));
        w.u8(s.retrigger);
        w.zeros(3);
        w.u32(s.choke);
        w.str(&s.id);
    }
    w.u32(t.chokes.len() as u32);
    t.chokes.iter().for_each(|s| w.str(s));
    sections.push((K_SOUND_IDS, w.0));

    // CODEC_TABLES (같은 표는 한 번), SOUND_DATA, SOUNDS
    let mut tables: Vec<&[u8]> = Vec::new();
    let mut table_of = Vec::new();
    for d in &t.datas {
        let tb = d.packed.codec_table.as_slice();
        let k = match tables.iter().position(|x| *x == tb) {
            Some(k) => k,
            None => {
                tables.push(tb);
                tables.len() - 1
            }
        };
        table_of.push(k as u32);
    }
    let mut data = W::default();
    let mut offs = Vec::new();
    for d in &t.datas {
        offs.push((data.0.len() as u64, d.packed.data.len() as u64));
        data.bytes(&d.packed.data);
    }
    let mut w = W::default();
    w.str(DECODERS);
    w.u32(t.datas.len() as u32);
    for (k, d) in t.datas.iter().enumerate() {
        let p = &d.packed;
        w.bytes(&d.hash);
        w.u8(p.codec as u8);
        w.u8(p.channels);
        w.u8(p.int16 as u8);
        w.u8(0);
        w.u32(p.rate);
        w.u64(p.frames);
        w.u32(p.skip_front);
        w.u32(table_of[k]);
        w.u64(offs[k].0);
        w.u64(offs[k].1);
    }
    sections.push((K_SOUNDS, w.0));
    let mut w = W::default();
    w.u32(tables.len() as u32);
    for tb in &tables {
        w.u32(tb.len() as u32);
        w.bytes(tb);
    }
    sections.push((K_CODEC_TABLES, w.0));
    sections.push((K_SOUND_DATA, data.0));

    // GRAPHICS
    let mut w = W::default();
    w.u32(t.media.len() as u32);
    for m in &t.media {
        w.str(&m.hash);
        w.str(&m.kind);
        w.str(&m.role);
        w.i64(m.start.unwrap_or(i64::MIN));
    }
    sections.push((K_GRAPHICS, w.0));

    // STATS
    let mut w = W::default();
    w.u32(t.charts.len() as u32);
    for c in &t.charts {
        w.str(&c.id);
        w.i64(BIN_NS);
        w.u32(c.stats.notes);
        w.u32(c.stats.max_nps);
        w.i64(c.stats.highlight);
        w.u32(c.stats.bins.len() as u32);
        c.stats.bins.iter().for_each(|b| w.u16(*b));
    }
    sections.push((K_STATS, w.0));

    // SOURCE
    let mut w = W::default();
    w.u32(t.source.len() as u32);
    for (p, b) in &t.source {
        w.str(p);
        w.u64(b.len() as u64);
        w.bytes(b);
    }
    sections.push((K_SOURCE, w.0));

    // 배치: 머리 128 + 색인, 구역은 64 B 정렬, SOUND_DATA는 4 KiB 정렬
    let n = sections.len();
    let mut out = W::default();
    out.zeros(HEADER + INDEX_ENTRY * n);
    let mut index = Vec::new();
    for (kind, body) in &sections {
        out.pad_to(if *kind == K_SOUND_DATA { 4096 } else { 64 });
        index.push((*kind, out.0.len() as u64, body.len() as u64, *blake3::hash(body).as_bytes()));
        out.bytes(body);
    }
    out.pad_to(64);
    let b = &mut out.0;
    b[0..4].copy_from_slice(MAGIC);
    b[4..6].copy_from_slice(&FORMAT_MAJOR.to_le_bytes());
    b[6..8].copy_from_slice(&FORMAT_MINOR.to_le_bytes());
    b[8..12].copy_from_slice(&(n as u32).to_le_bytes());
    b[12..16].copy_from_slice(&(HEADER as u32).to_le_bytes());
    b[16..32].copy_from_slice(&t.id);
    b[32..36].copy_from_slice(&t.revision.to_le_bytes());
    let comp = compiler_id();
    let cb = comp.as_bytes();
    b[40..40 + cb.len().min(32)].copy_from_slice(&cb[..cb.len().min(32)]);
    b[104..128].copy_from_slice(&t.source_key); // 컴파일 캐시 열쇠
    for (k, (kind, off, len, h)) in index.iter().enumerate() {
        let o = HEADER + INDEX_ENTRY * k;
        b[o..o + 4].copy_from_slice(&kind.to_le_bytes());
        b[o + 8..o + 16].copy_from_slice(&off.to_le_bytes());
        b[o + 16..o + 24].copy_from_slice(&len.to_le_bytes());
        b[o + 24..o + 56].copy_from_slice(h);
    }
    let h = *blake3::hash(b).as_bytes(); // 묶음 해시 칸(72..104)은 아직 0
    b[72..104].copy_from_slice(&h);
    out.0
}

// ------------------------------------------------------------------ 읽기

pub struct R<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        R { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.b.get(self.p..self.p + n).ok_or("구역이 잘림")?;
        self.p += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.take(n)
    }
    pub fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
    /// 개수 n × 최소 크기가 남은 바이트 안인지 (할당 전에 검사, 리뷰 3-1 #2)
    pub fn need(&self, n: usize, each: usize) -> Result<(), String> {
        match n.checked_mul(each) {
            Some(x) if x <= self.remaining() => Ok(()),
            _ => Err(format!("개수 {n}가 남은 바이트보다 많음 (손상)")),
        }
    }
    pub fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "UTF-8 아님".to_string())
    }
}

/// 읽은 묶음: 구역 목록(검증됨)과 해석한 표들.
pub struct Bundle {
    pub bytes: Vec<u8>,
    pub track_id: [u8; 16],
    pub revision: u32,
    pub minor: u16,
    pub sections: Vec<(u32, usize, usize)>,
}

impl Bundle {
    pub fn open(bytes: Vec<u8>) -> Result<Bundle, String> {
        if bytes.len() < HEADER || &bytes[0..4] != MAGIC {
            return Err("Track 묶음이 아님".into());
        }
        let major = u16::from_le_bytes([bytes[4], bytes[5]]);
        if major != FORMAT_MAJOR {
            return Err(format!("주 버전 {major}는 읽을 수 없음"));
        }
        let minor = u16::from_le_bytes([bytes[6], bytes[7]]);
        let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let hsize = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if hsize < HEADER || n.checked_mul(INDEX_ENTRY).and_then(|x| x.checked_add(hsize)).map(|x| x > bytes.len()).unwrap_or(true) {
            return Err("구역 색인이 파일 밖 (손상)".into());
        }
        let mut zeroed = bytes.clone();
        zeroed[72..104].fill(0);
        if blake3::hash(&zeroed).as_bytes() != &bytes[72..104] {
            return Err("묶음 해시가 맞지 않음".into());
        }
        let mut sections = Vec::new();
        for k in 0..n {
            let o = hsize + INDEX_ENTRY * k;
            let e = bytes.get(o..o + INDEX_ENTRY).ok_or("색인이 잘림")?;
            let kind = u32::from_le_bytes(e[0..4].try_into().unwrap());
            let off = u64::from_le_bytes(e[8..16].try_into().unwrap()) as usize;
            let len = u64::from_le_bytes(e[16..24].try_into().unwrap()) as usize;
            let body = off.checked_add(len).and_then(|e| bytes.get(off..e)).ok_or("구역이 파일 밖")?;
            if blake3::hash(body).as_bytes() != &e[24..56] {
                return Err(format!("구역 {k} (kind {kind}) 해시가 맞지 않음"));
            }
            sections.push((kind, off, len));
        }
        let mut track_id = [0u8; 16];
        track_id.copy_from_slice(&bytes[16..32]);
        let revision = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        Ok(Bundle { bytes, track_id, revision, minor, sections })
    }

    pub fn section(&self, kind: u32, nth: usize) -> Option<&[u8]> {
        self.sections.iter().filter(|s| s.0 == kind).nth(nth).map(|&(_, o, l)| &self.bytes[o..o + l])
    }
}

/// 묶음에서 읽은 소리 데이터 (디코딩용).
pub fn read_sounds(b: &Bundle) -> Result<Vec<([u8; 32], Packed)>, String> {
    let mut r = R::new(b.section(K_SOUNDS, 0).ok_or("SOUNDS 없음")?);
    let _decoders = r.str()?;
    let n = r.u32()? as usize;
    r.need(n, 72)?;
    let mut tr = R::new(b.section(K_CODEC_TABLES, 0).ok_or("CODEC_TABLES 없음")?);
    let nt = tr.u32()? as usize;
    tr.need(nt, 4)?;
    let mut tables = Vec::new();
    for _ in 0..nt {
        let l = tr.u32()? as usize;
        tables.push(tr.bytes(l)?.to_vec());
    }
    let data = b.section(K_SOUND_DATA, 0).ok_or("SOUND_DATA 없음")?;
    let mut out = Vec::new();
    for _ in 0..n {
        let mut h = [0u8; 32];
        h.copy_from_slice(r.bytes(32)?);
        let codec = match r.u8()? {
            1 => Codec::Vorbis,
            2 => Codec::Flac,
            3 => Codec::MsAdpcm,
            c => return Err(format!("알 수 없는 코덱 {c}")),
        };
        let channels = r.u8()?;
        if !(1..=8).contains(&channels) {
            return Err(format!("채널 수 {channels}"));
        }
        let int16 = r.u8()? != 0;
        r.u8()?;
        let rate = r.u32()?;
        let frames = r.u64()?;
        let skip_front = r.u32()?;
        let table = r.u32()? as usize;
        let off = r.u64()? as usize;
        let len = r.u64()? as usize;
        out.push((
            h,
            Packed {
                codec,
                channels,
                rate,
                frames,
                skip_front,
                int16,
                codec_table: tables.get(table).ok_or("코덱 표 번호 범위 밖")?.clone(),
                data: off.checked_add(len).and_then(|e| data.get(off..e)).ok_or("소리 데이터 범위 밖")?.to_vec(),
            },
        ));
    }
    Ok(out)
}

/// 차분 목록 항목.
#[derive(Clone, Debug)]
pub struct ChartEntry {
    pub id: String,
    pub name: String,
    pub level: String,
    pub hash: [u8; 32],
    pub section: u32,
    pub base: Option<u32>,
    pub scale: (i64, i64),
    pub shift: i64,
    pub local_to_track: Vec<u32>,
}

pub fn read_charts(b: &Bundle) -> Result<Vec<ChartEntry>, String> {
    let mut r = R::new(b.section(K_CHARTS, 0).ok_or("CHARTS 없음")?);
    let n = r.u32()?;
    r.need(n as usize, 4 * 3 + 32 + 4 + 4 + 8 * 3 + 4)?;
    let mut out = Vec::new();
    for _ in 0..n {
        let id = r.str()?;
        let name = r.str()?;
        let level = r.str()?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(r.bytes(32)?);
        let section = r.u32()?;
        let base = r.u32()?;
        let scale = (r.i64()?, r.i64()?);
        let shift = r.i64()?;
        let m = r.u32()? as usize;
        r.need(m, 4)?;
        let mut l2t = Vec::with_capacity(m);
        for _ in 0..m {
            l2t.push(r.u32()?);
        }
        out.push(ChartEntry { id, name, level, hash, section, base: (base != u32::MAX).then_some(base), scale, shift, local_to_track: l2t });
    }
    Ok(out)
}

/// CHART 구역 → ChartSec (명세 4장, stride로 레코드 건너뜀).
pub fn decode_chart(sec: &[u8]) -> Result<ChartSec, String> {
    if sec.len() < CHART_HEADER || &sec[0..4] != b"CHRT" {
        return Err("CHART 구역이 아님".into());
    }
    let mut h = R::new(sec);
    h.p = 8;
    let ntab = h.u16()? as usize;
    h.p = 16;
    let shift = h.i64()?;
    let end = h.i64()?;
    let reference_scroll = h.f64()?;
    let nlanes = h.u16()? as usize;
    let long_note = h.u8()?;
    let empty_lane = h.u8()?;
    let voice_budget = h.u32()?;
    if ntab < TABLES || CHART_HEADER + 12 * ntab > sec.len() {
        return Err("표 디렉터리가 모자라거나 구역 밖".into());
    }
    let mut dir = Vec::new();
    let mut d = R::new(sec);
    d.p = CHART_HEADER;
    for t in 0..ntab {
        let (off, n, st) = (d.u32()? as usize, d.u32()? as usize, d.u32()? as usize);
        if t < TABLES {
            if st < STRIDES[t] as usize {
                return Err(format!("표 {t}: stride {st} < {}", STRIDES[t]));
            }
            match n.checked_mul(st).and_then(|x| x.checked_add(off)) {
                Some(e) if e <= sec.len() => {}
                _ => return Err(format!("표 {t}가 구역 밖 (손상)")),
            }
        }
        dir.push((off, n, st));
    }
    let rec = |t: usize, i: usize| -> Result<R, String> {
        let (off, n, st) = dir[t];
        if i >= n {
            return Err(format!("표 {t} 범위 밖 {i}"));
        }
        let mut r = R::new(sec);
        r.p = off + st * i;
        Ok(r)
    };
    let strs = {
        let (off, n, _) = dir[13];
        sec.get(off..off + n).ok_or("문자열 표가 잘림")?
    };
    let sstr = |o: u32, l: u32| -> Result<String, String> {
        String::from_utf8(strs.get(o as usize..(o + l) as usize).ok_or("문자열 범위 밖")?.to_vec()).map_err(|_| "UTF-8 아님".into())
    };
    let mut sounds = Vec::new();
    for i in 0..dir[0].1 {
        let mut r = rec(0, i)?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(r.bytes(32)?);
        let start = r.u32()?;
        let gain = r.f32()?;
        let retrigger = r.u8()?;
        r.bytes(3)?;
        let choke = r.u32()?;
        sounds.push(LocalSound { hash, start, gain, retrigger, choke });
    }
    let mut notes_all = Vec::new();
    for i in 0..dir[4].1 {
        let mut r = rec(4, i)?;
        let t = r.i64()?;
        let end = r.i64()?;
        let ns_start = r.u32()?;
        let ns_len = r.u16()?;
        let kind = r.u8()?;
        r.u8()?;
        let cond = r.u32()?;
        let damage = r.i32()?;
        notes_all.push(NoteRec { t, end, ns_start, ns_len, kind, cond, damage });
    }
    let mut ls_all = Vec::new();
    for i in 0..dir[6].1 {
        let mut r = rec(6, i)?;
        ls_all.push((r.i64()?, r.u32()?, r.u32()?));
    }
    let mut lanes = Vec::new();
    for i in 0..dir[1].1 {
        let mut r = rec(1, i)?;
        let (o, l) = (r.u32()?, r.u32()?);
        let role = r.u8()?;
        r.bytes(3)?;
        let (ns, nn, ls, ln) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        lanes.push(LaneRec {
            id: sstr(o, l)?,
            role,
            notes: notes_all.get(ns..ns + nn).ok_or("노트 구간 범위 밖")?.to_vec(),
            lsnds: ls_all.get(ls..ls + ln).ok_or("빈 레인 소리 구간 범위 밖")?.to_vec(),
        });
    }
    if lanes.len() != nlanes {
        return Err("레인 수가 머리와 다름".into());
    }
    let mut groups = Vec::new();
    for i in 0..dir[2].1 {
        let mut r = rec(2, i)?;
        let (o, l) = (r.u32()?, r.u32()?);
        groups.push((sstr(o, l)?, r.u64()?));
    }
    let mut auto = Vec::new();
    for i in 0..dir[3].1 {
        let mut r = rec(3, i)?;
        auto.push((r.i64()?, r.u32()?, r.f32()?, r.u32()?));
    }
    let mut note_sounds = Vec::new();
    for i in 0..dir[5].1 {
        let mut r = rec(5, i)?;
        note_sounds.push((r.i64()?, r.u32()?, r.f32()?));
    }
    let mut scroll = Vec::new();
    for i in 0..dir[7].1 {
        let mut r = rec(7, i)?;
        scroll.push((r.i64()?, r.f64()?, r.f64()?));
    }
    let mut bars = Vec::new();
    for i in 0..dir[8].1 {
        bars.push(rec(8, i)?.i64()?);
    }
    let mut weights = Vec::new();
    for i in 0..dir[10].1 {
        weights.push(rec(10, i)?.u32()?);
    }
    let mut variants = Vec::new();
    for i in 0..dir[9].1 {
        let mut r = rec(9, i)?;
        let (o, l) = (r.u32()?, r.u32()?);
        let n = r.u16()? as usize;
        r.u16()?;
        let ws = r.u32()? as usize;
        variants.push((sstr(o, l)?, weights.get(ws..ws + n).ok_or("가중치 범위 밖")?.to_vec()));
    }
    let mut terms = Vec::new();
    for i in 0..dir[12].1 {
        let mut r = rec(12, i)?;
        terms.push((r.u16()?, r.u16()?));
    }
    let mut conds = Vec::new();
    for i in 0..dir[11].1 {
        let mut r = rec(11, i)?;
        let (s, n) = (r.u32()? as usize, r.u32()? as usize);
        conds.push(terms.get(s..s + n).ok_or("조건 항 범위 밖")?.to_vec());
    }
    let c = ChartSec { shift, end, reference_scroll, long_note, empty_lane, voice_budget, sounds, lanes, groups, auto, note_sounds, scroll, bars, variants, conds };
    validate_chart(&c)?;
    Ok(c)
}

/// 차분 구역의 의미 검사 (리뷰 3-1 #2): 참조 번호, enum, 레인 비트, 유한 실수, 정렬. 읽는 쪽(엔진·도구)은 이것을 통과한 것만 쓴다.
pub fn validate_chart(c: &ChartSec) -> Result<(), String> {
    let ns = c.sounds.len() as u32;
    let nc = c.conds.len() as u32;
    let fin = |x: f64, what: &str| if x.is_finite() { Ok(()) } else { Err(format!("{what}가 유한하지 않음")) };
    let gain_ok = |g: f32| g.is_finite() && g >= 0.0;
    if c.lanes.is_empty() || c.lanes.len() > 64 || c.long_note > 2 || c.empty_lane > 2 {
        return Err("레인 수·규칙 값 범위 밖".into());
    }
    fin(c.reference_scroll, "reference_scroll")?;
    if c.conds.is_empty() || !c.conds[0].is_empty() {
        return Err("조건 0번은 빈 조건이어야 함".into());
    }
    for cd in &c.conds {
        for &(v, ch) in cd {
            let nv = c.variants.get(v as usize).ok_or("조건이 없는 변형을 가리킴")?;
            if ch as usize >= nv.1.len() {
                return Err("조건이 없는 선택지를 가리킴".into());
            }
        }
    }
    if c.variants.iter().any(|v| v.1.is_empty() || v.1.iter().any(|w| *w == 0)) {
        return Err("변형 가중치 오류".into());
    }
    let nchoke = c.sounds.iter().filter(|s| s.choke > 0).map(|s| s.choke).max().unwrap_or(0);
    for s in &c.sounds {
        if !gain_ok(s.gain) || s.retrigger > 1 || s.choke > nchoke {
            return Err("차분 안 소리 값 범위 밖".into());
        }
    }
    let mut prev = i64::MIN;
    for a in &c.auto {
        if a.1 >= ns || a.3 >= nc || !gain_ok(a.2) || a.0 < prev {
            return Err("자동재생 레코드 오류 (참조·이득·정렬)".into());
        }
        prev = a.0;
    }
    for s in &c.note_sounds {
        if s.1 >= ns || !gain_ok(s.2) {
            return Err("노트 소리 레코드 오류".into());
        }
    }
    for l in &c.lanes {
        if l.role > 4 && l.role != 255 {
            return Err("레인 역할 값 범위 밖".into());
        }
        let mut prev = i64::MIN;
        for n in &l.notes {
            let end = n.ns_start as usize + n.ns_len as usize;
            if n.kind > 2 || n.cond >= nc || n.end < n.t || n.t < prev || end > c.note_sounds.len() || (n.kind == 1) != (n.end > n.t) {
                return Err(format!("노트 레코드 오류 (레인 {})", l.id));
            }
            if n.damage < -1 || n.damage > 100_000 || (n.damage != -1 && n.kind != 2) {
                return Err("지뢰 damage 범위 밖".into());
            }
            prev = n.t;
        }
        let mut prev = i64::MIN;
        for x in &l.lsnds {
            if x.1 >= ns || x.2 >= nc || x.0 < prev {
                return Err("빈 레인 소리 레코드 오류".into());
            }
            prev = x.0;
        }
    }
    let lane_mask = if c.lanes.len() == 64 { u64::MAX } else { (1u64 << c.lanes.len()) - 1 };
    if c.groups.iter().any(|g| g.1 & !lane_mask != 0) {
        return Err("묶음이 없는 레인을 가리킴".into());
    }
    let mut prev = i64::MIN;
    for s in &c.scroll {
        fin(s.1, "스크롤 위치")?;
        fin(s.2, "스크롤 속도")?;
        if s.0 < prev {
            return Err("스크롤 시간순 아님".into());
        }
        prev = s.0;
    }
    if c.scroll.is_empty() {
        return Err("스크롤 점 없음".into());
    }
    if c.bars.windows(2).any(|w| w[1] < w[0]) {
        return Err("마디선 시간순 아님".into());
    }
    Ok(())
}
