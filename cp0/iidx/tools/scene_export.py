r"""IIDX .1 채보의 앞 N소절을 잘라 JSON으로 내보냄 + 같은 장면의 BMS 표현을 계산.

python -I cp0\iidx\tools\scene_export.py <file.1> <slot> <소절수> > scene.json

BMS 변환 가정 (검증 안 됨):
  - 레인 param 0~6 → 채널 11,12,13,14,15,18,19 / 7 → 16 (스크래치)
  - 소리 번호(value)를 그대로 36진 2글자 WAV ID로 씀 (0부터/1부터 미확인)
"""
import sys, struct, json

B36 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
LANE_CH = {0: "11", 1: "12", 2: "13", 3: "14", 4: "15", 5: "18", 6: "19", 7: "16"}


def b36(n):
    return B36[n // 36] + B36[n % 36] if 0 < n < 1296 else "??"


def main():
    path, slot, nmeas = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    raw = open(path, "rb").read()
    off, ln = struct.unpack_from("<II", raw, slot * 8)
    events = []
    for k in range(ln // 8):
        o = off + k * 8
        t, ty, pa, va = struct.unpack_from("<IBBH", raw, o)
        events.append({"i": k, "off": o, "t": t, "type": ty, "param": pa, "value": va,
                       "hex": raw[o:o + 8].hex(" ")})

    # 소절 경계: 0 + type 12 시각들
    bars = [0] + [e["t"] for e in events if e["type"] == 12]
    end_t = bars[nmeas]
    scene = [e for e in events if e["t"] < end_t]

    # 템포/박자 상태 (장면 안에서는 시작값만 씀)
    tempo = next(e["value"] / e["param"] for e in scene if e["type"] == 4)
    sigs = [(e["t"], e["value"], e["param"]) for e in scene if e["type"] == 5]

    def measure_of(t):
        m = max(i for i in range(nmeas) if bars[i] <= t)
        return m, (t - bars[m]) / (bars[m + 1] - bars[m])

    # 레인 현재 소리 추적 → 노트에 소리 붙이기
    cur = {}
    notes, changes, autos = [], [], []
    for e in scene:
        if e["type"] == 2:
            cur[e["param"]] = e["value"]
            changes.append(e)
        elif e["type"] == 0:
            e["sound"] = cur.get(e["param"])
            notes.append(e)
        elif e["type"] == 7:
            autos.append(e)
    # 교체 후 다음 교체 전까지 노트가 없었던 교체 = 빈 레인 소리만 바꾼 것
    for c in changes:
        nxt_change = min((d["t"] for d in changes if d["param"] == c["param"] and d["t"] > c["t"]), default=end_t)
        c["used"] = any(n["param"] == c["param"] and c["t"] <= n["t"] < nxt_change for n in notes)

    # BMS: 소절마다 최소 분할 찾기 (오차 ±3ms 안)
    bms = []
    for m in range(nmeas):
        mlen = bars[m + 1] - bars[m]
        items = [e for e in notes + autos if bars[m] <= e["t"] < bars[m + 1]]
        div = None
        for d in (1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 192):
            if all(abs((e["t"] - bars[m]) / mlen * d - round((e["t"] - bars[m]) / mlen * d)) * mlen / d <= 3 for e in items):
                div = d
                break
        div = div or 192
        for e in items:
            pos = round((e["t"] - bars[m]) / mlen * div)
            e["bms"] = {"measure": m, "slot": pos, "div": div,
                        "err_ms": round(bars[m] + pos * mlen / div - e["t"], 2)}
        lines = []
        num, den = next((v, p) for (t, v, p) in reversed(sigs) if t <= bars[m])
        if num != den:  # 4/4가 아니면 소절 길이 배율 = 분자/분모
            lines.append({"ch": "02", "text": f"#{m:03d}02:{num / den:g}", "ids": []})
        # BGM: 같은 칸에 겹치면 줄을 늘림
        rows = []
        for e in [a for a in autos if a.get("bms", {}).get("measure") == m]:
            for r in rows:
                if r[e["bms"]["slot"]] is None:
                    r[e["bms"]["slot"]] = e
                    break
            else:
                r = [None] * div
                r[e["bms"]["slot"]] = e
                rows.append(r)
        for r in rows:
            lines.append({"ch": "01", "text": f"#{m:03d}01:" + "".join(b36(e["value"]) if e else "00" for e in r),
                          "ids": [e["i"] if e else None for e in r]})
        for lane in (7, 0, 1, 2, 3, 4, 5, 6):
            r = [None] * div
            for n in notes:
                if n["param"] == lane and n.get("bms", {}).get("measure") == m:
                    r[n["bms"]["slot"]] = n
            if any(r):
                lines.append({"ch": LANE_CH[lane],
                              "text": f"#{m:03d}{LANE_CH[lane]}:" + "".join(b36(n["sound"]) if n else "00" for n in r),
                              "ids": [n["i"] if n else None for n in r]})
        bms.append({"measure": m, "div": div, "lines": lines})

    out = {"file": path.replace("\\", "/").split("/")[-1], "slot": slot, "tempo": tempo,
           "bars": bars[:nmeas + 1], "sigs": sigs,
           "header": [e for e in scene if e["type"] in (16, 4, 5, 8) and e["t"] == 0],
           "events": scene, "bms": bms}
    for e in scene:
        if e["type"] == 0:
            e["sound36"] = b36(e["sound"]) if e["sound"] else None
    json.dump(out, sys.stdout, ensure_ascii=False)


main()
