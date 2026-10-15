"""오프라인 렌더(Rust 믹서)를 독립 구현과 대조한다 (2-1 완료 기준).

python -I -X utf8 cp2/tools/check_render.py <content 폴더> [--bms <변환 BMS>]

1. numpy 렌더: .rtc의 PCM 뱅크와 expected.json의 발음 목록만으로, 믹서 코드와 무관하게 float64로 섞는다.
   기대: 최대 차이가 더하는 순서 차이 수준(1e-5 이하). 시작 프레임 = 반올림(t × rate / 1e9), 반 프레임은 올림.
2. (선택) BMS 렌더: 같은 채보의 변환 BMS를 cp0/bms/converter/tools/bms_render.py로 렌더해 비교한다.
   BMS 쪽은 볼륨을 0.5 dB 단위로 묶은 사본과 ffmpeg 디코딩을 쓰므로 비트 일치가 아니라 상관도와 잔차로 본다.
결과는 <폴더>/render_check.json에도 남긴다.
"""
import argparse, json, pathlib, subprocess, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import rtc  # noqa: E402


def read_wav_f32(path):
    b = pathlib.Path(path).read_bytes()
    assert b[:4] == b"RIFF" and b[8:12] == b"WAVE"
    p, fmt = 12, None
    while p < len(b):
        cid, n = b[p:p + 4], int.from_bytes(b[p + 4:p + 8], "little")
        if cid == b"fmt ":
            fmt = b[p + 8:p + 8 + n]
        elif cid == b"data":
            tag, ch = int.from_bytes(fmt[0:2], "little"), int.from_bytes(fmt[2:4], "little")
            bits = int.from_bytes(fmt[14:16], "little")
            data = b[p + 8:p + 8 + n]
            if tag == 3 and bits == 32:
                return np.frombuffer(data, "<f4").reshape(-1, ch)
            if tag == 1 and bits == 16:
                return np.frombuffer(data, "<i2").reshape(-1, ch).astype(np.float32) / 32768
            raise SystemExit(f"지원하지 않는 WAV 형식 tag {tag} bits {bits}")
        p += 8 + n + (n & 1)
    raise SystemExit("data 청크 없음")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("content")
    ap.add_argument("--bms", default="")
    a = ap.parse_args()
    d = pathlib.Path(a.content)
    rtcfile = next(d.glob("*.rtc"))
    c = rtc.decode(rtcfile.read_bytes())
    ours = read_wav_f32(d / "render.wav").astype(np.float64)
    rate = c["rate"]
    exp = json.loads((d / "expected.json").read_text(encoding="utf-8"))["events"]
    ref = np.zeros((len(ours) + rate, 2))
    for t, s, g, _ in exp:
        f0 = (t * rate + NS // 2) // NS if t >= 0 else -((-t * rate + NS // 2) // NS)
        off, fr, _ = c["sounds"][s]
        seg = c["pcm"][off:off + fr].astype(np.float64) * np.float64(np.float32(g))
        ref[f0:f0 + fr] += seg
    ref = ref[:len(ours)]
    diff = np.abs(ours - ref)
    res = {"frames": len(ours), "numpy_max_abs_diff": float(diff.max()), "numpy_rms_diff": float(np.sqrt((diff ** 2).mean())),
           "peak": float(np.abs(ours).max())}
    ok = res["numpy_max_abs_diff"] <= 1e-5
    print(f"numpy 렌더와 최대 차이 {res['numpy_max_abs_diff']:.2e} (RMS {res['numpy_rms_diff']:.2e}) → {'일치' if ok else '불일치'}")
    if a.bms:
        out = d / "bms_render.wav"
        if not out.exists():
            subprocess.run([sys.executable, "-I", str(ROOT / "cp0/bms/converter/tools/bms_render.py"), a.bms, str(out), "--sr", str(rate)],
                           check=True, capture_output=True)
        theirs = read_wav_f32(out).astype(np.float64)
        n = min(len(theirs), len(ours))
        x, y = ours[:n].ravel(), theirs[:n].ravel()
        # 상대 위치 어긋남이 없는지: ±50프레임 범위 상호상관 최대 위치
        m = min(n, rate * 30)
        xs, ys = ours[:m, 0], theirs[:m, 0]
        lags = range(-50, 51)
        best = max(lags, key=lambda L: float(np.dot(xs[max(0, L):m + min(0, L)], ys[max(0, -L):m - max(0, L)])))
        corr = float(np.dot(x, y) / np.sqrt(np.dot(x, x) * np.dot(y, y)))
        k = float(np.dot(x, y) / np.dot(x, x))  # BMS 렌더는 전체 크기를 정규화하므로 배율을 먼저 맞춘다
        resid_db = float(10 * np.log10(np.sum((k * x - y) ** 2) / np.sum((k * x) ** 2)))
        res.update({"bms_frames": len(theirs), "bms_corr": corr, "bms_scale": k, "bms_residual_db": resid_db, "bms_best_lag_frames": best})
        print(f"BMS 렌더와 상관 {corr:.5f}, 배율 {k:.4f} 맞춘 뒤 잔차 {resid_db:.1f} dB, 어긋남 {best}프레임 (길이 {len(theirs)} / {len(ours)})")
    (d / "render_check.json").write_text(json.dumps(res, indent=1), encoding="utf-8")
    if not ok:
        sys.exit(1)


NS = rtc.NS

if __name__ == "__main__":
    main()
