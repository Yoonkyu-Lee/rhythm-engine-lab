//! 실행 데이터 파일 `.rtc` (runtime chart). 리틀 엔디언, 고정 순서. 같은 `Chart`는 항상 같은 바이트가 된다.
//!
//! ```text
//! "RTC1" u32 version(=1)
//! title:        u32 len, UTF-8
//! bank:         u32 rate, u16 channels, u16 0, u32 n_sounds
//!               n_sounds × { u64 offset, u32 frames, u8 kind(0 짧음/1 긺), u8 0 ×3 }
//!               u64 n_samples, n_samples × f32
//! auto:         u32 n, n × { i64 t, u32 sound, f32 gain }
//! note_sounds:  u32 n, n × { u32 sound, f32 gain }
//! lanes:        u32 n_lanes, 레인마다 { u32 n, n × { i64 t, i64 end, u32 sounds_start, u16 sounds_len, u16 0 } }
//! scroll:       u32 n, n × { i64 t, f64 pos, f64 speed }
//! f64 reference_bpm, i64 end
//! ```
//! 어댑터(코어 밖, `cp2/tools/rtc.py`)가 같은 배치로 쓴다.

use crate::runtime::*;

pub const MAGIC: &[u8; 4] = b"RTC1";
pub const VERSION: u32 = 1;

pub fn encode(c: &Chart) -> Vec<u8> {
    let mut w = Vec::with_capacity(64 + c.bank.pcm.len() * 4 + c.auto.len() * 16);
    w.extend_from_slice(MAGIC);
    put_u32(&mut w, VERSION);
    put_u32(&mut w, c.title.len() as u32);
    w.extend_from_slice(c.title.as_bytes());
    put_u32(&mut w, c.bank.rate);
    w.extend_from_slice(&c.bank.channels.to_le_bytes());
    w.extend_from_slice(&0u16.to_le_bytes());
    put_u32(&mut w, c.bank.sounds.len() as u32);
    for s in &c.bank.sounds {
        w.extend_from_slice(&s.offset.to_le_bytes());
        put_u32(&mut w, s.frames);
        w.extend_from_slice(&[(s.kind == SoundKind::Long) as u8, 0, 0, 0]);
    }
    w.extend_from_slice(&(c.bank.pcm.len() as u64).to_le_bytes());
    for x in &c.bank.pcm {
        w.extend_from_slice(&x.to_le_bytes());
    }
    put_u32(&mut w, c.auto.len() as u32);
    for e in &c.auto {
        w.extend_from_slice(&e.t.to_le_bytes());
        put_u32(&mut w, e.sound);
        w.extend_from_slice(&e.gain.to_le_bytes());
    }
    put_u32(&mut w, c.note_sounds.len() as u32);
    for s in &c.note_sounds {
        put_u32(&mut w, s.sound);
        w.extend_from_slice(&s.gain.to_le_bytes());
    }
    put_u32(&mut w, c.lanes.len() as u32);
    for lane in &c.lanes {
        put_u32(&mut w, lane.notes.len() as u32);
        for n in &lane.notes {
            w.extend_from_slice(&n.t.to_le_bytes());
            w.extend_from_slice(&n.end.to_le_bytes());
            put_u32(&mut w, n.sounds_start);
            w.extend_from_slice(&n.sounds_len.to_le_bytes());
            w.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    put_u32(&mut w, c.scroll.len() as u32);
    for p in &c.scroll {
        w.extend_from_slice(&p.t.to_le_bytes());
        w.extend_from_slice(&p.pos.to_le_bytes());
        w.extend_from_slice(&p.speed.to_le_bytes());
    }
    w.extend_from_slice(&c.reference_bpm.to_le_bytes());
    w.extend_from_slice(&c.end.to_le_bytes());
    w
}

fn put_u32(w: &mut Vec<u8>, x: u32) {
    w.extend_from_slice(&x.to_le_bytes());
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    BadMagic,
    BadVersion(u32),
    Truncated { at: usize, need: usize },
    BadUtf8,
    Trailing(usize),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for DecodeError {}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.b.len() - self.p < n {
            return Err(DecodeError::Truncated { at: self.p, need: n });
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.arr()?))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_le_bytes(self.arr()?))
    }
    /// 개수 × 크기가 남은 바이트를 넘지 않는지 먼저 본다 (잘못된 개수로 거대한 할당을 하지 않도록).
    fn count(&mut self, item: usize) -> Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(item) > self.b.len() - self.p {
            return Err(DecodeError::Truncated { at: self.p, need: n.saturating_mul(item) });
        }
        Ok(n)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Chart, DecodeError> {
    let mut r = R { b: bytes, p: 0 };
    if r.take(4)? != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let v = r.u32()?;
    if v != VERSION {
        return Err(DecodeError::BadVersion(v));
    }
    let n = r.count(1)?;
    let title = std::str::from_utf8(r.take(n)?).map_err(|_| DecodeError::BadUtf8)?.to_string();
    let rate = r.u32()?;
    let channels = r.u16()?;
    r.u16()?;
    let n = r.count(16)?;
    let mut sounds = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = r.u64()?;
        let frames = r.u32()?;
        let k = r.arr::<4>()?;
        sounds.push(SoundRef { offset, frames, kind: if k[0] == 1 { SoundKind::Long } else { SoundKind::Short } });
    }
    let ns = r.u64()? as usize;
    if ns.saturating_mul(4) > bytes.len() - r.p {
        return Err(DecodeError::Truncated { at: r.p, need: ns.saturating_mul(4) });
    }
    let raw = r.take(ns * 4)?;
    let pcm = raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    let n = r.count(16)?;
    let mut auto = Vec::with_capacity(n);
    for _ in 0..n {
        auto.push(SoundEvent { t: r.i64()?, sound: r.u32()?, gain: r.f32()? });
    }
    let n = r.count(8)?;
    let mut note_sounds = Vec::with_capacity(n);
    for _ in 0..n {
        note_sounds.push(NoteSound { sound: r.u32()?, gain: r.f32()? });
    }
    let n_lanes = r.count(4)?;
    let mut lanes = Vec::with_capacity(n_lanes);
    for _ in 0..n_lanes {
        let n = r.count(24)?;
        let mut notes = Vec::with_capacity(n);
        for _ in 0..n {
            let t = r.i64()?;
            let end = r.i64()?;
            let sounds_start = r.u32()?;
            let sounds_len = r.u16()?;
            r.u16()?;
            notes.push(Note { t, end, sounds_start, sounds_len });
        }
        lanes.push(Lane { notes });
    }
    let n = r.count(24)?;
    let mut scroll = Vec::with_capacity(n);
    for _ in 0..n {
        scroll.push(ScrollPoint { t: r.i64()?, pos: r.f64()?, speed: r.f64()? });
    }
    let reference_bpm = r.f64()?;
    let end = r.i64()?;
    if r.p != bytes.len() {
        return Err(DecodeError::Trailing(bytes.len() - r.p));
    }
    Ok(Chart { title, bank: SoundBank { rate, channels, pcm, sounds }, auto, note_sounds, lanes, scroll, reference_bpm, end })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::tiny;

    #[test]
    fn round_trip_is_exact() {
        let c = tiny();
        let b = encode(&c);
        assert_eq!(decode(&b).unwrap(), c);
        assert_eq!(encode(&decode(&b).unwrap()), b);
    }

    #[test]
    fn rejects_truncation_and_garbage() {
        let b = encode(&tiny());
        assert!(matches!(decode(&b[..b.len() - 1]), Err(DecodeError::Truncated { .. })));
        let mut x = b.clone();
        x.push(0);
        assert_eq!(decode(&x), Err(DecodeError::Trailing(1)));
        let mut x = b.clone();
        x[0] = b'X';
        assert_eq!(decode(&x), Err(DecodeError::BadMagic));
    }
}
