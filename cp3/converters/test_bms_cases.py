"""bms2track 합성 시험 (Codex 3-2 리뷰 반례 + SP Insane+에 없는 문법). 원본·출력은 임시 폴더에 만든다.

    python -I -X utf8 cp3/converters/test_bms_cases.py

1) 합성 폴더를 변환(--selfcheck: 국소 풀이 = 전체 펼침) → 2) trackc compile·verify → 3) check_bms(독립 해석기·PCM) 대조 →
4) 경우마다 기대 결과를 직접 확인한다.
"""
import collections, os, pathlib, subprocess, sys, tempfile

import numpy as np
import soundfile as sf

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import bms2track as M  # noqa: E402
import check_bms as C  # noqa: E402

TRACKC = os.environ.get("TRACKC") or str(HERE.parent / "target" / "release" / "trackc.exe")
C.TRACKC = TRACKC

HEAD = "#PLAYER 1\n#TITLE synth\n#BPM 120\n#WAV01 a.wav\n#WAV02 b.wav\n#WAV03 c.wav\n"
CASES = {
    # 리뷰 #1 반례: 같은 레인에 선택 1은 LN [0,2]박, 선택 2는 LN [1,3]박 → 변형마다 따로 짝
    "lnvar.bms": HEAD + "#RANDOM 2\n#IF 1\n#00151:01000100\n#ENDIF\n#IF 2\n#00151:00010001\n#ENDIF\n#ENDRANDOM\n#00212:01\n",
    # #ELSEIF는 앞 갈래를 뺀 나머지, #ELSE는 남은 것 전부
    "elseif.bms": HEAD + "#RANDOM 3\n#IF 1\n#00111:01\n#ELSEIF 2\n#00112:02\n#ELSE\n#00113:03\n#ENDIF\n#ENDRANDOM\n",
    # #ELSE가 선택지 둘(2, 3)을 덮음 → when 두 개로 복제
    "elsemulti.bms": HEAD + "#RANDOM 3\n#IF 1\n#00111:01\n#ELSE\n#00112:02\n#ENDIF\n#ENDRANDOM\n",
    # #SETRANDOM은 고정값: 변형이 아님
    "setrandom.bms": HEAD + "#SETRANDOM 2\n#IF 1\n#00111:01\n#ENDIF\n#IF 2\n#00112:02\n#ENDIF\n#ENDRANDOM\n",
    # 중첩 #RANDOM
    "nested.bms": HEAD + "#RANDOM 2\n#IF 1\n#RANDOM 2\n#IF 1\n#00111:01\n#ENDIF\n#IF 2\n#00112:02\n#ENDIF\n#ENDRANDOM\n#ENDIF\n#IF 2\n#00113:03\n#ENDIF\n#ENDRANDOM\n",
    # 조건부 #WAV: 갈래마다 다른 파일
    "condwav.bms": "#PLAYER 1\n#BPM 120\n#RANDOM 2\n#IF 1\n#WAV01 a.wav\n#ENDIF\n#IF 2\n#WAV01 b.wav\n#ENDIF\n#ENDRANDOM\n#00111:01\n",
    # #BASE 62: aa와 AA는 다른 ID
    "base62.bms": "#PLAYER 1\n#BPM 120\n#BASE 62\n#WAVaa a.wav\n#WAVAA b.wav\n#00111:aaAA\n",
    # 리뷰 #3: 같은 파일을 가리키는 두 #WAV → 다른 논리 소리
    "samefile.bms": "#PLAYER 1\n#BPM 120\n#WAV01 a.wav\n#WAV02 a.wav\n#00111:0102\n",
    # 리뷰 #8: #LNMODE 2 → cn, 지뢰만 있는 D8 → 7+1 레이아웃
    "lnmode_mine.bms": HEAD + "#LNMODE 2\n#00151:0101\n#001D8:0Z\n",
    # 같은 채널·위치 두 줄 → 뒤 정의가 덮어씀
    "dup.bms": HEAD + "#00111:01\n#00111:02\n",
    # LNOBJ
    "lnobj.bms": HEAD + "#LNOBJ ZZ\n#WAVZZ a.wav\n#00111:01ZZ\n",
    # 롱노트 안 노트·페달 → 소리만 자동재생, 손실
    "inside_pedal.bms": HEAD + "#00151:01000100\n#00111:0002\n#00117:03\n",
    # 실수 WAV 피크 2 → FLAC24 + gain 2 (리뷰 #7)
    "float.bms": "#PLAYER 1\n#BPM 120\n#WAV01 f.wav\n#00111:01\n",
    # 시간 지도를 바꾸는 #RANDOM → 그 변형으로만 차분을 나눔 (r2는 when으로 남음)
    "timing.bms": HEAD + "#RANDOM 2\n#IF 1\n#00102:0.5\n#ENDIF\n#ENDRANDOM\n#RANDOM 2\n#IF 1\n#00211:01\n#ENDIF\n#ENDRANDOM\n#00111:02\n",
    # #RANDOM 밖 #IF → 건너뜀
    "ifoutside.bms": HEAD + "#IF 1\n#00112:02\n#ENDIF\n#00111:01\n",
}


def make(src):
    rate = 44100
    for name, f in (("a.wav", 440), ("b.wav", 550), ("c.wav", 660)):
        x = (0.3 * np.sin(2 * np.pi * f * np.arange(4410) / rate)).astype(np.float64)
        sf.write(str(src / name), np.stack([x, x], 1), rate, subtype="PCM_16")
    x = 2.0 * np.sin(2 * np.pi * 330 * np.arange(4410) / rate)
    sf.write(str(src / "f.wav"), np.stack([x, -x], 1), rate, subtype="FLOAT")
    for n, t in CASES.items():
        (src / n).write_text(t, encoding="utf-8")


def main():
    tmp = pathlib.Path(tempfile.mkdtemp())
    src, out = tmp / "synth", tmp / "out"
    src.mkdir()
    make(src)
    M.SELFCHECK = 1024
    ids = {}
    auth, nc, ns, rep, counts = M.convert_folder(src, out, ids)
    print(f"변환: 차분 {nc}, 소리 {ns}, {dict(counts)}")
    trk = str(tmp / "s.trk")
    subprocess.run([TRACKC, "compile", str(auth), trk], check=True, capture_output=True)
    subprocess.run([TRACKC, "verify", trk], check=True, capture_output=True)
    r = subprocess.run([sys.executable, "-I", "-X", "utf8", str(HERE / "check_bms.py"), str(src), str(auth), trk], capture_output=True, text=True,
                       encoding="utf-8", env=dict(os.environ, TRACKC=TRACKC))
    print(r.stdout[-3000:], r.stderr[-1000:])
    assert r.returncode == 0, "check_bms 불일치"
    d = C.load_dump(trk)
    fails = []

    def check(what, cond):
        print(("  통과 " if cond else "  실패 ") + what)
        if not cond:
            fails.append(what)

    def notes(cid, vals):
        return sorted((n[0], n[1], n[2], n[3], n[4]) for n in C.got_for(d[cid], vals)["notes"])

    s = 1_000_000_000  # 120 BPM: 1박 = 0.5초, 마디 001은 2초에서 시작
    m1 = 2 * s
    ln1, ln2 = notes("lnvar", {0: 1}), notes("lnvar", {0: 2})
    check("리뷰 #1: 선택 1 = LN [0, 2박]", ("K1", m1, m1 + s, "long", ("a.wav@01",)) in ln1)
    check("리뷰 #1: 선택 2 = LN [1, 3박]", ("K1", m1 + s // 2, m1 + 3 * s // 2, "long", ("a.wav@01",)) in ln2)
    check("#ELSEIF·#ELSE 갈래", [x[0] for v in (1, 2, 3) for x in notes("elseif", {0: v})] == ["K1", "K2", "K3"])
    check("#ELSE가 선택 2·3을 덮음", [x[0] for v in (1, 2, 3) for x in notes("elsemulti", {0: v})] == ["K1", "K2", "K2"])
    check("#SETRANDOM 고정값, 변형 없음", not d["setrandom"]["variants"] and [x[0] for x in notes("setrandom", {})] == ["K2"])
    check("중첩 #RANDOM", [[x[0] for x in notes("nested", {0: a, 1: b})] for a in (1, 2) for b in (1, 2)] == [["K1"], ["K2"], ["K3"], ["K3"]])
    check("조건부 #WAV", notes("condwav", {0: 1})[0][4] == ("a.wav@01",) and notes("condwav", {0: 2})[0][4] == ("b.wav@01",))
    check("#BASE 62: aa ≠ AA", [x[4] for x in notes("base62", {})] == [("a.wav@aa",), ("b.wav@AA",)])
    check("리뷰 #3: 같은 파일 다른 #WAV = 다른 소리", [x[4] for x in notes("samefile", {})] == [("a.wav@01",), ("a.wav@02",)])
    ln_mine = notes("lnmode_mine", {})
    check("리뷰 #8: 지뢰만 있는 D8 → K6 지뢰 (7+1)", any(x[0] == "K6" and x[3] == "mine" for x in ln_mine))
    check("리뷰 #8: #LNMODE 2 → cn", 'long_note = "cn"' in (auth / "charts" / "lnmode_mine.toml").read_text(encoding="utf-8"))
    check("같은 채널·위치 → 뒤 정의", [x[4] for x in notes("dup", {})] == [("b.wav@02",)])
    check("LNOBJ", notes("lnobj", {}) == [("K1", m1, m1 + s, "long", ("a.wav@01",))])
    ip = d["inside_pedal"]
    check("롱노트 안 노트·페달 → 손실 기록", ip["losses"].get("롱노트 안 노트 → 소리만 자동재생") == 1 and ip["losses"].get("페달 노트 → 소리만 자동재생") == 1)
    check("롱노트 안 노트·페달 → 소리는 자동재생", sorted(n for _, n, _, _ in ip["autos"]) == ["b.wav@02", "c.wav@03"])
    check("실수 WAV: gain 2로 되돌림", any(x["name"] == "f.wav@01" and abs(x["gain"] - 2.0) < 1e-3 for x in d["float"]["sounds"].values()))
    check("시간 지도 #RANDOM → r1만으로 차분 둘", {"timing~r1=1", "timing~r1=2"} <= set(d) and d["timing~r1=1"]["variants"] == ["r2"])
    check("#RANDOM 밖 #IF → 건너뜀", [x[0] for x in notes("ifoutside", {})] == ["K1"])
    print("결과:", "모두 통과" if not fails else f"실패 {len(fails)}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
