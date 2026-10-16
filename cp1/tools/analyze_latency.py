"""CP1-4 4-4: 누름 → 내 키음 출력 (경로별 총 지연과 흔들림).

python -I -X utf8 cp1/tools/analyze_latency.py <runs/...perfect...> [...]

가정 (사용자 결정, 2026-10-15): 오디오 인터페이스 내부 경로(재생 → 루프백)는 무시할 만하다.
- WASAPI: 루프백 녹음 패킷의 QPC 시각표(그 패킷 첫 프레임이 녹음된 순간)로 키음 시작점의 시각을 구하고, 그것을 출력 시각으로 본다.
- ASIO: 녹음은 콜백 단위라 시각표가 없다. 키음을 놓은 프레임 F가 장치 시계 직선상 지나는 시각 + 드라이버가 보고한 출력 지연(버퍼를 만든 뒤 값)을 출력 시각으로 본다.
누름 시각 = 봇이 SendInput을 부른 QPC.
구성: 입력 전달(SendInput → 입력 시각) + D + 나머지(장치 시계 기준점 → 출력).
"""
import csv, json, pathlib, sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from analyze_perfect import onsets  # noqa: E402


def pct(x):
    x = np.asarray(x, dtype=float)
    return dict(p50=float(np.median(x)), p1=float(np.percentile(x, 1)), p99=float(np.percentile(x, 99)), min=float(x.min()), max=float(x.max()), n=int(x.size))


def fmt(d):
    return f"중앙 {d['p50']:.2f}  p1 {d['p1']:.2f}  p99 {d['p99']:.2f}  (범위 {d['min']:.2f}~{d['max']:.2f}, n={d['n']}) ms"


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate = meta["render"]["rate"]
    qf = meta["qpc_freq"]
    d_ms = meta.get("d_ms")
    asio = meta.get("path") == "ASIO"
    cap = np.fromfile(run / "capture_2ch.f32", dtype="<f4").reshape(-1, meta["capture"]["channels"])
    ro = onsets(cap[:, 1], rate)
    bot = list(csv.DictReader(open(run / "bot.csv", newline="")))
    sent = np.array([int(b["q_sent"]) for b in bot], dtype=np.int64)
    jr = list(csv.DictReader(open(run / "judge.csv", newline="")))
    stamp = np.array([int(r["q_stamp"]) for r in jr], dtype=np.int64)
    cfr = np.array([float(r["c_frame"]) for r in jr])

    if not asio:
        cp = list(csv.DictReader(open(run / "capture_packets.csv", newline="")))
        idx = np.cumsum([0] + [int(r["frames"]) for r in cp[:-1]])
        t100 = np.array([int(r["qpc100ns"]) for r in cp], dtype=np.int64)
        k = np.searchsorted(idx, ro, side="right") - 1
        t_out = (t100[k] + (ro - idx[k]) / rate * 1e7) * qf / 1e7  # QPC
        how = "루프백 녹음 시각표"
    else:
        # 장치 시계 직선: 콜백 (위치, 진입 QPC)로 맞춘다. 키음 프레임 = 판정이 예약한 C(반올림)
        cb = list(csv.DictReader(open(run / "asio_callbacks.csv", newline="")))
        f = np.array([int(r["frame"]) for r in cb], dtype=float)
        q = np.array([int(r["qpc_in"]) for r in cb], dtype=float)
        b, a = np.polyfit(f, q, 1)
        out_lat = meta.get("asio_latency_after_create", {}).get("output", 0)
        t_out = (b * (np.round(cfr) + out_lat) + a)
        ro = None
        how = f"장치 시계 + 보고 출력 지연 {out_lat} 샘플"

    # 누름마다 그 뒤 첫 키음 출력과 짝 (300 ms 안)
    t_out = np.sort(t_out)
    lat, inp = [], []
    for s, st in zip(sent, stamp[: len(sent)]):
        i = int(np.searchsorted(t_out, s))
        if i < len(t_out) and t_out[i] - s < 0.3 * qf:
            lat.append((t_out[i] - s) * 1e3 / qf)
            inp.append((st - s) * 1e3 / qf)
    lat, inp = np.array(lat), np.array(inp)
    rest = lat - inp - (d_ms or 0)
    rep = {"path": meta.get("path"), "d_ms": d_ms, "faults": meta.get("faults"), "how": how,
           "press_to_keysound_ms": pct(lat), "input_delivery_ms": pct(inp), "after_D_ms": pct(rest)}
    (run / "latency_summary.json").write_text(json.dumps(rep, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"{run.name}  [{rep['path']}, D {d_ms} ms, 출력 시각: {how}]")
    print(f"  누름 → 내 키음 출력   {fmt(rep['press_to_keysound_ms'])}")
    print(f"    = 입력 전달         {fmt(rep['input_delivery_ms'])}")
    print(f"    + D {d_ms} ms")
    print(f"    + 기준점 → 출력     {fmt(rep['after_D_ms'])}")


if __name__ == "__main__":
    for a in sys.argv[1:]:
        analyze(pathlib.Path(a))
