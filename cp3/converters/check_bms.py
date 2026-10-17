"""bms2track 결과 검증: 원본 BMS를 1차 연구의 독립 파서(아래, design/BMS_to_Track.md 1-2 방식)로 따로 읽은 발음 목록 ↔
컴파일한 묶음(trackc dump)을 차분마다 1:1 대조. 독립 파서는 #RANDOM에서 #IF 1 갈래만 취하므로, 묶음 쪽도 모든 변형이 첫 선택지일 때만 센다.

    python -X utf8 cp3/converters/check_bms.py <BMS 원본 폴더> <작성 폴더> <묶음.trk>
대조: 소리 시작 (ns, 소리 파일 상대 경로) — 자동재생 + 노트 소리(보이는 노트·롱노트 시작). 투명 노트·지뢰·LNOBJ 끝은 소리 시작이 아님.
"""
import collections, os, pathlib, subprocess, sys

TRACKC = os.environ.get("TRACKC") or str(pathlib.Path(__file__).resolve().parents[1] / "target" / "release" / "trackc.exe")

# ---------------------------------------------------------------- 1차 연구 독립 파서 (scratchpad bmsdup/bmsparse.py, resolve.py 그대로)
import re, bisect
from fractions import Fraction
NS = 10**9


def decode_text(b):
    for enc in ("utf-8-sig", "cp932", "cp949"):
        try:
            return b.decode(enc), enc
        except UnicodeDecodeError:
            pass
    return b.decode("cp932", errors="replace"), "cp932?"


RX_DATA = re.compile(r"#(\d{3})([0-9A-Za-z]{2}):(.*)$")
RX_KV = re.compile(r"#([A-Za-z][A-Za-z0-9]*)(?:\s+(.*))?$")


def parse(raw):
    text, enc = decode_text(raw)
    head, wavs, bpms, stops, data = {}, {}, {}, {}, []
    flags = set()
    rstack = []  # #RANDOM: 각 분기에서 #IF 1 만 취함
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("#"):
            continue
        u = line.upper()
        if u.startswith("#RANDOM") or u.startswith("#SETRANDOM"):
            flags.add("random"); rstack.append(["rand", True]); continue
        if u.startswith("#IF"):
            flags.add("random")
            try:
                val = int(u[3:].strip())
            except ValueError:
                val = -1
            rstack.append(["if", val == 1]); continue
        if u.startswith("#ELSEIF") or u.startswith("#ELSE"):
            if rstack and rstack[-1][0] == "if":
                rstack[-1][1] = False
            continue
        if u.startswith("#ENDIF") or u.startswith("#END IF"):
            if rstack and rstack[-1][0] == "if":
                rstack.pop()
            continue
        if u.startswith("#ENDRANDOM"):
            while rstack and rstack[-1][0] == "if":
                rstack.pop()
            if rstack:
                rstack.pop()
            continue
        if any(not x[1] for x in rstack):
            continue
        m = RX_DATA.match(line)
        if m:
            data.append((int(m.group(1)), m.group(2).upper(), m.group(3).strip()))
            continue
        m = RX_KV.match(line)
        if not m:
            continue
        k, v = m.group(1).upper(), (m.group(2) or "").strip()
        if k.startswith("WAV") and len(k) == 5:
            wavs[k[3:]] = v
        elif k.startswith("BPM") and len(k) == 5:
            try:
                bpms[k[3:]] = Fraction(v)
            except Exception:
                pass
        elif k.startswith("STOP") and len(k) == 6:
            try:
                stops[k[4:]] = Fraction(v)
            except Exception:
                pass
        else:
            head.setdefault(k, v)
    return dict(head=head, wavs=wavs, bpms=bpms, stops=stops, data=data, flags=flags, enc=enc)


LANE_CH = set([f"1{c}" for c in "123456789"] + [f"2{c}" for c in "123456789"])
LN_CH = {f"5{c}": f"1{c}" for c in "123456789"} | {f"6{c}": f"2{c}" for c in "123456789"}


def _nonempty(pl):
    return any(pl[i:i + 2] != "00" for i in range(0, len(pl) - 1, 2))


def lanes_used(p):
    s = set()
    for meas, ch, pl in p["data"]:
        c = ch if ch in LANE_CH else LN_CH.get(ch)
        if c and _nonempty(pl):
            s.add(c)
    return s


def events(p):
    """모든 소리 이벤트 [(ns, wavid, kind)], kind = 'bgm' | 'note:<채널>'.
    투명(3x/4x)·지뢰(Dx/Ex) 제외, LNOBJ 끝 제외, LNTYPE 1 쌍은 시작만."""
    head, data = p["head"], p["data"]
    upper = head.get("BASE", "36").strip() != "62"
    try:
        bpm0 = Fraction(head.get("BPM", "130") or "130")
    except Exception:
        bpm0 = Fraction(130)
    lnobj = head.get("LNOBJ", "")
    if upper:
        lnobj = lnobj.upper()
    mlen = {}
    for meas, ch, v in data:
        if ch == "02":
            try:
                mlen[meas] = Fraction(v)
            except Exception:
                pass
    if not data:
        return [], {}
    last = max(meas for meas, _, _ in data)
    mstart = [Fraction(0)]
    for k in range(last + 2):
        mstart.append(mstart[-1] + 4 * mlen.get(k, Fraction(1)))

    def objs(meas, payload):
        n = len(payload) // 2
        L = 4 * mlen.get(meas, Fraction(1))
        for i in range(n):
            v = payload[2 * i:2 * i + 2]
            if upper:
                v = v.upper()
            if v != "00":
                yield mstart[meas] + L * Fraction(i, n), v

    chg = []
    for meas, ch, pl in data:
        if ch == "03":
            for pos, v in objs(meas, pl):
                try:
                    chg.append((pos, 0, Fraction(int(v, 16))))
                except ValueError:
                    pass
        elif ch == "08":
            for pos, v in objs(meas, pl):
                if v in p["bpms"]:
                    chg.append((pos, 0, p["bpms"][v]))
        elif ch == "09":
            for pos, v in objs(meas, pl):
                if v in p["stops"]:
                    chg.append((pos, 1, p["stops"][v] / 48))
    chg.sort(key=lambda c: (c[0], c[1]))
    pts, Ts, bp, st = [Fraction(0)], [Fraction(0)], [bpm0], [Fraction(0)]
    bad = False

    def eff(b):
        nonlocal bad
        if b <= 0:
            bad = True
            return Fraction(1, 10**6) if b == 0 else -b
        return b

    for pos, kind, val in chg:
        if pos != pts[-1]:
            b = eff(bp[-1])
            T = Ts[-1] + st[-1] * 60 / b + (pos - pts[-1]) * 60 / b
            pts.append(pos); Ts.append(T); bp.append(bp[-1]); st.append(Fraction(0))
        if kind == 0:
            bp[-1] = val
        else:
            st[-1] += val

    def ns(pos):
        i = bisect.bisect_right(pts, pos) - 1
        if pos == pts[i]:
            t = Ts[i]
        else:
            b = eff(bp[i])
            t = Ts[i] + st[i] * 60 / b + (pos - pts[i]) * 60 / b
        return round(t * NS)

    ev = []
    lntype = head.get("LNTYPE", "1").strip() or "1"
    lane_ev = {}
    for meas, ch, pl in data:
        if ch == "01":
            for pos, v in objs(meas, pl):
                ev.append((pos, v, "bgm"))
        elif ch in LANE_CH:
            lane_ev.setdefault(ch, []).extend(objs(meas, pl))
        elif ch in LN_CH and lntype == "1":
            lane_ev.setdefault("LN" + LN_CH[ch], []).extend(objs(meas, pl))
    for k, lst in lane_ev.items():
        lst.sort(key=lambda x: x[0])
        if k.startswith("LN"):
            for i in range(0, len(lst), 2):
                ev.append((lst[i][0], lst[i][1], "note:" + k[2:]))
        else:
            for pos, v in lst:
                if lnobj and v == lnobj:
                    continue
                ev.append((pos, v, "note:" + k))
    out = [(ns(pos), v, kind) for pos, v, kind in ev]
    out.sort()
    info = dict(bpm_changes=sum(1 for c in chg if c[1] == 0), stops=sum(1 for c in chg if c[1] == 1), bad_bpm=bad)
    return out, info

import os, collections
SND = {".ogg", ".wav", ".flac", ".mp3", ".m4a"}
ORDER = [".ogg", ".wav", ".flac", ".mp3", ".m4a"]


class Resolver:
    def __init__(self, sound_files, chart_rel):
        self.exact = {}
        self.stem = collections.defaultdict(list)
        for rel in sound_files:
            r = rel.replace("\\", "/").lower()
            self.exact[r] = rel
            s, e = os.path.splitext(r)
            self.stem[s].append((ORDER.index(e) if e in ORDER else 9, rel))
        d = os.path.dirname(chart_rel.replace("\\", "/")).lower()
        self.base = (d + "/") if d else ""

    def __call__(self, name):
        n = name.replace("\\", "/").strip().lower()
        for cand in (self.base + n, n):
            if cand in self.exact:
                return self.exact[cand]
            s = os.path.splitext(cand)[0]
            if s in self.stem:
                return sorted(self.stem[s])[0][1]
        return None


# ---------------------------------------------------------------- 대조
def main():
    src, auth, trk = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
    dump = subprocess.run([TRACKC, "dump", trk], capture_output=True, text=True, encoding="utf-8", check=True).stdout
    got = collections.defaultdict(list)
    conds = collections.defaultdict(dict)
    cur = None
    for line in dump.splitlines():
        f = line.split(" ")
        if f[0] == "chart":
            cur = f[1]
        elif f[0] == "cond":
            conds[cur][int(f[1])] = [tuple(map(int, x.split(":"))) for x in f[2:]]
        elif f[0] == "auto":
            got[cur].append((int(f[1]), " ".join(f[2:-2]), int(f[-1])))
    # 노트 소리: nsnd 줄에는 조건이 없으므로 바로 앞 note 줄의 조건을 쓴다
    cur, ncond = None, 0
    for line in dump.splitlines():
        f = line.split(" ")
        if f[0] == "chart":
            cur = f[1]
        elif f[0] == "note":
            ncond = int(f[5])
        elif f[0] == "nsnd":
            got[cur].append((int(f[2]), " ".join(f[3:-1]), ncond))
    files = [os.path.relpath(os.path.join(dp, fn), src).replace("\\", "/") for dp, _, fs in os.walk(src) for fn in fs]
    snd = [r for r in files if os.path.splitext(r)[1].lower() in SND]
    bad = 0
    for ct in sorted((auth / "charts").glob("*.toml")):
        first = ct.read_text(encoding="utf-8").splitlines()[0]
        rel = first[2:first.rindex(" (인코딩")]
        cid = ct.stem
        p = parse((src / rel).read_bytes())
        ev, _ = events(p)
        res = Resolver(snd, rel)
        exp = sorted((t, res(p["wavs"][w])) for t, w, k in ev if w in p["wavs"] and res(p["wavs"][w]))
        on = lambda c: all(ch == 0 for _, ch in conds[cid].get(c, []))
        mine = sorted((t, nm) for t, nm, c in got[cid] if on(c))
        ok = exp == mine
        bad += not ok
        print(f"{cid:28} 소리 시작 {len(mine):5}/{len(exp):5}  {'1:1 일치' if ok else '다름'}")
        if not ok:
            d = sorted(set(exp) ^ set(mine))
            print("   차이 (앞 6개):", d[:6])
    print("결과:", "모두 일치" if not bad else f"불일치 {bad}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
