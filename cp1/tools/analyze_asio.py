r"""CP1-2: ASIO 기록 전용 분석 (analyze_sched / analyze_faults로는 안 보이는 것).

python -I -X utf8 cp1\tools\analyze_asio.py <runs\asio-...> [...]

1. 콜백 간격과 재생 위치(getSamplePosition) 간격: 고장 A 때 장치가 주기를 건너뛰었는지.
2. 드라이버 시각(systime_ns)과 QPC: 같은 시계인지, 차이가 흔들리는지.
3. 재생 위치 ↔ 시각의 직선 잔차: WASAPI에서 본 1 ms 톱니가 ASIO에도 있는지 (드라이버 시각 기준, 콜백 진입 QPC 기준 둘 다).
"""
import csv, json, sys, pathlib
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate = meta["render"]["rate"]
    buf = meta["render"]["buffer_frames"]
    with open(run / "asio_callbacks.csv", newline="") as f:
        rows = [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]
    spos = np.array([r["spos"] for r in rows], dtype=np.int64)
    sysns = np.array([r["systime_ns"] for r in rows], dtype=np.float64)
    qin = np.array([r["qpc_in"] for r in rows], dtype=np.float64) * 1e9 / meta["qpc_freq"]
    qout = np.array([r["qpc_out"] for r in rows], dtype=np.float64) * 1e9 / meta["qpc_freq"]
    print(f"\n{run.name}: 콜백 {len(rows)}개, 버퍼 {buf} ({buf / rate * 1e3:.2f} ms), 보고 지연 {meta.get('asio_latency')}, 세기 방식 {meta.get('index')}")
    d = np.diff(spos)
    vals, cnt = np.unique(d, return_counts=True)
    print("  재생 위치 간격 (샘플): " + ", ".join(f"{v}×{c}" for v, c in zip(vals, cnt)))
    for i in np.flatnonzero(d != buf):
        print(f"    {qin[i + 1] / 1e9 - qin[0] / 1e9:7.3f}초: 위치 {spos[i]} → {spos[i + 1]} (+{d[i]}, 버퍼 {d[i] / buf:.1f}개분)")
    iv = np.diff(qin) / 1e6
    print(f"  콜백 간격 ms: 평균 {iv.mean():.3f}, p1 {np.percentile(iv, 1):.3f}, p99 {np.percentile(iv, 99):.3f}, 최대 {iv.max():.3f}")
    print(f"  콜백 처리 시간 ms: p99 {np.percentile((qout - qin) / 1e6, 99):.3f}, 최대 {((qout - qin) / 1e6).max():.3f}")
    off = sysns - qin
    print(f"  드라이버 시각 − 콜백 진입 QPC: 중앙값 {np.median(off) / 1e6:+.3f} ms, 범위 {np.percentile(off, 1) / 1e6:+.3f} … {np.percentile(off, 99) / 1e6:+.3f} ms")
    steps = np.unique(np.round(np.diff(sysns) / 1e6, 3))
    print(f"  드라이버 시각 간격 고유값 (ms, 앞 6개): {steps[:6].tolist()}")
    ok = np.ones(len(spos), bool)
    ok[: int(0.5 * rate / buf)] = False  # 시작 0.5초 제외
    out = {}
    for name, t in (("드라이버 시각", sysns), ("콜백 진입 QPC", qin)):
        if ok.sum() < 10:
            break
        b, a = np.polyfit(spos[ok].astype(np.float64), t[ok], 1)
        res = (t[ok] - (a + b * spos[ok])) / 1e6
        ppm = ((1e9 / rate) / b - 1) * 1e6
        nb = np.diff(res)
        big = np.flatnonzero(np.abs(nb) > 0.5)
        print(f"  [{name}] 장치 시계 vs 이 시계 {ppm:+.2f} ppm, 직선 잔차 σ {res.std():.3f} ms, 이웃 차분 σ {nb.std():.3f} ms, 0.5 ms 넘는 이웃 점프 {len(big)}회")
        out[name] = {"ppm": ppm, "resid_sd_ms": res.std()}
    return out


if __name__ == "__main__":
    for a in sys.argv[1:]:
        analyze(pathlib.Path(a))
