"""CP3 싱크 분석 (3-5): CP2 analyze_v2.py의 지표를 여러 레인 실행 기록에 맞게 옮긴 것.

python -I -X utf8 cp3/tools/analyze_sync.py <runs/...> [...] [--no-audio]   (환경 변수 CP3GAME = cp3game.exe 경로, 없으면 target/release)

1. PERFECT 키음 vs 반주 (믹서 기록, 샘플 단위). 모든 레인의 PGREAT 판정마다:
   - 키음: 그 노트의 소리 하나하나(`cp3game notes`: 소리 번호, 노트 시각 + offset)를 믹서 기록의 플레이어 발음(소리 번호, 곡 시각 t_ns)과
     **일대일로** 짝짓는다(같은 짝은 한 번만 쓴다 — 리뷰 3-5 #2: 같은 프레임의 다른 레인 키음이 누락을 가리지 않게). 짝이 없는 소리는 "키음 누락"으로 센다.
     키음 vs 반주·잔차는 **소리마다** 그 소리의 기준 시각(노트 시각 + offset)에서 잰다(CP3 마감 리뷰 #2: 둘째 소리부터 검사하지 않던 것, 아르페지오 offset을 오차로 세던 것).
   - 오프셋 A: 판정 오차(err)가 이미 A를 포함한다(판정 시각 = 청취 시각 + A, 키음 목표 = 판정 시각 + offset). 그래서 "키음 vs 반주" = err이고
     잔차 = (키음 vs 반주) − err 이 A와 무관하게 0이어야 한다. A를 따로 빼지 않는다(마감 리뷰 #2: A를 두 번 빼던 것).
   - 반주(새 소리): 곡 시각이 노트 시각 T에 가장 가까운 자동재생(2초 안, 제때 놓인 것)이 그 자리에서 본 T의 위치. 같은 거리면 더 크게 어긋난 쪽.
   - 반주(긴 소리): T에 울리고 있는 긴 자동재생 소리들의 T 위치 중 키음과 가장 크게 어긋난 것.
   - 값 = 키음 − 반주 (ms, +면 키음이 늦음). 잔차 = 값 − 판정 오차 (판정이 맞다고 한 만큼 소리도 맞았나, |잔차| > 1 ms = 판정·소리 불일치).
   - 오프셋 A가 있으면 키음은 반주 + A가 정상이다 → 값에서 A를 뺀 것을 보고한다 (meta offset_a_ns).
   - 손실: 키음을 버린 PGREAT, 목표 프레임보다 늦게 놓인 PGREAT.
2. 도장 − 실제 누름 (OS 입력 봇 실행): 판정이 쓴 누름 시각 도장(q_in) − **같은 레인·노트**를 노린 봇 누름의 SendInput 직후 QPC (bot.csv).
   봇이 보내지 않은 누름(q_sent = −1, 앞 창 아님)과 짝 없는 판정을 따로 세고, 큰 값도 버리지 않는다(50 ms 넘음 개수를 따로 낸다).
3. 시간 이동 검사 (루프백 녹음): `cp3game restream`으로 믹서 기록에서 다시 만든 출력(시간축) ↔ 녹음(장치 위치)을 1초 창마다 맞대어
   어긋남이 처음과 같은지 (양 채널, 창마다 정규 상관 — 낮은 상관 창 수와 최솟값을 함께 낸다). 파형이 같은지까지 보장하지는 않는다(리뷰 3-5 #6).
   시간축 = 장치 스트림 순번 + 건너뛴 양(블록 기록의 offset)이므로, 창마다 그 자리의 offset만큼 녹음 위치를 옮겨 맞댄다
   (CP2는 offset을 상수로 보아 콘텐츠 건너뜀 뒤 창이 모두 어긋난 것으로 나왔다). offset이 바뀌는 블록 ± 1초 창은 뺀다.
결과: <run>/report_sync.json
"""
import csv, json, os, pathlib, subprocess, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CP3 = HERE.parent
GAME = os.environ.get("CP3GAME", str(CP3 / "target" / "release" / "cp3game.exe"))
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
    return int(lags[int(np.argmax(vals))])


def load_csv(p):
    return list(csv.DictReader(open(p, newline="", encoding="utf-8")))


def read_wav_f32(p):
    b = pathlib.Path(p).read_bytes()
    i = b.index(b"data")
    n = int.from_bytes(b[i + 4:i + 8], "little")
    return np.frombuffer(b[i + 8:i + 8 + n], "<f4").reshape(-1, 2)


def analyze(run, audio=True):
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate, origin, d = meta["rate"], meta["origin"], meta.get("d_frames", 0)
    qf = meta["qpc_freq"]
    a_ns = meta.get("offset_a_ns", 0)
    snd = load_csv_text(subprocess.run([GAME, "sounds", str(run)], capture_output=True, text=True, encoding="utf-8", check=True).stdout)
    snd_len = np.array([int(r["frames"]) / rate for r in snd])
    is_long = np.array([r["long"] == "1" for r in snd]) | (snd_len > LONG_S)

    nts = load_csv_text(subprocess.run([GAME, "notes", str(run)], capture_output=True, text=True, encoding="utf-8", check=True).stdout)
    note_snd = {}
    for r in nts:
        note_snd.setdefault((int(r["lane"]), int(r["note"])), []).append((int(r["offset_ns"]), int(r["sound"]), int(r["t_ns"]) + int(r["offset_ns"])))
    for v in note_snd.values():
        v.sort()
    mix = load_csv(run / "mix_events.csv")
    # 자동재생: 제때 놓인 것(start == due), 곡 시각(t_ns)으로 짝짓는다
    ex = [(int(r["t_ns"]), int(r["sound"]), int(r["start"])) for r in mix if r["kind"] == "start" and r["player"] == "0" and r["start"] == r["due"] and r["t_ns"]]
    ex.sort(key=lambda e: e[0])
    aT = np.array([e[0] for e in ex], dtype=float)
    aS = np.array([e[1] for e in ex], dtype=int)
    aF = np.array([e[2] for e in ex], dtype=float)
    want = origin + d + np.floor(aT * rate / 1e9 + 0.5)
    mism = int(np.sum(np.abs(aF - want) > 0))
    if mism:
        print(f"  경고: 자동재생 자리 검산 불일치 {mism}개 (원점 {origin} + D {d})")

    p_rows = [r for r in mix if r["kind"] == "start" and r["player"] == "1"]
    # (소리, 곡 시각) → 놓인 발음들 (시작 프레임 순). 판정마다 노트 소리 하나당 하나씩 소비한다
    ks_pool = {}
    for r in sorted((r for r in p_rows if r["t_ns"]), key=lambda r: int(r["start"])):
        ks_pool.setdefault((int(r["sound"]), int(r["t_ns"])), []).append((int(r["start"]), int(r["late"])))
    cut_end = np.full(len(aF), np.inf)
    by_sound = {}
    for i in range(len(aF)):
        by_sound.setdefault(int(aS[i]), []).append(i)
    for r in mix:
        if r["kind"] == "cut":
            at = int(r["due"])  # cut 행은 due 칸에 끊은 프레임
            idx = [i for i in by_sound.get(int(r["sound"]), []) if aF[i] < at and not np.isfinite(cut_end[i])]
            if idx:
                cut_end[idx[-1]] = at
    dropped = sum(1 for r in mix if r["kind"] == "dropped" and r["player"] == "1")
    late = sum(1 for r in p_rows if int(r["late"]) > 0)

    # 봇 누름: (레인, 노트) → q_sent (−1 = 보내지 않음)
    bot = {}
    if (run / "bot.csv").exists() and not meta.get("bot_direct"):
        for b in load_csv(run / "bot.csv"):
            if b["down"] == "1":
                bot.setdefault((int(b["lane"]), int(b["note"])), int(b["q_sent"]))
    stamp_unpaired = [0, 0]  # [짝 없음, 봇이 보내지 않음]

    def stamp_err(lane, note, q_in):
        if not bot:
            return None
        q = bot.get((lane, note))
        if q is None:
            stamp_unpaired[0] += 1
            return None
        if q < 0:
            stamp_unpaired[1] += 1
            return None
        return (q_in - q) * 1e3 / qf

    hits = []
    for r in load_csv(run / "judgments.csv"):
        kind, grade = int(r["kind"]), int(r["grade"])
        if kind not in (0, 7):
            continue
        T = (int(r["t_ns"]) - int(r["err_ns"])) / 1e9
        lane, note = int(r["lane"]), int(r["note"])
        # 노트 소리마다 일대일로 소비 (짝 없는 소리 = 누락)
        got, missing = [], 0
        for off, sound, t_snd in note_snd.get((lane, note), []):
            q = ks_pool.get((sound, t_snd))
            if q:
                st_, lf_ = q.pop(0)
                got.append((st_, lf_, off))
            else:
                missing += 1
        if not got:
            hits.append(dict(T=T, lane=lane, grade=grade, ks=None, missing=missing))
            continue
        err = int(r["err_ns"]) / 1e6
        stamp = stamp_err(lane, note, int(r["q_in"]))
        # 소리마다: 기준 시각 = 노트 시각 + offset, 그 시각의 반주 위치와 비교
        for si, (ks, late_f, off) in enumerate(got):
            late_ms = late_f * 1e3 / rate if late_f > 0 else None
            Tn = T * 1e9 + off
            dist = np.abs(aT - Tn)
            if dist.size and dist.min() <= 2e9:
                cand = np.nonzero(dist <= dist.min() + 1)[0]
                pos = aF[cand] + (Tn - aT[cand]) / 1e9 * rate
                new = pos[np.argmax(np.abs(ks - pos))]
            else:
                new = np.nan
            L = np.nonzero(is_long[aS] & (aT <= Tn) & (aT + snd_len[aS] * 1e9 > Tn) & (aF + (Tn - aT) / 1e9 * rate < cut_end))[0] if aS.size else np.array([], dtype=int)
            if L.size:
                devs = ks - (aF[L] + (Tn - aT[L]) / 1e9 * rate)
                lng = ks - devs[np.argmax(np.abs(devs))]
            else:
                lng = np.nan
            hits.append(dict(T=T, lane=lane, grade=grade, ks=ks, err=err, sound_i=si, missing=missing if si == 0 else 0,
                             new=(ks - new) * 1e3 / rate if np.isfinite(new) else None,
                             long=(ks - lng) * 1e3 / rate if L.size else None,
                             late_ms=late_ms, stamp_ms=stamp if si == 0 else None))

    def summarize(hs):
        placed = [h for h in hs if h["grade"] == 0 and h["ks"] is not None]  # 소리 단위
        pf = [h for h in placed if h.get("late_ms") is None]
        lt = [h for h in placed if h.get("late_ms") is not None]
        nv = [h["new"] for h in pf if h["new"] is not None]
        lv = [h["long"] for h in pf if h["long"] is not None]
        rv_all = [(h["new"] - h["err"]) for h in placed if h["new"] is not None]
        rv_long = [(h["long"] - h["err"]) for h in placed if h["long"] is not None]
        st = [h["stamp_ms"] for h in placed if h.get("stamp_ms") is not None]
        return dict(perfect=sum(1 for h in pf if h.get("sound_i", 0) == 0), sounds=len(placed), perfect_no_keysound=sum(1 for h in hs if h["grade"] == 0 and h["ks"] is None),
                    keysound_missing=sum(h.get("missing", 0) for h in hs if h["grade"] == 0),
                    stamp_over_50ms=int(sum(abs(v) > 50 for v in st)),
                    perfect_late_keysound=len(lt), late_ms=pct([h["late_ms"] for h in lt]),
                    late_vs_backing=pct([h["new"] for h in lt if h["new"] is not None]),
                    ks_vs_new=pct(nv), ks_vs_long=pct(lv), residual=pct(rv_all), residual_long=pct(rv_long),
                    mismatch_1ms=int(sum(abs(v) > 1 for v in rv_all)), mismatch_long_1ms=int(sum(abs(v) > 1 for v in rv_long)),
                    stamp_vs_press=pct(st), stamp_off_1ms=int(sum(abs(v) > 1 for v in st)),
                    off_new_5ms=int(sum(abs(v) > 5 for v in nv)), off_long_5ms=int(sum(abs(v) > 5 for v in lv)))

    rep_unpaired = {"stamp_unpaired": stamp_unpaired[0], "bot_not_sent": stamp_unpaired[1],
                    "player_starts_unused": sum(len(v) for v in ks_pool.values())}
    rep = {"run": run.name, **rep_unpaired, "title": meta.get("title"), "chart": meta.get("chart"), "lanes": meta.get("lanes"), "path": meta.get("path"),
           "d_ms": round(d * 1e3 / rate, 2), "skips": meta.get("skips"), "offset_a_ms": a_ns / 1e6, "score": meta.get("score"),
           "late_player": late, "dropped_player": dropped, "all": summarize(hits)}
    rep["by_lane"] = {str(l): summarize([h for h in hits if h["lane"] == l]) for l in sorted({h["lane"] for h in hits})}

    blocks = load_csv(run / "audio_blocks.csv")
    if audio and (run / "capture_2ch.f32").exists() and (run / "capture_packets.csv").exists():
        rs = run / "restream.wav"
        if not rs.exists():
            subprocess.run([GAME, "restream", str(run), str(rs)], check=True, capture_output=True)
        st2 = read_wav_f32(rs).astype(np.float64)
        raw2 = np.fromfile(run / "capture_2ch.f32", "<f4").reshape(-1, 2)
        stream, raw = st2[:, 0], raw2[:, 0]
        pk = load_csv(run / "capture_packets.csv")
        pos = np.array([int(p["device_pos"]) for p in pk], dtype=np.int64)
        fr = np.array([int(p["frames"]) for p in pk], dtype=np.int64)
        cap = np.zeros(int(pos.max() + fr.max()) - int(pos.min()) + 1)
        capr = np.zeros_like(cap)
        k = 0
        for p0, nfr in zip(pos - pos.min(), fr):
            cap[p0:p0 + nfr] = raw[k:k + nfr]
            capr[p0:p0 + nfr] = raw2[k:k + nfr, 1]
            k += nfr
        seg = min(len(cap), 20 * rate)
        L0 = xcorr_lag(cap[:seg], stream[:seg + 2 * rate], 2 * rate)
        bad = [int(b["tl"]) for b in blocks if int(b["injected_ms"]) > 0]
        offs = np.array([int(b["offset"]) for b in blocks])
        btl = np.array([int(b["tl"]) for b in blocks])
        bad += [int(blocks[i]["tl"]) for i in np.nonzero(np.diff(offs))[0] + 1]
        off0 = int(offs[0]) if len(offs) else 0

        def off_at(tl):
            i = int(np.searchsorted(btl, tl, side="right")) - 1
            return int(offs[max(i, 0)]) if len(offs) else 0
        win, fine = rate, rate // 100
        shifts, checked, corrs = [], 0, []
        for s0 in range(0, len(stream) - win, win):
            if any(abs(s0 + win // 2 - b) < win for b in bad):
                continue
            c0 = s0 + L0 - (off_at(s0 + win // 2) - off0)
            if c0 - fine < 0 or c0 + win + fine > len(cap):
                continue
            r = stream[s0:s0 + win]
            if np.sqrt(np.mean(r * r)) < 1e-3:
                continue
            for chan, (strm, cp) in enumerate(((stream, cap), (st2[:, 1], capr))):
                r = strm[s0:s0 + win]
                c = cp[c0 - fine:c0 + win + fine]
                dl = xcorr_lag(c, np.concatenate([np.zeros(fine), r, np.zeros(fine)]), fine)
                seg = cp[c0 + dl:c0 + dl + win]
                den = np.sqrt(np.sum(r * r) * np.sum(seg * seg))
                corrs.append(float(np.sum(r * seg) / den) if den > 0 else 0.0)
                if abs(dl) > 1:
                    shifts.append({"song_s": round((s0 - origin) / rate, 2), "ch": chan, "shift_ms": round(dl * 1e3 / rate, 2)})
            checked += 1
        corrs = np.asarray(corrs)
        rep["integrity"] = {"lag_ms": round(L0 * 1e3 / rate, 2), "windows": checked, "shifted": len(shifts), "shifts": shifts,
                            "corr_min": round(float(corrs.min()), 5) if corrs.size else None, "corr_below_0.99": int(np.sum(corrs < 0.99)),
                            "excluded_windows": len(range(0, len(stream) - win, win)) - checked}

    rep["hits"] = [{k: (round(v, 3) if isinstance(v, float) else v) for k, v in h.items()} for h in hits]
    (run / "report_sync.json").write_text(json.dumps(rep, ensure_ascii=False, indent=1), encoding="utf-8")
    a = rep["all"]
    s = rep["score"] or {}
    print(f"{run.name}  {rep['title']} [{rep['chart']}] 레인 {rep['lanes']} · {rep['path']} · D {rep['d_ms']} ms · 건너뜀 {rep['skips']} · A {rep['offset_a_ms']:+.1f} ms")
    print(f"  점수 PG {s.get('pgreat')} GR {s.get('great')} GD {s.get('good')} BD {s.get('bad')} PR {s.get('poor')} | 늦은 키음 {late} · 버린 키음 {dropped}")
    print(f"  PGREAT {a['perfect']}개 (키음 {a['sounds']}개, 소리마다 기준 = 노트 시각 + offset): 키음 vs 반주(새 소리) {fmt(a['ks_vs_new'])} · 5 ms 넘음 {a['off_new_5ms']}")
    print(f"                키음 vs 반주(긴 소리) {fmt(a['ks_vs_long'])} · 5 ms 넘음 {a['off_long_5ms']}")
    print(f"                잔차 = 키음 vs 반주 − 판정 오차: 새 소리 {fmt(a['residual'])}")
    print(f"                                              긴 소리 {fmt(a['residual_long'])}")
    if a["perfect_no_keysound"] or a["perfect_late_keysound"]:
        print(f"  손실: 키음 없는 PGREAT {a['perfect_no_keysound']}개 · 목표보다 늦게 놓인 PGREAT {a['perfect_late_keysound']}개 (목표 대비 {fmt(a['late_ms'])}; 반주 대비 {fmt(a['late_vs_backing'])})")
    print(f"  키음 누락(노트 소리 일대일): {a['keysound_missing']}개 · 짝 없이 남은 플레이어 발음 {rep['player_starts_unused']}개")
    print(f"  판정·소리 불일치 (|잔차| > 1 ms): 새 소리 {a['mismatch_1ms']}개 · 긴 소리 {a['mismatch_long_1ms']}개" + (f" · 도장 − 실제 누름(봇 SendInput, 같은 레인·노트) {fmt(a['stamp_vs_press'])}, 1 ms 넘음 {a['stamp_off_1ms']}개, 50 ms 넘음 {a['stamp_over_50ms']}개, 짝 없음 {rep['stamp_unpaired']} · 봇 미전송 {rep['bot_not_sent']}" if a['stamp_vs_press'] else ""))
    if "integrity" in rep:
        g = rep["integrity"]
        print(f"  시간 이동 검사: 녹음 ↔ 재구성 어긋남 {g['lag_ms']} ms, 1초 창 {g['windows']}개(양 채널, 뺀 창 {g['excluded_windows']}) 중 어긋남이 바뀐 것 {g['shifted']}개 {g['shifts'][:5]} · 정규 상관 최소 {g['corr_min']}, 0.99 미만 {g['corr_below_0.99']}")
    return rep


def load_csv_text(t):
    return list(csv.DictReader(t.splitlines()))


if __name__ == "__main__":
    for a in [x for x in sys.argv[1:] if not x.startswith("--")]:
        analyze(pathlib.Path(a), audio="--no-audio" not in sys.argv)
