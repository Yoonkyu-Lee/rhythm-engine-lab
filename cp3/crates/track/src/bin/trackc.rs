//! Track 컴파일러 도구 (cp3/Track_형식.md).
//!
//!   trackc compile <작성 형식 폴더> <출력.trk> [--force]   컴파일 (경고는 표준 오류로, 입력·컴파일러가 같으면 캐시 적중)
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
            // 컴파일 캐시: 출력이 이미 있고 머리의 열쇠(입력 전체 + 컴파일러)가 같으면 다시 만들지 않는다
            let force = args.iter().any(|a| a == "--force");
            if !force {
                if let Ok(old) = std::fs::read(out) {
                    let key = compile::source_key(Path::new(dir))?;
                    if old.len() >= 128 && old[104..128] == key && pack::Bundle::open(old).is_ok() {
                        println!("캐시 적중: {out} (입력·컴파일러가 같음)");
                        return Ok(());
                    }
                }
            }
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
            let n_ids = {
                let mut r = pack::R::new(b.section(pack::K_SOUND_IDS, 0).ok_or("SOUND_IDS 없음")?);
                r.u32()? as usize
            };
            for (i, (h, p)) in sounds.iter().enumerate() {
                let pcm = audio::decode(p)?;
                if &audio::content_hash(p, &pcm) != h {
                    return Err(format!("소리 데이터 {i}: 디코딩 해시가 표와 다름"));
                }
            }
            let charts = pack::read_charts(&b)?;
            for c in &charts {
                let sec = b.section(pack::K_CHART, c.section as usize).ok_or("CHART 없음")?;
                let d = pack::decode_chart(sec).map_err(|e| format!("차분 {}: {e}", c.id))?; // 구조·의미 검사 먼저 (리뷰 3-1 #3)
                if pack::chart_hash(sec) != c.hash {
                    return Err(format!("차분 {}: 해시가 다름", c.id));
                }
                if c.local_to_track.len() != d.sounds.len() || c.local_to_track.iter().any(|&x| x as usize >= n_ids) {
                    return Err(format!("차분 {}: 소리 대응 표 오류", c.id));
                }
                // 읽고 다시 쓰기 비교는 이 컴파일러와 같은 부 버전에만 (새 부 버전의 꼬리 필드·새 표는 허용, 리뷰 3-1 #14)
                if b.minor != track::compile::FORMAT_MINOR {
                    continue;
                }
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
            r.need(n as usize, 20)?;
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
                if c.local_to_track.len() != s.sounds.len() || c.local_to_track.iter().any(|&x| x as usize >= names.len()) {
                    return Err(format!("차분 {}: 소리 대응 표 오류", c.id));
                }
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
            // 먼저 전부 읽고 경로를 검사한 뒤 쓴다 (리뷰 3-1 #1: ../, 절대 경로, 드라이브, 링크를 통한 탈출 차단)
            let mut files: Vec<(String, Vec<u8>)> = Vec::new();
            let mut r = pack::R::new(b.section(pack::K_SOURCE, 0).ok_or("SOURCE 없음")?);
            let nf = r.u32()?;
            r.need(nf as usize, 12)?;
            for _ in 0..nf {
                let p = r.str()?;
                let n = r.u64()? as usize;
                safe_rel(&p)?;
                files.push((p, r.bytes(n)?.to_vec()));
            }
            let tr_text = files.iter().find(|f| f.0 == "track.toml").ok_or("SOURCE에 track.toml 없음")?.1.clone();
            let tr: track::source::TrackToml =
                toml::from_str(std::str::from_utf8(&tr_text).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            for s in &tr.sound {
                safe_rel(&s.file)?;
            }
            std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
            let root = std::fs::canonicalize(out).map_err(|e| e.to_string())?;
            for (p, bytes) in &files {
                write_inside(&root, p, bytes)?;
            }
            // 오디오: 작성 원본(track.toml)의 file 경로로, SOUND_IDS → SOUNDS 데이터에서 되살림
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
                let def = tr.sound.get(k).ok_or("SOUND_IDS가 track.toml보다 많음")?;
                if def.id != id {
                    return Err("SOUND_IDS 순서가 track.toml과 다름".into());
                }
                let (bytes, _ext) = audio::to_file(&sounds.get(data).ok_or("소리 데이터 번호 범위 밖")?.1)?;
                write_inside(&root, &def.file, &bytes)?;
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

/// 상대 경로 검사: 빈 경로, 절대 경로, 드라이브·루트, `..`, `:`(드라이브·대체 스트림)를 거부.
fn safe_rel(p: &str) -> Result<(), String> {
    use std::path::Component;
    let path = Path::new(p);
    if p.is_empty() || p.contains(':') || p.contains('\\') || path.is_absolute() {
        return Err(format!("허용하지 않는 경로: {p:?}"));
    }
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("허용하지 않는 경로: {p:?}"));
    }
    Ok(())
}

/// root 안에만 쓴다: 부모 폴더를 만든 뒤 실제 경로(링크 해석)가 root 안인지 확인.
fn write_inside(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
    safe_rel(rel)?;
    let dst = root.join(rel);
    let parent = dst.parent().ok_or("부모 폴더 없음")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let real = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    if !real.starts_with(root) {
        return Err(format!("출력 폴더 밖으로 나가는 경로: {rel:?}"));
    }
    if dst.is_symlink() {
        return Err(format!("링크 파일은 덮어쓰지 않음: {rel:?}"));
    }
    std::fs::write(&dst, bytes).map_err(|e| e.to_string())
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
