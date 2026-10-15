r"""두 BMS 변환본 비교 (같은 IIDX 차분을 서로 다른 변환기로 만든 것).

python -I cp0\bms\converter\tools\compare_bms.py <ours.bme> <theirs.bme> [--sound-map ours|theirs ...]

비교 항목
  - 헤더 값
  - 객체 수: 노트, 롱노트, 투명 노트, 자동재생(01), BPM 변경
  - 노트 타이밍: 레인별로 시각(초)을 짝짓고(±2 ms) 차이 통계, 짝 없는 노트
  - 소리: 짝지은 노트가 같은 원본 소리를 가리키는지 (파일 이름 → 원본 번호 함수로 비교)
"""
import re, sys, pathlib, collections
from fractions import Fraction

sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bms_render import timeline, B62  # noqa: E402


def read_text(p):
    b = pathlib.Path(p).read_bytes()
    for enc in ("utf-8", "cp932"):
        try:
            return b.decode(enc), enc
        except UnicodeDecodeError:
            pass
    return b.decode("cp932", "replace"), "cp932?"


def parse(path):
    text, enc = read_text(path)
    base = 62 if re.search(r"^#BASE\s+62", text, re.M | re.I) else 36
    num = lambda s: B62.index(s[0] if base == 62 else s[0].upper()) * base + B62.index(s[1] if base == 62 else s[1].upper())
    hdr, wav, bpmdef, lens, objs = {}, {}, {}, {}, []
    init_bpm = Fraction(130)
    for ln in text.splitlines():
        ln = ln.strip()
        if m := re.match(r"#WAV(\w\w)\s+(.+)", ln, re.I):
            wav[num(m.group(1))] = m.group(2).strip()
        elif m := re.match(r"#BPM(\w\w)\s+([\d.]+)", ln, re.I):
            bpmdef[num(m.group(1))] = Fraction(m.group(2))
        elif m := re.match(r"#(\d{3})02:([\d.]+)", ln):
            lens[int(m.group(1))] = Fraction(m.group(2))
        elif m := re.match(r"#(\d{3})(\w\w):(\w+)", ln):
            mea, ch, data = int(m.group(1)), m.group(2).upper(), m.group(3)
            n = len(data) // 2
            for i in range(n):
                s = data[2 * i: 2 * i + 2]
                if s != "00":
                    objs.append((mea, Fraction(i, n), ch, s))
        elif m := re.match(r"#([A-Z]+)\s+(.*)", ln):
            hdr[m.group(1)] = m.group(2).strip()
            if m.group(1) == "BPM":
                init_bpm = Fraction(m.group(2))
    beat, sec = timeline(base, bpmdef, lens, objs, init_bpm, num)
    out = collections.defaultdict(list)
    for o in objs:
        ch = o[2]
        t = float(sec(beat(o)))
        if re.match(r"[12][1-9]", ch):
            out["note"].append((ch, t, wav.get(num(o[3]))))
        elif re.match(r"[56][1-9]", ch):
            out["ln"].append((ch, t, wav.get(num(o[3]))))
        elif re.match(r"[34][1-9]", ch):
            out["inv"].append((ch, t, wav.get(num(o[3]))))
        elif ch == "01":
            out["bgm"].append((ch, t, wav.get(num(o[3]))))
        elif ch in ("03", "08"):
            out["bpm"].append((ch, t, o[3]))
    return hdr, out, enc, base


def src_index(fname):
    """파일 이름 → 원본 소리 번호. ours: 14014_0123.wav (10진), theirs: 14014\\007B.ogg (36진 ID)."""
    if fname is None:
        return None
    stem = pathlib.PureWindowsPath(fname).stem
    if m := re.search(r"_(\d{4})$", stem):
        return int(m.group(1))
    if re.fullmatch(r"[0-9A-Z]{4}", stem.upper()):
        return int(stem, 36)
    return stem


def lane(ch):
    # 1x/5x → 1P, 2x/6x → 2P. 끝 글자 = 레인
    return ("1P" if ch[0] in "153" else "2P") + ch[1]


def match(a, b, tol=0.002):
    """레인별 시각 짝짓기 (탐욕적, 정렬된 두 목록)."""
    by_a, by_b = collections.defaultdict(list), collections.defaultdict(list)
    for x in a:
        by_a[lane(x[0])].append(x)
    for x in b:
        by_b[lane(x[0])].append(x)
    pairs, only_a, only_b = [], [], []
    for ln in set(by_a) | set(by_b):
        xa, xb = sorted(by_a[ln], key=lambda x: x[1]), sorted(by_b[ln], key=lambda x: x[1])
        i = j = 0
        while i < len(xa) and j < len(xb):
            d = xa[i][1] - xb[j][1]
            if abs(d) <= tol:
                pairs.append((xa[i], xb[j]))
                i += 1
                j += 1
            elif d < 0:
                only_a.append(xa[i])
                i += 1
            else:
                only_b.append(xb[j])
                j += 1
        only_a += xa[i:]
        only_b += xb[j:]
    return pairs, only_a, only_b


def main():
    pa, pb = sys.argv[1], sys.argv[2]
    ha, A, ea, ba = parse(pa)
    hb, B, eb, bb = parse(pb)
    print(f"A = {pathlib.Path(pa).name} ({ea}, base {ba})   B = {pathlib.Path(pb).name} ({eb}, base {bb})")
    for k in sorted(set(ha) | set(hb)):
        if k in ("WAV",):
            continue
        va, vb = ha.get(k, "-"), hb.get(k, "-")
        print(f"  {'==' if va == vb else '!='} #{k:10s} A={va[:50]!s:50s} B={vb[:50]}")
    for k in ("note", "ln", "inv", "bgm", "bpm"):
        print(f"  count {k:5s} A={len(A[k]):5d}  B={len(B[k]):5d}")
    for k in ("note", "ln"):
        pairs, oa, ob = match(A[k], B[k])
        diffs = sorted(abs(x[1] - y[1]) * 1000 for x, y in pairs)
        same_snd = sum(1 for x, y in pairs if src_index(x[2]) == src_index(y[2]))
        print(f"  {k}: matched {len(pairs)}  only A {len(oa)}  only B {len(ob)}"
              + (f"  |dt| ms mean {sum(diffs) / len(diffs):.3f} max {diffs[-1]:.3f}" if diffs else "")
              + f"  same source sound {same_snd}/{len(pairs)}")
        for x in (oa[:3] + ob[:3]):
            print("     unmatched", x)
        bad = [(x, y) for x, y in pairs if src_index(x[2]) != src_index(y[2])][:4]
        for x, y in bad:
            print(f"     sound differs t={x[1]:.3f} {x[0]} A={x[2]} B={y[2]}")
    # 자동재생 시각 비교 (멀티셋)
    ca = collections.Counter((round(t, 3), src_index(f)) for _, t, f in A["bgm"])
    cb = collections.Counter((round(t, 3), src_index(f)) for _, t, f in B["bgm"])
    print(f"  bgm events identical (time to 1 ms, sound): {sum((ca & cb).values())} / A {len(A['bgm'])} B {len(B['bgm'])}")


if __name__ == "__main__":
    main()
