"""BMS 채보 하나 → 1건반 실행 데이터 (시험 콘텐츠 어댑터). BMS 출신 곡은 BMS가 원본이므로 BMS를 직접 읽는다.

python -I -X utf8 cp2/tools/adapt_bms.py <채보.bms> [--channel 11] [--out cp2/content/<이름>]

- 건반: 고른 플레이 채널(기본: 노트가 가장 많은 채널, 1P 11–19)의 노트 → 레인 0. 롱노트는 LNTYPE 1(51–59 쌍)과 LNOBJ 둘 다.
- 자동재생: BGM 채널(01)과 나머지 플레이 채널의 노트 소리 전부. 보이지 않는 노트(31–39)는 소리를 내지 않으므로 뺀다.
- 시각: 마디 길이(02), BPM(03 16진, 08 확장), 정지(09, 1/192 마디)를 유리수로 계산하고 마지막에 한 번 ns로 반올림.
  같은 위치의 노트는 정지 전에 울린다.
- 소리: #WAVxx 파일을 libsndfile로 디코딩 (이름의 확장자가 없으면 .ogg/.wav/.flac 순으로 찾음). 44.1 kHz만. 볼륨은 1.
- #RANDOM이 있으면 중단 (무작위 분기는 다루지 않음).
출력: chart.rtc, names.json (소리 번호 → 파일), expected.json (모든 발음: 시각 ns, 소리, 이득, 출처).
"""
import argparse, json, pathlib, re, sys, warnings
from fractions import Fraction

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import rtc  # noqa: E402

NS = rtc.NS
PLAY = [f"1{c}" for c in "123456789"]
LN = {f"5{c}": f"1{c}" for c in "123456789"}


def read_text(p):
    b = p.read_bytes()
    for enc in ("utf-8-sig", "shift_jis", "cp949"):
        try:
            return b.decode(enc)
        except UnicodeDecodeError:
            pass
    return b.decode("latin-1")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bms")
    ap.add_argument("--channel", default="")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    src = pathlib.Path(a.bms)
    text = read_text(src)
    head, wavs, bpms, stops, data = {}, {}, {}, {}, []
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("#"):
            continue
        if re.match(r"#RANDOM|#IF", line, re.I):
            raise SystemExit("#RANDOM/#IF 분기는 다루지 않음")
        m = re.match(r"#(\d{3})([0-9A-Za-z]{2}):(.*)$", line)
        if m:
            data.append((int(m.group(1)), m.group(2).upper(), m.group(3).strip()))
            continue
        m = re.match(r"#WAV([0-9A-Za-z]{2})\s+(.+)$", line, re.I)
        if m:
            wavs[m.group(1).upper()] = m.group(2).strip()
            continue
        m = re.match(r"#BPM([0-9A-Za-z]{2})\s+(.+)$", line, re.I)
        if m:
            bpms[m.group(1).upper()] = Fraction(m.group(2).strip())
            continue
        m = re.match(r"#STOP([0-9A-Za-z]{2})\s+(.+)$", line, re.I)
        if m:
            stops[m.group(1).upper()] = Fraction(m.group(2).strip())
            continue
        m = re.match(r"#(\w+)\s*(.*)$", line)
        if m:
            head[m.group(1).upper()] = m.group(2).strip()
    bpm0 = Fraction(head["BPM"])
    lnobj = head.get("LNOBJ", "").upper()

    # 마디 시작 위치 (박, 4박 = 1마디 × 길이)
    mlen = {meas: Fraction(v) for meas, ch, v in data if ch == "02"}
    last = max(meas for meas, _, _ in data)
    mstart = [Fraction(0)]
    for k in range(last + 1):
        mstart.append(mstart[-1] + 4 * mlen.get(k, Fraction(1)))

    def objs(meas, payload):
        n = len(payload) // 2
        for i in range(n):
            v = payload[2 * i:2 * i + 2].upper()
            if v != "00":
                yield mstart[meas] + 4 * mlen.get(meas, Fraction(1)) * Fraction(i, n), v

    changes = []  # (위치, 순서, 종류, 값): 같은 위치면 BPM 먼저, 정지 나중
    for meas, ch, p in data:
        if ch == "03":
            for pos, v in objs(meas, p):
                changes.append((pos, 0, "bpm", Fraction(int(v, 16))))
        elif ch == "08":
            for pos, v in objs(meas, p):
                changes.append((pos, 0, "bpm", bpms[v]))
        elif ch == "09":
            for pos, v in objs(meas, p):
                changes.append((pos, 1, "stop", stops[v] / 48))  # 1/192 마디 = 1/48 박
    changes.sort(key=lambda c: (c[0], c[1]))

    def time_s(pos):
        """박 위치 → 초 (같은 위치의 정지는 아직 안 지남)."""
        t, p, bpm = Fraction(0), Fraction(0), bpm0
        for cp, _, kind, val in changes:
            if cp > pos or (cp == pos and kind == "stop"):
                break
            t += (cp - p) * 60 / bpm
            p = cp
            if kind == "bpm":
                bpm = val
            else:
                t += val * 60 / bpm
        return t + (pos - p) * 60 / bpm

    def ns(pos):
        return round(time_s(pos) * NS)

    # 플레이 채널 노트와 롱노트
    notes_by_ch = {c: [] for c in PLAY}
    for meas, ch, p in data:
        if ch in PLAY:
            notes_by_ch[ch] += [(pos, v, None) for pos, v in objs(meas, p)]
    if head.get("LNTYPE", "1") == "1":
        for lch, pch in LN.items():
            evs = sorted(pos_v for meas, ch, p in data if ch == lch for pos_v in objs(meas, p))
            for i in range(0, len(evs) - 1, 2):
                notes_by_ch[pch].append((evs[i][0], evs[i][1], evs[i + 1][0]))
    if lnobj:
        for c in PLAY:
            ev = sorted(notes_by_ch[c], key=lambda x: x[0])
            out = []
            for pos, v, end in ev:
                if v == lnobj and out:
                    out[-1] = (out[-1][0], out[-1][1], pos)
                else:
                    out.append((pos, v, end))
            notes_by_ch[c] = out
    counts = {c: len(v) for c, v in notes_by_ch.items() if v}
    key = a.channel or max(counts, key=counts.get)
    print(f"플레이 채널 노트 수 {counts} → 건반 = 채널 {key}")

    # 소리 디코딩
    import soundfile as sf
    sound_ix, clips, order, missing = {}, [], [], set()

    def sound(v):
        if v not in wavs:
            return None
        if v in sound_ix:
            return sound_ix[v]
        name = wavs[v]
        cand = [src.parent / name] + [src.parent / (pathlib.Path(name).stem + ext) for ext in (".ogg", ".wav", ".flac")]
        f = next((c for c in cand if c.exists()), None)
        if f is None:
            missing.add(name)
            sound_ix[v] = None
            return None
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            pcm, rate = sf.read(str(f), dtype="float32", always_2d=True)
        if rate != 44100:
            raise SystemExit(f"{f.name}: {rate} Hz (44.1 kHz만 지원)")
        if pcm.shape[1] == 1:
            pcm = np.repeat(pcm, 2, 1)
        sound_ix[v] = len(clips)
        clips.append(pcm[:, :2])
        order.append(f.name)
        return sound_ix[v]

    auto = []
    for meas, ch, p in data:
        if ch == "01":
            for pos, v in objs(meas, p):
                s = sound(v)
                if s is not None:
                    auto.append((ns(pos), s, 1.0))
    for c, evs in notes_by_ch.items():
        if c == key:
            continue
        for pos, v, _ in evs:
            s = sound(v)
            if s is not None:
                auto.append((ns(pos), s, 1.0))
    auto.sort(key=lambda e: e[0])
    notes, note_sounds = [], []
    for pos, v, end in sorted(notes_by_ch[key], key=lambda x: x[0]):
        s = sound(v)
        start = len(note_sounds)
        if s is not None:
            note_sounds.append((s, 1.0))
        t0 = ns(pos)
        notes.append((t0, ns(end) if end is not None else t0, start, len(note_sounds) - start))
    bad = [i for i in range(1, len(notes)) if notes[i][0] <= notes[i - 1][1]]
    if bad:
        raise SystemExit(f"레인 안 노트 겹침 {len(bad)}개")
    pcm, sounds = rtc.bank(clips, 44100)
    end = max([e[0] for e in auto] + [n[1] for n in notes]) + NS
    # 스크롤: 위치 = 박, 속도 = 그 구간 BPM/60 (정지 동안 0). 1배속의 기준은 #BPM
    scroll, t, p, bpm = [(0, 0.0, float(bpm0) / 60)], Fraction(0), Fraction(0), bpm0
    for cp, _, kind, val in changes:
        t += (cp - p) * 60 / bpm
        p = cp
        if kind == "bpm":
            bpm = val
            scroll.append((round(t * NS), float(p), float(bpm) / 60))
        else:
            scroll.append((round(t * NS), float(p), 0.0))
            t += val * 60 / bpm
            scroll.append((round(t * NS), float(p), float(bpm) / 60))
    # 같은 시각의 점은 마지막 것만
    dedup = {}
    for s in scroll:
        dedup[s[0]] = s
    scroll = sorted(dedup.values())
    title = f"{head.get('TITLE', src.stem)} [{src.stem}] ch{key}"
    chart = dict(title=title, rate=44100, channels=2, pcm=pcm, sounds=sounds, auto=auto, note_sounds=note_sounds,
                 lanes=[notes], scroll=scroll, reference_bpm=float(bpm0), end=end)
    out = pathlib.Path(a.out or HERE.parent / "content" / re.sub(r"[^0-9A-Za-z_]+", "_", f"bms_{src.stem}_{key}").strip("_").lower())
    out.mkdir(parents=True, exist_ok=True)
    blob = rtc.encode(chart)
    assert rtc.encode(rtc.decode(blob)) == blob
    (out / "chart.rtc").write_bytes(blob)
    (out / "names.json").write_text(json.dumps(order, ensure_ascii=False), encoding="utf-8")
    exp = [(t, s, g, "auto") for t, s, g in auto]
    for t0, _, s0, n in notes:
        exp += [(t0, note_sounds[s0 + i][0], note_sounds[s0 + i][1], "note") for i in range(n)]
    exp.sort(key=lambda e: (e[0], e[3] != "auto"))
    (out / "expected.json").write_text(json.dumps({"rate": 44100, "events": exp}), encoding="utf-8")
    print(f"{title}: 노트 {len(notes)} (롱 {sum(1 for n in notes if n[1] > n[0])}), 자동재생 {len(auto)}, 소리 {len(clips)}, "
          f"BPM 변화 {sum(1 for c in changes if c[2] == 'bpm')}, 정지 {sum(1 for c in changes if c[2] == 'stop')}, 없는 파일 {len(missing)} → {out}")


if __name__ == "__main__":
    main()
