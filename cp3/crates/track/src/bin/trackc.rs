//! Track 컴파일러 도구 (cp3/Track_형식.md).
//!
//!   trackc compile <작성 형식 폴더> <출력.trk>     컴파일 (경고는 표준 오류로)
//!   trackc verify  <묶음.trk>                      해시·구역·소리 디코딩·차분 왕복 검사
//!   trackc dump    <묶음.trk>                      차분 내용을 줄 단위 텍스트로 (검산기 대조용)
//!   trackc unpack  <묶음.trk> <폴더>               작성 형식 폴더로 되살림 (오디오는 SOUND_DATA에서)
//!   trackc decode  <소리 파일> <출력.raw>          기준 디코더 출력 (정수 i32 LE / Vorbis f32 LE)
//!   trackc info    <묶음.trk>                      묶음·구역·차분·소리 해시 (시험 벡터용)
//!   trackc index   <라이브러리 폴더>               tracks/*.trk → index.bin (라이브러리 색인)
use std::path::Path;
use std::process::ExitCode;
use track::{audio, compile, pack};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn run(args: &[String]) -> Result<(), String> {
    match args.get(1).map(|s| s.as_str()) {
        Some("compile") => {
            let (dir, out) = (args.get(2).ok_or("폴더")?, args.get(3).ok_or("출력")?);
            let t = compile::compile_dir(Path::new(dir))?;
            for w in &t.warnings {
                eprintln!("경고: {w}");
            }
            let b = pack::write_bundle(&t);
            std::fs::write(out, &b).map_err(|e| e.to_string())?;
            println!("{} B, 차분 {}, 소리 ID {}, 소리 데이터 {}, 묶음 해시 {}", b.len(), t.charts.len(), t.sound_ids.len(), t.datas.len(), hex(&b[72..104]));
            Ok(())
        }
        Some("verify") => {
            let b = pack::Bundle::open(std::fs::read(args.get(2).ok_or("파일")?).map_err(|e| e.to_string())?)?;
            let sounds = pack::read_sounds(&b)?;
            for (i, (h, p)) in sounds.iter().enumerate() {
                let pcm = audio::decode(p)?;
                if &audio::sound_hash(p.rate, p.channels, p.frames, &pcm) != h {
                    return Err(format!("소리 데이터 {i}: 디코딩 해시가 표와 다름"));
                }
            }
            let charts = pack::read_charts(&b)?;
            for c in &charts {
                let sec = b.section(pack::K_CHART, c.section as usize).ok_or("CHART 없음")?;
                if pack::chart_hash(sec) != c.hash {
                    return Err(format!("차분 {}: 해시가 다름", c.id));
                }
                let d = pack::decode_chart(sec)?;
                let re = pack::encode_chart(&d);
                if re != sec {
                    let k = re.iter().zip(sec.iter()).position(|(x, y)| x != y).unwrap_or(re.len().min(sec.len()));
                    return Err(format!("차분 {}: 읽고 다시 쓴 바이트가 다름 (오프셋 {k}, 길이 {} / {})", c.id, re.len(), sec.len()));
                }
            }
            println!("통과: 구역 {}, 소리 데이터 {} (디코딩 해시 일치), 차분 {} (해시·왕복 일치)", b.sections.len(), sounds.len(), charts.len());
            Ok(())
        }
        Some("dump") => {
            let b = pack::Bundle::open(std::fs::read(args.get(2).ok_or("파일")?).map_err(|e| e.to_string())?)?;
            let mut r = pack::R::new(b.section(pack::K_SOUND_IDS, 0).ok_or("SOUND_IDS 없음")?);
            let n = r.u32()?;
            let mut names = Vec::new();
            for _ in 0..n {
                r.u32()?;
                r.u32()?;
                r.f32()?;
                r.u8()?;
                r.bytes(3)?;
                r.u32()?;
                names.push(r.str()?);
            }
            for c in pack::read_charts(&b)? {
                let s = pack::decode_chart(b.section(pack::K_CHART, c.section as usize).ok_or("CHART 없음")?)?;
                let nm = |l: u32| names[c.local_to_track[l as usize] as usize].clone();
                println!("chart {} shift {} end {} hash {}", c.id, s.shift, s.end, hex(&c.hash[..8]));
                for a in &s.auto {
                    println!("auto {} {} {} {}", a.0, nm(a.1), a.2, a.3);
                }
                for l in &s.lanes {
                    for n in &l.notes {
                        println!("note {} {} {} {} {} {}", l.id, n.t, n.end, n.kind, n.cond, n.damage);
                        for x in &s.note_sounds[n.ns_start as usize..(n.ns_start + n.ns_len as u32) as usize] {
                            println!("nsnd {} {} {} {}", l.id, n.t + x.0, nm(x.1), x.2);
                        }
                    }
                    for x in &l.lsnds {
                        println!("lsnd {} {} {} {}", l.id, x.0, nm(x.1), x.2);
                    }
                }
                for p in &s.scroll {
                    println!("scroll {} {} {}", p.0, p.1, p.2);
                }
                for t in &s.bars {
                    println!("bar {t}");
                }
            }
            Ok(())
        }
        Some("unpack") => {
            let b = pack::Bundle::open(std::fs::read(args.get(2).ok_or("파일")?).map_err(|e| e.to_string())?)?;
            let out = Path::new(args.get(3).ok_or("폴더")?);
            let mut r = pack::R::new(b.section(pack::K_SOURCE, 0).ok_or("SOURCE 없음")?);
            for _ in 0..r.u32()? {
                let p = r.str()?;
                let n = r.u64()? as usize;
                let dst = out.join(&p);
                std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(dst, r.bytes(n)?).map_err(|e| e.to_string())?;
            }
            // 오디오: 작성 원본(track.toml)의 file 경로로, SOUND_IDS → SOUNDS 데이터에서 되살림
            let tr: track::source::TrackToml =
                toml::from_str(&std::fs::read_to_string(out.join("track.toml")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let sounds = pack::read_sounds(&b)?;
            let mut r = pack::R::new(b.section(pack::K_SOUND_IDS, 0).ok_or("SOUND_IDS 없음")?);
            let n = r.u32()? as usize;
            for k in 0..n {
                let data = r.u32()? as usize;
                r.u32()?;
                r.f32()?;
                r.u8()?;
                r.bytes(3)?;
                r.u32()?;
                let id = r.str()?;
                let def = &tr.sound[k];
                if def.id != id {
                    return Err("SOUND_IDS 순서가 track.toml과 다름".into());
                }
                let (bytes, _ext) = audio::to_file(&sounds[data].1)?;
                let dst = out.join(&def.file);
                std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(dst, bytes).map_err(|e| e.to_string())?;
            }
            println!("되살림: {}", out.display());
            Ok(())
        }
        Some("info") => {
            let b = pack::Bundle::open(std::fs::read(args.get(2).ok_or("파일")?).map_err(|e| e.to_string())?)?;
            println!("bundle {} {} B minor {} revision {}", hex(&b.bytes[72..104]), b.bytes.len(), b.minor, b.revision);
            for (k, o, l) in &b.sections {
                println!("section {k} {o} {l} {}", hex(blake3::hash(&b.bytes[*o..o + l]).as_bytes()));
            }
            for c in pack::read_charts(&b)? {
                println!("chart {} {}", c.id, hex(&c.hash));
            }
            for (h, p) in pack::read_sounds(&b)? {
                println!("sound {} {:?} {} {} {}", hex(&h), p.codec, p.rate, p.channels, p.frames);
            }
            Ok(())
        }
        Some("index") => {
            let lib = Path::new(args.get(2).ok_or("라이브러리 폴더")?);
            let n = track::library::build_index(lib)?;
            println!("index.bin: Track {n}개");
            Ok(())
        }
        Some("decode") => {
            let bytes = std::fs::read(args.get(2).ok_or("파일")?).map_err(|e| e.to_string())?;
            let p = audio::pack_file(&bytes)?;
            let pcm = audio::decode(&p)?;
            let mut out = Vec::new();
            match &pcm {
                audio::Pcm::Int(v) => v.iter().for_each(|s| out.extend_from_slice(&s.to_le_bytes())),
                audio::Pcm::Float(v) => v.iter().for_each(|s| out.extend_from_slice(&s.to_le_bytes())),
            }
            std::fs::write(args.get(3).ok_or("출력")?, out).map_err(|e| e.to_string())?;
            println!(
                "{:?} ch {} rate {} frames {} skip_front {} int16 {} table {} B data {} B hash {}",
                p.codec,
                p.channels,
                p.rate,
                p.frames,
                p.skip_front,
                p.int16,
                p.codec_table.len(),
                p.data.len(),
                hex(&audio::sound_hash(p.rate, p.channels, p.frames, &pcm)[..8])
            );
            Ok(())
        }
        _ => Err("사용: trackc compile|verify|dump|unpack|decode … (파일 머리 주석 참고)".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::FAILURE
        }
    }
}
