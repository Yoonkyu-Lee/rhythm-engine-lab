r"""CP1 실측 결과를 그래프용 JSON 하나로 모은다 (렉 도감 4장, 이후 ASIO 재측정 비교용).

python -I cp1\tools\export_cp1.py [출력.json]   (기본: cp1\results\cp1_wasapi_shared.json)

어느 기록을 쓰는지는 아래 RUNS에 고정한다. 기록 폴더(runs\)는 저장소에 올리지 않고, 이 JSON만 올린다.
"""
import contextlib, csv, io, json, pathlib, sys
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import analyze_faults, analyze_input, analyze_sched  # noqa: E402

RUNS = HERE.parent / "runs"
CFG = {
    "step0": "step0-1791821264",
    "sched": {"sample": "sched-sample-la100-1791854233", "immediate_60": "sched-immediate-60fps-1791854295",
              "immediate_144": "sched-immediate-144fps-1791854357", "immediate_240": "sched-immediate-240fps-1791854419"},
    "faults": {"sample_cut": "sched-sample-la100-cut-1791856105", "sample_whole": "sched-sample-la100-whole-1791856157",
               "immediate_144": "sched-immediate-144fps-1791856209"},
    "long": {"sample_cut": "sched-sample-la100-cut-1791857376", "sample_whole": "sched-sample-la100-whole-1791857407"},
    "input": "input-1791857160",
    "clockjump": "sched-sample-la100-cut-1791856105",
}


def quiet(fn, *a):
    with contextlib.redirect_stdout(io.StringIO()):
        return fn(*a)


def decimate(t, v, bins):
    """구간마다 절댓값이 가장 큰 점을 남긴다 (튀는 값이 그래프에서 사라지지 않게)."""
    t, v = np.asarray(t), np.asarray(v)
    edges = np.linspace(t.min(), t.max(), bins + 1)
    out = []
    for i in range(bins):
        m = (t >= edges[i]) & (t < edges[i + 1])
        if m.any():
            j = np.flatnonzero(m)[np.argmax(np.abs(v[m]))]
            out.append([round(float(t[j]), 3), round(float(v[j]), 4)])
    return out


def step0_staircase():
    run = RUNS / CFG["step0"]
    meta = json.loads((run / "meta.json").read_text(encoding="utf-8"))
    R = meta["render"]
    with open(run / "render_packets.csv", newline="") as f:
        rows = list(csv.DictReader(f))
    pos = np.array([int(r["clock_pos"]) for r in rows], float) * R["rate"] / R["clock_freq"]
    q = np.array([int(r["clock_qpc100ns"]) for r in rows], float)
    ok = pos > R["rate"] * 0.05
    pos, q = pos[ok], q[ok]
    d, c = np.polyfit(pos, q, 1)
    t = (q - q[0]) / 1e7
    resid = (q - (c + d * pos)) / 1e4
    return {"run": run.name, "ppm": round(((1e7 / R["rate"]) / d - 1) * 1e6, 2),
            "resid_ms": decimate(t, resid, 600)}


def clockjump():
    run = RUNS / CFG["clockjump"]
    out = []
    for f in sorted(run.glob("clockjump_*.csv")):
        lines = f.read_text(encoding="utf-8").splitlines()
        name, epochs = lines[0][2:].split("; epochs=")
        arr = np.array([[float(x) for x in l.split(",")] for l in lines[2:]])
        out.append({"policy": name, "epochs_s": [float(x) for x in epochs.split()],
                    "err_ms": decimate(arr[:, 0], arr[:, 1], 700)})
    return {"run": run.name, "policies": out}


def main():
    dst = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else HERE.parent / "results" / "cp1_wasapi_shared.json"
    data = {
        "path": "WASAPI 공유 모드, 이벤트 구동, 44.1 kHz float32, 엔진 주기 10 ms, TOPPING E2x2 OTG (Playback 1/2 → 내부 Loopback 1/2)",
        "staircase": step0_staircase(),
        "sched": {k: {"run": v, "err_ms": [round(x, 3) for x in quiet(analyze_sched.analyze, RUNS / v)["err_ms"]]}
                  for k, v in CFG["sched"].items()},
        "faults": {k: quiet(analyze_faults.analyze, RUNS / v) for k, v in CFG["faults"].items()},
        "long": {k: quiet(analyze_faults.analyze, RUNS / v) for k, v in CFG["long"].items()},
        "input": quiet(analyze_input.analyze, RUNS / CFG["input"]),
        "clockjump": clockjump(),
    }
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    print(f"{dst} ({dst.stat().st_size / 1024:.0f} KB)")


if __name__ == "__main__":
    main()
