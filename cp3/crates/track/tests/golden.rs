//! 예시 Track(cp3/example/synth_track) 시험 벡터 (명세 5-4): 결정성, 정답 해시, verify·되살리기 왕복.
use std::path::PathBuf;
use track::{compile, pack};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../example/synth_track")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn deterministic_and_golden() {
    let a = pack::write_bundle(&compile::compile_dir(&example()).unwrap());
    let b = pack::write_bundle(&compile::compile_dir(&example()).unwrap());
    assert_eq!(a, b, "같은 입력 → 같은 바이트");
    let golden = std::fs::read_to_string(example().join("../golden/golden.txt")).unwrap();
    let want = golden.lines().find_map(|l| l.strip_prefix("bundle ")).unwrap().split_whitespace().next().unwrap().to_string();
    assert_eq!(hex(&a[72..104]), want, "묶음 해시가 시험 벡터와 다름 (형식·컴파일러를 바꿨다면 golden.txt를 의도적으로 갱신)");
    let bundle = pack::Bundle::open(a).unwrap();
    for c in pack::read_charts(&bundle).unwrap() {
        let line = format!("chart {} {}", c.id, hex(&c.hash));
        assert!(golden.lines().any(|l| l == line), "차분 해시 다름: {line}");
        let sec = bundle.section(pack::K_CHART, c.section as usize).unwrap();
        assert_eq!(pack::encode_chart(&pack::decode_chart(sec).unwrap()), sec);
    }
}

#[test]
fn audio_roundtrip_same_hash() {
    // 예시 소리마다: 파일 → 묶음 형태 → 파일로 되살리기 → 묶음 형태 → 같은 소리 해시·같은 PCM (명세 5-2)
    use track::audio;
    for e in std::fs::read_dir(example().join("audio")).unwrap() {
        let p = e.unwrap().path();
        if !matches!(p.extension().and_then(|x| x.to_str()), Some("ogg" | "flac" | "wav")) {
            continue;
        }
        let a = audio::pack_file(&std::fs::read(&p).unwrap()).unwrap();
        let pa = audio::decode(&a).unwrap();
        let (bytes, _) = audio::to_file(&a).unwrap();
        let b = audio::pack_file(&bytes).unwrap();
        let pb = audio::decode(&b).unwrap();
        assert_eq!(audio::content_hash(&a, &pa), audio::content_hash(&b, &pb), "{}", p.display());
        assert_eq!(pa, pb, "{}", p.display());
        assert_eq!((a.frames, a.skip_front, a.rate, a.channels), (b.frames, b.skip_front, b.rate, b.channels));
    }
}
