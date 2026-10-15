"""CP1-4 4-1: 요청 → 쓰기 위치 지연 (기록 시계 D가 덮어야 하는 양).

python -I -X utf8 cp1/tools/analyze_d.py <runs/...> [...]

sched / asiorun 을 --probe-hz 로 돌린 기록의 starts.csv에서 sound=1(요청 탐침) 행을 읽는다.
  requested = 요청 순간의 장치 위치 A(q) (장치 시계 직선으로 본 프레임)
  block     = 믹서가 그 요청을 처리한 블록의 첫 프레임 (그 순간 소리를 놓을 수 있는 가장 이른 위치)
  필요량    = block − requested (프레임 → ms). D가 이보다 작으면 그 요청은 제자리에 놓이지 못한다.
결과: <run>/d_summary.json
"""
import json, pathlib, sys

import numpy as np


def main():
    for a in sys.argv[1:]:
        run = pathlib.Path(a)
        meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
        rate = float(meta["render"]["rate"])
        rows = np.genfromtxt(run / "starts.csv", delimiter=",", names=True, dtype=np.int64)
        p = rows[rows["sound"] == 1]
        need = (p["block"] - p["requested"]) * 1e3 / rate
        path = meta.get("path", "WASAPI 공유")
        buf = meta["render"].get("buffer_frames")
        qs = [50, 90, 99, 99.9, 100]
        st = {f"p{q}": round(float(np.percentile(need, q)), 3) for q in qs}
        st.update(n=int(need.size), mean=round(float(need.mean()), 3), min=round(float(need.min()), 3), rate=rate, path=path, buffer_frames=buf)
        (run / "d_summary.json").write_text(json.dumps(st, ensure_ascii=False, indent=1), encoding="utf-8")
        print(f"{run.name} [{path}, 버퍼 {buf}]: 요청 {need.size}개, 필요량 ms  최소 {st['min']}  중앙 {st['p50']}  p99 {st['p99']}  p99.9 {st['p99.9']}  최대 {st['p100']}")


if __name__ == "__main__":
    main()
