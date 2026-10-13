r"""3-2 단계(입력 도장 + 고장 C) 분석.

python -I cp1\tools\analyze_input.py <runs\input-...>

정답: 가짜 플레이어가 SendInput을 부른 직후의 QPC (qpc_after).
(a) 폴링 1 kHz: 키 상태가 바뀐 것을 본 순간의 QPC.
(b) Raw Input 수신: 입력 스레드가 메시지를 받은 순간의 QPC.
(c) 메시지 시각: 받은 순간의 QPC − (받은 순간의 틱 − 메시지에 붙은 틱) × 1 ms. 틱 해상도(15–16 ms)만큼 거칠다.

짝짓기:
- (b)(c): 이벤트 수가 정답과 같으면 순서대로. Raw Input은 메시지를 버리지 않는다고 보고, 수가 다르면 경고.
- (a): 폴링은 상태만 보므로, 정답 탭 k의 [누름 − 5 ms, 뗌 + 5 ms] 안에서 처음 본 누름만 그 탭의 누름으로 친다 (뗌도 같은 방식).
"""
import csv, json, sys, pathlib
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")


def rows(p):
    with open(p, newline="") as f:
        return [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]


def analyze(run):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    qf, q0 = meta["qpc_freq"], meta["q0"]
    to_ms = lambda dq: dq * 1e3 / qf
    bot = rows(run / "bot.csv")
    raw = [r for r in rows(run / "raw.csv") if r["vkey"] == meta["vkey"]]
    poll = rows(run / "poll.csv")
    ticks = rows(run / "ticks.csv")
    stalls = [(q0 + t * qf, q0 + (t + d / 1e3) * qf, f"{t:g}초 {d}ms 멈춤") for t, d in meta["stalls"]]

    taps = {}
    for b in bot:
        taps.setdefault(b["k"], {})["hold"] = b["hold_ms"]
        taps[b["k"]]["down" if b["down"] else "up"] = b["qpc_after"]
    K = sorted(taps)

    tv = np.array([t["tick"] for t in ticks]); steps = sorted(set(np.diff(tv[: len(tv) // 2]).tolist()))


    def phase(q):
        for s, e, name in stalls:
            if s - 0.005 * qf <= q <= e + 0.05 * qf:
                return name
        return "평상시"


    def by_order(events, est):
        out = {}
        for d in (1, 0):
            ev = [e for e in events if e["down"] == d]
            tr = [taps[k]["down" if d else "up"] for k in K]
            if len(ev) != len(tr):
                print(f"  경고: Raw Input {'누름' if d else '뗌'} {len(ev)}개 ≠ 정답 {len(tr)}개")
            for k, e in zip(K, ev):
                out[(k, d)] = est(e) - taps[k]["down" if d else "up"]
        return out


    def poll_match():
        out = {}
        for d in (1, 0):
            cand = np.array([p["qpc"] for p in poll if p["down"] == d], dtype=np.int64)
            used = set()
            for k in K:
                lo, hi = taps[k]["down"] - 0.005 * qf, taps[k]["up"] + 0.005 * qf
                if d == 0:  # 뗌은 [뗌 − 5 ms, 다음 누름 + 5 ms]
                    nxt = taps[k + 1]["down"] if k + 1 in taps else taps[k]["up"] + qf
                    lo, hi = taps[k]["up"] - 0.005 * qf, nxt + 0.005 * qf
                j = np.searchsorted(cand, lo)
                if j < len(cand) and cand[j] <= hi and j not in used:
                    used.add(j)
                    out[(k, d)] = cand[j] - taps[k]["down" if d else "up"]
                else:
                    out[(k, d)] = None
        return out


    methods = {
        "(a) 폴링 1 kHz": poll_match(),
        "(b) Raw Input 수신 시각": by_order(raw, lambda e: e["qpc"]),
        "(c) 메시지에 붙은 틱": by_order(raw, lambda e: e["qpc"] - (e["tick_ms"] - e["msg_time_ms"]) * qf / 1000),
    }

    print(f"{run.name}: 탭 {len(K)}개 (누른 길이 5/30/80 ms 반복), Raw Input {len(raw)}개, 폴링 변화 {len(poll)}개, 틱 증가폭 {steps} ms")
    for d, label in ((1, "누름"), (0, "뗌")):
        print(f"\n  [{label}] 오차 = 찍힌 시각 − 실제 시각 (ms)")
        for name, m in methods.items():
            parts = []
            for ph in ["평상시"] + [s[2] for s in stalls]:
                v = [m[(k, d)] for k in K if phase(taps[k]["down" if d else "up"]) == ph]
                if not v:
                    continue
                got = np.array([to_ms(x) for x in v if x is not None])
                lost = sum(1 for x in v if x is None)
                s = f"{ph} {len(v)}개: " + (f"평균 {got.mean():+6.2f}, 범위 {got.min():+6.1f}…{got.max():+6.1f}" if got.size else "")
                parts.append(s + (f", 유실 {lost}" if lost else ""))
            print(f"    {name:<22} " + " | ".join(parts))

    print("\n  고장 구간의 탭 (누름 오차 ms):")
    for s, e, name in stalls:
        for k in K:
            t = taps[k]["down"]
            if s - 0.005 * qf <= t <= e + 0.05 * qf:
                line = f"    [{name}] 탭 {k:3d}: 멈춘 지 {to_ms(t - s):6.1f} ms에 {taps[k]['hold']:2d} ms 누름 →"
                for name2, m in methods.items():
                    v = m[(k, 1)]
                    line += f"  {name2[:3]} {('유실' if v is None else f'{to_ms(v):+7.1f}'):>7}"
                print(line)
    return {
        "run": run.name,
        "stalls": [{"at_s": t, "ms": d} for t, d in meta["stalls"]],
        "taps": [{"k": k, "t_s": round(to_ms(taps[k]["down"] - q0) / 1e3, 3), "hold_ms": taps[k]["hold"],
                  **{key: (None if m[(k, 1)] is None else round(to_ms(m[(k, 1)]), 3)) for key, m in zip(("poll", "receipt", "msgtick"), methods.values())}}
                 for k in K],
    }


if __name__ == "__main__":
    analyze(pathlib.Path(sys.argv[1]))
