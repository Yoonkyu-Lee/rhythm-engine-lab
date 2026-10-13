r"""BMS 파일을 독립적으로 해석해 "모든 노트를 정확히 친" 오디오로 렌더링 (변환 검증용).

python -I tools\bms_render.py <file.bme> <out.wav> [--sr 44100]

변환기(iidx2bms.py)와 코드를 공유하지 않는 별도 해석기. 지원: #WAV, #BPM, #BPMxx, #BASE 62,
채널 01(자동재생), 02(소절 길이), 03, 08(BPM), 1x/2x(노트), 5x/6x(LNTYPE 1, 시작에서 발음).
투명 노트(3x/4x)는 치지 않으므로 울리지 않음. 소리 디코딩은 ffmpeg (f32 스테레오).
"""
import re, subprocess, sys, pathlib, struct
from fractions import Fraction
import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"


def decode(path, sr):
    r = subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-i", str(path), "-f", "f32le",
                        "-ac", "2", "-ar", str(sr), "-"], capture_output=True)
    a = np.frombuffer(r.stdout, dtype="<f4")
    return a.reshape(-1, 2) if a.size else np.zeros((0, 2), "f4")


def parse(path):
    text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    base = 36
    wav, bpmdef, lens, objs = {}, {}, {}, []
    init_bpm = Fraction(130)
    for ln in text.splitlines():
        ln = ln.strip()
        if m := re.match(r"#BASE\s+(\d+)", ln, re.I):
            base = int(m.group(1))
    def num(s):
        s = s if base == 62 else s.upper()
        return B62.index(s[0]) * base + B62.index(s[1])
    for ln in text.splitlines():
        ln = ln.strip()
        if m := re.match(r"#WAV(\w\w)\s+(.+)", ln, re.I):
            wav[num(m.group(1))] = m.group(2).strip()
        elif m := re.match(r"#BPM(\w\w)\s+([\d.]+)", ln, re.I):
            bpmdef[num(m.group(1))] = Fraction(m.group(2))
        elif m := re.match(r"#BPM\s+([\d.]+)", ln, re.I):
            init_bpm = Fraction(m.group(1))
        elif m := re.match(r"#(\d{3})02:([\d.]+)", ln):
            lens[int(m.group(1))] = Fraction(m.group(2))
        elif m := re.match(r"#(\d{3})(\w\w):(\w+)", ln):
            mea, ch, data = int(m.group(1)), m.group(2).upper(), m.group(3)
            n = len(data) // 2
            for i in range(n):
                s = data[2 * i: 2 * i + 2]
                if s != "00":
                    objs.append((mea, Fraction(i, n), ch, s))
    return base, wav, bpmdef, lens, objs, init_bpm, num


def timeline(base, bpmdef, lens, objs, init_bpm, num):
    nmeas = max([o[0] for o in objs] + [0]) + 2
    starts = [Fraction(0)]
    for m in range(nmeas):
        starts.append(starts[-1] + 4 * lens.get(m, Fraction(1)))
    beat = lambda o: starts[o[0]] + o[1] * 4 * lens.get(o[0], Fraction(1))
    changes = []
    for o in objs:
        if o[2] == "03":
            changes.append((beat(o), Fraction(int(o[3], 16))))
        elif o[2] == "08":
            changes.append((beat(o), bpmdef[num(o[3])]))
    changes.sort()
    segs = [(Fraction(0), Fraction(0), init_bpm)]  # (beat, sec, bpm)
    for b, bpm in changes:
        b0, s0, bp = segs[-1]
        segs.append((b, s0 + (b - b0) * 60 / bp, bpm))
    def sec(b):
        s = segs[0]
        for seg in segs:
            if seg[0] <= b:
                s = seg
        return s[1] + (b - s[0]) * 60 / s[2]
    return beat, sec


def main():
    src, out = sys.argv[1], sys.argv[2]
    sr = 44100
    base, wav, bpmdef, lens, objs, init_bpm, num = parse(src)
    beat, sec = timeline(base, bpmdef, lens, objs, init_bpm, num)
    play = []
    for o in objs:
        ch = o[2]
        if ch == "01" or re.match(r"[12][1-9]", ch) or re.match(r"[56][1-9]", ch):
            play.append((float(sec(beat(o))), num(o[3]), ch))
    # LN: 시작과 끝 객체가 쌍 → 시작만 발음
    ln_seen = {}
    final = []
    for t, i, ch in sorted(play):
        if ch[0] in "56":
            if ln_seen.get(ch):
                ln_seen[ch] = False
                continue
            ln_seen[ch] = True
        final.append((t, i))
    d = pathlib.Path(src).parent
    cache = {}
    end = max(t for t, _ in final) + 10
    mix = np.zeros((int(end * sr), 2), "f4")
    missing = 0
    for t, i in final:
        if i not in wav:
            missing += 1
            continue
        if i not in cache:
            cache[i] = decode(d / wav[i], sr)
        a = cache[i]
        s = int(round(t * sr))
        e = min(len(mix), s + len(a))
        mix[s:e] += a[: e - s]
    peak = float(np.abs(mix).max()) or 1.0
    pcm = (mix / max(peak, 1.0) * 32767).astype("<i2")
    with open(out, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", 36 + pcm.nbytes) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 2, sr, sr * 4, 4, 16)
                + b"data" + struct.pack("<I", pcm.nbytes) + pcm.tobytes())
    print(f"rendered {len(final)} events, {len(cache)} sounds, missing {missing}, {end:.1f}s, peak {peak:.2f} → {out}")


if __name__ == "__main__":
    main()
