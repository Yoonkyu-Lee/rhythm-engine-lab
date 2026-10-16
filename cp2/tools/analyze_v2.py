"""CP2 2차 실행 분석: PERFECT 키음 vs 반주, 그리고 그 기록이 실제 출력과 같은지.

python -I -X utf8 cp2/tools/analyze_v2.py <runs/...> [...] [--no-audio]

1. PERFECT 키음 vs 반주 (믹서 기록, 샘플 단위):
   - 키음: 그 누름이 요청한 플레이어 소리가 실제로 놓인 시간축 프레임.
   - 반주(새 소리): 같은 곡 시각 T의 자동재생이 실제로 놓인 프레임 (T 앞뒤 2초 안에 실제로 시작한 자동재생들을 곡 시각으로 보간).
   - 반주(긴 소리): T에 이미 울리고 있던 긴 자동재생 소리(2초 넘는 것)의 해당 위치 = 그 소리의 시작 + (T − 그 소리의 곡 시각).
   - 값 = 키음 − 반주 (ms, +면 키음이 늦음). PERFECT 판정을 받은 누름만 센다.
   - 잔차 = 값 − 그 게임이 매긴 판정 오차. 판정이 "맞다"고 한 만큼 키음이 반주에 맞았는가 (우리 방식이 정확하면 0).
2. 무결성 (루프백): 믹서 기록으로 재구성한 출력(시간축)을 녹음(장치 위치)과 1초 창마다 맞대어,
   어긋남이 처음과 같은지 본다. 시간축이 끊기지 않았다면 어긋남은 곡 내내 하나의 상수다. 고장으로 소리가 빠진 창은 뺀다.
3. 고장 구간: 고장마다 그 뒤 10초 안의 PERFECT 누름으로 1을 다시 센다.
결과: <run>/report_v2.json
"""
import csv, json, pathlib, subprocess, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CP2 = HERE.parent
sys.path.insert(0, str(HERE))
import rtc  # noqa: E402
from check_render import read_wav_f32  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")
LONG_S = 2.0


def pct(x):
    x = np.asarray(x, dtype=float)
    if x.size == 0:
        return {}
    return {k: round(float(v), 3) for k, v in dict(p50=np.median(x), p1=np.percentile(x, 1), p99=np.percentile(x, 99), min=x.min(), max=x.max()).items()} | {"n": int(x.size)}


def fmt(d):
    return "-" if not d else f"중앙 {d['p50']:+.3f}  p1 {d['p1']:+.3f}  p99 {d['p99']:+.3f}  범위 {d['min']:+.3f}~{d['max']:+.3f} ms (n={d['n']})"


def xcorr_lag(a, b, max_lag):
    """a[i] ≈ b[i − L] 인 L (|L| ≤ max_lag)."""
    n = len(a) + len(b)
    size = 1 << (n - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(a, size) * np.conj(np.fft.rfft(b, size)), size)
    lags = np.concatenate([np.arange(0, max_lag + 1), np.arange(-max_lag, 0)])
    vals = np.concatenate([c[:max_lag + 1], c[size - max_lag:]])
    i = int(np.argmax(vals))
    return int(lags[i])


def load_csv(p):
    return list(csv.DictReader(open(p, newline="", encoding="utf-8")))


def analyze(run, audio=True):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate, origin, gain = meta["rate"], meta["origin"], meta["master_gain"]
    qf = meta["qpc_freq"]
    d = meta.get("d_frames", 0)
    chart_path = pathlib.Path(meta["chart"])
    chart_path = chart_path if chart_path.is_absolute() else CP2 / chart_path
    exp = json.loads((chart_path.parent / "expected.json").read_text(encoding="utf-8"))["events"]
    cd = rtc.decode(chart_path.read_bytes())
    snd_len = np.array([fr / cd["rate"] for _, fr, _ in cd["sounds"]])
    is_long = np.array([k for _, _, k in cd["sounds"]], dtype=bool) | (snd_len > LONG_S)

    # 자동재생: 곡 순서대로의 (곡 시각, 소리)와 실제 시작 프레임 (믹서 기록, 곡 순서로 놓인다)
    auto = [(t, s) for t, s, g, src in exp if src == "auto"]
    mix = load_csv(run / "mix_events.csv")
    a_starts = [int(r["start"]) for r in mix if r["kind"] == "start" and r["player"] == "0"]
    n = min(len(auto), len(a_starts))
    aT = np.array([t for t, _ in auto[:n]], dtype=float)
    aS = np.array([s for _, s in auto[:n]])
    aF = np.array(a_starts[:n], dtype=float)
    short = ~is_long[aS]

    # 플레이어 키음: 예약 프레임(due) → 실제 시작, 그리고 블록 순
    p_rows = [r for r in mix if r["kind"] == "start" and r["player"] == "1"]
    ks_by_due = {}
    for r in p_rows:
        ks_by_due.setdefault(int(r["due"]), int(r["start"]))
    p_blocks = np.array(sorted(int(r["block"]) for r in p_rows), dtype=float)
    p_start_by_block = {}
    for r in p_rows:
        p_start_by_block.setdefault(int(r["block"]), int(r["start"]))
    dropped = sum(1 for r in mix if r["kind"] == "dropped" and r["player"] == "1")
    late = sum(1 for r in p_rows if int(r["late"]) > 0)

    # 블록: QPC ↔ 시간축 (프레임 구동 프로필의 키음 짝짓기, 고장 구간)
    blocks = load_csv(run / "audio_blocks.csv")
    bq = np.array([int(b["qpc_wake"]) for b in blocks], dtype=float)
    btl = np.array([int(b["tl"]) for b in blocks], dtype=float)

    hits = []
    for r in load_csv(run / "judgments.csv"):
        kind, grade = int(r["kind"]), int(r["grade"])
        if kind not in (0, 7):
            continue
        T = (int(r["t_ns"]) - int(r["err_ns"])) / 1e9  # 노트 곡 시각 (초)
        kf = int(r["key_frame"])
        if kf >= 0:
            ks = ks_by_due.get(kf)
        else:
            # 프레임 구동: 이 판정을 처리한 순간 이후 처음 채운 블록의 플레이어 소리
            tl_proc = np.interp(int(r["q_proc"]), bq, btl)
            i = int(np.searchsorted(p_blocks, tl_proc - 1))
            ks = p_start_by_block.get(int(p_blocks[i])) if i < len(p_blocks) and p_blocks[i] - tl_proc < 0.3 * rate else None
        if ks is None:
            hits.append(dict(T=T, grade=grade, ks=None))
            continue
        Tn = T * 1e9
        # 새로 울린 소리: 이 시각 앞뒤 2초 안에 시작한 자동재생들로 보간 (범위 밖으로 끝값을 끌어오지 않게)
        near = np.abs(aT - Tn) <= 2e9
        new = np.interp(Tn, aT[near], aF[near]) if near.any() and aT[near].min() <= Tn <= aT[near].max() else np.nan
        L = np.nonzero(is_long[aS] & (aT <= Tn) & (aT + snd_len[aS] * 1e9 > Tn))[0]
        lng = aF[L[-1]] + (Tn - aT[L[-1]]) / 1e9 * rate if L.size else np.nan
        err = int(r["err_ns"]) / 1e6  # 그 게임이 매긴 판정 오차
        hits.append(dict(T=T, grade=grade, ks=ks, err=err, new=(ks - new) * 1e3 / rate if np.isfinite(new) else None, long=(ks - lng) * 1e3 / rate if L.size else None))

    def summarize(hs):
        pf = [h for h in hs if h["grade"] == 0 and h["ks"] is not None]
        nv = [h["new"] for h in pf if h["new"] is not None]
        rv = [h["new"] - h["err"] for h in pf if h["new"] is not None]
        lv = [h["long"] for h in pf if h["long"] is not None]
        return dict(perfect=len(pf), perfect_no_keysound=sum(1 for h in hs if h["grade"] == 0 and h["ks"] is None),
                    ks_vs_new=pct(nv), ks_vs_long=pct(lv), residual=pct(rv),
                    off_new_5ms=int(sum(abs(v) > 5 for v in nv)), off_long_5ms=int(sum(abs(v) > 5 for v in lv)))

    rep = {"run": run.name, "profile": meta.get("profile"), "path": meta.get("path"), "d_ms": round(d * 1e3 / rate, 2),
           "need_max_ms": round(meta.get("need_max_frames", 0) * 1e3 / rate, 2), "skips": meta.get("skips"),
           "tally": meta.get("tally"), "late_player": late, "dropped_player": dropped, "all": summarize(hits)}

    faults = load_csv(run / "faults.csv") if (run / "faults.csv").exists() else []
    rep["faults"] = []
    for f in faults:
        at = float(f["at_song_s"])
        rep["faults"].append({"kind": f["kind"], "at_s": at, "ms": int(f["ms"]), **summarize([h for h in hits if at <= h["T"] < at + 10])})

    # 무결성: 재구성 출력(시간축) ↔ 녹음(장치 위치)
    if audio and (run / "capture_2ch.f32").exists() and (run / "capture_packets.csv").exists():
        rs = run / "restream.wav"
        if not rs.exists():
            subprocess.run([str(CP2 / "target" / "release" / "restream.exe"), str(chart_path), str(run / "mix_events.csv"), str(rate), str(gain), str(rs)],
                           check=True, capture_output=True)
        stream = read_wav_f32(rs)[:, 0].astype(np.float64)
        raw = np.fromfile(run / "capture_2ch.f32", "<f4").reshape(-1, 2)[:, 0]
        pk = load_csv(run / "capture_packets.csv")
        pos = np.array([int(p["device_pos"]) for p in pk], dtype=np.int64)
        fr = np.array([int(p["frames"]) for p in pk], dtype=np.int64)
        cap = np.zeros(int(pos.max() + fr.max()) - int(pos.min()) + 1)
        k = 0
        for p0, nfr in zip(pos - pos.min(), fr):
            cap[p0:p0 + nfr] = raw[k:k + nfr]
            k += nfr
        seg = min(len(cap), 20 * rate)
        L0 = xcorr_lag(cap[:seg], stream[:seg + 2 * rate], 2 * rate)
        # 고장으로 소리가 빠진 시간축 구간 (오디오 정지·건너뛰기 블록 ± 0.5초)
        bad = [int(b["tl"]) for b in blocks if int(b["injected_ms"]) > 0]
        offs = np.array([int(b["offset"]) for b in blocks])
        bad += [int(blocks[i]["tl"]) for i in np.nonzero(np.diff(offs))[0] + 1]
        win, fine = rate, rate // 100
        shifts, checked = [], 0
        for s0 in range(0, len(stream) - win, win):
            if any(abs(s0 + win // 2 - b) < win for b in bad):
                continue
            c0 = s0 + L0
            if c0 - fine < 0 or c0 + win + fine > len(cap):
                continue
            r = stream[s0:s0 + win]
            if np.sqrt(np.mean(r * r)) < 1e-3:
                continue
            c = cap[c0 - fine:c0 + win + fine]
            dl = xcorr_lag(c, np.concatenate([np.zeros(fine), r, np.zeros(fine)]), fine)
            checked += 1
            if abs(dl) > 1:
                shifts.append({"song_s": round((s0 - origin) / rate, 2), "shift_ms": round(dl * 1e3 / rate, 2)})
        rep["integrity"] = {"lag_ms": round(L0 * 1e3 / rate, 2), "windows": checked, "shifted": len(shifts), "shifts": shifts[:20]}

    (run / "report_v2.json").write_text(json.dumps(rep, ensure_ascii=False, indent=1), encoding="utf-8")
    a = rep["all"]
    print(f"{run.name}  [{rep['profile']}, {rep['path']}, D {rep['d_ms']} ms (필요 최대 {rep['need_max_ms']}), 건너뜀 {rep['skips']}]")
    t = rep["tally"] or {}
    print(f"  판정 P{t.get('perfect')} G{t.get('great')} Good{t.get('good')} Bad{t.get('bad')} Miss{t.get('miss')} 무효{t.get('void')} | 늦은 키음 {late} · 버린 키음 {dropped}")
    print(f"  PERFECT {a['perfect']}개: 키음 vs 반주(새 소리) {fmt(a['ks_vs_new'])} · 5 ms 넘음 {a['off_new_5ms']}")
    print(f"                키음 vs 반주(긴 소리) {fmt(a['ks_vs_long'])} · 5 ms 넘음 {a['off_long_5ms']}")
    print(f"                잔차 = 키음 vs 반주(새 소리) − 판정 오차 {fmt(a['residual'])}")
    if a["perfect_no_keysound"]:
        print(f"  키음이 안 난 PERFECT {a['perfect_no_keysound']}개")
    for f in rep["faults"]:
        print(f"  고장 {f['kind']} {f['ms']} ms @ {f['at_s']:.1f}s → 10초 안 PERFECT {f['perfect']}: 새 소리 {fmt(f['ks_vs_new'])} | 긴 소리 {fmt(f['ks_vs_long'])}")
    if "integrity" in rep:
        g = rep["integrity"]
        print(f"  무결성: 녹음 ↔ 재구성 어긋남 {g['lag_ms']} ms, 1초 창 {g['windows']}개 중 어긋남이 바뀐 창 {g['shifted']}개 {g['shifts'][:5]}")
    return rep


if __name__ == "__main__":
    args = [x for x in sys.argv[1:] if not x.startswith("--")]
    for a in args:
        analyze(pathlib.Path(a), audio="--no-audio" not in sys.argv)
