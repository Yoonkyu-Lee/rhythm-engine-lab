r"""렌더링한 곡과 게임 미리듣기(_pre.2dx 추출본)를 상호상관으로 비교.

python -I tools\compare_preview.py <render.wav> <preview.wav>

출력: 미리듣기가 렌더 안에서 시작하는 위치(초), 정규화 상관계수(전체),
      1초 창마다의 국소 지연(샘플) → 위치가 구간마다 흔들리는지(박 위치 복원 오류) 확인.
"""
import subprocess, sys
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
SR = 44100


def load(path):
    r = subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-i", path, "-f", "f32le", "-ac", "1",
                        "-ar", str(SR), "-"], capture_output=True)
    return np.frombuffer(r.stdout, "<f4").astype("f8")


def xcorr_lag(a, b):
    """b 가 a 안에서 시작하는 위치 (FFT 상호상관)."""
    n = 1 << (len(a) + len(b)).bit_length()
    c = np.fft.irfft(np.fft.rfft(a, n) * np.conj(np.fft.rfft(b, n)), n)
    lag = int(np.argmax(c[: len(a)]))
    return lag, c


def main():
    ren, pre = load(sys.argv[1]), load(sys.argv[2])
    # 거친 탐색: 1/8 해상도의 포락선 대신 그대로 (곡 길이 수 분이면 충분히 빠름)
    lag, _ = xcorr_lag(ren, pre)
    seg = ren[lag: lag + len(pre)]
    m = min(len(seg), len(pre))
    r = float(np.corrcoef(seg[:m], pre[:m])[0, 1])
    print(f"preview starts at {lag / SR:.4f} s in render, corr={r:.4f}, preview {len(pre) / SR:.1f}s")
    # 1초 창마다 국소 지연 (±20 ms 탐색)
    lags = []
    w, rng = SR, int(0.02 * SR)
    for k in range(0, m - w, w):
        p = pre[k: k + w]
        best, bl = -1, 0
        a = ren[lag + k - rng: lag + k + w + rng]
        if len(a) < w + 2 * rng or np.std(p) < 1e-4:
            continue
        c = np.correlate(a, p, "valid")
        bl = int(np.argmax(c)) - rng
        lr = float(np.corrcoef(a[bl + rng: bl + rng + w], p)[0, 1])
        lags.append((k // SR, bl, round(lr, 3)))
    print("per-second (sec, lag_samples, corr):")
    print(lags)


if __name__ == "__main__":
    main()
