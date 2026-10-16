"""CP2 2차 실행 분석: PERFECT 키음 vs 반주, 그리고 그 기록이 실제 출력과 같은지.

python -I -X utf8 cp2/tools/analyze_v2.py <runs/...> [...] [--no-audio]

1. PERFECT 키음 vs 반주 (믹서 기록, 샘플 단위):
   - 키음: 그 누름이 요청한 플레이어 소리가 실제로 놓인 시간축 프레임.
   - 반주(새 소리): 곡 시각이 T에 가장 가까운 자동재생(2초 안)이 실제로 놓인 자리에서 본 T의 위치. 같은 거리면 더 크게 어긋난 쪽.
   - 반주(긴 소리): T에 울리고 있는 긴 자동재생 소리(2초 넘는 것)들의 해당 위치 = 그 소리의 시작 + (T − 그 소리의 곡 시각).
     그중 키음과 가장 크게 어긋난 것을 쓴다 (귀에 들리는 가장 나쁜 어긋남).
   - 값 = 키음 − 반주 (ms, +면 키음이 늦음). PERFECT 판정을 받은 누름만 센다.
   - 손실은 따로 센다: 키음을 버린 것, 예약 프레임을 지나 도착해 늦게 놓인 것 (정지 중에 누른 노트 — 제자리에 낼 수 없음).
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
    # 자동재생 사건(시작·버림·넘침)을 곡 순서대로 짝짓는다 (버린 것까지 넣어야 뒤가 밀리지 않는다).
    # 기준으로는 제때 놓인 것(start == due)만 쓴다. 우리 방식은 due에서 곡 시각을 직접 복원해 검산한다.
    rows = [r for r in mix if r["player"] == "0" and r["kind"] in ("start", "dropped", "overflow")]
    n = min(len(auto), len(rows))
    keep = [i for i in range(n) if rows[i]["kind"] == "start" and rows[i]["start"] == rows[i]["due"]]
    aT = np.array([auto[i][0] for i in keep], dtype=float)
    aS = np.array([auto[i][1] for i in keep], dtype=int)
    aF = np.array([int(rows[i]["start"]) for i in keep], dtype=float)
    if mix and "t_ns" in mix[0]:
        # 믹서 기록에 곡 시각(t_ns)이 있으면 순서 대신 그것으로 정확히 짝짓는다 (순서 짝짓기는 한 블록에 몰린 요청에서 밀릴 수 있다)
        ex = [(int(r["t_ns"]), int(r["sound"]), int(r["start"])) for r in rows if r["kind"] == "start" and r["start"] == r["due"] and r.get("t_ns")]
        aT = np.array([e[0] for e in ex], dtype=float)
        aS = np.array([e[1] for e in ex], dtype=int)
        aF = np.array([e[2] for e in ex], dtype=float)
        o = np.argsort(aT, kind="stable")
        aT, aS, aF = aT[o], aS[o], aF[o]
    if meta.get("profile") == "ours":
        want = origin + d + np.floor(aT * rate / 1e9 + 0.5)
        mism = int(np.sum(np.abs(aF - want) > 0))
        if mism:
            print(f"  경고: 자동재생 짝짓기 검산 불일치 {mism}개")

    # 플레이어 키음: 예약 프레임(due) → 실제 시작, 그리고 블록 순
    p_rows = [r for r in mix if r["kind"] == "start" and r["player"] == "1"]
    ks_by_due, late_by_due = {}, {}
    for r in p_rows:
        ks_by_due.setdefault(int(r["due"]), int(r["start"]))
        late_by_due.setdefault(int(r["due"]), int(r["late"]))
    p_blocks = np.array(sorted(int(r["block"]) for r in p_rows), dtype=float)
    p_start_by_block = {}
    for r in p_rows:
        p_start_by_block.setdefault(int(r["block"]), int(r["start"]))
    # 플레이어 키음: 노트 곡 시각(t_ns, 믹서가 노트에서 붙인 값) → 실제 시작. 기록에 있으면 시각 어림 대신 이것으로 정확히 짝짓는다
    has_t = "t_ns" in (mix[0] if mix else {})
    # 같은 노트 시각의 키음이 여럿일 수 있다 (빈 누름도 다음 노트의 소리를 울린다) → 놓인 순서대로 두고 판정 순서대로 하나씩 쓴다
    ks_by_t = {}
    for r in sorted((r for r in p_rows if r.get("t_ns")), key=lambda r: int(r["start"])):
        ks_by_t.setdefault(int(r["t_ns"]), []).append(int(r["start"]))
    note_t = [n[0] for n in cd["lanes"][0]]
    # 재트리거 끊기(BMS식): 끊긴 자동재생 소리는 그 프레임에서 끝난다 → "울리고 있는 긴 소리"에서 뺀다
    cut_end = np.full(len(aF), np.inf)
    by_sound = {}
    for i in range(len(aF)):
        by_sound.setdefault(int(aS[i]), []).append(i)
    for r in mix:
        if r["kind"] == "cut":
            at = int(r["due"])  # cut 행은 due 칸에 끊은 프레임을 쓴다
            idx = [i for i in by_sound.get(int(r["sound"]), []) if aF[i] < at and not np.isfinite(cut_end[i])]
            if idx:
                cut_end[idx[-1]] = at
    dropped = sum(1 for r in mix if r["kind"] == "dropped" and r["player"] == "1")
    late = sum(1 for r in p_rows if int(r["late"]) > 0)

    # 블록: QPC ↔ 시간축 (프레임 구동 프로필의 키음 짝짓기, 고장 구간)
    blocks = load_csv(run / "audio_blocks.csv")
    bq = np.array([int(b["qpc_wake"]) for b in blocks], dtype=float)
    btl = np.array([int(b["tl"]) for b in blocks], dtype=float)

    # 봇이 실제로 누른 시각 (SendInput 직후 QPC). 도장 − 실제 누름 = 입력 시각 도장의 오차 (봇 실행에서만)
    bot_q = np.array(sorted(int(b["q_sent"]) for b in load_csv(run / "bot.csv") if b["down"] == "1"), dtype=np.int64) if (run / "bot.csv").exists() else np.array([], dtype=np.int64)

    def stamp_err(q_in):
        if not bot_q.size:
            return None
        i = int(np.searchsorted(bot_q, q_in))
        c = bot_q[max(i - 1, 0):i + 1]
        s0 = c[np.argmin(np.abs(c - q_in))]
        e = (q_in - s0) * 1e3 / qf
        return e if abs(e) < 50 else None

    hits = []
    for r in load_csv(run / "judgments.csv"):
        kind, grade = int(r["kind"]), int(r["grade"])
        if kind == 1 and has_t and int(r["key_frame"]) < 0 and int(r["note"]) < len(note_t):
            # 빈 누름이 울린 다음 노트의 소리: 그 노트의 키음 목록에서 하나를 소비한다
            q = ks_by_t.get(note_t[int(r["note"])])
            if q:
                q.pop(0)
            continue
        if kind not in (0, 7):
            continue
        T = (int(r["t_ns"]) - int(r["err_ns"])) / 1e9  # 노트 곡 시각 (초)
        kf = int(r["key_frame"])
        late_f = 0
        if kf >= 0:
            ks = ks_by_due.get(kf)
            late_f = late_by_due.get(kf, 0)
        elif has_t:
            # 프레임 구동: 그 노트 시각을 붙인 플레이어 소리 (정확한 짝)
            q = ks_by_t.get(int(r["t_ns"]) - int(r["err_ns"]))
            ks = q.pop(0) if q else None
        else:
            # (t_ns 열이 없는 옛 기록) 이 판정을 처리한 순간 이후 처음 채운 블록의 플레이어 소리 — 옆 노트와 잘못 짝지어질 수 있다
            tl_proc = np.interp(int(r["q_proc"]), bq, btl)
            i = int(np.searchsorted(p_blocks, tl_proc - 1))
            ks = p_start_by_block.get(int(p_blocks[i])) if i < len(p_blocks) and p_blocks[i] - tl_proc < 0.3 * rate else None
        if ks is None:
            hits.append(dict(T=T, grade=grade, ks=None))
            continue
        # 예약 프레임(누름 도장 + D)을 지나 도착한 키음: 목표보다 늦게 놓였다. 반주 기준으로 실제 어디 놓였는지는 아래에서 똑같이 잰다
        late_ms = late_f * 1e3 / rate if late_f > 0 else None
        Tn = T * 1e9
        # 새로 울린 소리: 곡 시각이 가장 가까운 자동재생(2초 안)이 그 자리에서 가리키는 위치. 같은 거리면 더 크게 어긋난 쪽
        # (보간은 오디오 정지 앞뒤처럼 자동재생 자리가 계단으로 바뀌는 곳에서 없는 값을 만든다)
        dist = np.abs(aT - Tn)
        if dist.size and dist.min() <= 2e9:
            cand = np.nonzero(dist <= dist.min() + 1)[0]
            pos = aF[cand] + (Tn - aT[cand]) / 1e9 * rate
            new = pos[np.argmax(np.abs(ks - pos))]
        else:
            new = np.nan
        # 그 순간 울리고 있는 긴 소리 전부와 비교해 가장 크게 어긋난 것 (귀에 들리는 가장 나쁜 어긋남).
        # 가장 최근 것 하나만 보면, 고장 뒤에 새로 시작한 긴 소리가 고장 전부터 울리던 소리의 어긋남을 가린다.
        L = np.nonzero(is_long[aS] & (aT <= Tn) & (aT + snd_len[aS] * 1e9 > Tn) & (aF + (Tn - aT) / 1e9 * rate < cut_end))[0]
        if L.size:
            devs = ks - (aF[L] + (Tn - aT[L]) / 1e9 * rate)
            lng = ks - devs[np.argmax(np.abs(devs))]
        else:
            lng = np.nan
        err = int(r["err_ns"]) / 1e6  # 그 게임이 매긴 판정 오차
        hits.append(dict(T=T, grade=grade, ks=ks, err=err, new=(ks - new) * 1e3 / rate if np.isfinite(new) else None, long=(ks - lng) * 1e3 / rate if L.size else None,
                         late_ms=late_ms, stamp_ms=stamp_err(int(r["q_in"]))))

    def summarize(hs):
        placed = [h for h in hs if h["grade"] == 0 and h["ks"] is not None]
        pf = [h for h in placed if h.get("late_ms") is None]           # 목표 자리에 제때 놓인 PERFECT 키음
        lt = [h for h in placed if h.get("late_ms") is not None]       # 목표 자리보다 늦게 놓인 PERFECT 키음
        nv = [h["new"] for h in pf if h["new"] is not None]
        lv = [h["long"] for h in pf if h["long"] is not None]
        # 잔차 = (키음 − 반주) − 판정 오차: 늦게 놓인 것도 넣는다. 1 ms 넘으면 판정과 소리가 서로 다른 말을 한다
        rv_all = [(h["new"] - h["err"]) for h in placed if h["new"] is not None]
        st = [h["stamp_ms"] for h in placed if h.get("stamp_ms") is not None]
        return dict(perfect=len(pf), perfect_no_keysound=sum(1 for h in hs if h["grade"] == 0 and h["ks"] is None),
                    perfect_late_keysound=len(lt), late_ms=pct([h["late_ms"] for h in lt]),
                    late_vs_backing=pct([h["new"] for h in lt if h["new"] is not None]),
                    ks_vs_new=pct(nv), ks_vs_long=pct(lv), residual=pct(rv_all),
                    mismatch_1ms=int(sum(abs(v) > 1 for v in rv_all)),
                    stamp_vs_press=pct(st), stamp_off_1ms=int(sum(abs(v) > 1 for v in st)),
                    off_new_5ms=int(sum(abs(v) > 5 for v in nv)), off_long_5ms=int(sum(abs(v) > 5 for v in lv)))

    rep = {"run": run.name, "profile": meta.get("profile"), "path": meta.get("path"), "d_ms": round(d * 1e3 / rate, 2),
           "need_max_ms": round(meta.get("need_max_frames", 0) * 1e3 / rate, 2), "skips": meta.get("skips"),
           "tally": meta.get("tally"), "late_player": late, "dropped_player": dropped, "all": summarize(hits)}

    faults = load_csv(run / "faults.csv") if (run / "faults.csv").exists() else []
    rep["faults"] = []
    for f in faults:
        at = float(f["at_song_s"])
        rep["faults"].append({"kind": f["kind"], "at_s": at, "ms": int(f["ms"]), **summarize([h for h in hits if at <= h["T"] < at + 10])})

    # 렉 감지: 고장마다 그 종류의 정지를 감지했나, 감지까지 걸린 시간, 잰 길이. 그 밖의 사건 수.
    names = ["오디오 정지", "언더런", "시계 점프", "화면 정지", "판정 정지", "입력 정지", "상태", "재동기화 완료", "콘텐츠 건너뜀"]
    lags = [(int(r["kind"]), int(r["q_start"]), int(r["q_detect"]), int(r["q_end"]), int(r["magnitude_us"])) for r in load_csv(run / "lag_events.csv")]
    want = {"Audio": 0, "Render": 3, "Judge": 4, "Input": 5}
    det = []
    for f in faults:
        q = int(f["q"])
        c = [e for e in lags if e[0] == want[f["kind"]] and q - qf // 20 <= e[1] <= q + qf]
        if c:
            e = c[0]
            det.append({"kind": f["kind"], "ms": int(f["ms"]), "detected": True, "after_stall_ms": round((e[2] - e[1]) * 1e3 / qf, 1), "measured_ms": round(e[4] / 1e3, 1)})
        else:
            det.append({"kind": f["kind"], "ms": int(f["ms"]), "detected": False})
    counts = {names[k]: sum(1 for e in lags if e[0] == k) for k in range(len(names)) if any(e[0] == k for e in lags)}
    skips = [round(e[4] / 1e3, 1) for e in lags if e[0] == 8]
    rep["lag"] = {"faults": det, "counts": counts, "skips_ms": skips}

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
        rep["integrity"] = {"lag_ms": round(L0 * 1e3 / rate, 2), "windows": checked, "shifted": len(shifts), "shifts": shifts}

    rep["hits"] = [{k: (round(v, 3) if isinstance(v, float) else v) for k, v in h.items()} for h in hits]
    (run / "report_v2.json").write_text(json.dumps(rep, ensure_ascii=False, indent=1), encoding="utf-8")
    a = rep["all"]
    print(f"{run.name}  [{rep['profile']}, {rep['path']}, D {rep['d_ms']} ms (필요 최대 {rep['need_max_ms']}), 건너뜀 {rep['skips']}]")
    if meta.get("env"):
        e = meta["env"]
        print(f"  환경: 포그라운드 아님 {e['not_foreground_s']:.1f}초 · 최소화 {e['iconic_s']:.1f}초 · 가려짐 {e['occluded_s']:.1f}초 · 타이머 해상도 최대 {e['timer_res_max_us']/1e3:.1f} ms · 입력 스레드 깨어남 간격 최대 {e['input_gap_max_us']/1e3:.1f} ms")
    t = rep["tally"] or {}
    print(f"  판정 P{t.get('perfect')} G{t.get('great')} Good{t.get('good')} Bad{t.get('bad')} Miss{t.get('miss')} 무효{t.get('void')} | 늦은 키음 {late} · 버린 키음 {dropped}")
    print(f"  PERFECT {a['perfect']}개: 키음 vs 반주(새 소리) {fmt(a['ks_vs_new'])} · 5 ms 넘음 {a['off_new_5ms']}")
    print(f"                키음 vs 반주(긴 소리) {fmt(a['ks_vs_long'])} · 5 ms 넘음 {a['off_long_5ms']}")
    print(f"                잔차 = 키음 vs 반주(새 소리) − 판정 오차 {fmt(a['residual'])}")
    if a["perfect_no_keysound"] or a["perfect_late_keysound"]:
        print(f"  손실: 키음을 버린 PERFECT {a['perfect_no_keysound']}개 · 키음이 목표보다 늦게 놓인 PERFECT {a['perfect_late_keysound']}개 (목표 대비 {fmt(a['late_ms'])}; 반주 대비 {fmt(a['late_vs_backing'])})")
    print(f"  판정·소리 불일치 (|잔차| > 1 ms, 늦게 놓인 것 포함): {a['mismatch_1ms']}개" + (f" · 도장 − 실제 누름(봇) {fmt(a['stamp_vs_press'])}, 1 ms 넘음 {a['stamp_off_1ms']}개" if a['stamp_vs_press'] else ""))
    for f in rep["faults"]:
        print(f"  고장 {f['kind']} {f['ms']} ms @ {f['at_s']:.1f}s → 10초 안 PERFECT {f['perfect']}: 새 소리 {fmt(f['ks_vs_new'])} | 긴 소리 {fmt(f['ks_vs_long'])} | 손실 {f['perfect_no_keysound']}+{f['perfect_late_keysound']}")
    if rep["lag"]["faults"]:
        print("  렉 감지: " + " · ".join(f"{x['kind']} {x['ms']} ms → " + (f"감지 (멈춘 뒤 {x['after_stall_ms']} ms, 잰 길이 {x['measured_ms']} ms)" if x["detected"] else "감지 못함") for x in rep["lag"]["faults"]))
        print(f"  사건 수 {rep['lag']['counts']} · 콘텐츠 건너뜀 {rep['lag']['skips_ms']} ms")
    if "integrity" in rep:
        g = rep["integrity"]
        print(f"  무결성: 녹음 ↔ 재구성 어긋남 {g['lag_ms']} ms, 1초 창 {g['windows']}개 중 어긋남이 바뀐 창 {g['shifted']}개 {g['shifts'][:5]}")
    return rep


if __name__ == "__main__":
    args = [x for x in sys.argv[1:] if not x.startswith("--")]
    for a in args:
        analyze(pathlib.Path(a), audio="--no-audio" not in sys.argv)
