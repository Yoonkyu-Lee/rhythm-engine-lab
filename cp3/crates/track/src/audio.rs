//! 소리 파일 → 묶음에 담을 형태(코덱 표 + 데이터) + 기준 디코더 + 소리 해시 (명세 3-5, 5-2).
//!
//! 엔진 디코더 셋: Ogg Vorbis(패킷 보존, lewton), FLAC(프레임 보존, claxon), MS-ADPCM(블록 보존, 직접 구현 —
//! Microsoft ADPCM 명세대로 예측값은 0 쪽 버림 나눗셈. IIDX가 쓰는 Windows ACM과 같은 규칙, ffmpeg와 비트 단위로 같음.
//! libsndfile은 산술 시프트(내림)라 음수 예측에서 1 차이가 날 수 있다).

use std::io::Cursor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Vorbis = 1,
    Flac = 2,
    MsAdpcm = 3,
}

/// 디코딩한 PCM (채널 끼움). 정수 코덱은 원래 정수 값, Vorbis는 f32.
#[derive(Clone, Debug, PartialEq)]
pub enum Pcm {
    Int(Vec<i32>),
    Float(Vec<f32>),
}

impl Pcm {
    pub fn len(&self) -> usize {
        match self {
            Pcm::Int(v) => v.len(),
            Pcm::Float(v) => v.len(),
        }
    }
}

/// 묶음에 담는 소리 데이터 하나.
#[derive(Clone, Debug)]
pub struct Packed {
    pub codec: Codec,
    pub channels: u8,
    pub rate: u32,
    /// 코덱 트림 뒤 샘플 수 (채널당)
    pub frames: u64,
    pub skip_front: u32,
    /// 디코딩 결과가 16비트 정수인가 (메모리 형식 판단 재료, README 4-3)
    pub int16: bool,
    /// 코덱 표 (Vorbis: ident + setup 머리, FLAC: STREAMINFO 34 B, MS-ADPCM: fmt 청크 본문)
    pub codec_table: Vec<u8>,
    /// 데이터 (Vorbis: 패킷 수·길이 표 + 패킷들, FLAC: 프레임열, MS-ADPCM: 블록열)
    pub data: Vec<u8>,
}

/// 소리 파일 바이트 → 묶음 형태. 확장자가 아니라 내용(매직)으로 판별한다.
pub fn pack_file(bytes: &[u8]) -> Result<Packed, String> {
    if bytes.starts_with(b"OggS") {
        pack_ogg_vorbis(bytes)
    } else if bytes.starts_with(b"fLaC") {
        pack_flac(bytes)
    } else if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WAVE" {
        pack_wav_adpcm(bytes)
    } else {
        Err("엔진 디코더 셋(Ogg Vorbis, FLAC, MS-ADPCM WAV) 밖의 파일".into())
    }
}

/// 기준 디코더: 묶음 형태 → PCM (코덱 트림 적용, `start` 적용 전).
pub fn decode(p: &Packed) -> Result<Pcm, String> {
    let ch = p.channels as usize;
    let want = p.frames as usize * ch;
    let skip = p.skip_front as usize * ch;
    match p.codec {
        Codec::Vorbis => {
            let v = decode_vorbis(&p.codec_table, &p.data)?;
            if v.len() < skip + want {
                return Err(format!("Vorbis 디코딩 길이 부족: {} < {}", v.len(), skip + want));
            }
            Ok(Pcm::Float(v[skip..skip + want].to_vec()))
        }
        Codec::Flac => {
            let v = decode_flac(&p.codec_table, &p.data)?;
            if v.len() < want {
                return Err(format!("FLAC 디코딩 길이 부족: {} < {}", v.len(), want));
            }
            Ok(Pcm::Int(v[..want].to_vec()))
        }
        Codec::MsAdpcm => {
            let v = decode_adpcm(&p.codec_table, &p.data)?;
            if v.len() < want {
                return Err(format!("ADPCM 디코딩 길이 부족: {} < {}", v.len(), want));
            }
            Ok(Pcm::Int(v[..want].to_vec()))
        }
    }
}

/// 소리 해시 (명세 3-5, v1.4). 정수 코덱(FLAC·MS-ADPCM) = BLAKE3("SND1" ‖ rate ‖ channels ‖ frames ‖ 디코딩 PCM i32 LE).
/// Vorbis = BLAKE3("SNDV" ‖ rate ‖ channels ‖ frames ‖ skip_front ‖ 코덱 표 길이 u64 ‖ 코덱 표 ‖ 데이터) — 보존한 패킷 자체로 정한다.
/// Vorbis 디코더(lewton)는 표를 실행 중 f32 sin/cos로 만들어 플랫폼마다 PCM 비트가 다를 수 있으므로(리뷰 3-1 #11),
/// 소리·차분 해시가 환경에 따라 바뀌지 않게 디코딩 결과를 해시에 넣지 않는다.
pub fn content_hash(p: &Packed, pcm: &Pcm) -> [u8; 32] {
    match p.codec {
        Codec::Vorbis => {
            let mut h = blake3::Hasher::new();
            h.update(b"SNDV");
            h.update(&p.rate.to_le_bytes());
            h.update(&(p.channels as u16).to_le_bytes());
            h.update(&p.frames.to_le_bytes());
            h.update(&p.skip_front.to_le_bytes());
            h.update(&(p.codec_table.len() as u64).to_le_bytes());
            h.update(&p.codec_table);
            h.update(&p.data);
            *h.finalize().as_bytes()
        }
        _ => sound_hash(p.rate, p.channels, p.frames, pcm),
    }
}

/// PCM 해시 = BLAKE3("SND1" ‖ rate u32 ‖ channels u16 ‖ frames u64 ‖ PCM). 정수는 i32 LE, Vorbis는 f32 LE.
pub fn sound_hash(rate: u32, channels: u8, frames: u64, pcm: &Pcm) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"SND1");
    h.update(&rate.to_le_bytes());
    h.update(&(channels as u16).to_le_bytes());
    h.update(&frames.to_le_bytes());
    match pcm {
        Pcm::Int(v) => {
            for s in v {
                h.update(&s.to_le_bytes());
            }
        }
        Pcm::Float(v) => {
            for s in v {
                h.update(&s.to_le_bytes());
            }
        }
    }
    *h.finalize().as_bytes()
}

// ------------------------------------------------------------------ Ogg Vorbis

/// Ogg 페이지를 훑어 패킷과 (그 페이지에서 끝난 패킷 수 누계, granule) 목록을 만든다. 논리 스트림 하나만.
fn ogg_packets(bytes: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<(usize, i64, bool)>), String> {
    // 페이지 검증: 버전 0, 순번 0부터 1씩, CRC, BOS는 첫 페이지만, EOS 뒤 데이터 없음, 이어짐 표시 = 앞 패킷이 끝나지 않음 (리뷰 3-1 #10)
    let mut pos = 0usize;
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut open = false;
    let mut granules = Vec::new();
    let mut serial = None;
    let mut seq_expect = 0u32;
    let mut eos_seen = false;
    while pos < bytes.len() {
        if eos_seen {
            return Err("Ogg EOS 페이지 뒤에 데이터가 있음".into());
        }
        let head = bytes.get(pos..pos + 27).ok_or("Ogg 페이지 머리가 잘림")?;
        if &head[0..4] != b"OggS" || head[4] != 0 {
            return Err(format!("Ogg 페이지가 아님 (오프셋 {pos})"));
        }
        let header_type = head[5];
        let granule = i64::from_le_bytes(head[6..14].try_into().unwrap());
        let ser = u32::from_le_bytes(head[14..18].try_into().unwrap());
        let seq = u32::from_le_bytes(head[18..22].try_into().unwrap());
        let crc = u32::from_le_bytes(head[22..26].try_into().unwrap());
        match serial {
            None => serial = Some(ser),
            Some(s) if s != ser => return Err("Ogg 스트림이 둘 이상 (연결·다중 스트림은 미지원)".into()),
            _ => {}
        }
        if seq != seq_expect {
            return Err(format!("Ogg 페이지 순번이 이어지지 않음 ({seq} != {seq_expect})"));
        }
        seq_expect = seq_expect.wrapping_add(1);
        if (header_type & 0x02 != 0) != (seq == 0) {
            return Err("Ogg BOS 표시가 첫 페이지에만 있지 않음 (연결 스트림은 미지원)".into());
        }
        if (header_type & 0x01 != 0) != open {
            return Err("Ogg 이어짐 표시가 패킷 상태와 맞지 않음".into());
        }
        let nseg = head[26] as usize;
        let segs = bytes.get(pos + 27..pos + 27 + nseg).ok_or("Ogg 세그먼트 표가 잘림")?;
        let body_len: usize = segs.iter().map(|&x| x as usize).sum();
        let page_end = pos + 27 + nseg + body_len;
        let page = bytes.get(pos..page_end).ok_or("Ogg 페이지가 잘림")?;
        let mut zeroed = page.to_vec();
        zeroed[22..26].fill(0);
        if ogg_crc(&zeroed) != crc {
            return Err(format!("Ogg CRC가 맞지 않음 (페이지 {seq})"));
        }
        let mut body = pos + 27 + nseg;
        for &len in segs {
            let len = len as usize;
            cur.extend_from_slice(&bytes[body..body + len]);
            body += len;
            if len < 255 {
                packets.push(std::mem::take(&mut cur));
            }
        }
        open = segs.last().map(|&l| l == 255).unwrap_or(open);
        if granule != -1 {
            granules.push((packets.len(), granule, header_type & 0x04 != 0)); // (끝난 패킷 누계, granule, EOS)
        }
        eos_seen = header_type & 0x04 != 0;
        pos = page_end;
    }
    if !cur.is_empty() || open {
        return Err("마지막 Ogg 패킷이 끝나지 않음".into());
    }
    Ok((packets, granules))
}

fn pack_ogg_vorbis(bytes: &[u8]) -> Result<Packed, String> {
    let (packets, granules) = ogg_packets(bytes)?;
    if packets.len() < 4 {
        return Err("Vorbis 머리 3개와 오디오 패킷이 없음".into());
    }
    let ident = lewton::header::read_header_ident(&packets[0]).map_err(|e| format!("Vorbis ident: {e:?}"))?;
    let setup = lewton::header::read_header_setup(&packets[2], ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
        .map_err(|e| format!("Vorbis setup: {e:?}"))?;
    // 패킷별 출력 길이를 재서 granule과 맞춘다
    let mut pwr = lewton::audio::PreviousWindowRight::new();
    let ch = ident.audio_channels as usize;
    let mut cum = vec![0i64; packets.len() + 1]; // cum[k] = 패킷 k 전까지의 출력 샘플 수(채널당)
    for (k, pkt) in packets.iter().enumerate().skip(3) {
        let s: lewton::samples::InterleavedSamples<f32> =
            lewton::audio::read_audio_packet_generic(&ident, &setup, pkt, &mut pwr).map_err(|e| format!("Vorbis 패킷 {k}: {e:?}"))?;
        cum[k + 1] = cum[k] + (s.samples.len() / ch) as i64;
    }
    for k in 0..3 {
        cum[k + 1] = 0;
    }
    let decoded = cum[packets.len()];
    // 첫 오디오 granule이 있는 페이지와 마지막 granule
    let (first_n, first_g, first_eos) = *granules.iter().find(|(n, _, _)| *n > 3).ok_or("오디오 granule 없음")?;
    let (_, last_g, _) = *granules.last().unwrap();
    // 출력 샘플 0의 granule. 첫 오디오 페이지가 곧 마지막 페이지(EOS)면 그 granule은 끝 자르기라 앞은 0 (Vorbis I 명세 A.2)
    let base = if first_eos { 0 } else { first_g - cum[first_n] };
    let skip_front = if base < 0 { (-base) as u64 } else { 0 };
    let end = (last_g - base).min(decoded);
    if end <= skip_front as i64 {
        return Err("Vorbis 길이 0".into());
    }
    let frames = end as u64 - skip_front;
    let mut table = Vec::new();
    for p in [&packets[0], &packets[2]] {
        table.extend_from_slice(&(p.len() as u32).to_le_bytes());
        table.extend_from_slice(p);
    }
    let audio = &packets[3..];
    let mut data = Vec::new();
    data.extend_from_slice(&(audio.len() as u32).to_le_bytes());
    for p in audio {
        data.extend_from_slice(&(p.len() as u32).to_le_bytes());
    }
    for p in audio {
        data.extend_from_slice(p);
    }
    Ok(Packed {
        codec: Codec::Vorbis,
        channels: ident.audio_channels,
        rate: ident.audio_sample_rate,
        frames,
        skip_front: skip_front as u32,
        int16: false,
        codec_table: table,
        data,
    })
}

fn decode_vorbis(table: &[u8], data: &[u8]) -> Result<Vec<f32>, String> {
    let rd = |b: &[u8], o: usize| -> Result<u32, String> {
        b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).ok_or_else(|| "Vorbis 표가 잘림".to_string())
    };
    let l0 = rd(table, 0)? as usize;
    let ident_b = table.get(4..4 + l0).ok_or("Vorbis 표가 잘림")?;
    let l2 = rd(table, 4 + l0)? as usize;
    let setup_b = table.get(8 + l0..8 + l0 + l2).ok_or("Vorbis 표가 잘림")?;
    let ident = lewton::header::read_header_ident(ident_b).map_err(|e| format!("Vorbis ident: {e:?}"))?;
    let setup = lewton::header::read_header_setup(setup_b, ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
        .map_err(|e| format!("Vorbis setup: {e:?}"))?;
    let n = rd(data, 0)? as usize;
    let mut off = 4 + 4 * n;
    let mut pwr = lewton::audio::PreviousWindowRight::new();
    let mut out = Vec::new();
    for i in 0..n {
        let len = rd(data, 4 + 4 * i)? as usize;
        let pkt = data.get(off..off + len).ok_or("Vorbis 데이터가 잘림")?;
        off += len;
        let s: lewton::samples::InterleavedSamples<f32> =
            lewton::audio::read_audio_packet_generic(&ident, &setup, pkt, &mut pwr).map_err(|e| format!("Vorbis 패킷 {i}: {e:?}"))?;
        out.extend_from_slice(&s.samples);
    }
    Ok(out)
}

// ------------------------------------------------------------------ FLAC

fn pack_flac(bytes: &[u8]) -> Result<Packed, String> {
    let mut pos = 4usize;
    let mut streaminfo: Option<Vec<u8>> = None;
    loop {
        let h = bytes.get(pos..pos + 4).ok_or("FLAC 메타데이터가 잘림")?;
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7f;
        let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as usize;
        let body = bytes.get(pos + 4..pos + 4 + len).ok_or("FLAC 메타데이터가 잘림")?;
        if kind == 0 {
            if len != 34 {
                return Err("STREAMINFO 길이가 34가 아님".into());
            }
            streaminfo = Some(body.to_vec());
        }
        pos += 4 + len;
        if last {
            break;
        }
    }
    let si = streaminfo.ok_or("STREAMINFO 없음")?;
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let channels = ((si[12] >> 1) & 0x07) + 1;
    let bps = (((si[12] & 1) << 4) | (si[13] >> 4)) + 1;
    let total = ((u64::from(si[13] & 0x0f)) << 32) | u64::from(u32::from_be_bytes(si[14..18].try_into().unwrap()));
    let data = bytes[pos..].to_vec();
    let frames = if total > 0 {
        total
    } else {
        (decode_flac(&si, &data)?.len() / channels as usize) as u64
    };
    Ok(Packed { codec: Codec::Flac, channels, rate, frames, skip_front: 0, int16: bps == 16, codec_table: si, data })
}

fn decode_flac(streaminfo: &[u8], frames: &[u8]) -> Result<Vec<i32>, String> {
    let mut file = Vec::with_capacity(8 + 34 + frames.len());
    file.extend_from_slice(b"fLaC");
    file.extend_from_slice(&[0x80, 0, 0, 34]); // 마지막 블록, STREAMINFO, 길이 34
    file.extend_from_slice(streaminfo);
    file.extend_from_slice(frames);
    let mut r = claxon::FlacReader::new(Cursor::new(file)).map_err(|e| format!("FLAC: {e}"))?;
    let ch = r.streaminfo().channels as usize;
    let mut out = Vec::new();
    let mut blocks = r.blocks();
    let mut buf = Vec::new();
    loop {
        match blocks.read_next_or_eof(buf) {
            Ok(Some(b)) => {
                for i in 0..b.duration() as usize {
                    for c in 0..ch {
                        out.push(b.channel(c as u32)[i]);
                    }
                }
                buf = b.into_buffer();
            }
            Ok(None) => break,
            Err(e) => return Err(format!("FLAC 프레임: {e}")),
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ MS-ADPCM WAV

struct AdpcmFmt {
    channels: u16,
    rate: u32,
    block_align: usize,
    samples_per_block: usize,
    coefs: Vec<(i32, i32)>,
}

fn parse_adpcm_fmt(f: &[u8]) -> Result<AdpcmFmt, String> {
    if f.len() < 22 {
        return Err("WAV fmt가 짧음 (MS-ADPCM은 22바이트 이상)".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([f[o], f[o + 1]]);
    let tag = u16_at(0);
    if tag != 2 {
        return Err(format!("WAV이지만 MS-ADPCM이 아님 (형식 태그 {tag}) — 컨버터가 FLAC으로 바꿔야 함"));
    }
    let channels = u16_at(2);
    let rate = u32::from_le_bytes(f[4..8].try_into().unwrap());
    let block_align = u16_at(12) as usize;
    let samples_per_block = u16_at(18) as usize;
    let ncoef = u16_at(20) as usize;
    let mut coefs = Vec::new();
    if ncoef > 256 {
        return Err("ADPCM 계수가 너무 많음".into());
    }
    for i in 0..ncoef {
        let o = 22 + 4 * i;
        if f.len() < o + 4 {
            return Err("ADPCM 계수 표가 잘림".into());
        }
        coefs.push((i16::from_le_bytes([f[o], f[o + 1]]) as i32, i16::from_le_bytes([f[o + 2], f[o + 3]]) as i32));
    }
    if !(1..=2).contains(&channels) || block_align < 7 * channels as usize || samples_per_block < 2 || coefs.is_empty() {
        return Err("ADPCM fmt 값이 이상함".into());
    }
    Ok(AdpcmFmt { channels, rate, block_align, samples_per_block, coefs })
}

fn pack_wav_adpcm(bytes: &[u8]) -> Result<Packed, String> {
    let mut pos = 12usize;
    let mut fmt = None;
    let mut data = None;
    let mut fact = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = bytes.get(pos + 8..pos + 8 + len).ok_or("WAV 청크가 잘림")?;
        match id {
            b"fmt " => fmt = Some(body.to_vec()),
            b"data" => data = Some(body.to_vec()),
            b"fact" if len >= 4 => fact = Some(u32::from_le_bytes(body[0..4].try_into().unwrap())),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let fmt_b = fmt.ok_or("WAV fmt 없음")?;
    let data = data.ok_or("WAV data 없음")?;
    let f = parse_adpcm_fmt(&fmt_b)?;
    let decoded = decode_adpcm(&fmt_b, &data)?.len() / f.channels as usize;
    let frames = match fact {
        Some(n) if (n as usize) <= decoded => n as u64,
        Some(n) => return Err(format!("ADPCM fact 길이 {n}가 실제 디코딩 길이 {decoded}보다 큼 (손상)")),
        None => decoded as u64,
    };
    Ok(Packed { codec: Codec::MsAdpcm, channels: f.channels as u8, rate: f.rate, frames, skip_front: 0, int16: true, codec_table: fmt_b, data })
}

const ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];

/// MS-ADPCM 블록열 → i16 값(i32로). 마지막 블록이 짧으면 그만큼만.
fn decode_adpcm(fmt_b: &[u8], data: &[u8]) -> Result<Vec<i32>, String> {
    let f = parse_adpcm_fmt(fmt_b)?;
    let ch = f.channels as usize;
    let mut out = Vec::new();
    for block in data.chunks(f.block_align) {
        if block.len() < 7 * ch {
            break;
        }
        let mut pred = [0usize; 2];
        let mut delta = [0i64; 2];
        let mut s1 = [0i64; 2];
        let mut s2 = [0i64; 2];
        let rd16 = |o: usize| i16::from_le_bytes([block[o], block[o + 1]]) as i64;
        for c in 0..ch {
            pred[c] = block[c] as usize;
            if pred[c] >= f.coefs.len() {
                return Err(format!("ADPCM 예측 번호 범위 밖: {}", pred[c]));
            }
            delta[c] = rd16(ch + 2 * c);
            s1[c] = rd16(3 * ch + 2 * c);
            s2[c] = rd16(5 * ch + 2 * c);
        }
        // 블록 머리의 두 샘플: sample2 먼저, 그다음 sample1
        let mut frame_out = 2usize;
        for c in 0..ch {
            out.push(s2[c] as i32);
        }
        for c in 0..ch {
            out.push(s1[c] as i32);
        }
        let body = &block[7 * ch..];
        let mut nib = 0usize; // 니블 순번 (높은 니블 먼저, 채널 교대)
        let total_nibbles = body.len() * 2;
        while frame_out < f.samples_per_block && nib + ch <= total_nibbles {
            for c in 0..ch {
                let byte = body[nib / 2];
                let n = if nib % 2 == 0 { byte >> 4 } else { byte & 0x0f } as i64;
                nib += 1;
                let (c1, c2) = f.coefs[pred[c]];
                // 64비트 중간 연산 (리뷰 3-1 #8). 예측값은 0 쪽 버림 (Microsoft 명세)
                let predict = (s1[c] * c1 as i64 + s2[c] * c2 as i64) / 256;
                let signed = if n & 8 != 0 { n - 16 } else { n };
                let sample = (predict + signed * delta[c]).clamp(-32768, 32767);
                // delta 상한은 ffmpeg와 같은 i32::MAX / 768 (손상 파일에서 끝없이 커지지 않게)
                delta[c] = ((ADAPT[n as usize] as i64 * delta[c]) / 256).clamp(16, (i32::MAX / 768) as i64);
                s2[c] = s1[c];
                s1[c] = sample;
                out.push(sample as i32);
            }
            frame_out += 1;
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ 되살리기 (묶음 → 파일)

const OGG_CRC_POLY: u32 = 0x04c1_1db7;

fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ OGG_CRC_POLY } else { crc << 1 };
        }
    }
    crc
}

/// 패킷 하나를 페이지 여러 개로 (페이지당 세그먼트 255개까지). 패킷이 끝나는 마지막 페이지에만 granule, 나머지는 -1.
fn ogg_packet_pages(out: &mut Vec<u8>, first_flags: u8, last_flags: u8, granule: i64, serial: u32, seq: &mut u32, packet: &[u8]) {
    let mut lacing = Vec::new();
    let mut n = packet.len();
    loop {
        if n >= 255 {
            lacing.push(255u8);
            n -= 255;
        } else {
            lacing.push(n as u8);
            break;
        }
    }
    let chunks: Vec<&[u8]> = lacing.chunks(255).collect();
    let mut off = 0usize;
    for (i, segs) in chunks.iter().enumerate() {
        let first = i == 0;
        let last = i + 1 == chunks.len();
        let mut flags = if first { first_flags } else { 0x01 };
        if last {
            flags |= last_flags;
        }
        let body_len: usize = segs.iter().map(|&x| x as usize).sum();
        let start = out.len();
        out.extend_from_slice(b"OggS");
        out.push(0);
        out.push(flags);
        out.extend_from_slice(&(if last { granule } else { -1 }).to_le_bytes());
        out.extend_from_slice(&serial.to_le_bytes());
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.push(segs.len() as u8);
        out.extend_from_slice(segs);
        out.extend_from_slice(&packet[off..off + body_len]);
        off += body_len;
        let crc = ogg_crc(&out[start..]);
        out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        *seq += 1;
    }
}

/// 묶음 형태 → 파일 바이트와 확장자. 되살린 파일을 다시 `pack_file` + `decode`하면 소리 해시가 같다.
/// (Ogg는 패킷마다 페이지를 새로 시작하고, 큰 패킷은 이어지는 페이지로 나눈다. comment 머리는 비운다.)
pub fn to_file(p: &Packed) -> Result<(Vec<u8>, &'static str), String> {
    match p.codec {
        Codec::Flac => {
            let mut f = b"fLaC".to_vec();
            f.extend_from_slice(&[0x80, 0, 0, 34]);
            f.extend_from_slice(&p.codec_table);
            f.extend_from_slice(&p.data);
            Ok((f, "flac"))
        }
        Codec::MsAdpcm => {
            let fmt = &p.codec_table;
            let mut f = Vec::new();
            let riff_len = 4 + (8 + fmt.len() + (fmt.len() & 1)) + 12 + (8 + p.data.len() + (p.data.len() & 1));
            f.extend_from_slice(b"RIFF");
            f.extend_from_slice(&(riff_len as u32).to_le_bytes());
            f.extend_from_slice(b"WAVEfmt ");
            f.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
            f.extend_from_slice(fmt);
            if fmt.len() & 1 == 1 {
                f.push(0);
            }
            f.extend_from_slice(b"fact");
            f.extend_from_slice(&4u32.to_le_bytes());
            f.extend_from_slice(&(p.frames as u32).to_le_bytes());
            f.extend_from_slice(b"data");
            f.extend_from_slice(&(p.data.len() as u32).to_le_bytes());
            f.extend_from_slice(&p.data);
            if p.data.len() & 1 == 1 {
                f.push(0);
            }
            Ok((f, "wav"))
        }
        Codec::Vorbis => {
            let rd = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
            let t = &p.codec_table;
            let l0 = rd(t, 0);
            let ident = &t[4..4 + l0];
            let l2 = rd(t, 4 + l0);
            let setup = &t[8 + l0..8 + l0 + l2];
            let comment: Vec<u8> = [&[3u8][..], b"vorbis", &0u32.to_le_bytes(), &0u32.to_le_bytes(), &[1u8]].concat();
            let id = lewton::header::read_header_ident(ident).map_err(|e| format!("{e:?}"))?;
            let st = lewton::header::read_header_setup(setup, id.audio_channels, (id.blocksize_0, id.blocksize_1)).map_err(|e| format!("{e:?}"))?;
            let n = rd(&p.data, 0);
            let mut lens = Vec::new();
            for i in 0..n {
                lens.push(rd(&p.data, 4 + 4 * i));
            }
            let mut off = 4 + 4 * n;
            let serial = 0x5452_4b31; // "TRK1"
            let mut f = Vec::new();
            let mut seq = 0u32;
            ogg_packet_pages(&mut f, 0x02, 0, 0, serial, &mut seq, ident);
            for h in [&comment[..], setup] {
                ogg_packet_pages(&mut f, 0, 0, 0, serial, &mut seq, h);
            }
            let mut pwr = lewton::audio::PreviousWindowRight::new();
            let mut cum: i64 = 0;
            let ch = id.audio_channels as i64;
            for i in 0..n {
                let pkt = &p.data[off..off + lens[i]];
                off += lens[i];
                let s: crate::audio::VorbisOut = lewton::audio::read_audio_packet_generic(&id, &st, pkt, &mut pwr).map_err(|e| format!("{e:?}"))?;
                cum += s.samples.len() as i64 / ch;
                let last = i + 1 == n;
                let granule = if last { p.frames as i64 } else { cum - p.skip_front as i64 };
                ogg_packet_pages(&mut f, 0, if last { 0x04 } else { 0 }, granule, serial, &mut seq, pkt);
            }
            Ok((f, "ogg"))
        }
    }
}

pub type VorbisOut = lewton::samples::InterleavedSamples<f32>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ogg_pages_roundtrip_large_packets() {
        // 큰 패킷(세그먼트 255개 넘음)을 이어지는 페이지로 나눴다가 다시 읽으면 같은 패킷 (리뷰 3-1 #10)
        let sizes = [0usize, 1, 254, 255, 256, 255 * 255 - 1, 255 * 255, 70_000, 200_000];
        let mut f = Vec::new();
        let mut seq = 0u32;
        let pkts: Vec<Vec<u8>> = sizes.iter().map(|&n| (0..n).map(|i| (i * 7 + n) as u8).collect()).collect();
        for (i, p) in pkts.iter().enumerate() {
            let first = if i == 0 { 0x02 } else { 0 };
            let last = if i + 1 == pkts.len() { 0x04 } else { 0 };
            ogg_packet_pages(&mut f, first, last, i as i64, 7, &mut seq, p);
        }
        let (got, gr) = ogg_packets(&f).unwrap();
        assert_eq!(got, pkts);
        assert_eq!(gr.last().unwrap().1, (pkts.len() - 1) as i64);
        // CRC 한 바이트를 바꾸면 거부
        let mut bad = f.clone();
        bad[30] ^= 1;
        assert!(ogg_packets(&bad).is_err());
        // 잘린 머리는 패닉 없이 거부
        assert!(ogg_packets(&f[..20]).is_err());
        let mut short = b"OggS\0\x02".to_vec();
        short.extend_from_slice(&[0; 20]);
        short.push(1); // nseg = 1 인데 세그먼트 표 없음
        assert!(ogg_packets(&short).is_err());
    }

    #[test]
    fn adpcm_extreme_delta_no_overflow() {
        // 초기 delta 32767, 니블 8 반복: 64비트 중간 연산으로 넘치지 않고 결정적으로 디코딩 (리뷰 3-1 #8)
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&2u16.to_le_bytes()); // MS-ADPCM
        fmt.extend_from_slice(&1u16.to_le_bytes()); // 모노
        fmt.extend_from_slice(&22050u32.to_le_bytes());
        fmt.extend_from_slice(&0u32.to_le_bytes());
        fmt.extend_from_slice(&64u16.to_le_bytes()); // block_align
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&116u16.to_le_bytes()); // samples per block
        fmt.extend_from_slice(&1u16.to_le_bytes()); // 계수 1개
        fmt.extend_from_slice(&256i16.to_le_bytes());
        fmt.extend_from_slice(&0i16.to_le_bytes());
        let mut block = vec![0u8, 0xff, 0x7f, 0, 0, 0, 0]; // pred 0, delta 32767, s1 0, s2 0
        block.extend(std::iter::repeat(0x88).take(57));
        let out = decode_adpcm(&fmt, &block).unwrap();
        assert_eq!(out.len(), 116);
        assert!(out.iter().all(|&s| (-32768..=32767).contains(&s)));
        // fmt가 짧으면 패닉 없이 거부
        assert!(decode_adpcm(&fmt[..21], &block).is_err());
    }
}
