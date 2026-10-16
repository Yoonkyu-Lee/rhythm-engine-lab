"""실시간 재생 기록(cp2game play) 분석: 루프백 녹음을 정답 렌더와 맞춰 본다.

python -I -X utf8 cp2/tools/analyze_play.py <runs/...> [--render-exe cp2/target/release/render.exe]

1. 정답: 같은 곡을 같은 장치 속도로 오프라인 렌더(게임과 같은 리샘플러·믹서) × 마스터 이득. 곡 폴더에 캐시한다.
2. 전체 어긋남 L: 녹음[i] ≈ 정답[i − L] (앞 20초 FFT 상호상관).
3. 구간 검사: 2초 창을 1초 간격으로 옮기며 ±2,000프레임 안에서 어긋남을 다시 찾는다 → 밀림(언더런)·끊김 구간.
4. 일치도: 어긋남을 맞춘 뒤 잔차 (dB), 샘플이 비트 단위로 같은 비율.
5. 오디오 블록: 깨어난 간격, 섞기 시간, 버퍼 여유(padding)의 분포.
결과: <run>/summary.json
"""
import argparse, json, pathlib, subprocess, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
CP2 = HERE.parent
sys.path.insert(0, str(HERE))
from check_render import read_wav_f32  # noqa: E402


def xcorr_lag(a, b, max_lag):
    """a[i] ≈ b[i − L] 인 L (|L| ≤ max_lag). FFT 상호상관."""
    n = len(a) + len(b)
    size = 1 << (n - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(a, size) * np.conj(np.fft.rfft(b, size)), size)
    lags = np.concatenate([np.arange(0, max_lag + 1), np.arange(-max_lag, 0)])
    vals = np.concatenate([c[:max_lag + 1], c[size - max_lag:]])
    i = int(np.argmax(vals))
    return int(lags[i]), float(vals[i])


def reference(meta, render_exe):
    chart = pathlib.Path(meta["chart"])
    if not chart.is_absolute():
        chart = CP2 / chart
    ref = chart.parent / f"ref_{meta['rate']}.wav"
    if not ref.exists():
        subprocess.run([str(render_exe), str(chart), str(ref), "--rate", str(meta["rate"])], check=True, capture_output=True)
    return read_wav_f32(ref).astype(np.float32)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--render-exe", default=str(CP2 / "target" / "release" / "render.exe"))
    a = ap.parse_args()
    run = pathlib.Path(a.run)
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    rate, origin, gain = meta["rate"], meta["origin"], meta["master_gain"]
    origin += int(meta.get("sound_delay_frames", 0))  # 소리 시간축 = origin + D (CP2 2-7)
    if "gain" in (run / "mix_events.csv").open(encoding="utf-8").readline():
        # 믹서 사건 로그로 실제 출력 스트림을 재구성 (어떤 정책·입력이든 "냈다고 한 것"과 "나간 것"을 비교)
        rs = run / "restream.wav"
        if not rs.exists():
            chart = pathlib.Path(meta["chart"])
            chart = chart if chart.is_absolute() else CP2 / chart
            subprocess.run([str(pathlib.Path(a.render_exe).with_name("restream.exe")), str(chart), str(run / "mix_events.csv"), str(rate), str(gain), str(rs)],
                           check=True, capture_output=True)
        stream = read_wav_f32(rs).astype(np.float32)
        res_ref = "restream"
    else:
        ref = reference(meta, a.render_exe) * np.float32(gain)
        # 정답을 스트림 프레임 좌표로: 곡 시간 0 = 스트림 프레임 origin
        stream = np.zeros((origin + len(ref), 2), np.float32)
        stream[origin:] = ref
        res_ref = "perfect_render"
    cap = np.fromfile(run / "capture_2ch.f32", "<f4").reshape(-1, 2)
    res = {"run": run.name, "reference": res_ref, "rate": rate, "capture_frames": len(cap), "stream_frames": int(meta["written"])}

    # 2. 전체 어긋남: 녹음의 앞 20초를 스트림의 앞 22초와
    seg = min(len(cap), 20 * rate)
    L, _ = xcorr_lag(cap[:seg, 0].astype(np.float64), stream[:seg + 2 * rate, 0].astype(np.float64), 2 * rate)
    res["lag_frames"] = L
    res["lag_ms"] = L * 1000 / rate

    # 3. 장치 보고 밀림: 블록 기록의 (장치 위치, QPC) 쌍을 처음 20초의 직선과 비교한 부족분(프레임).
    #    언더런이 나면 장치 위치가 그만큼 계단처럼 뒤처진다 (CP1 3-1). 스트림 프레임 → 그 시점의 부족분.
    b = np.genfromtxt(run / "audio_blocks.csv", delimiter=",", names=True, dtype=np.int64)
    ok = b["dev_frame"] > 0
    tq, fr, wr = b["dev_qpc100"][ok] / 1e7, b["dev_frame"][ok].astype(np.float64), b["written"][ok]
    early = tq < tq[0] + 20
    slope, icpt = np.polyfit(tq[early], fr[early], 1)
    deficit = np.round(slope * tq + icpt - fr).astype(np.int64)
    # 공유 모드 위치 보고에는 1 ms 톱니가 있다(CP1 1단계). 2 ms보다 작은 변화는 밀림으로 보지 않는다.
    deficit[np.abs(deficit) < rate // 500] = 0

    def predicted(stream_frame):
        i = min(np.searchsorted(wr, stream_frame), len(deficit) - 1)
        return int(deficit[i])

    # 4. 구간 검사 (소리가 있는 창만): 장치 보고로 예측한 밀림 근처에서 녹음 어긋남을 찾는다. 두 단계:
    #    (가) 예측 ±5 ms에서 정밀하게 → 정규화 상관 ≥ 0.95면 채택. (나) 아니면 ±100 ms로 넓혀 찾고 "넓게 찾음"으로 표시.
    #    규칙적인 곡은 박 간격만큼 어긋나도 상관이 높아서, 처음부터 넓게 찾으면 엉뚱한 박에 걸린다.
    win, hop = 2 * rate, rate
    fine, wide = rate // 200, rate // 10
    segs, widened = [], 0

    def best_lag(r, c0, search):
        c = cap[c0 - search:c0 + win + search, 0].astype(np.float64)
        dl, _ = xcorr_lag(c, np.concatenate([np.zeros(search), r, np.zeros(search)]), search)
        cc = c[search + dl:search + dl + win]
        return dl, float(np.dot(cc, r) / (np.linalg.norm(cc) * np.linalg.norm(r) + 1e-30))

    for s0 in range(0, len(stream) - win, hop):
        pred = predicted(s0 + win // 2)
        if abs(predicted(s0) - predicted(s0 + win)) > rate // 500:  # 밀림 경계에 걸친 창은 어긋남이 둘이라 뺀다
            continue
        c0 = s0 + L + pred
        if c0 - wide < 0 or c0 + win + wide > len(cap):
            continue
        r = stream[s0:s0 + win, 0].astype(np.float64)
        if np.sqrt(np.mean(r * r)) < 1e-3:
            continue
        dl, nc = best_lag(r, c0, fine)
        if nc < 0.95:
            dl, nc = best_lag(r, c0, wide)
            widened += 1
        segs.append((s0, pred + dl, pred))
    res["windows_widened"] = widened
    res["report_vs_loopback_max_frames"] = int(max((abs(m - p) for _, m, p in segs), default=0))
    segs = [(s0, m) for s0, m, _ in segs]
    shifts = [(s0, d) for s0, d in segs if d != 0]
    res["windows"] = len(segs)
    res["windows_shifted"] = len(shifts)
    # 어긋남이 바뀌는 지점 (스트림 초, 새 추가 어긋남 프레임)
    changes, prev = [], 0
    for s0, d in segs:
        if d != prev:
            changes.append({"stream_s": round(s0 / rate, 2), "song_s": round((s0 - origin) / rate, 2), "extra_lag_frames": d, "extra_lag_ms": round(d * 1000 / rate, 2)})
            prev = d
    res["lag_changes"] = changes

    # 4. 경로 필터: 출력 경로에 장치·OS의 선형 EQ가 있으면 비트 일치가 불가능하다. 소리가 있는 첫 창에서 33탭 FIR을
    #    최소제곱으로 맞추고(지연이 없어야 가운데 탭이 최대), 정답에 적용한 뒤 잔차를 본다. 필터 모양은 기록한다.
    T, h = 33, 16
    fir = None
    A, Bv, used = np.zeros((T, T)), np.zeros(T), 0
    for s0, d in segs:  # 어긋남이 0인 창들을 1초씩, 최대 20개 모아 정규방정식을 쌓는다
        if d != 0 or s0 < origin + h or used >= 20:
            continue
        n = min(rate, len(stream) - s0 - h, len(cap) - s0 - L)
        r = stream[s0 - h:s0 + n + h, 0].astype(np.float64)
        c = cap[s0 + L:s0 + L + n, 0].astype(np.float64)
        X = np.stack([r[k:k + n] for k in range(T)], 1)[:, ::-1]
        A += X.T @ X
        Bv += X.T @ c
        used += 1
    if used:
        lam = 1e-6 * np.trace(A) / T  # 고음 성분이 적은 곡에서 해가 튀지 않게 (ridge)
        fir = np.linalg.solve(A + lam * np.eye(T), Bv)
        res["path_fir_windows"] = used
    if fir is not None:
        res["path_fir_center"] = float(fir[h])
        res["path_fir_peak_index"] = int(np.argmax(np.abs(fir))) - h
        H = np.abs(np.fft.rfft(fir, 4096))
        fr = np.fft.rfftfreq(4096, 1 / rate)
        res["path_gain_db"] = {f"{int(f0)}Hz": round(float(20 * np.log10(H[np.argmin(np.abs(fr - f0))])), 2) for f0 in (100, 1000, 5000, 10000, 15000, 20000)}
        eq = np.stack([np.convolve(stream[:, c], fir, mode="full")[h:h + len(stream)] for c in range(2)], 1).astype(np.float32)
    else:
        eq = stream

    # 5. 일치도: 어긋남이 0인 창들에서 잔차(경로 필터 보정 전·후)·비트 일치
    exact, total, err2, err2eq, sig2 = 0, 0, 0.0, 0.0, 0.0
    for s0, d in segs:
        if d != 0:
            continue
        r = stream[s0:s0 + hop].astype(np.float64)
        e = eq[s0:s0 + hop].astype(np.float64)
        c = cap[s0 + L:s0 + L + hop].astype(np.float64)
        exact += int(np.count_nonzero(stream[s0:s0 + hop] == cap[s0 + L:s0 + L + hop]))
        total += r.size
        err2 += float(np.sum((c - r) ** 2))
        err2eq += float(np.sum((c - e) ** 2))
        sig2 += float(np.sum(r ** 2))
    res["bit_exact_ratio"] = exact / total if total else None
    res["residual_db"] = float(10 * np.log10(err2 / sig2)) if sig2 and err2 else None
    res["residual_eq_db"] = float(10 * np.log10(err2eq / sig2)) if sig2 and err2eq else None

    # 6. 오디오 블록
    qf = meta["qpc_freq"]
    wake = np.diff(b["qpc_wake"][1:]) * 1000 / qf
    res["blocks"] = {
        "n": int(len(b)),
        "wake_ms_p50": float(np.percentile(wake, 50)), "wake_ms_p99": float(np.percentile(wake, 99)), "wake_ms_max": float(wake.max()),
        "fill_us_p50": float(np.percentile(b["fill_ns"], 50) / 1e3), "fill_us_p99": float(np.percentile(b["fill_ns"], 99) / 1e3), "fill_us_max": float(b["fill_ns"].max() / 1e3),
        "padding_min_frames": int(b["padding"][1:].min()),
        "voices_max": int(b["voices"].max()),
        "injected_ms": int(b["injected_ms"].sum()),
    }
    m = np.genfromtxt(run / "mix_events.csv", delimiter=",", names=True, dtype=None, encoding="utf-8")
    kinds = {k: int(np.sum(m["kind"] == k)) for k in ("start", "dropped", "overflow")}
    late = m["late"][m["kind"] == "start"].astype(np.int64)
    res["mix"] = {**kinds, "late_starts": int(np.sum(late > 0)), "late_max_frames": int(late.max()) if len(late) else 0}
    (run / "summary.json").write_text(json.dumps(res, indent=1, ensure_ascii=False), encoding="utf-8")
    bk = res["blocks"]
    print(f"{run.name}: 전체 어긋남 {L} 프레임 ({res['lag_ms']:.2f} ms), 창 {len(segs)}개 중 밀림 {len(shifts)}")
    for ch in changes:
        print(f"  곡 {ch['song_s']:.0f}초부터 추가 어긋남 {ch['extra_lag_ms']:+.2f} ms")
    print(f"  장치 위치 보고와 루프백의 밀림 차이 최대 {res['report_vs_loopback_max_frames']} 프레임")
    ber = res["bit_exact_ratio"]
    if ber is not None:
        print(f"  일치: 비트 동일 {ber * 100:.3f}%, 잔차 {res['residual_db']:.1f} dB → 경로 필터 보정 뒤 {res['residual_eq_db']:.1f} dB")
    if "path_gain_db" in res:
        print(f"  출력 경로 필터: 지연 {res['path_fir_peak_index']} 프레임, 이득 {res['path_gain_db']}")
    print(f"  블록 {bk['n']}: 깨어난 간격 p50 {bk['wake_ms_p50']:.2f} / p99 {bk['wake_ms_p99']:.2f} / 최대 {bk['wake_ms_max']:.2f} ms, "
          f"섞기 p99 {bk['fill_us_p99']:.0f} µs (최대 {bk['fill_us_max']:.0f}), 버퍼 여유 최소 {bk['padding_min_frames']} 프레임, 보이스 최대 {bk['voices_max']}")
    print(f"  믹서: 발음 {kinds['start']}, 늦음 {res['mix']['late_starts']} (최대 {res['mix']['late_max_frames']} 프레임), 버림 {kinds['dropped']}, 넘침 {kinds['overflow']}")


if __name__ == "__main__":
    main()
