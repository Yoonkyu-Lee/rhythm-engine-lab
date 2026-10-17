"""엔진 정답 렌더 ↔ 독립 Python 렌더 대조 (3-3 검증; 3-2 완료 기준 "정답 렌더 일치"의 엔진 쪽).

    python -I -X utf8 cp3/tools/render_check.py <작성 폴더> <묶음.trk> <차분 id> [--rate 44100] [--choice 0,1]

Python 쪽: trackc dump의 차분 안 소리 표(start·length·gain·끊기·choke)와 발음(자동재생 + 노트 소리, 변형 선택 적용)을 읽고,
작성 형식 track.toml의 소리 파일을 libsndfile로 디코딩해 numpy로 섞는다. 엔진 쪽: cp3game render (load + mixer::render_perfect).
장치 속도를 소리 속도와 같게 두면 리샘플링이 빠져 로더(자르기·스테레오·i16/f32)·믹서(이벤트 시각 분할·끊기·choke·이득)만 남는다.
기준: 최대 차 ≤ 1e-4 (Vorbis 디코더 차 3e-7 × 겹친 소리 수 + f32 합 순서), 발음 시작 (소리, 프레임) 목록 일치(엔진 render의 .starts.csv).
보이스 예산도 모델링한다 (끊긴·끝난 것을 뺀 실제로 울리는 수가 예산이면 버림, 리뷰 3-3 검증 맹점).
"""
import argparse, collections, os, pathlib, subprocess, sys, tempfile, tomllib

import numpy as np
import soundfile as sf

ROOT = pathlib.Path(__file__).resolve().parents[1]
TRACKC = os.environ.get("TRACKC") or str(ROOT / "target" / "release" / "trackc.exe")
GAME = os.environ.get("CP3GAME") or str(ROOT / "target-dev" / "release" / "cp3game.exe")


def ns_to_frame(t, rate):
    x = t * rate
    h = 10**9 // 2
    return (x + h) // 10**9 if x >= 0 else -((-x + h) // 10**9)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("auth")
    ap.add_argument("trk")
    ap.add_argument("chart")
    ap.add_argument("--rate", type=int, default=44100)
    ap.add_argument("--choice", default="")
    a = ap.parse_args()
    auth = pathlib.Path(a.auth)
    tt = tomllib.loads((auth / "track.toml").read_text(encoding="utf-8"))
    files = {s["id"]: s["file"] for s in tt["sound"]}
    dump = subprocess.run([TRACKC, "dump", a.trk], capture_output=True, text=True, encoding="utf-8", check=True).stdout
    cur, sounds, starts, variants, conds, end = None, {}, [], [], {}, 0
    note_cond = 0
    for line in dump.splitlines():
        f = line.split(" ")
        if f[0] == "chart":
            cur = f[1]
            if cur == a.chart:
                end = int(f[5])
                budget = int(f[9]) if len(f) > 9 else 1 << 30
            continue
        if cur != a.chart:
            continue
        if f[0] == "variant":
            variants.append(int(f[3]))
        elif f[0] == "cond":
            conds[int(f[1])] = [tuple(map(int, x.split(":"))) for x in f[2:]]
        elif f[0] == "sound":
            sounds[" ".join(f[7:])] = dict(start=int(f[2]), length=int(f[3]), gain=np.float32(f[4]), cut=f[5] == "1", choke=int(f[6]), idx=int(f[1]))
        elif f[0] == "auto":
            starts.append((int(f[1]), 0, " ".join(f[2:-2]), np.float32(f[-2]), int(f[-1])))
        elif f[0] == "note":
            note_cond = int(f[5])
        elif f[0] == "nsnd":
            starts.append((int(f[2]), 1, " ".join(f[3:-1]), np.float32(f[-1]), note_cond))
    choice = [int(x) for x in a.choice.split(",") if x] or [0] * len(variants)
    on = lambda c: all(choice[v] == ch for v, ch in conds.get(c, []))
    starts = [s for s in starts if on(s[4])]
    # 정규 발음 순서: 같은 ns면 자동재생 먼저 (dump 순서 = 레인 선언·노트 순서)
    order = sorted(range(len(starts)), key=lambda i: (starts[i][0], starts[i][1], i))
    rate = a.rate
    pcm = {}
    for nm in {s[2] for s in starts}:
        x, r = sf.read(str(auth / files[nm]), dtype="float32", always_2d=True)
        if r != rate:
            sys.exit(f"소리 {nm}: {r} Hz ≠ --rate {rate} (리샘플링 없는 대조만 지원)")
        if x.shape[1] == 1:
            x = np.repeat(x, 2, axis=1)
        s = sounds[nm]
        x = x[s["start"]:] if not s["length"] else x[s["start"]:s["start"] + s["length"]]
        pcm[nm] = x
    total = ns_to_frame(end, rate) + 1 + max((len(v) for v in pcm.values()), default=0)
    out = np.zeros((total, 2), dtype=np.float64)
    live = []  # (시작 프레임, 끝 프레임, 소리)
    placed = []
    dropped = 0
    for i in order:
        t, kind, nm, g, _ = starts[i]
        f0 = ns_to_frame(t, rate)
        s = sounds[nm]
        # 끊기·choke: 지금 울리는 앞 소리를 f0에서 끊는다
        nl = []
        for (b, e, onm, idx) in live:
            if e > f0 and ((onm == nm and s["cut"]) or (s["choke"] and sounds[onm]["choke"] == s["choke"] and onm != nm)):
                e = f0
                placed[idx] = (placed[idx][0], f0, placed[idx][2], placed[idx][3])
            nl.append((b, e, onm, idx))
        live = [x for x in nl if x[1] > f0]
        if len(live) >= budget:  # 보이스 예산: 끊긴·끝난 것을 뺀 실제로 울리는 수 (엔진 admit과 같은 규칙)
            dropped += 1
            continue
        n = len(pcm[nm])
        placed.append((f0, f0 + n, nm, np.float32(g * s["gain"])))
        live.append((f0, f0 + n, nm, len(placed) - 1))
    for b, e, nm, g in placed:
        out[b:e] += pcm[nm][:e - b] * float(g)
    with tempfile.TemporaryDirectory() as td:
        o = os.path.join(td, "e.f32")
        r = subprocess.run([GAME, "render", a.trk, "--chart", a.chart, "--rate", str(rate)] + (["--choice", a.choice] if a.choice else []) + [o],
                           capture_output=True, text=True, encoding="utf-8")
        if r.returncode:
            sys.exit(r.stderr)
        eng = np.fromfile(o, dtype="<f4").reshape(-1, 2).astype(np.float64)
        es = sorted(tuple(map(int, l.split(","))) for l in open(o + ".starts.csv", encoding="utf-8").read().split())
    ps = sorted((sounds[nm]["idx"], b) for b, _, nm, _ in placed)
    starts_ok = es == ps
    n = max(len(eng), len(out))
    e2 = np.zeros((n, 2)); e2[:len(eng)] = eng
    o2 = np.zeros((n, 2)); o2[:len(out)] = out
    d = np.abs(e2 - o2)
    worst = float(d.max())
    at = int(d.max(axis=1).argmax())
    print(f"{a.chart}: 발음 {len(placed)} (예산 {budget}, 넘쳐 버림 {dropped}), 시작 프레임 {'같음' if starts_ok else '다름'}, 길이 {len(eng)} / {len(out)} 프레임, 최대 차 {worst:.2e} (프레임 {at}, {at / rate:.3f} s), "
          f"RMS 차 {float(np.sqrt((d ** 2).mean())):.2e}, 정답 최댓값 {float(np.abs(o2).max()):.3f} — {'일치' if worst <= 1e-4 and starts_ok else '다름'}")
    sys.exit(0 if worst <= 1e-4 and starts_ok else 1)


if __name__ == "__main__":
    main()
