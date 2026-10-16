"""CP1-4 4-3: PERFECT 누름 분석 — 정확히 친 키음이 반주와 같은 샘플에 놓였나.

python -I -X utf8 cp1/tools/analyze_perfect.py <runs/...> [...]

1. 루프백 녹음: 왼쪽(반주) 시작점과 오른쪽(키음) 시작점의 차이 = 키음 vs 반주 (ms, +면 키음이 늦음).
2. 기대값: 판정 스레드가 예약한 키음 프레임 C(도장) − 반주 프레임 T. 봇은 C = T인 순간에 SendInput을 부르므로,
   이 값은 SendInput → 입력 도장 사이의 전달 지연(플레이어가 몸으로 예상하는 입력 지연의 일부)이다.
3. 잔차 = 1 − 2. 엔진(예약·믹서·출력)이 정확하면 0 샘플.
4. 늦은 키음 요청: 믹서가 받았을 때 예약 프레임이 이미 지난 요청 (D가 모자랐거나 판정·입력이 멈춤).
결과: <run>/perfect_summary.json
"""
import csv, json, pathlib, sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")


def onsets(x, rate):
    x = np.abs(x)
    thr = 0.3 * x.max() if x.size and x.max() > 0 else 1
    out, last = [], -10 ** 12
    for h in np.flatnonzero(x > thr):
        if h - last > rate * 0.2:
            out.append(h)
        last = h
    return np.array(out, dtype=np.int64)


def pct(x):
    x = np.asarray(x, dtype=float)
    if x.size == 0:
        return {}
    return {k: round(float(v), 4) for k, v in
            dict(mean=x.mean(), p50=np.percentile(x, 50), p99=np.percentile(x, 99), min=x.min(), max=x.max(), n=x.size).items()}


def fmt(d):
    return "-" if not d else f"중앙 {d['p50']:+.3f}  p99 {d['p99']:+.3f}  최소 {d['min']:+.3f}  최대 {d['max']:+.3f} ms (n={int(d['n'])})"


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate = meta["render"]["rate"]
    cap = np.fromfile(run / "capture_2ch.f32", dtype="<f4").reshape(-1, meta["capture"]["channels"])
    lo, ro = onsets(cap[:, 0], rate), onsets(cap[:, 1], rate)
    ev = np.array([int(r["event_frame"]) for r in csv.DictReader(open(run / "game_sends.csv", newline=""))], dtype=np.int64)
    period = float(np.median(np.diff(ev)))
    # 반주 시작점마다 ±150 ms 안의 가장 가까운 키음 시작점. 노트 번호 = 첫 반주 시작점으로부터 몇 주기째인가
    # (반주는 고르게 0.5007초 간격이고, 오디오 정지로 녹음에 구멍이 나도 250 ms보다 작으면 반올림이 맞는다)
    meas_k = {}
    for l in lo:
        if ro.size == 0:
            break
        j = int(np.argmin(np.abs(ro - l)))
        if abs(ro[j] - l) < 0.15 * rate:
            meas_k[int(round((l - lo[0]) / period))] = (ro[j] - l) * 1e3 / rate
    meas = np.array(list(meas_k.values()))
    jr = list(csv.DictReader(open(run / "judge.csv", newline="")))
    cf = np.array([float(r["c_frame"]) for r in jr])
    k = np.clip(np.searchsorted(ev, cf), 1, len(ev) - 1)
    k = np.where(np.abs(cf - ev[k - 1]) <= np.abs(cf - ev[k]), k - 1, k)
    expect = (np.round(cf) - ev[k]) * 1e3 / rate  # 예약 기준 기대값 (입력 전달 지연)

    st = np.genfromtxt(run / "starts.csv", delimiter=",", names=True, dtype=np.int64)
    ks = st[st["sound"] == 2]
    late = ks[ks["block"] > ks["requested"]]
    late_ms = (late["block"] - late["requested"]) * 1e3 / rate

    # 잔차 = 측정 − 기대, 같은 노트끼리 (첫 반주 시작점 = 노트 0이라고 본다)
    exp_k = {int(kk): e for kk, e in zip(k, expect)}
    resid = np.array([meas_k[kk] - exp_k[kk] for kk in meas_k if kk in exp_k])
    rep = {
        "path": meta.get("path", "ASIO" if "asio" in run.name else "?"), "d_ms": meta.get("d_ms"), "faults": meta.get("faults"),
        "notes": int(len(ev)), "pairs": int(len(meas)), "presses": int(len(jr)),
        "ks_vs_backing_ms": pct(meas), "input_delivery_ms": pct(expect), "residual_ms": pct(resid),
        "late_requests": int(len(late)), "late_ms": pct(late_ms),
    }
    (run / "perfect_summary.json").write_text(json.dumps(rep, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"{run.name}  [{rep['path']}, D {rep['d_ms']} ms, 고장 '{rep['faults']}']  노트 {rep['notes']}, 누름 {rep['presses']}, 녹음 짝 {rep['pairs']}")
    print(f"  키음 vs 반주 (녹음)      {fmt(rep['ks_vs_backing_ms'])}")
    print(f"  입력 전달 (예약 기대값)  {fmt(rep['input_delivery_ms'])}")
    print(f"  잔차 (엔진 오차)         {fmt(rep['residual_ms'])}")
    print(f"  늦은 키음 요청 {rep['late_requests']}개  {fmt(rep['late_ms']) if len(late) else ''}")


if __name__ == "__main__":
    for a in sys.argv[1:]:
        analyze(pathlib.Path(a))
