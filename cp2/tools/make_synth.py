"""합성 시험 곡 → content/synth/synth.rtc (+ expected.json: 모든 발음의 정답 시각).

python -I -X utf8 cp2/tools/make_synth.py [--bars 64] [--bpm 120] [--rate 44100]

구성 (4/4, 기본 120 BPM, 64마디 ≈ 128초):
- 건반 (레인 0): 매 박 노트 (4비트 축연타). 소리는 짧은 클릭(1 kHz 톤 + 감쇠, 30 ms). 8마디마다 한 번은 화음 노트(클릭 + 낮은 톤).
- 자동재생: 8분 뒷박 하이햇(짧은 잡음 20 ms), 마디마다 베이스(400 ms, 이득을 마디마다 바꿈),
  4마디마다 긴 패드(9초, LONG — 늦으면 앞을 잘라 맞춰야 하는 소리).
- 같은 소리가 겹치는 구간: 패드 9초 > 4마디 8초 → 매 4마디 1초씩 같은 패드가 두 번 겹친다 (겹치기 정책 확인용).
노트 시각은 박의 정확한 ns (BPM 120이면 정수로 나눠떨어진다). 루프백 분석에서 클릭 시작을 찾기 쉽게 시작은 급하게 만든다.
"""
import argparse, json, pathlib, sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import rtc  # noqa: E402

NS = rtc.NS


def tone(rate, secs, freq, decay, amp, noise=0.0, seed=0):
    t = np.arange(int(rate * secs)) / rate
    x = np.sin(2 * np.pi * freq * t) if freq else np.zeros_like(t)
    if noise:
        x = x + noise * np.random.default_rng(seed).standard_normal(len(t))
    x = amp * x * np.exp(-t / decay)
    fade = min(len(x), int(rate * 0.002))  # 끝 2 ms 페이드 (딸깍 방지)
    x[-fade:] *= np.linspace(1, 0, fade)
    return np.stack([x, x], 1).astype(np.float32)


def pad(rate, secs):
    t = np.arange(int(rate * secs)) / rate
    env = np.minimum(1, t / 0.3) * np.minimum(1, (secs - t) / 1.0)
    x = 0.12 * env * (np.sin(2 * np.pi * 220 * t) + 0.5 * np.sin(2 * np.pi * 330 * t))
    return np.stack([x, x * 0.9], 1).astype(np.float32)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bars", type=int, default=64)
    ap.add_argument("--bpm", type=float, default=120.0)
    ap.add_argument("--rate", type=int, default=44100)
    ap.add_argument("--out", default=str(HERE.parent / "content" / "synth"))
    a = ap.parse_args()
    rate, beat_ns = a.rate, round(60 * NS / a.bpm)
    clips = [
        tone(rate, 0.030, 1000, 0.010, 0.6),            # 0 클릭 (건반)
        tone(rate, 0.060, 150, 0.040, 0.5),             # 1 낮은 톤 (화음 노트)
        tone(rate, 0.020, 0, 0.006, 0.25, noise=1, seed=1),  # 2 하이햇
        tone(rate, 0.400, 55, 0.200, 0.5),              # 3 베이스
        pad(rate, 9.0),                                 # 4 패드 (LONG)
    ]
    pcm, sounds = rtc.bank(clips, rate)
    auto, notes, note_sounds = [], [], []
    for bar in range(a.bars):
        b0 = bar * 4 * beat_ns
        auto.append((b0, 3, 0.4 + 0.6 * (bar % 4) / 3))
        if bar % 4 == 0:
            auto.append((b0, 4, 1.0))
        for k in range(4):
            auto.append((b0 + k * beat_ns + beat_ns // 2, 2, 1.0))
            t = b0 + k * beat_ns
            start = len(note_sounds)
            note_sounds.append((0, 1.0))
            if bar % 8 == 7 and k == 3:
                note_sounds.append((1, 0.8))
            notes.append((t, t, start, len(note_sounds) - start))
    auto.sort(key=lambda e: e[0])
    end = a.bars * 4 * beat_ns + 9 * NS
    chart = dict(title=f"synth {a.bpm:g} BPM {a.bars} bars", rate=rate, channels=2, pcm=pcm, sounds=sounds,
                 auto=auto, note_sounds=note_sounds, lanes=[notes],
                 scroll=[(0, 0.0, a.bpm / 60)], reference_bpm=a.bpm, end=end)
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    data = rtc.encode(chart)
    assert rtc.encode(rtc.decode(data)) == data
    (out / "synth.rtc").write_bytes(data)
    # 정답 발음 목록 (노트는 제때 쳤다고 가정): (시각 ns, 소리, 이득, 출처)
    exp = [(t, s, g, "auto") for t, s, g in auto]
    for t, _, s0, n in notes:
        exp += [(t, s, g, "note") for s, g in note_sounds[s0:s0 + n]]
    exp.sort()
    (out / "expected.json").write_text(json.dumps({"rate": rate, "events": exp}), encoding="utf-8")
    print(f"{out / 'synth.rtc'}: {len(data) / 1e6:.1f} MB, 소리 {len(sounds)}, 자동 {len(auto)}, 노트 {len(notes)}, 끝 {end / NS:.1f}초")


if __name__ == "__main__":
    main()
