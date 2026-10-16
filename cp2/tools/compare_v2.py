"""CP2 2차 2-5: 비교 표 — 프로필 × 오디오 경로 × 고장, 지표는 PERFECT 키음 vs 반주.

python -I -X utf8 cp2/tools/compare_v2.py [--song bms_unshakable_6_11] [--out cp2/results/2-5_v2.md]

runs/f-<곡>-<프로필>-<경로>-* (각각 가장 최근 것)를 analyze_v2.py로 분석해 한 장으로 모은다.
"""
import argparse, json, pathlib, sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from analyze_v2 import analyze  # noqa: E402

PROFILES = ["ours", "iidx", "iidx-dev", "djmax", "bms"]
PNAME = {"ours": "우리 방식", "iidx": "IIDX 개량형 (반주 트랙 기준)", "iidx-dev": "IIDX 개량형 (장치 시계 기준)", "djmax": "DJMAX식", "bms": "BMS식"}
PATHS = ["shared", "exclusive", "asio"]
PATHNAME = {"shared": "공유", "exclusive": "배타", "asio": "ASIO"}
FNAME = {"Audio": "오디오", "Render": "화면", "Judge": "판정", "Input": "입력"}


def worst(d):
    if not d:
        return None
    return d["max"] if abs(d["max"]) >= abs(d["min"]) else d["min"]


def cell(d, off):
    w = worst(d)
    return "–" if w is None else f"{w:+.1f} ({off})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--song", default="bms_unshakable_6_11")
    ap.add_argument("--runs", default=str(HERE.parent / "runs"))
    ap.add_argument("--out", default=str(HERE.parent / "results" / "2-5_v2.md"))
    a = ap.parse_args()
    runs = pathlib.Path(a.runs)
    reps = {}
    for prof in PROFILES:
        for p in PATHS:
            c = sorted(runs.glob(f"f-{a.song}-{prof}-{p}-[0-9]*"))
            if c:
                print(f"분석: {c[-1].name}")
                reps[(prof, p)] = analyze(c[-1], audio=False)
    L = ["# CP2 2-5 비교 표 (2차) — PERFECT 키음 vs 반주", "",
         f"곡: {a.song} (BMS 출신 표본 Unshakable, 노트 209). 고장 다섯 개: 오디오 150 ms(15초) · 화면 300 ms(30초) · 판정 500 ms(45초) · 입력 300 ms(60초) · 오디오 1초(75초).",
         "봇은 그 게임의 시계로 PERFECT 순간에 친다 (우리 방식은 기록 시계, 프레임 구동 프로필은 그 프로필의 곡 시각).",
         "키음 짝짓기: 믹서 기록의 노트 곡 시각으로 정확히 (2026-10-16 외부 검토 반영, 이전 표의 시각 어림 짝짓기를 대체). BMS식의 재트리거로 끊긴 반주는 긴 소리 기준에서 뺀다.",
         "",
         "- **키음 vs 반주** = PERFECT 판정을 받은 누름의 키음 − 같은 곡 시각의 반주 (ms, 가장 크게 어긋난 값, 괄호 = 5 ms 넘게 어긋난 개수).",
         "  - 새 소리: 그 시각 앞뒤에 새로 울린 자동재생 기준. 긴 소리: 그 순간 이미 울리고 있던 2초 넘는 소리 기준.",
         "- **손실** = PERFECT인데 키음을 목표 자리에 낼 수 없었던 누름 (버림 + 목표보다 늦게 놓임). 목표 = 누름 도장 + D (프레임 구동은 지금).",
         "- **판정·소리 불일치** = |잔차| > 1 ms인 PERFECT (잔차 = 키음 vs 반주 − 판정 오차, 늦게 놓인 것 포함). 판정이 말하는 어긋남과 실제 소리의 어긋남이 다르다.",
         "- **도장 − 실제 누름** = 게임이 기록한 누른 시각 − 봇이 SendInput을 부른 시각 (ms, 가장 크게 벗어난 값, 괄호 = 1 ms 넘음).",
         "", "## 곡 전체", "",
         "| 프로필 | 경로 | 판정 (P / G / Good / Bad / Miss) | PERFECT | 키음 vs 반주: 새 소리 | 키음 vs 반주: 긴 소리 | 손실 | 판정·소리 불일치 | 도장 − 실제 누름 |",
         "|---|---|---|---|---|---|---|---|---|"]
    for prof in PROFILES:
        for p in PATHS:
            r = reps.get((prof, p))
            if not r:
                continue
            t, al = r["tally"] or {}, r["all"]
            L.append(f"| {PNAME[prof]} | {PATHNAME[p]} | {t.get('perfect')} / {t.get('great')} / {t.get('good')} / {t.get('bad')} / {t.get('miss')} | {al['perfect']} | "
                     f"{cell(al['ks_vs_new'], al['off_new_5ms'])} | {cell(al['ks_vs_long'], al['off_long_5ms'])} | {al['perfect_no_keysound']} + {al['perfect_late_keysound']} | "
                     f"{al['mismatch_1ms']} | {cell(al['stamp_vs_press'], al['stamp_off_1ms'])} |")
    L += ["", "## 고장마다 (그 뒤 10초 안의 PERFECT 누름)", "",
          "| 프로필 | 경로 | " + " | ".join(f"{FNAME[k]} {ms} ms" for k, ms in [("Audio", 150), ("Render", 300), ("Judge", 500), ("Input", 300), ("Audio", 1000)]) + " |",
          "|---|---|---|---|---|---|---|"]
    for prof in PROFILES:
        for p in PATHS:
            r = reps.get((prof, p))
            if not r:
                continue
            cells = []
            for f in r["faults"]:
                nw, lg = worst(f["ks_vs_new"]), worst(f["ks_vs_long"])
                cells.append(("–" if nw is None else f"새 {nw:+.0f}") + " / " + ("–" if lg is None else f"긴 {lg:+.0f}") + f" (P{f['perfect']})")
            L.append(f"| {PNAME[prof]} | {PATHNAME[p]} | " + " | ".join(cells) + " |")
    out = pathlib.Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(L) + "\n", encoding="utf-8")
    print("\n".join(L))


if __name__ == "__main__":
    main()
