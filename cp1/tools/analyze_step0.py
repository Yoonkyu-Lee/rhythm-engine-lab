r"""step0 기록 분석: 루프백 왕복 지연, 반복성, 드리프트.

python -I cp1\tools\analyze_step0.py <runs\step0-...>

시각 단위: WASAPI가 주는 qpc100ns (100 ns). 우리 쪽 QPC 틱은 qpc_freq로 100 ns로 바꾼다.
- 재생 시각 t_play(F): 렌더 IAudioClock 보고값 (장치 위치 ↔ 100 ns)을 직선으로 맞춰서, 클릭 프레임 F가 "재생된" 시각을 구한다.
- 녹음 시각 t_rec: 캡처 패킷의 (장치 위치, 녹음 시각)으로 루프백에서 클릭이 시작된 샘플의 시각을 구한다.
- 왕복 = t_rec − t_play. 같은 장치라면 두 보고값 사이의 차이(장치 안의 지연)만 남는다.
- 프레임 오프셋 = 루프백 장치 위치 − 렌더 프레임 번호. 두 스트림이 같은 시계면 일정해야 한다.
"""
import csv, json, sys, pathlib
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
run = pathlib.Path(sys.argv[1])
meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
QF = meta["qpc_freq"]
R = meta["render"]; C = meta["capture"]
rate_r, rate_c = R["rate"], C["rate"]


def rows(name):
    with open(run / name, newline="") as f:
        return [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]


clicks = np.array([r["frame"] for r in rows("clicks.csv")], dtype=np.int64)
rp = rows("render_packets.csv")
cp = rows("capture_packets.csv")
cap = np.fromfile(run / "capture_ch0.f32", dtype="<f4")

# ── 렌더 시계 모델: 재생 프레임 ↔ 100 ns
pos = np.array([r["clock_pos"] for r in rp], dtype=np.float64) * rate_r / R["clock_freq"]
qp = np.array([r["clock_qpc100ns"] for r in rp], dtype=np.float64)
ok = pos > rate_r * 0.05
b, a = np.polyfit(pos[ok], qp[ok], 1)          # qpc = a + b*frame
resid_ms = (qp[ok] - (a + b * pos[ok])) / 1e4
ppm_render = ((1e7 / rate_r) / b - 1) * 1e6


def t_play(frame):
    return a + b * frame

# ── 캡처 패킷: 파일 안 위치 ↔ 장치 위치 ↔ 녹음 시각
idx = np.cumsum([0] + [r["frames"] for r in cp[:-1]])
dpos = np.array([r["device_pos"] for r in cp], dtype=np.int64)
cq = np.array([r["qpc100ns"] for r in cp], dtype=np.float64)
frames = np.array([r["frames"] for r in cp], dtype=np.int64)
gaps = int(np.sum(dpos[1:] != dpos[:-1] + frames[:-1]))
flagged = int(sum(1 for r in cp if r["flags"] & ~0x2))   # 0x2 = SILENT 외 플래그 (불연속 등)
silent = int(sum(1 for r in cp if r["flags"] & 0x2))
cb, ca = np.polyfit(dpos.astype(np.float64), cq, 1)
ppm_capture = ((1e7 / rate_c) / cb - 1) * 1e6

# ── 클릭 시작점 검출
x = np.abs(cap)
thr = 0.3 * x.max() if x.size else 0
hits = np.flatnonzero(x > thr)
onsets = []
last = -10 ** 12
for h in hits:
    if h - last > rate_c * 0.25:
        onsets.append(h)
    last = h
onsets = np.array(onsets, dtype=np.int64)

n = min(len(onsets), len(clicks))
pk = np.searchsorted(idx, onsets[:n], side="right") - 1
o_dpos = dpos[pk] + (onsets[:n] - idx[pk])
t_rec = cq[pk] + (onsets[:n] - idx[pk]) * 1e7 / rate_c
rt_ms = (t_rec - t_play(clicks[:n])) / 1e4
off = (o_dpos - clicks[:n]).astype(np.float64)
dur_s = (clicks[n - 1] - clicks[0]) / rate_r if n > 1 else 0
drift_ppm = (np.polyfit(clicks[:n].astype(np.float64), off, 1)[0] * 1e6) if n > 2 else float("nan")

# ── 출력 버퍼 깊이: 쓴 프레임 − 재생된 프레임
now100 = np.array([r["qpc"] for r in rp], dtype=np.float64) * 1e7 / QF
written = np.array([r["written_before"] + r["frames"] for r in rp], dtype=np.float64)
played = (now100 - a) / b
depth_ms = (written - played) / rate_r * 1e3

# 웨이크업 간격 (렌더 이벤트)
wake_ms = np.diff(now100) / 1e4


def st(v):
    v = np.asarray(v, dtype=np.float64)
    return {"n": int(v.size), "mean": float(v.mean()), "std": float(v.std()), "min": float(v.min()), "p50": float(np.median(v)),
            "p99": float(np.percentile(v, 99)), "max": float(v.max())}


summary = {
    "run": run.name, "secs": meta["secs"], "mmcss": [R["mmcss"], C["mmcss"]],
    "clicks_scheduled": int(len(clicks)), "onsets_found": int(len(onsets)),
    "roundtrip_ms": st(rt_ms) if n else None,
    "frame_offset": {"unique": sorted(set(off.astype(int).tolist()))[:10], "min": float(off.min()), "max": float(off.max())} if n else None,
    "drift_ppm_capture_vs_render_frames": drift_ppm,
    "render_clock_ppm_vs_qpc": ppm_render, "capture_clock_ppm_vs_qpc": ppm_capture,
    "render_clock_fit_resid_ms": st(resid_ms),
    "output_buffer_depth_ms": st(depth_ms[depth_ms.size // 10:]),
    "render_wake_interval_ms": st(wake_ms),
    "capture_gaps": gaps, "capture_flagged_packets": flagged, "capture_silent_packets": silent,
}
(run / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")


def line(label, s, unit="ms"):
    print(f"  {label:<22} 평균 {s['mean']:8.3f}  표준편차 {s['std']:7.3f}  p50 {s['p50']:8.3f}  p99 {s['p99']:8.3f}  최소 {s['min']:8.3f}  최대 {s['max']:8.3f} {unit}")


print(f"{run.name}: {meta['secs']} s, MMCSS 렌더 {R['mmcss']} / 녹음 {C['mmcss']}")
print(f"  클릭 예약 {len(clicks)}개, 루프백에서 찾은 시작점 {len(onsets)}개")
if n:
    line("왕복 (보고 시각 기준)", summary["roundtrip_ms"])
    print(f"  프레임 오프셋           {summary['frame_offset']['min']:.0f} … {summary['frame_offset']['max']:.0f} 프레임 (값 종류 {summary['frame_offset']['unique']})")
    print(f"  드리프트 (루프백−렌더)  {drift_ppm:.3f} ppm  ({dur_s:.0f} s 동안)")
print(f"  장치 시계 vs QPC        렌더 {ppm_render:+.2f} ppm, 녹음 {ppm_capture:+.2f} ppm")
line("렌더 시계 맞춤 잔차", summary["render_clock_fit_resid_ms"])
line("출력 버퍼 깊이", summary["output_buffer_depth_ms"])
line("렌더 깨어남 간격", summary["render_wake_interval_ms"])
print(f"  캡처 위치 끊김 {gaps}회, 플래그 패킷 {flagged}개, 무음 패킷 {silent}개")
