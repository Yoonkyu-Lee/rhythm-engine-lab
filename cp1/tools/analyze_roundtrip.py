"""CP1-4: ASIO 보수적 측정 — 장치 시계 기준점 → 루프백 입력 버퍼 도착 (③ + ⑤ 왕복).

python -I -X utf8 cp1/tools/analyze_roundtrip.py <runs/asio-perfect-...> [...]

- 시작: 판정 스레드가 키음을 놓은 프레임 F(= C 반올림)가 장치 시계 직선(콜백의 샘플 위치 ↔ 콜백 진입 QPC)상 지나는 시각.
- 끝: 루프백 녹음(오른쪽 채널)에서 그 키음 시작점이 들어 있는 입력 버퍼를 받은 콜백의 진입 QPC.
  녹음은 콜백마다 버퍼 하나씩 차례로 붙으므로, 녹음 인덱스 // 버퍼 크기 = 콜백 번호.
"""
import csv, json, pathlib, sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from analyze_perfect import onsets  # noqa: E402


def main():
    for a in sys.argv[1:]:
        run = pathlib.Path(a)
        meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
        rate = meta["render"]["rate"]
        buf = meta["render"]["buffer_frames"]
        qf = meta["qpc_freq"]
        cb = list(csv.DictReader(open(run / "asio_callbacks.csv", newline="")))
        f = np.array([int(r["frame"]) for r in cb], dtype=float)
        q = np.array([int(r["qpc_in"]) for r in cb], dtype=float)
        b, a0 = np.polyfit(f, q, 1)  # QPC = b·frame + a0 (장치 시계)
        cap = np.fromfile(run / "capture_2ch.f32", dtype="<f4").reshape(-1, meta["capture"]["channels"])
        ro = onsets(cap[:, 1], rate)
        jr = list(csv.DictReader(open(run / "judge.csv", newline="")))
        F = np.sort(np.array([round(float(r["c_frame"])) for r in jr], dtype=float))
        t_ref = b * F + a0
        rt = []
        for i in ro:
            k = int(i // buf)
            if k >= len(q):
                continue
            arrive = q[k]
            # 이 키음 = 도착 전에 놓인 키음 중 가장 늦은 것 (0.3초 안)
            j = int(np.searchsorted(t_ref, arrive)) - 1
            if j >= 0 and arrive - t_ref[j] < 0.3 * qf:
                rt.append((arrive - t_ref[j]) * 1e3 / qf)
        rt = np.array(rt)
        s = {k: round(float(v), 3) for k, v in dict(p50=np.median(rt), p1=np.percentile(rt, 1), p99=np.percentile(rt, 99), min=rt.min(), max=rt.max()).items()}
        s.update(n=int(rt.size), buffer=buf, faults=meta.get("faults"))
        (run / "roundtrip_summary.json").write_text(json.dumps(s, ensure_ascii=False, indent=1), encoding="utf-8")
        print(f"{run.name} [버퍼 {buf}, 고장 '{s['faults']}']  기준점 → 입력 버퍼 도착: 중앙 {s['p50']}  p1 {s['p1']}  p99 {s['p99']}  범위 {s['min']}~{s['max']} ms (n={s['n']})")


if __name__ == "__main__":
    main()
