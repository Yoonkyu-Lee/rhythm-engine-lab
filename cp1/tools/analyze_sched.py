r"""2번 단계(예약기) 기록 분석.

python -I cp1\tools\analyze_sched.py <runs\sched-...> [...]

1. 오른쪽 채널(기준 클릭)의 시작점 ↔ 예정 프레임으로, 루프백 녹음 위치를 재생 프레임 번호로 바꾸는 오프셋을 구한다.
2. 왼쪽 채널(시험 이벤트)의 시작점을 재생 프레임 번호로 바꿔, 이벤트 예정 프레임과의 차이를 잰다.
   양수 = 늦게 울림.
"""
import csv, json, sys, pathlib
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


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate = meta["render"]["rate"]
    cap = np.fromfile(run / "capture_2ch.f32", dtype="<f4").reshape(-1, meta["capture"]["channels"])
    with open(run / "capture_packets.csv", newline="") as f:
        cp = [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]
    idx = np.cumsum([0] + [r["frames"] for r in cp[:-1]])
    dpos = np.array([r["device_pos"] for r in cp], dtype=np.int64)
    fr = np.array([r["frames"] for r in cp], dtype=np.int64)
    gaps = int(np.sum(dpos[1:] != dpos[:-1] + fr[:-1]))

    def to_dpos(i):
        k = np.searchsorted(idx, i, side="right") - 1
        return dpos[k] + (i - idx[k])

    pilots = np.loadtxt(run / "pilots.csv", skiprows=1, dtype=np.int64, ndmin=1)
    p_on = to_dpos(onsets(cap[:, 1], rate))
    n = min(len(p_on), len(pilots))
    offs = p_on[:n] - pilots[:n]
    offset = int(np.median(offs))

    with open(run / "game_sends.csv", newline="") as f:
        sends = list(csv.DictReader(f))
    ev = np.array([int(r["event_frame"]) for r in sends], dtype=np.int64)
    e_on = to_dpos(onsets(cap[:, 0], rate)) - offset
    # 시작점마다 가장 가까운 이전 이벤트와 짝짓기 (늦게 울리는 쪽만 가능하다고 보지 않고 최근접)
    j = np.clip(np.searchsorted(ev, e_on), 1, len(ev) - 1)
    near = np.where(np.abs(e_on - ev[j - 1]) <= np.abs(e_on - ev[j]), j - 1, j)
    err_ms = (e_on - ev[near]) / rate * 1e3

    with open(run / "starts.csv", newline="") as f:
        st = [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]
    internal = []
    for r, k in zip(st, range(len(ev))):
        internal.append((r["start"] - ev[k]) / rate * 1e3)
    internal = np.array(internal)

    def s(v):
        return f"평균 {v.mean():7.2f}  σ {v.std():6.3f}  p50 {np.median(v):7.2f}  p99 {np.percentile(v, 99):7.2f}  최소 {v.min():7.2f}  최대 {v.max():7.2f} ms"

    label = meta["mode"] + (f" {meta['fps']:.0f}fps" if meta["mode"] == "immediate" else f" 미리 {meta['lookahead_ms']:.0f}ms")
    print(f"{run.name}  [{label}]")
    print(f"  기준 클릭 {n}개, 오프셋 {offset} (범위 {offs.min()}…{offs.max()}), 캡처 끊김 {gaps}")
    print(f"  시험 이벤트 {len(ev)}개 보냄, 루프백 시작점 {len(e_on)}개, 짝 겹침 {len(near) - len(set(near.tolist()))}")
    print(f"  실제 오차 (루프백)   {s(err_ms)}")
    print(f"  예약기 기록 오차     {s(internal)}")
    return {"run": run.name, "label": label, "err_ms": err_ms.tolist()}


if __name__ == "__main__":
    for a in sys.argv[1:]:
        analyze(pathlib.Path(a))
