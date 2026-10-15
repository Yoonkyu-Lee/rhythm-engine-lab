# 고장 뒤 자동재생 소리가 실제 시간으로 얼마나 밀렸나 (짧은 소리 vs 긴 패드). 쓰는 법: python -I -X utf8 cp2/tools/split_auto.py <runs/...>
# 고장 뒤 자동재생: 긴 소리(이미 울리던 보이스)와 짧은 소리가 "실제 시간"으로 어디에 놓였나
# 스트림 프레임 → 실제 QPC(스트림 기준 대응)로 바꿔, 렉 없을 때의 위치와 비교 (ms, +면 늦게 들림)
import json, pathlib, sys
import numpy as np
run = pathlib.Path(sys.argv[1])
meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
qf, rate, origin = meta["qpc_freq"], meta["rate"], meta["origin"]
ab = np.genfromtxt(run / "audio_blocks.csv", delimiter=",", names=True, dtype=np.int64)
ab = ab[ab["written"] > 0]
pos = (ab["written"] - ab["padding"]).astype(float); q = ab["qpc_wake"].astype(float)
def f2q(f): return np.interp(f, pos, q)
exp = json.loads((pathlib.Path(meta["chart"]).parent / "expected.json").read_text(encoding="utf-8"))["events"]
auto = [(t, s) for t, s, g, src in exp if src == "auto"]
mm = np.genfromtxt(run / "mix_events.csv", delimiter=",", names=True, dtype=None, encoding="utf-8")
st = mm[(mm["kind"] == "start") & (mm["player"] == 0)]
n = min(len(st), len(auto))
fault = [ln.split(",") for ln in (run / "faults.csv").read_text().splitlines()[1:]]
fs = float(fault[0][1]) if fault else 10.0
rows = []
for k in range(n):
    t, s = auto[k]
    real_q = f2q(float(st["start"][k]))
    rows.append((t / 1e9, int(st["sound"][k]), real_q))
rows = np.array(rows)
# 렉 없을 때의 관계: 곡 시각 → 실제 시각 직선을 고장 전 소리들로 맞춘다
pre = rows[:, 0] < fs - 0.5
A = np.vstack([rows[pre, 0], np.ones(pre.sum())]).T
a, b = np.linalg.lstsq(A, rows[pre, 2], rcond=None)[0]
dev = (rows[:, 2] - (a * rows[:, 0] + b)) * 1e3 / qf
print(run.name, f"고장 {fault[0][0]} {fault[0][2]} ms @ {fs}s" if fault else "")
for lo, hi in [(fs + 0.5, fs + 3), (fs + 5, fs + 12)]:
    w = (rows[:, 0] >= lo) & (rows[:, 0] < hi)
    for name, m in [("짧은 소리", rows[:, 1] != 4), ("긴 패드 시작", rows[:, 1] == 4)]:
        x = dev[w & m]
        if x.size: print(f"  곡 {lo:.1f}~{hi:.1f}s {name}: 실제 시간 어긋남 중앙 {np.median(x):+.1f} ms (n={x.size})")
# 고장 때 이미 울리던 패드: 그 보이스의 시작 위치 기준 (스트림은 멈췄다 이어지므로 남은 부분은 정지 길이만큼 밀림)
