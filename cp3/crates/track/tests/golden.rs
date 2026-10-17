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
