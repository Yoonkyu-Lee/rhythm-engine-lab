"""게임 실행 기록(cp2game run) 분석.

python -I -X utf8 cp2/tools/analyze_run.py <runs/...> [--no-audio]

1. 판정: 노트 판정 오차(err = 입력의 청취 곡 시각 − 노트 시각) 분포와 등급. 봇이 오차 0으로 쳤으면 이것이 곧 엔진의 판정 시각 오차(M2 근사).
2. 입력 경로: 봇이 SendInput 한 QPC → 입력 도장 QPC(추정) → 판정 스레드가 처리한 QPC.
3. 키음: 입력(보낸 순간) → 그 키음의 첫 프레임이 장치 기준점을 지나는 시각(장치 시계) = M8 근사. 믹서 지각량(프레임)도.
   (장치 기준점 → 실제 귀까지의 고정 지연은 경로 교정값이라 별도. 루프백 왕복으로 잰다.)
4. 화면: 프레임 간격, present 시간, DXGI 통계 유무, 고장 주입 흔적.
5. 키음 vs 음악: 정확히 쳤다면(프로필이 매긴 오차 0) 그 키음이 음악보다 몇 ms 늦게/이르게 나오나.
   음악 = 같은 곡 시각의 자동재생이 실제로 놓인 샘플 (새로 울린 소리들로 보간) / 그 순간 이미 울리던 긴 소리의 해당 위치.
   올바른 리듬게임이면 0이어야 한다 (CP2 2-7).
6. 고장 구간: 고장마다 그 뒤 3초 동안 차트↔오디오 어긋남(기준 대비 최대·5초 뒤 잔여), 자동재생 발음 오차의 변화, 판정 등급.
   오디오 쪽 곡 시각 = 블록마다 (써 넣은 프레임 − 남은 패딩) — 스트림 기준이라 언더런 동안 멈춘 것까지 그대로 담는다.
6. 오디오 출력 무결성: analyze_play.py (믹서 사건 로그로 재구성한 스트림 ↔ 루프백).
결과: <run>/report.json
"""
import argparse, json, pathlib, subprocess, sys

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

HERE = pathlib.Path(__file__).resolve().parent


def pct(x, ps=(50, 90, 99)):
    x = np.asarray(x, dtype=np.float64)
    if x.size == 0:
        return {}
    d = {f"p{p}": round(float(np.percentile(x, p)), 3) for p in ps}
    d.update(mean=round(float(x.mean()), 3), std=round(float(x.std()), 3), min=round(float(x.min()), 3), max=round(float(x.max()), 3), n=int(x.size))
    return d


def load(path, **kw):
    return np.genfromtxt(path, delimiter=",", names=True, dtype=np.int64, **kw)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--no-audio", action="store_true")
    a = ap.parse_args()
    run = pathlib.Path(a.run)
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    qf, rate = meta["qpc_freq"], meta["rate"]
    ck = meta["clock"]

    def frame_to_qpc(f):  # 장치 시계(마지막 직선)로 스트림 프레임이 나가는 QPC
        x = (f - ck["f0"] - ck["a"]) / ck["b"]
        t100 = ck["t0"] + x * 1e7
        return t100 * qf / 1e7

    origin = meta["origin"]
    sd = int(meta.get("sound_delay_frames", 0))  # 소리 지연 D: 자동재생은 origin + D 기준
    out_off = meta.get("out_offset_ns", 0)

    def qpc_to_song_ns(q):  # 장치 시계(마지막 직선)로 QPC → 청취 곡 시각
        t100 = q * 1e7 / qf
        frame = ck["f0"] + ck["a"] + ck["b"] * (t100 - ck["t0"]) * 1e-7
        return (frame - origin) / rate * 1e9 + out_off

    # 스트림 기준 청취 곡 시각: 블록마다 (써 넣은 프레임 − 남은 패딩)를 QPC로 보간 + 상수 (마지막 15초에서 장치 시계의 마지막 직선과 맞춘 값).
    # 마지막 시계 직선 하나로는 오디오 고장(시계 점프) 앞뒤를 같이 잴 수 없다.
    ab = load(run / "audio_blocks.csv")
    ab = ab[ab["written"] > 0]
    ab_pos = (ab["written"] - ab["padding"]).astype(np.float64)
    ab_q = ab["qpc_wake"].astype(np.float64)
    _qs = ab_q[ab_q > ab_q[-1] - 15 * qf]
    _c = float(np.median([qpc_to_song_ns(q) - ((np.interp(q, ab_q, ab_pos) - origin) / rate * 1e9 + out_off) for q in _qs])) if len(_qs) else 0.0

    def heard_ns(q):  # QPC → 청취 곡 시각 (스트림 기준, 고장 앞뒤 모두)
        return (np.interp(q, ab_q, ab_pos) - origin) / rate * 1e9 + out_off + _c

    _cq = _c * qf / 1e9

    def stream_to_qpc(fr):  # 스트림 프레임이 장치 기준점을 지나는 QPC (스트림 기준, 고장 앞뒤 모두)
        return np.interp(fr, ab_pos, ab_q) - _cq

    rep = {"run": run.name, "profile": meta.get("profile"), "slew": meta.get("slew"), "tally": meta.get("tally")}
    # 자동재생 발음 오차 (M1): 예정 프레임 vs 믹서가 실제로 놓은 프레임 (순서대로 짝)
    exp_path = pathlib.Path(meta["chart"])
    exp_path = (exp_path if exp_path.is_absolute() else HERE.parent / exp_path).parent / "expected.json"
    if exp_path.exists():
        exp = json.loads(exp_path.read_text(encoding="utf-8"))["events"]
        auto_t = [t for t, s, g, src in exp if src == "auto"]
        auto_s = [s for t, s, g, src in exp if src == "auto"]
        mm = np.genfromtxt(run / "mix_events.csv", delimiter=",", names=True, dtype=None, encoding="utf-8")
        st = mm[(mm["kind"] == "start") & (mm["player"] == 0)]["start"].astype(np.int64)
        n = min(len(st), len(auto_t))
        want = np.array([origin + sd + int(np.floor(t * rate / 1e9 + 0.5)) for t in auto_t[:n]])
        auto_T, auto_F, auto_S = np.array(auto_t[:n], dtype=np.float64), st[:n].astype(np.float64), np.array(auto_s[:n])
        rep["autoplay_err_ms"] = pct((st[:n] - want) * 1e3 / rate, (50, 99, 99.9))
        m1 = (np.array(auto_t[:n]) / 1e9, (st[:n] - want) * 1e3 / rate)

    j = load(run / "judgments.csv")
    hits = j[j["kind"] == 0]
    rep["judge_err_ms"] = pct(hits["err_ns"] / 1e6)
    rep["judge_proc_ms"] = pct((j["q_proc"] - j["q_in"])[np.isin(j["kind"], [0, 1, 5])] * 1e3 / qf)
    rep["miss"] = int(np.sum(j["kind"] == 2))
    rep["violations"] = int(np.sum(j["kind"] == 6))

    bot_path = run / "bot.csv"
    if bot_path.exists() and bot_path.stat().st_size > 60:
        b = load(bot_path)
        downs = b[b["down"] == 1]
        # 봇 누름과 판정의 누름(노트·빈 누름)을 순서대로 짝짓는다 (F24만)
        presses = j[np.isin(j["kind"], [0, 1]) & (j["vkey"] == 135)]
        n = min(len(downs), len(presses))
        rep["input_stamp_minus_sent_ms"] = pct((presses["q_in"][:n] - downs["q_sent"][:n]) * 1e3 / qf)
        rep["bot_send_minus_target_ms"] = pct((downs["q_sent"] - downs["q_target"]) * 1e3 / qf)
        # 키음: 이 누름의 요청 프레임 → 믹서가 실제로 놓은 시작 프레임 → 장치 시각
        m = np.genfromtxt(run / "mix_events.csv", delimiter=",", names=True, dtype=None, encoding="utf-8")
        ps = m[(m["kind"] == "start") & (m["player"] == 1)]
        due = ps["due"].astype(np.int64)
        start = ps["start"].astype(np.int64)
        lat_frames, m8, m8_q = [], [], []
        sent_by_frame = {}
        for k in range(n):
            if presses["key_frame"][k] >= 0:
                sent_by_frame.setdefault(int(presses["key_frame"][k]), int(downs["q_sent"][k]))
        if sent_by_frame:
            for d, s in zip(due, start):
                if d in sent_by_frame:
                    lat_frames.append(s - d)
                    m8.append((stream_to_qpc(s) - sent_by_frame[d]) * 1e3 / qf)
                    m8_q.append(sent_by_frame[d])
        else:
            # 프레임 구동: 키음은 "지금 울려"라 예정 프레임이 없다 → 누름마다 그 뒤 처음 나온 플레이어 소리와 짝짓는다
            sq = np.sort(stream_to_qpc(start.astype(np.float64)))
            for k in range(len(downs)):
                i = int(np.searchsorted(sq, downs["q_sent"][k]))
                if i < len(sq):
                    m8.append((sq[i] - downs["q_sent"][k]) * 1e3 / qf)
                    m8_q.append(int(downs["q_sent"][k]))
        rep["keysound_mixer_late_ms"] = pct(np.array(lat_frames) * 1e3 / rate)
        # 실제로 누른 시각의 오차(장치 시계 기준) vs 프로필이 매긴 오차 → 프로필의 판정 시각 오차
        notes_t = {}
        exp_path = pathlib.Path(meta["chart"]).parent / "expected.json"
        exp_path = exp_path if exp_path.is_absolute() else HERE.parent / exp_path
        hit_rows = j[j["kind"] == 0]
        sent_by_note = {int(o["note"]): int(o["q_sent"]) for o in downs}
        true_err, judged_err, hit_q = [], [], []
        for r in hit_rows:
            q = sent_by_note.get(int(r["note"]))
            if q is None:
                continue
            judged_err.append(r["err_ns"] / 1e6)
            note_t = r["t_ns"] - r["err_ns"]
            true_err.append((heard_ns(q) - note_t) / 1e6)
            hit_q.append(q)
        if true_err:
            te, je = np.array(true_err), np.array(judged_err)
            rep["press_true_err_ms"] = pct(te)
            rep["judging_error_ms"] = pct(je - te)
            m2 = (np.array(hit_q, dtype=np.float64), je - te)

        rep["keysound_sent_to_device_ms"] = pct(m8)

        # 키음 vs 음악 (정확히 쳤을 때)
        if "auto_T" in locals() and len(ps):
            import rtc as _rtc
            cp = pathlib.Path(meta["chart"])
            cp = cp if cp.is_absolute() else HERE.parent / cp
            cd = _rtc.decode(cp.read_bytes())
            snd_len_s = np.array([fr / cd["rate"] for _, fr, _ in cd["sounds"]])
            is_long = np.array([k for _, _, k in cd["sounds"]])
            short = ~is_long[auto_S]
            ks_q = np.sort(stream_to_qpc(start.astype(np.float64)))
            ks_f = np.sort(start.astype(np.float64))
            new_d, long_d, ks_qin = [], [], []
            ks_by_due = {int(d): int(s_) for d, s_ in zip(due, start)}
            for r in j[np.isin(j["kind"], [0, 7]) & (j["grade"] < 4)]:
                if r["key_frame"] >= 0:
                    # 우리 방식: 요청한 예약 프레임으로 정확히 짝 (버려진 키음은 짝 없음 → 제외)
                    if int(r["key_frame"]) not in ks_by_due:
                        continue
                    i = int(np.searchsorted(ks_f, ks_by_due[int(r["key_frame"])]))
                else:
                    # 프레임 구동: 누름 뒤 250 ms 안의 첫 플레이어 소리
                    i = int(np.searchsorted(ks_q, r["q_in"]))
                    if i >= len(ks_f) or ks_q[i] - r["q_in"] > qf // 4:
                        continue
                T = float(r["t_ns"] - r["err_ns"])  # 노트 시각
                ideal = ks_f[i] - r["err_ns"] / 1e9 * rate  # 프로필 기준 오차 0으로 쳤다면 키음이 놓였을 프레임
                # 새로 울린 짧은 소리들의 (곡 시각 → 실제 프레임)으로 노트 시각의 음악 위치를 보간
                mf = np.interp(T, auto_T[short], auto_F[short])
                new_d.append((ideal - mf) * 1e3 / rate)
                # 그 순간 이미 울리던 긴 소리: 시작이 T 이전이고 아직 안 끝난 것 중 가장 최근
                L = np.nonzero(is_long[auto_S] & (auto_T <= T) & (auto_T + snd_len_s[auto_S] * 1e9 > T))[0]
                if L.size:
                    k = L[-1]
                    long_d.append((ideal - (auto_F[k] + (T - auto_T[k]) / 1e9 * rate)) * 1e3 / rate)
                else:
                    long_d.append(np.nan)
                ks_qin.append(r["q_in"])
            rep["ks_vs_music_ms"] = pct(new_d)
            ld = np.array(long_d)
            rep["ks_vs_long_ms"] = pct(ld[~np.isnan(ld)])
            ks_series = (np.array(ks_qin, dtype=np.float64), np.array(new_d), ld)

    f = load(run / "frames.csv")
    we = f["q_wait_end"]
    rep["frame_interval_ms"] = pct(np.diff(we) * 1e3 / qf, (50, 99, 99.9))
    rep["present_ms"] = pct((f["q_present_end"] - f["q_present_start"]) * 1e3 / qf)
    rep["dxgi_stats_frames"] = int(np.sum(f["sync_qpc"] > 0))
    rep["fps_mean"] = round(float(len(we) / ((we[-1] - we[0]) / qf)), 2) if len(we) > 1 else None
    rep["render_injected_ms"] = int(f["injected_ms"].sum())
    fl = run / "faults.csv"
    rep["faults"] = [ln for ln in fl.read_text(encoding="utf-8").splitlines()[1:]] if fl.exists() else []

    # 렉 감지: 주입(faults.csv)과 감지(lag_events.csv)를 맞댄다
    lag_path = run / "lag_events.csv"
    if lag_path.exists():
        names = {0: "AudioStall", 1: "Underrun", 2: "ClockJump", 3: "RenderStall", 4: "JudgeStall", 5: "InputStall", 6: "State", 7: "ResyncDone"}
        want = {"Audio": 0, "Render": 3, "Judge": 4, "Input": 5}
        ev = [ln.split(",") for ln in lag_path.read_text(encoding="utf-8").splitlines()[1:]]
        ev = [(int(k), int(qs), int(qd), int(qe), int(mg)) for k, _, qs, qd, qe, mg in ev]
        used, matches = set(), []
        for ln in rep["faults"]:
            kind, at_s, ms_, q, src = ln.split(",")
            q, ms_ = int(q), int(ms_)
            k = want[kind]
            cand = [i for i, e in enumerate(ev) if e[0] == k and i not in used and e[1] >= q - qf // 20 and e[1] <= q + qf]
            m = {"kind": kind, "at_song_s": float(at_s), "injected_ms": ms_}
            if cand:
                i = cand[0]
                used.add(i)
                e = ev[i]
                m.update(detected=True, detect_after_stall_ms=round((e[2] - e[1]) * 1e3 / qf, 2), detect_after_inject_ms=round((e[2] - q) * 1e3 / qf, 2),
                         measured_ms=round(e[4] / 1e3, 1))
                if kind == "Audio":
                    jumps = [x for x in ev if x[0] == 2 and x[1] >= e[1] and x[1] <= e[1] + qf]
                    unders = [x for x in ev if x[0] == 1 and x[1] >= e[1] - qf // 100 and x[1] <= e[1] + qf]
                    done = [x for x in ev if x[0] == 7 and x[1] >= e[1] and x[1] <= e[1] + 3 * qf]
                    m.update(underrun_seen=bool(unders), clock_jump_ms=round(jumps[0][4] / 1e3, 2) if jumps else None,
                             clock_jump_after_inject_ms=round((jumps[0][2] - q) * 1e3 / qf, 1) if jumps else None,
                             resync_total_ms=round(done[0][4] / 1e3, 2) if done else None,
                             resync_done_after_inject_ms=round((done[0][2] - q) * 1e3 / qf, 1) if done else None)
                    for x in jumps[:1] + unders[:1]:
                        used.add(ev.index(x))
            else:
                m.update(detected=False)
            matches.append(m)
        # 프레임 구동 프로필은 판정이 화면 스레드에 있다: 화면 멈춤과 판정 멈춤은 같은 멈춤을 두 심장박동이 본 것 (오탐 아님)
        if rep["profile"] not in (None, "ours"):
            for i, e in enumerate(ev):
                if e[0] in (3, 4) and i not in used:
                    if any(x[0] in (3, 4) and x is not e and abs(x[1] - e[1]) < qf // 10 for x in ev) or any(
                            abs(int(ln.split(",")[3]) - e[1]) < qf // 10 for ln in rep["faults"] if ln.split(",")[0] in ("Render", "Judge")):
                        used.add(i)
        stalls = [i for i, e in enumerate(ev) if e[0] in (0, 3, 4, 5) and i not in used]
        states = [e for e in ev if e[0] == 6]
        rep["lag"] = {"matches": matches, "unmatched_stalls": [{"kind": names[ev[i][0]], "ms": round(ev[i][4] / 1e3, 1)} for i in stalls],
                      "state_changes": len(states)}
        jstate = j["state"] if "state" in j.dtype.names else np.zeros(len(j), int)
        rep["judgments_not_running"] = int(np.sum(jstate != 0))

    # 고장 구간
    if rep["faults"]:
        qref = f["q_display"] if rep["profile"] in (None, "ours") else f["q_wait_end"]
        ok = np.abs(f["song_ns"]) < 10**15
        aud = heard_ns(qref)
        desync = np.where(ok, (f["song_ns"] - aud) / 1e6, np.nan)  # 차트 − 오디오 (ms, +면 차트가 앞섬)
        chart_s = f["song_ns"] / 1e9
        base_mask = ok & (chart_s > 5) & (chart_s < 15)
        base = float(np.nanmedian(desync[base_mask])) if base_mask.any() else 0.0
        rep["desync_base_ms"] = round(base, 2)
        first_at = min(float(ln.split(",")[1]) for ln in rep["faults"])
        m1_base = float(np.median(m1[1][m1[0] < first_at])) if "m1" in locals() and np.any(m1[0] < first_at) else None
        wins = []
        for ln in rep["faults"]:
            kind, at_s, ms_, q, src = ln.split(",")
            q, at_s = int(q), float(at_s)
            w = (qref >= q) & (qref < q + 3 * qf) & ok
            after = (qref >= q + 3 * qf) & (qref < q + 5 * qf) & ok
            d = desync[w] - base
            r = {"kind": kind, "ms": int(ms_), "at_s": at_s}
            if d.size:
                i = int(np.nanargmax(np.abs(d)))
                r["desync_peak_ms"] = round(float(d[i]), 1)
            if after.any():
                r["desync_after_ms"] = round(float(np.nanmedian(desync[after]) - base), 1)
            if m1_base is not None:
                mw = (m1[0] >= at_s) & (m1[0] < at_s + 3)
                if mw.any():
                    dm = m1[1][mw] - m1_base
                    r["m1_peak_ms"] = round(float(dm[np.argmax(np.abs(dm))]), 1)
                    r["m1_off_5ms"] = int(np.sum(np.abs(dm) > 5))
            jw = j[(j["q_proc"] >= q) & (j["q_proc"] < q + 3 * qf)]
            g = jw[jw["kind"] == 0]["grade"]
            r["grades"] = [int(np.sum(g == k)) for k in range(4)] + [int(np.sum(jw["kind"] == 2))]
            # 10초 창: 판정 시각 오차(M2), 복구 시간(M3), 화면 불연속(M4), 키음 지연(M8)
            W = q + 10 * qf
            if "m2" in locals():
                mw = (m2[0] >= q) & (m2[0] < W)
                pre = m2[0] < q
                b2 = float(np.median(m2[1][pre])) if pre.any() else 0.0
                if mw.any():
                    d2 = m2[1][mw] - b2
                    r["m2_peak_ms"] = round(float(d2[np.argmax(np.abs(d2))]), 1)
                    late = mw & (m2[0] >= q + 5 * qf)
                    r["m2_after_ms"] = round(float(np.median(m2[1][late]) - b2), 1) if late.any() else None
            ww = (qref >= q) & (qref < W) & ok
            dw = desync[ww] - base
            if dw.size:
                # 복구 = 어긋남이 기준에서 10 ms 안으로 돌아와 창 끝까지 머문 시점 (끝까지 못 돌아오면 None)
                bad = np.nonzero(np.abs(dw) > 10)[0]
                if bad.size == 0:
                    r["m3_recover_ms"] = 0.0
                elif bad[-1] == dw.size - 1:
                    r["m3_recover_ms"] = None  # 창 끝까지 5 ms 밖 = 복구 안 됨
                else:
                    r["m3_recover_ms"] = round(float((qref[ww][bad[-1] + 1] - q) * 1e3 / qf), 0)
            ii = np.nonzero(ww)[0]
            ii = ii[ii > 0]
            if ii.size:
                jump = ((f["song_ns"][ii] - f["song_ns"][ii - 1]) - (qref[ii] - qref[ii - 1]) * 1e9 / qf) / 1e6
                jump = jump[ok[ii - 1]]
                if jump.size:
                    r["m4_jump_ms"] = round(float(jump[np.argmax(np.abs(jump))]), 1)
            if "ks_series" in locals():
                kq, kn, kl = ks_series
                for key, arr in (("ks_new", kn), ("ks_long", kl)):
                    mw = (kq >= q) & (kq < W) & ~np.isnan(arr)
                    late = mw & (kq >= q + 5 * qf)
                    if mw.any():
                        x = arr[mw]
                        r[key + "_peak_ms"] = round(float(x[np.argmax(np.abs(x))]), 1)
                        r[key + "_after_ms"] = round(float(np.median(arr[late])), 1) if late.any() else None
            if "m8_q" in locals() and m8_q:
                aq, av = np.array(m8_q, dtype=np.float64), np.array(m8)
                pre = aq < q
                b8 = float(np.median(av[pre])) if pre.any() else 0.0
                mw = (aq >= q) & (aq < W)
                if mw.any():
                    r["m8_ms"] = round(b8, 1)
                    r["m8_peak_ms"] = round(float(av[mw].max()), 1)
                    r["keysound_dropped"] = 0
            wins.append(r)
        rep["fault_windows"] = wins

    if not a.no_audio and (run / "capture_2ch.f32").exists():
        r = subprocess.run([sys.executable, "-I", "-X", "utf8", str(HERE / "analyze_play.py"), str(run)], capture_output=True, text=True, encoding="utf-8")
        print(r.stdout.strip())
        if (run / "summary.json").exists():
            rep["audio"] = json.loads((run / "summary.json").read_text(encoding="utf-8"))
    (run / "report.json").write_text(json.dumps(rep, indent=1, ensure_ascii=False), encoding="utf-8")

    t = rep["tally"] or {}
    print(f"판정: P {t.get('perfect')} G {t.get('great')} Good {t.get('good')} Bad {t.get('bad')} Miss {t.get('miss')} | 오차 {fmt(rep['judge_err_ms'])}")
    print(f"  판정 처리 지연 {fmt(rep['judge_proc_ms'])}, watermark 위반 {rep['violations']}")
    if "input_stamp_minus_sent_ms" in rep:
        print(f"입력 경로: 봇 송신 정확도 {fmt(rep['bot_send_minus_target_ms'])}, 송신 → 입력 도장 {fmt(rep['input_stamp_minus_sent_ms'])}")
        print(f"키음: 믹서 지각 {fmt(rep['keysound_mixer_late_ms'])}, 송신 → 장치 출력 {fmt(rep['keysound_sent_to_device_ms'])}")
    if "lag" in rep:
        for m in rep["lag"]["matches"]:
            if m["detected"]:
                extra = (f", 언더런 {'예' if m.get('underrun_seen') else '아니오'}, 첫 시계 점프 {m.get('clock_jump_ms')} ms ({m.get('clock_jump_after_inject_ms')} ms 뒤),"
                         f" 재동기화 완료 총 밀림 {m.get('resync_total_ms')} ms ({m.get('resync_done_after_inject_ms')} ms 뒤)") if m["kind"] == "Audio" else ""
                print(f"렉 {m['kind']} {m['injected_ms']} ms @ {m['at_song_s']:.1f}s: 감지 (멈춘 뒤 {m['detect_after_stall_ms']} ms), 잰 길이 {m['measured_ms']} ms{extra}")
            else:
                print(f"렉 {m['kind']} {m['injected_ms']} ms @ {m['at_song_s']:.1f}s: 감지 못함")
        print(f"  짝 없는 감지(오탐 후보) {rep['lag']['unmatched_stalls']}, 상태 변화 {rep['lag']['state_changes']}회, Running 아닌 동안의 판정 {rep['judgments_not_running']}")
    if "autoplay_err_ms" in rep:
        print(f"자동재생 발음 오차(M1): {fmt(rep['autoplay_err_ms'])}")
    if "ks_vs_music_ms" in rep:
        print(f"키음 vs 음악 (정확히 쳤을 때, +면 키음이 늦음): 새로 울린 소리 대비 {fmt(rep['ks_vs_music_ms'])} | 이미 울리던 긴 소리 대비 {fmt(rep['ks_vs_long_ms'])}")
    if "judging_error_ms" in rep:
        print(f"실제 누른 시각 오차 {fmt(rep['press_true_err_ms'])} → 프로필 판정 시각 오차 {fmt(rep['judging_error_ms'])}")
    if "fault_windows" in rep:
        print(f"고장 구간 (기준 어긋남 {rep['desync_base_ms']} ms 대비, 차트 − 오디오, +면 차트가 앞섬):")
        for r in rep["fault_windows"]:
            g = r["grades"]
            print(f"  {r['kind']} {r['ms']} ms @ {r['at_s']:.0f}s: 어긋남 최대 {r.get('desync_peak_ms')} → 5초 뒤 {r.get('desync_after_ms')} ms | "
                  f"자동재생 오차 변화 최대 {r.get('m1_peak_ms')} ms (5 ms 넘음 {r.get('m1_off_5ms')}개) | 3초 판정 P{g[0]} G{g[1]} Good{g[2]} Bad{g[3]} Miss{g[4]}")
            if "ks_new_peak_ms" in r:
                print(f"    키음 vs 음악: 새 소리 대비 최대 {r.get('ks_new_peak_ms')} → 5~10초 {r.get('ks_new_after_ms')} ms | 긴 소리 대비 최대 {r.get('ks_long_peak_ms')} → 5~10초 {r.get('ks_long_after_ms')} ms")
            print(f"    판정 시각 오차 변화 최대 {r.get('m2_peak_ms')} → 5~10초 {r.get('m2_after_ms')} ms | 복구 {r.get('m3_recover_ms')} ms | 화면 튐 최대 {r.get('m4_jump_ms')} ms | 키음 지연 {r.get('m8_ms')} → 최대 {r.get('m8_peak_ms')} ms")
    print(f"화면: {rep['fps_mean']} fps, 프레임 간격 {fmt(rep['frame_interval_ms'])}, present {fmt(rep['present_ms'])}, DXGI 통계 {rep['dxgi_stats_frames']}/{len(we)} 프레임")


def fmt(d):
    if not d:
        return "-"
    keys = [k for k in ("mean", "p50", "p99", "p99.9", "max") if k in d]
    return " ".join(f"{k} {d[k]:.2f}" for k in keys) + " ms"


if __name__ == "__main__":
    main()
