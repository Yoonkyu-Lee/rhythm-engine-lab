"""합성 예시 Track의 소리를 만든다 (사인파·노이즈로 직접 합성, 공개 가능).

코덱은 엔진 디코더 셋(Ogg Vorbis, FLAC, MS-ADPCM WAV)을 모두 쓰고, 샘플레이트도 일부러 섞는다
(44.1k, 48k, 22.05k, 비표준 30,697 Hz — CP2 리샘플러의 L > 8192 문제를 건드리는 레이트).

    python cp3/example/make_audio.py
"""
import pathlib
import shutil
import subprocess
import tempfile

import numpy as np
import soundfile as sf

OUT = pathlib.Path(__file__).parent / "synth_track" / "audio"
SEED = 20261017
rng = None  # main()에서 실행마다 같은 씨앗으로 초기화


def env(n, attack, rate):
    a = min(n, int(attack * rate))
    e = np.exp(-np.arange(n) / (0.25 * n))
    e[:a] *= np.linspace(0, 1, a, endpoint=False)
    return e


def tone(freqs, sec, rate, decay=True):
    t = np.arange(int(sec * rate)) / rate
    x = sum(np.sin(2 * np.pi * f * t) for f in freqs) / len(freqs)
    return x * (env(len(t), 0.003, rate) if decay else 1.0)


def kick(rate):
    t = np.arange(int(0.25 * rate)) / rate
    f = 50 + 120 * np.exp(-t * 30)
    return np.sin(2 * np.pi * np.cumsum(f) / rate) * np.exp(-t * 12)


def noise(sec, rate, k):
    n = int(sec * rate)
    return rng.standard_normal(n) * np.exp(-np.arange(n) / (k * rate))


def stereo(x, gain=0.8):
    x = np.asarray(x) * gain
    return np.stack([x, x], 1)


def encode(name, pcm, rate, codec):
    """codec: vorbis | flac16 | flac24 | adpcm"""
    dst = OUT / name
    with tempfile.TemporaryDirectory() as d:
        src = pathlib.Path(d) / "src.wav"
        sf.write(src, pcm, rate, subtype="FLOAT")
        args = {
            "vorbis": ["-c:a", "libvorbis", "-q:a", "4"],
            "flac16": ["-c:a", "flac", "-sample_fmt", "s16"],
            "flac24": ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
            "adpcm": ["-c:a", "adpcm_ms"],
        }[codec]
        subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(src), *args,
                        "-fflags", "+bitexact", "-flags:a", "+bitexact", "-map_metadata", "-1", str(dst)], check=True)


def main():
    global rng
    rng = np.random.default_rng(SEED)
    OUT.mkdir(parents=True, exist_ok=True)
    encode("kick.ogg", stereo(kick(44100)), 44100, "vorbis")
    encode("snare.ogg", stereo(noise(0.2, 48000, 0.05) * 0.6 + tone([190], 0.2, 48000) * 0.4), 48000, "vorbis")
    encode("side_swell.ogg", stereo(tone([330, 495], 1.5, 44100) * np.linspace(0.2, 1, int(1.5 * 44100))), 44100, "vorbis")
    encode("hat_closed.flac", stereo(noise(0.06, 44100, 0.015), 0.5), 44100, "flac16")
    encode("hat_open.flac", stereo(noise(0.4, 44100, 0.12), 0.5), 44100, "flac16")
    # 반주 stem: 8초, 24비트 FLAC (긴 소리)
    t = np.arange(8 * 44100) / 44100
    bass = np.sin(2 * np.pi * 55 * t) * 0.6 + np.sin(2 * np.pi * 110 * t) * 0.2
    encode("bass_long.flac", stereo(bass * np.minimum(1, (8 - t) * 4)), 44100, "flac24")
    encode("tick_odd.flac", stereo(tone([1760], 0.08, 30697)), 30697, "flac16")
    encode("piano_c.wav", stereo(tone([261.63, 523.25], 0.6, 22050)), 22050, "adpcm")
    encode("piano_e.wav", stereo(tone([329.63, 659.25], 0.6, 22050)), 22050, "adpcm")
    # 같은 내용 다른 ID 확인용: 바이트까지 같은 사본
    shutil.copyfile(OUT / "piano_c.wav", OUT / "piano_c_alt.wav")
    # 재현 기록: 도구 버전 + 파일 해시 + PCM 해시. 같은 버전이면 같은 바이트가 나와야 한다(다른 버전에서는 바이트가
    # 달라질 수 있음 — 컴파일러 결정성은 저장된 오디오를 입력으로 하므로 이것과 별개).
    import hashlib
    ff = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True).stdout.splitlines()[0]
    lines = [f"numpy {np.__version__}", f"soundfile {sf.__version__} / {sf.__libsndfile_version__}", ff, ""]
    for p in sorted(OUT.iterdir()):
        if p.suffix not in (".ogg", ".flac", ".wav"):
            continue
        a, _ = sf.read(p, dtype="float32", always_2d=True)
        lines.append(f"{p.name}  {p.stat().st_size} B  file {hashlib.blake2b(p.read_bytes(), digest_size=16).hexdigest()}"
                     f"  pcm {hashlib.blake2b(a.tobytes(), digest_size=16).hexdigest()}")
    (OUT / "MANIFEST.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
