r"""3-1 단계(고장 A·B) 기록 분석.

python -I cp1\tools\analyze_faults.py <runs\sched-...> [...]

- 기준 클릭(오른쪽)의 오프셋 변화 = 언더런 등으로 스트림 전체가 실제 시간에서 밀린 양.
- 시험 이벤트(왼쪽, 50 ms 노이즈 버스트)마다: 예약기 기록 위치 ±30 ms에서 상호상관으로 실제 위치를 찾고,
  지연(실제 − 예정, ms)과 남은 비율(들린 부분 / 전체)을 잰다. 오프셋은 그 시점 직전 기준 클릭 값을 쓴다.
"""
import csv, json, sys, pathlib
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")


def rows(path):
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def onsets(x, rate, thr_ratio=0.3, gap_s=0.2):
    a = np.abs(x)
    thr = thr_ratio * a.max()
    out, last = [], -10 ** 12
    for h in np.flatnonzero(a > thr):
        if h - last > rate * gap_s:
            out.append(h)
        last = h
    return np.array(out, dtype=np.int64)


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate = meta["render"]["rate"]
    ch = meta["capture"]["channels"]
    cap = np.fromfile(run / "capture_2ch.f32", dtype="<f4").reshape(-1, ch)
    tmpl = np.fromfile(run / "sound0.f32", dtype="<f4").astype(np.float64)
    cp = [{k: int(v) for k, v in r.items()} for r in rows(run / "capture_packets.csv")]
    idx = np.cumsum([0] + [r["frames"] for r in cp[:-1]])
    dpos = np.array([r["device_pos"] for r in cp], dtype=np.int64)
    fr = np.array([r["frames"] for r in cp], dtype=np.int64)
    if np.any(dpos[1:] != dpos[:-1] + fr[:-1]):
        print("  주의: 캡처 위치 끊김 있음")
    cap_start = dpos[0]  # 캡처 파일 0번 샘플의 장치 위치 (끊김 없다고 가정)

    # 기준 클릭 오프셋: 녹음 파일 위치 − 재생 프레임
    pil = np.loadtxt(run / "pilots.csv", skiprows=1, dtype=np.int64, ndmin=1)
    pon = onsets(cap[:, 1], rate)
    n = min(len(pon), len(pil))
    # 기준 클릭은 언더런으로 사라지거나 밀릴 수 있으니 가장 가까운 예정 프레임과 짝짓는다.
    first_off = pon[0] - pil[0]
    pairs = []
    for o in pon:
        j = int(np.argmin(np.abs(pil + first_off - o)))
        pairs.append((pil[j], o - pil[j]))
    pairs.sort()
    pf = np.array([p[0] for p in pairs]); po = np.array([p[1] for p in pairs])
    # 혼자 튀는 짝(검출 오류)은 이웃 3개 중앙값으로 덮는다. 실제 밀림은 이후 계속 유지되므로 남는다.
    if len(po) >= 3:
        po = np.array([po[0]] + [int(np.median(po[i - 1:i + 2])) for i in range(1, len(po) - 1)] + [po[-1]])
    changes = [(pf[0], po[0])] + [(pf[i], po[i]) for i in range(1, len(po)) if po[i] != po[i - 1]]
    gain = np.median(np.abs(cap[pon, 1])) / 0.25 if len(pon) else 1.0

    def off_at(frame):
        i = np.searchsorted(pf, frame, side="right") - 1
        return po[max(i, 0)]

    sends = rows(run / "game_sends.csv")
    ev = np.array([int(r["event_frame"]) for r in sends], dtype=np.int64)
    st = [{k: int(v) for k, v in r.items()} for r in rows(run / "starts.csv")]
    E = float(np.dot(tmpl, tmpl))
    half = int(0.03 * rate)
    res = []
    for k, s in enumerate(st[:len(ev)]):
        guess = s["start"] + off_at(s["start"])
        lo = max(guess - half, 0)
        seg = cap[lo: guess + half + len(tmpl), 0].astype(np.float64)
        if len(seg) < len(tmpl):
            break
        c = np.correlate(seg, tmpl, mode="valid")
        b = int(np.argmax(c))
        pos = lo + b
        lag_ms = (pos - off_at(s["start"]) - ev[k]) / rate * 1e3
        frac = c[b] / (gain * E)
        res.append((k, ev[k] / rate, lag_ms, frac, s["block"] - ev[k]))
    res = np.array(res)

    faults = []
    for name in ("faults_render.csv", "faults_game.csv"):
        if (run / name).exists():
            faults += [(r["kind"], float(r["at_s"]), int(r["ms"])) for r in rows(run / name)]
    faults.sort(key=lambda f: f[1])

    label = meta["mode"] + (f" {meta['fps']:.0f}fps" if meta["mode"] == "immediate" else f" 미리 {meta['lookahead_ms']:.0f}ms, 늦으면 {'앞부분 자르기' if meta['late'] == 'cut' else '통째로 밀기'}")
    print(f"\n{run.name}  [{label}]  고장 {meta.get('faults', '')}")
    print("  기준 클릭 오프셋 변화 (스트림이 실제 시간에서 밀린 양):")
    for f0, o in changes:
        print(f"    {f0 / rate:7.2f}초부터 {o:6d} ({(o - changes[0][1]) / rate * 1e3:+7.1f} ms)")

    def near(t):
        for kind, at, ms in faults:
            if at - 0.3 <= t <= at + ms / 1e3 + 1.2:
                return f"{kind}@{at:g}:{ms}"
        return None

    normal = np.array([r for r in res if near(r[1]) is None])
    if len(normal):
        base = np.median(normal[:, 3])  # 평상시 값을 100%로 (녹음 이득 차이 보정)
        res[:, 3] /= base
        normal[:, 3] /= base
        print(f"  평상시 {len(normal)}개: 지연 평균 {normal[:, 2].mean():6.2f} ms, σ {normal[:, 2].std():.2f}, 남은 비율 {normal[:, 3].mean() * 100:5.1f}%")
    print("  고장 근처 이벤트:")
    for r in res:
        tag = near(r[1])
        if tag:
            print(f"    [{tag:>9}] {r[1]:7.2f}초  지연 {r[2]:+8.2f} ms  남은 비율 {max(r[3], 0) * 100:5.1f}%")
    after = np.array([r for r in res if faults and r[1] > faults[-1][1] + faults[-1][2] / 1e3 + 1.2])
    if len(after):
        print(f"  마지막 고장 뒤 {len(after)}개: 지연 평균 {after[:, 2].mean():6.2f} ms, σ {after[:, 2].std():.2f}")


if __name__ == "__main__":
    for a in sys.argv[1:]:
        analyze(pathlib.Path(a))
