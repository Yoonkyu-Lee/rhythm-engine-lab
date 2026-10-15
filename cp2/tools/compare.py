"""CP2 2-5 비교 표: "고장 시나리오 × 프로필".

python -I -X utf8 cp2/tools/compare.py [--runs cp2/runs] [--out cp2/results/2-5_matrix.md]

run_matrix.sh가 만든 runs/m-<프로필>-<시나리오>-* (각각 가장 최근 것)를 analyze_run.py로 분석하고
고장 구간 지표를 표 하나로 모은다. 판정 결정성(M7)은 같은 프로필의 고장 없는 실행과 노트별 등급을 비교한 것.
추가 실험(runs/x-*)은 표 아래에 따로 적는다.
"""
import argparse, json, pathlib, subprocess, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
PROFILES = ["ours", "iidx", "djmax", "bms"]
PNAME = {"ours": "우리", "iidx": "IIDX 개량", "djmax": "DJMAX식", "bms": "BMS식"}
SCEN = ["clean", "audio150", "render300", "judge500", "input300", "audio50"]
SNAME = {"clean": "고장 없음", "audio150": "오디오 150 ms", "render300": "화면 300 ms", "judge500": "판정 500 ms", "input300": "입력 300 ms", "audio50": "오디오 50 ms"}
GRADE = ["P", "G", "Good", "Bad", "Miss"]


def latest(runs, tag):
    c = sorted(runs.glob(f"{tag}-*"))
    return c[-1] if c else None


def analyze(run):
    subprocess.run([sys.executable, "-I", "-X", "utf8", str(HERE / "analyze_run.py"), str(run), "--no-audio"], capture_output=True, text=True, encoding="utf-8")
    return json.loads((run / "report.json").read_text(encoding="utf-8"))


def grades(run):
    """노트 → 등급 번호 (0~3, MISS = 4)."""
    j = np.genfromtxt(run / "judgments.csv", delimiter=",", names=True, dtype=np.int64)
    out = {}
    for r in j:
        if r["kind"] == 0:
            out[int(r["note"])] = int(r["grade"])
        elif r["kind"] == 2:
            out[int(r["note"])] = 4
    return out


def judged_states(run):
    """판정마다 (노트, 등급, 상태, 오차 ms) — 노트 판정과 MISS만."""
    j = np.genfromtxt(run / "judgments.csv", delimiter=",", names=True, dtype=np.int64)
    return [(int(r["note"]), int(r["grade"]) if r["kind"] == 0 else 4, int(r["state"]), r["err_ns"] / 1e6) for r in j if r["kind"] in (0, 2)]


def v(x, unit=""):
    return "–" if x is None else f"{x:g}{unit}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default=str(HERE.parent / "runs"))
    ap.add_argument("--out", default=str(HERE.parent / "results" / "2-5_matrix.md"))
    a = ap.parse_args()
    runs = pathlib.Path(a.runs)
    lines = ["# CP2 2-5 비교 표 (고장 시나리오 × 프로필)", "",
             "합성 곡(120 BPM 4분음표), 자동 플레이어(들리는 음악에 맞춰 침), 고장은 곡 10초에 하나, 22초에 끝. 지표는 고장부터 10초 창(판정 등급은 3초).",
             "- **판정**: 고장 뒤 3초의 등급 (P/G/Good/Bad/Miss)",
             "- **M1**: 자동재생 발음 오차의 변화 최대 (고장 전 중앙값 대비, ms)",
             "- **M2**: 판정 시각 오차(프로필이 매긴 오차 − 실제 누른 오차)의 변화 최대 → 5~10초 뒤 잔여 (ms)",
             "- **M3**: 차트↔오디오 어긋남이 10 ms 안으로 돌아와 머문 시각 (ms, 고장부터. – = 10초 안에 못 돌아옴)",
             "- **M4**: 화면 튐 최대 = 프레임 사이 차트 이동 − 실제 흐른 시간 (ms, −면 차트가 멈칫/뒤로)",
             "- **M7**: 고장 없는 실행과 등급이 달라진 노트 수 (22초까지)",
             "- **M8**: 누름 → 키음 장치 출력, 고장 전 중앙값 → 창 안 최대 (ms)", ""]
    clean = {p: latest(runs, f"m-{p}-clean") for p in PROFILES}
    clean_g = {p: grades(r) for p, r in clean.items() if r}
    data = {}
    for s in SCEN:
        for p in PROFILES:
            r = latest(runs, f"m-{p}-{s}")
            if r is None:
                continue
            rep = analyze(r)
            w = (rep.get("fault_windows") or [{}])[0]
            g = grades(r)
            cg = clean_g.get(p, {})
            m7 = sum(1 for n, x in g.items() if n in cg and cg[n] != x)
            t = rep.get("tally") or {}
            data[(s, p)] = dict(rep=rep, w=w, m7=m7, tally=t)
    lines += ["| 시나리오 | 프로필 | 판정(3초) | M1 | M2 최대 → 잔여 | M3 복구 | M4 튐 | M7 | M8 |", "|---|---|---|---|---|---|---|---|---|"]
    for s in SCEN:
        for p in PROFILES:
            d = data.get((s, p))
            if not d:
                continue
            w, rep = d["w"], d["rep"]
            if s == "clean":
                t = d["tally"]
                gr = f"전체 P{t.get('perfect')} G{t.get('great')} Bad{t.get('bad')} Miss{t.get('miss')}"
                je = rep.get("judging_error_ms", {})
                ap_ = rep.get("autoplay_err_ms", {})
                ks = rep.get("keysound_sent_to_device_ms", {})
                lines.append(f"| {SNAME[s]} | {PNAME[p]} | {gr} | 평균 {v(ap_.get('mean'))} | p99 {v(je.get('p99'))} | – | – | – | 중앙 {v(ks.get('p50'))} |")
                continue
            g = w.get("grades", [0] * 5)
            gr = " ".join(f"{GRADE[i]}{g[i]}" for i in range(5) if g[i])
            lines.append(f"| {SNAME[s]} | {PNAME[p]} | {gr} | {v(w.get('m1_peak_ms'))} | {v(w.get('m2_peak_ms'))} → {v(w.get('m2_after_ms'))} | {v(w.get('m3_recover_ms'))} | {v(w.get('m4_jump_ms'))} | {d['m7']} | {v(w.get('m8_ms'))} → {v(w.get('m8_peak_ms'))} |")
    # 추가 실험
    lines += ["", "## 추가 실험", ""]
    for ad in (300, 0):
        r = latest(runs, f"x-adapt{ad}-ours")
        if not r:
            continue
        analyze(r)
        js = judged_states(r)
        nr = [x for x in js if x[2] != 0]
        off = [x for x in js if x[1] != 0]
        who = "사람 흉내 봇(시계 점프를 300 ms 뒤에 알아챔)" if ad else "즉시 따르는 봇"
        lines.append(f"- 재동기화 중 판정 · {who} · 우리 · 오디오 150 ms ×3: 판정 {len(js)}개, PERFECT 아님 {len(off)}개"
                     + (" (" + ", ".join(f"{GRADE[x[1]]} {x[3]:+.0f} ms 상태 {['Running', 'Suspect', 'Resyncing'][x[2]]}" for x in off) + ")" if off else "")
                     + f"; Running 아닌 동안의 판정 {len(nr)}개" + (" (" + ", ".join(f"{GRADE[x[1]]} {x[3]:+.0f} ms" for x in nr) + ")" if nr else ""))
    r = latest(runs, "x-adapt300void-ours")
    if r:
        rep = analyze(r)
        t_ = rep.get("tally") or {}
        lines.append(f"- 재동기화 중 판정 무효(기본값) · 사람 흉내 봇 · 같은 고장: P{t_.get('perfect')} Bad{t_.get('bad')} Miss{t_.get('miss')} 무효 {t_.get('void')} 최대 콤보 {t_.get('max_combo')}")
    for dr in (100, 0):
        r = latest(runs, f"x-drop{dr}-ours")
        if not r:
            continue
        rep = analyze(r)
        mm = (r / "mix_events.csv").read_text(encoding="utf-8").splitlines()
        dropped = sum(1 for ln in mm if ln.startswith("dropped,") and ln.split(",")[6] == "1")
        peaks = ", ".join(f"{w['kind']} {w['ms']} ms → {v(w.get('m8_peak_ms'))}" for w in rep.get("fault_windows", []))
        t_ = rep.get("tally") or {}
        lines.append(f"- 늦은 키음 · {'100 ms 넘으면 버리기' if dr else '밀기(끝없이)'} · 우리: 버린 플레이어 키음 {dropped}개, 고장 창별 키음 지연 최대 [{peaks}] ms, 판정 P{t_.get('perfect')} Miss{t_.get('miss')}")
    out = pathlib.Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
