"""컴파일러·묶음 읽기 반례 시험: 잘못된 입력은 모두 거부되고(종료 코드 ≠ 0) 패닉하지 않아야 한다.

    python -X utf8 cp3/example/negative_cases.py
작성 형식 반례는 예시 Track 사본을 바꿔 `trackc compile`, 묶음 반례는 정상 묶음을 바꿔 `trackc verify`·`unpack`.
"""
import pathlib, shutil, struct, subprocess, sys, tempfile

import blake3  # noqa: F401  (없으면 pip install blake3)

HERE = pathlib.Path(__file__).resolve().parent
T = HERE.parent / "target" / "release" / "trackc.exe"
SRC = HERE / "synth_track"

CASES = [
    ("알 수 없는 차분 키", "charts/8b-a.toml", 'level = "7"', 'level = "7"\nfoo = "1"'),
    ("lane_sound 없는 소리", "charts/7k-b.toml", 'lane = "K6"\nsound = "hat_open"', 'lane = "K6"\nsound = "nope"'),
    ("lane_sound 없는 레인", "charts/7k-b.toml", 'lane = "K6"\nsound = "hat_open"', 'lane = "X9"\nsound = "hat_open"'),
    ("음수 이벤트 이득", "charts/7k-c.toml", 'gain = "0.8"', 'gain = "-1"'),
    ("f32 범위 밖 이득", "charts/7k-c.toml", 'gain = "0.8"', 'gain = "1' + "0" * 60 + '"'),
    ("배속 변형에 layout", "charts/7k-b-x1.25.toml", 'time_scale = "5/4"', 'time_scale = "5/4"\nlayout = "7+1"'),
    ("음수 배속", "charts/7k-b-x1.25.toml", 'time_scale = "5/4"', 'time_scale = "-5/4"'),
    ("ns 반올림 뒤 겹침", "charts/7k-c.toml", 'at = "500"\nlane = "K1"', 'at = "3500.0000001"\nlane = "K7"'),
    ("중복 소리 ID", "track.toml", 'id = "snare"', 'id = "kick"'),
    ("없는 변형 선택", "charts/7k-b.toml", 'when = "r1=a"', 'when = "r1=z"'),
    ("지뢰가 아닌데 damage", "charts/8b-a.toml", 'kind = "tap"\nsounds = ["snare"]', 'kind = "tap"\ndamage = "3"\nsounds = ["snare"]'),
    ("damage 범위 밖", "charts/7k-b.toml", 'damage = "4"', 'damage = "101"'),
    ("tempo가 0에서 시작 안 함", "charts/8b-a.toml", '{ at = "0", bpm = "120" }, { at = "1536"', '{ at = "1", bpm = "120" }, { at = "1536"'),
    ("0 BPM (+ 정지로 0 나눗셈 유도)", "charts/7k-b.toml", '{ at = "9", bpm = "180" }', '{ at = "9", bpm = "0" }'),
    ("롱노트 끝 ≤ 시작", "charts/8b-a.toml", 'end = "1536"\nsounds = ["hat_open"]', 'end = "1152"\nsounds = ["hat_open"]'),
    ("빈 스크롤 점", "charts/7k-c.toml", 'points = [\n  { at = "0", pos = "0", speed = "120" },\n  { at = "4000", pos = "8", speed = "180" },   # 179.94 → 180 (IIDX 정수 반올림)\n]', 'points = []'),
    ("UUID 버전이 7이 아님", "track.toml", 'id = "0192f3a0-0000-7000-8000-00000000e001"', 'id = "0192f3a0-0000-4000-8000-00000000e001"'),
    ("UUID에 비ASCII", "track.toml", 'id = "0192f3a0-0000-7000-8000-00000000e001"', 'id = "aé00000000000000000000000000000----"'),
    ("엔진 디코더 셋 밖 (MP3를 .ogg로)", None, None, None),
]


def run(args):
    r = subprocess.run([str(T), *map(str, args)], capture_output=True, text=True, encoding="utf-8", errors="replace")
    panic = "panicked" in r.stderr
    return r.returncode, (r.stderr.strip().splitlines() or [""])[-1], panic


def rehash(b):
    """구역 해시·묶음 해시를 다시 계산 (공격자가 할 수 있는 일)."""
    b = bytearray(b)
    n = struct.unpack_from("<I", b, 8)[0]
    for k in range(min(n, (len(b) - 128) // 56)):
        o = 128 + 56 * k
        off, ln = struct.unpack_from("<QQ", b, o + 8)
        if off + ln <= len(b):
            b[o + 24:o + 56] = blake3.blake3(bytes(b[off:off + ln])).digest()
    b[72:104] = bytes(32)
    b[72:104] = blake3.blake3(bytes(b)).digest()
    return bytes(b)


def main():
    bad = 0
    with tempfile.TemporaryDirectory() as td:
        td = pathlib.Path(td)
        for name, f, a, b in CASES:
            d = td / "t"
            shutil.rmtree(d, ignore_errors=True)
            shutil.copytree(SRC, d)
            if f:
                p = d / f
                s = p.read_text(encoding="utf-8")
                assert a in s, (name, a)
                p.write_text(s.replace(a, b, 1), encoding="utf-8")
            else:
                subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=0.2", "-c:a", "libmp3lame", "-f", "mp3", str(d / "audio/kick.ogg")], check=True)
            code, err, panic = run(["compile", d, td / "x.trk", "--force"])
            ok = code != 0 and not panic
            bad += not ok
            print(f"{'거부' if ok else '문제!'}  {name}: {err[:100]}")
        # 묶음 반례
        good = td / "good.trk"
        assert run(["compile", SRC, good, "--force"])[0] == 0
        g = good.read_bytes()
        n = struct.unpack_from("<I", g, 8)[0]
        idx = {struct.unpack_from("<I", g, 128 + 56 * k)[0]: struct.unpack_from("<QQ", g, 128 + 56 * k + 8) for k in range(n)}
        cases = []
        cases.append(("묶음 해시 틀림", g[:72] + bytes(32) + g[104:], "verify"))
        cases.append(("잘린 묶음", g[:200], "verify"))
        b = bytearray(g); b[8:12] = struct.pack("<I", 10**8); cases.append(("구역 수 과대", rehash(b), "verify"))
        off, ln = idx[3]  # CHARTS: 첫 차분의 local_to_track 개수를 u32::MAX로
        b = bytearray(g)
        p = off + 4
        for _ in range(3):
            p += 4 + struct.unpack_from("<I", b, p)[0]
        p += 32 + 4 + 4 + 8 + 8 + 8
        b[p:p + 4] = struct.pack("<I", 0xFFFFFFFF); cases.append(("차분 소리 표 개수 과대 (16 GiB 할당 유도)", rehash(b), "verify"))
        off, ln = idx[4]  # 첫 CHART 구역: 표 0 stride를 0으로
        b = bytearray(g); b[off + 64 + 8:off + 64 + 12] = struct.pack("<I", 0); cases.append(("CHART stride 0", rehash(b), "verify"))
        b = bytearray(g); b[off + 64 + 3 * 12 + 4:off + 64 + 3 * 12 + 8] = struct.pack("<I", 10**9); cases.append(("CHART 표 개수 과대", rehash(b), "verify"))
        # SOURCE의 track.toml 경로를 ../escape.toml 로 (같은 길이로 바꿔 넣음)
        off, ln = idx[11]
        b = bytearray(g); sec = bytes(b[off:off + ln]); i = sec.index(b"track.toml")
        b[off + i:off + i + len(b"track.toml")] = b"../x.tomlx"
        cases.append(("unpack 경로 탈출 (../)", rehash(b), "unpack"))
        for name, data, cmd in cases:
            p = td / "bad.trk"
            p.write_bytes(data)
            args = [cmd, p] + ([td / "out"] if cmd == "unpack" else [])
            code, err, panic = run(args)
            ok = code != 0 and not panic
            bad += not ok
            print(f"{'거부' if ok else '문제!'}  {name}: {err[:100]}")
        escaped = (td / "x.tomlx").exists()
        if escaped:
            bad += 1
            print("문제!  unpack이 출력 폴더 밖에 파일을 씀")
    print("놓친 반례", bad)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
