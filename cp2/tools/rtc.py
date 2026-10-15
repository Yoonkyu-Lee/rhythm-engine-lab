"""실행 데이터 파일 `.rtc`를 쓰고 읽는다 (코어 `crates/rcore/src/rtc.rs`와 같은 배치). 시험 콘텐츠 어댑터가 쓴다.

Chart = dict(
    title=str, rate=int, channels=int,
    pcm=numpy float32 [frames, channels],
    sounds=[(offset_frames, frames, is_long)],
    auto=[(t_ns, sound, gain)],                     # 시간순
    note_sounds=[(sound, gain)],
    lanes=[[(t_ns, end_ns, sounds_start, sounds_len)]],
    scroll=[(t_ns, pos, speed)], reference_bpm=float, end=int,
)
"""
import struct

import numpy as np

MAGIC, VERSION = b"RTC1", 1
NS = 1_000_000_000
LONG_SOUND_NS = 2 * NS  # 코어의 LONG_SOUND_NS와 같아야 한다


def is_long(frames, rate):
    return -(-frames * NS // rate) > LONG_SOUND_NS


def encode(c):
    pcm = np.ascontiguousarray(c["pcm"], dtype="<f4")
    out = [MAGIC, struct.pack("<I", VERSION)]
    t = c["title"].encode("utf-8")
    out += [struct.pack("<I", len(t)), t]
    out.append(struct.pack("<IHHI", c["rate"], c["channels"], 0, len(c["sounds"])))
    for off, fr, lg in c["sounds"]:
        out.append(struct.pack("<QIB3x", off, fr, 1 if lg else 0))
    out.append(struct.pack("<Q", pcm.size))
    out.append(pcm.tobytes())
    out.append(struct.pack("<I", len(c["auto"])))
    out.append(b"".join(struct.pack("<qIf", t, s, g) for t, s, g in c["auto"]))
    out.append(struct.pack("<I", len(c["note_sounds"])))
    out.append(b"".join(struct.pack("<If", s, g) for s, g in c["note_sounds"]))
    out.append(struct.pack("<I", len(c["lanes"])))
    for lane in c["lanes"]:
        out.append(struct.pack("<I", len(lane)))
        out.append(b"".join(struct.pack("<qqIH2x", t, e, s, n) for t, e, s, n in lane))
    out.append(struct.pack("<I", len(c["scroll"])))
    out.append(b"".join(struct.pack("<qdd", t, p, v) for t, p, v in c["scroll"]))
    out.append(struct.pack("<dq", c["reference_bpm"], c["end"]))
    return b"".join(out)


def decode(b):
    p = 0

    def take(fmt):
        nonlocal p
        v = struct.unpack_from(fmt, b, p)
        p += struct.calcsize(fmt)
        return v

    assert b[:4] == MAGIC
    p = 4
    (ver,) = take("<I")
    assert ver == VERSION, ver
    (n,) = take("<I")
    title = b[p:p + n].decode("utf-8")
    p += n
    rate, ch, _, ns = take("<IHHI")
    sounds = [take("<QIB3x") for _ in range(ns)]
    (nsamp,) = take("<Q")
    pcm = np.frombuffer(b, "<f4", nsamp, p).reshape(-1, ch)
    p += nsamp * 4
    (n,) = take("<I")
    auto = [take("<qIf") for _ in range(n)]
    (n,) = take("<I")
    note_sounds = [take("<If") for _ in range(n)]
    (nl,) = take("<I")
    lanes = []
    for _ in range(nl):
        (n,) = take("<I")
        lanes.append([take("<qqIH2x") for _ in range(n)])
    (n,) = take("<I")
    scroll = [take("<qdd") for _ in range(n)]
    ref, end = take("<dq")
    assert p == len(b), (p, len(b))
    return dict(title=title, rate=rate, channels=ch, pcm=pcm, sounds=[(o, f, bool(k)) for o, f, k in sounds],
                auto=auto, note_sounds=note_sounds, lanes=lanes, scroll=scroll, reference_bpm=ref, end=end)


def bank(clips, rate):
    """[float32 [frames, ch]] → (이어 붙인 pcm, sounds)"""
    sounds, off = [], 0
    for c in clips:
        sounds.append((off, len(c), is_long(len(c), rate)))
        off += len(c)
    return np.concatenate(clips).astype("<f4"), sounds
