"""게임 실행 기록(cp2game run) 분석.

python -I -X utf8 cp2/tools/analyze_run.py <runs/...> [--no-audio]

1. 판정: 노트 판정 오차(err = 입력의 청취 곡 시각 − 노트 시각) 분포와 등급. 봇이 오차 0으로 쳤으면 이것이 곧 엔진의 판정 시각 오차(M2 근사).
2. 입력 경로: 봇이 SendInput 한 QPC → 입력 도장 QPC(추정) → 판정 스레드가 처리한 QPC.
3. 키음: 입력(보낸 순간) → 그 키음의 첫 프레임이 장치 기준점을 지나는 시각(장치 시계) = M8 근사. 믹서 지각량(프레임)도.
   (장치 기준점 → 실제 귀까지의 고정 지연은 경로 교정값이라 별도. 루프백 왕복으로 잰다.)
4. 화면: 프레임 간격, present 시간, DXGI 통계 유무, 고장 주입 흔적.
5. 오디오 출력 무결성: analyze_play.py (믹서 사건 로그로 재구성한 스트림 ↔ 루프백).
결과: <run>/report.json
"""
import argparse, json, pathlib, subprocess, sys

import numpy as np

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

    rep = {"run": run.name, "profile": meta.get("profile"), "tally": meta.get("tally")}
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
        lat_frames, m8 = [], []
        sent_by_frame = {}
        for k in range(n):
            if presses["key_frame"][k] >= 0:
                sent_by_frame.setdefault(int(presses["key_frame"][k]), int(downs["q_sent"][k]))
        for d, s in zip(due, start):
            if d in sent_by_frame:
                lat_frames.append(s - d)
                m8.append((frame_to_qpc(s) - sent_by_frame[d]) * 1e3 / qf)
        rep["keysound_mixer_late_ms"] = pct(np.array(lat_frames) * 1e3 / rate)
        rep["keysound_sent_to_device_ms"] = pct(m8)

    f = load(run / "frames.csv")
    we = f["q_wait_end"]
    rep["frame_interval_ms"] = pct(np.diff(we) * 1e3 / qf, (50, 99, 99.9))
    rep["present_ms"] = pct((f["q_present_end"] - f["q_present_start"]) * 1e3 / qf)
    rep["dxgi_stats_frames"] = int(np.sum(f["sync_qpc"] > 0))
    rep["fps_mean"] = round(float(len(we) / ((we[-1] - we[0]) / qf)), 2) if len(we) > 1 else None
    rep["render_injected_ms"] = int(f["injected_ms"].sum())
    fl = run / "faults.csv"
    rep["faults"] = [ln for ln in fl.read_text(encoding="utf-8").splitlines()[1:]] if fl.exists() else []

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
    print(f"화면: {rep['fps_mean']} fps, 프레임 간격 {fmt(rep['frame_interval_ms'])}, present {fmt(rep['present_ms'])}, DXGI 통계 {rep['dxgi_stats_frames']}/{len(we)} 프레임")


def fmt(d):
    if not d:
        return "-"
    keys = [k for k in ("mean", "p50", "p99", "p99.9", "max") if k in d]
    return " ".join(f"{k} {d[k]:.2f}" for k in keys) + " ms"


if __name__ == "__main__":
    main()
