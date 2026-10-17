"""bms2track 결과 검증 0.2 (Codex 3-2 리뷰 #5·#6 반영): 원본 BMS/BMSON을 **컨버터와 코드를 나누지 않는** 해석기로 읽어,
컴파일한 묶음(trackc dump)과 차분마다·선택 조합마다 대조한다.

    python -I -X utf8 cp3/converters/check_bms.py <BMS 원본 폴더> <작성 폴더> <묶음.trk> [--combos 64] [--no-pcm]

해석기는 실제 플레이어처럼 "선택 조합 하나를 정하고 위에서부터 읽는다"(#RANDOM 값 공급, #IF/#ELSEIF/#ELSE, #SETRANDOM 고정값).
컨버터는 "줄마다 조건을 붙이고 묶음으로 풀어 합친다"는 다른 방식이라, 두 결과가 같으면 합치기·조건 표기가 맞다는 뜻이다.
변형 번호 r<k> = 파일 안 k번째 #RANDOM 문장(#SETRANDOM의 거짓 갈래 안은 세지 않음) — 컨버터와의 약속(인터페이스).

대조 (조합마다)
- 노트: (레인, 시작 ns, 끝 ns, 종류, 소리 ID들) — 소리를 노트에 묶어 비교한다(같은 시각 두 노트의 소리가 바뀌어도 잡힘).
- 자동재생 (ns, 소리 ID, 이득 f32) 다중집합, 빈 레인 소리 (레인, ns, 소리 ID).
- 스크롤: 노트마다 화면 위치(박) = 원본 박 위치를 BPM 부호·#SCROLL로 적분한 값 ↔ 묶음 스크롤 지도 (1e-6 박 + 그 자리 속도 × 1 ns: 시각이 ns 정수).
- 손실: 해석기가 센 판정 손실(롱노트 안·페달·레이아웃 밖)이 묶음 LOSSES의 같은 항목 이하, 변형이 없으면 같음.
- 차분 목록: 원본 차분 파일마다 status.json에 결과가 있어야 하고, ok·incomplete면 그 차분들이 묶음에 있어야 한다. 미지원·실패는 이유를 출력.
- 조합: 전체가 --combos 이하면 모두, 넘으면 첫 조합·끝 조합 + 무작위(시드 고정)로 --combos개.
소리 (--no-pcm이면 건너뜀)
- 소리 파일마다 원본을 기준 디코더(libsndfile, 그 밖은 ffmpeg)로, 변환 파일을 엔진 디코더(trackc decode)로 디코딩해 비교.
  무손실 경로는 정수 PCM이 같고(Vorbis는 |차| ≤ 1e-6), 손실 경로는 소리 gain을 곱한 값이 원본과 FLAC24 양자화(×gain) 이내.
"""
import argparse, bisect, collections, json, os, pathlib, random, re, struct, subprocess, sys, tempfile, tomllib
from fractions import Fraction

import numpy as np
import soundfile as sf

TRACKC = os.environ.get("TRACKC") or str(pathlib.Path(__file__).resolve().parents[1] / "target" / "release" / "trackc.exe")
NS = 10**9
SND_EXT = [".ogg", ".wav", ".flac", ".mp3", ".m4a", ".opus"]


def f32(x):
    return struct.unpack("<f", struct.pack("<f", float(x)))[0]


def ns_of(sec):
    """유리수 초 → ns (짝수 반올림)."""
    x = sec * NS
    n, r = divmod(x.numerator, x.denominator)
    if 2 * r > x.denominator or (2 * r == x.denominator and n % 2):
        n += 1
    return n


def text_of(raw):
    for enc in ("utf-8-sig", "cp932", "cp949"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            pass
    return raw.decode("cp932", errors="replace")


# ------------------------------------------------------------------ 실제 플레이어처럼 읽기
def ctrl(s):
    u = s.upper()
    if re.match(r"#END\s+IF(?![0-9A-Z_])", u):
        return "ENDIF", ""
    for w in ("SETRANDOM", "ENDRANDOM", "RANDOM", "ELSEIF", "ELSE", "ENDIF", "IFEND", "IF"):
        if u.startswith("#" + w):
            rest = s[len(w) + 1:]
            if not rest or not (rest[0].isalnum() or rest[0] == "_"):
                return ("ENDIF" if w == "IFEND" else w), rest.strip()
    return None, None


def first_int(arg):
    m = re.match(r"[+-]?\d+", arg or "")
    return int(m.group()) if m else None


_TOK = {}
_B62 = {}
_SEC = {}
_LINE = {}


def run(text, value, all_on=False):
    """조합대로 읽어 켜진 줄 목록과 무작위 문장의 선택지 수 목록을 돌려준다. value(k, n) = k번째 #RANDOM의 값.
    all_on이면 #RANDOM 갈래를 모두 켠다(레이아웃 판별용). #SETRANDOM은 언제나 고정값."""
    stack, sizes, active = [], [], []
    toks = _TOK.get(text)
    if toks is None:  # 줄 나누기·명령 판별은 원본마다 한 번 (조합마다 다시 하지 않음)
        toks = _TOK[text] = [(s, *ctrl(s)) for s in (l.strip() for l in text.splitlines()) if s.startswith("#")]
    for s, w, arg in toks:
        off = [f for f in stack if f["t"] == "if" and not f["on"]]
        if w in ("RANDOM", "SETRANDOM"):
            n = first_int(arg)
            if w == "SETRANDOM":
                stack.append({"t": "rand", "val": n, "fixed": True})
            elif any(f["fixed"] for f in off):  # #SETRANDOM의 거짓 갈래 안: 세지 않음
                stack.append({"t": "rand", "val": None, "fixed": True})
            else:
                k = len(sizes)
                sizes.append(n)
                stack.append({"t": "rand", "val": "*" if all_on else value(k, n), "fixed": False})
        elif w == "IF":
            r = next((f for f in reversed(stack) if f["t"] == "rand"), {"t": "rand", "val": None, "fixed": True})  # #RANDOM 밖: 맞는 갈래 없음
            on = (r["val"] == "*" and not r["fixed"]) or r["val"] == first_int(arg)
            stack.append({"t": "if", "on": on, "done": on, "fixed": r["fixed"], "rand": r})
        elif w in ("ELSEIF", "ELSE"):
            f = stack[-1]
            if f["rand"]["val"] == "*":
                f["on"] = True
            elif w == "ELSE":
                f["on"], f["done"] = not f["done"], True
            else:
                f["on"] = not f["done"] and f["rand"]["val"] == first_int(arg)
                f["done"] = f["done"] or f["on"]
        elif w == "ENDIF":
            if stack and stack[-1]["t"] == "if":
                stack.pop()
        elif w == "ENDRANDOM":
            while stack and stack[-1]["t"] == "if":
                stack.pop()
            if stack:
                stack.pop()
        elif not off:
            active.append(s)
    return active, sizes


def read(active, base62):
    """켜진 줄 → 머리, 정의(종류 → ID → 값), 데이터 [(줄 순번, 마디, 채널, 값)]."""
    head, defs, data = {}, collections.defaultdict(dict), []
    for i, s in enumerate(active):
        c = _LINE.get((s, base62))
        if c is not None:  # 같은 줄의 해석은 한 번 (조합마다 다시 하지 않음)
            kind, a, b, v = c
            if kind == 0:
                data.append((i, a, b, v))
            elif kind == 1:
                defs[a][b] = v
            elif kind == 2:
                head[a] = v
            continue
        m = re.match(r"#(\d\d\d)([0-9A-Za-z][0-9A-Za-z]):(.*)$", s)
        if m:
            data.append((i, int(m.group(1)), m.group(2).upper(), "".join(m.group(3).split())))
            _LINE[(s, base62)] = (0, data[-1][1], data[-1][2], data[-1][3])
            continue
        u = s.upper()
        for name, kind in (("#WAV", "WAV"), ("#EXBPM", "BPM"), ("#BPM", "BPM"), ("#STOP", "STOP"), ("#SCROLL", "SCROLL")):
            L = len(name)
            if u.startswith(name) and len(s) >= L + 2 and s[L:L + 2].isalnum() and (len(s) == L + 2 or s[L + 2] in " \t"):
                k = s[L:L + 2] if base62 else s[L:L + 2].upper()
                defs[kind][k] = s[L + 2:].strip()
                _LINE[(s, base62)] = (1, kind, k, defs[kind][k])
                break
        else:
            parts = s[1:].split(None, 1)
            if parts:
                head[parts[0].upper().rstrip(":")] = parts[1].strip() if len(parts) > 1 else ""
                _LINE[(s, base62)] = (2, parts[0].upper().rstrip(":"), None, head[parts[0].upper().rstrip(":")])
            else:
                _LINE[(s, base62)] = (3, None, None, None)
    return head, defs, data


def num(s):
    try:
        return Fraction(s.strip())
    except (ValueError, ZeroDivisionError):
        return Fraction(float(s))


LANE_MAP = {
    "5+1": dict(zip(["16", "11", "12", "13", "14", "15"], ["SC", "K1", "K2", "K3", "K4", "K5"])),
    "7+1": dict(zip(["16", "11", "12", "13", "14", "15", "18", "19"], ["SC", "K1", "K2", "K3", "K4", "K5", "K6", "K7"])),
    "10+2": dict(zip(["16", "11", "12", "13", "14", "15", "21", "22", "23", "24", "25", "26"],
                     ["SC1", "K1", "K2", "K3", "K4", "K5", "K6", "K7", "K8", "K9", "K10", "SC2"])),
    "14+2": dict(zip(["16", "11", "12", "13", "14", "15", "18", "19", "21", "22", "23", "24", "25", "28", "29", "26"],
                     ["SC1"] + [f"K{i}" for i in range(1, 15)] + ["SC2"])),
    "9K": dict(zip(["11", "12", "13", "14", "15", "22", "23", "24", "25"], [f"K{i}" for i in range(1, 10)])),
}
BASE_CH = {"1": "1", "3": "1", "5": "1", "D": "1", "2": "2", "4": "2", "6": "2", "E": "2"}  # 채널 첫 자리 → 1P/2P 노트 채널


def layout_of(path, text):
    act, _ = run(text, None, all_on=True)
    used = set()
    for s in act:
        m = re.match(r"#\d\d\d([1256DE])([1-9]):(.*)$", s, re.I)
        if m and m.group(2) != "7" and any(p != "00" for p in re.findall("..", "".join(m.group(3).split()))):
            used.add(BASE_CH[m.group(1).upper()] + m.group(2))
    if path.suffix.lower() == ".pms":
        return "9K"
    wide = any(c[1] in "89" for c in used)
    if any(c[0] == "2" for c in used):
        return "14+2" if wide else "10+2"
    return "7+1" if wide else "5+1"


class Files:
    def __init__(self, folder):
        self.by = {}
        self.stem = collections.defaultdict(list)
        for dp, _, fs in os.walk(folder):
            for fn in fs:
                rel = os.path.relpath(os.path.join(dp, fn), folder).replace("\\", "/")
                e = os.path.splitext(fn)[1].lower()
                if e in SND_EXT:
                    self.by[rel.lower()] = rel
                    self.stem[os.path.splitext(rel.lower())[0]].append((SND_EXT.index(e), rel))

    def find(self, name, chart_rel):
        d = os.path.dirname(chart_rel).lower()
        n = name.replace("\\", "/").strip().lower()
        for c in ([f"{d}/{n}"] if d else []) + [n]:
            if c in self.by:
                return self.by[c]
            if os.path.splitext(c)[0] in self.stem:
                return min(self.stem[os.path.splitext(c)[0]])[1]
        return None


def expect_bms(text, chart_rel, files, layout, value):
    """선택 조합 하나 → 기대값 dict(notes, autos, lsnds, losses, pos)."""
    if text not in _B62:
        _B62[text] = any(re.match(r"#BASE\s+62\s*$", s, re.I) for s in run(text, lambda k, n: 1, all_on=True)[0])
    base62 = _B62[text]
    act, _ = run(text, value)
    head, defs, data = read(act, base62)
    norm = (lambda v: v) if base62 else (lambda v: v.upper())
    lnobj = norm(head.get("LNOBJ", "").strip())
    vw = head.get("VOLWAV", "").strip()
    gain = f32(Fraction(int(vw), 100)) if vw.isdigit() else 1.0
    meas_len = {}
    for _, m, ch, pl in data:
        if ch == "02":
            meas_len[m] = num(pl)
    top = max([m for _, m, _, _ in data] + [0]) + 2
    start = [Fraction(0)]
    for m in range(top):
        start.append(start[-1] + 4 * meas_len.get(m, 1))
    objs = []  # (채널, 박, 순번, 값)
    for i, m, ch, pl in data:
        if ch == "02":
            continue
        cells = [pl[j:j + 2] for j in range(0, len(pl) - 1, 2)]
        for j, c in enumerate(cells):
            if c != "00":
                objs.append((ch, start[m] + 4 * meas_len.get(m, 1) * Fraction(j, len(cells)), (i, j), norm(c)))
    last = {}
    bgm = []
    for o in objs:
        if o[0] == "01":
            bgm.append(o)
        else:
            last[(o[0], o[1])] = o  # 같은 채널·위치: 뒤가 이김 (objs는 줄·칸 순서)
    # 시간
    bpm = {Fraction(0): num(head.get("BPM", "130") or "130")}
    stops = collections.Counter()
    scr = {}
    for o in sorted((o for o in last.values() if o[0] in ("03", "08", "09", "SC")), key=lambda o: (o[1], o[2])):
        ch, b, _, v = o
        if ch == "03":
            bpm[b] = Fraction(int(v, 16))
        elif ch == "08" and v in defs["BPM"]:
            bpm[b] = num(defs["BPM"][v])
        elif ch == "09" and v in defs["STOP"]:
            stops[b] += num(defs["STOP"][v]) / 48
        elif ch == "SC" and v in defs["SCROLL"]:
            scr[b] = num(defs["SCROLL"][v])
    marks = sorted(set(bpm) | set(stops))
    # 표시마다 (위치, 그 위치까지의 시각(정지 전), 그 뒤 BPM) — 같은 시간 지도면 조합 사이에 결과를 공유 (검증 속도)
    sig = (tuple(sorted(bpm.items())), tuple(sorted(stops.items())))
    memo = _SEC.setdefault(sig, {})
    pre, t_acc, at, cur = [], Fraction(0), Fraction(0), abs(bpm[Fraction(0)])
    for m in marks:
        t_acc += (m - at) * 60 / cur
        at = m
        if m in bpm:
            cur = abs(bpm[m])
        pre.append((m, t_acc, cur))
        t_acc += stops[m] * 60 / cur
    mlist = [m for m, _, _ in pre]

    def sec(b):
        r = memo.get(b)
        if r is None:
            k = bisect.bisect_left(mlist, b)  # b보다 앞선 표시들
            if k == 0:
                r = b * 60 / abs(bpm[Fraction(0)])
            else:
                m, tm, c = pre[k - 1]
                r = tm + stops[m] * 60 / c + (b - m) * 60 / c
            memo[b] = r
        return r

    explicit = bool(scr) or any(v < 0 for v in bpm.values())

    def vis(b):  # 화면 위치(박)
        if not explicit:
            return b
        pts = sorted(set(bpm) | set(scr) | {b})
        p, cb, cs = Fraction(0), bpm[Fraction(0)], scr.get(Fraction(0), Fraction(1))
        for x, y in zip(pts, pts[1:]):
            if x in bpm:
                cb = bpm[x]
            if x in scr:
                cs = scr[x]
            if y > b:
                break
            p += (y - x) * cs * (1 if cb > 0 else -1)
        return p

    def key(v):
        name = defs["WAV"].get(v)
        rel = files.find(name, chart_rel) if name is not None else None
        return f"{rel}@{v}" if rel else None

    lanes = LANE_MAP[layout]
    autos = [(ns_of(sec(b)), k, gain) for _, b, _, v in bgm if (k := key(v))]
    losses = collections.Counter()
    lsnds = []
    per_lane = collections.defaultdict(list)  # 레인 → [(박, 순번, 종류, 끝, 값)]
    chans = collections.defaultdict(list)
    for o in last.values():
        if re.fullmatch(r"[123456DE][1-9]", o[0]):
            chans[o[0]].append(o)
    for ch, lst in chans.items():
        lst.sort(key=lambda o: o[1])
        lane = lanes.get(BASE_CH[ch[0]] + ch[1]) if ch[1] != "7" else None
        made = []
        if ch[0] in "34":
            if lane:
                lsnds += [(lane, ns_of(sec(b)), k) for _, b, _, v in lst if (k := key(v))]
            continue
        if ch[0] in "56":
            pairs = list(zip(lst[0::2], lst[1::2]))
            made = [(a[1], a[2], "long", z[1], a[3]) for a, z in pairs]
            if len(lst) % 2:
                made.append((lst[-1][1], lst[-1][2], "tap", None, lst[-1][3]))
        elif ch[0] in "DE":
            made = [(b, i, "mine", None, v) for _, b, i, v in lst]
        else:
            for _, b, i, v in lst:
                if lnobj and v == lnobj:
                    if made and made[-1][2] == "tap":
                        made[-1] = (made[-1][0], made[-1][1], "long", b, made[-1][4])
                    continue
                made.append((b, i, "tap", None, v))
        if lane is None:
            what = "페달" if ch[1] == "7" else f"레이아웃 {layout} 밖 채널 {ch}"
            for b, _, kind, _, v in made:
                if kind == "mine":
                    losses[f"{what} 지뢰 버림"] += 1
                else:
                    losses[f"{what} 노트 → 소리만 자동재생"] += 1
                    if k := key(v):
                        autos.append((ns_of(sec(b)), k, gain))
            continue
        per_lane[lane] += made
    notes, pos = [], {}
    for lane, made in per_lane.items():
        win = {}
        for x in sorted(made, key=lambda x: x[1]):  # 같은 레인·위치: 파일 순서상 뒤
            win[x[0]] = x
        end_of_prev = None
        for b in sorted(win):
            _, _, kind, end, v = win[b]
            if end_of_prev is not None and b <= end_of_prev:
                if kind == "mine":
                    losses["롱노트 안 지뢰 버림"] += 1
                else:
                    losses["롱노트 안 노트 → 소리만 자동재생"] += 1
                    if k := key(v):
                        autos.append((ns_of(sec(b)), k, gain))
                continue
            k = key(v) if kind != "mine" else None
            t = ns_of(sec(b))
            notes.append((lane, t, ns_of(sec(end)) if kind == "long" else t, kind, (k,) if k else (), gain if k else None))
            pos[(lane, t)] = vis(b)
            end_of_prev = end if kind == "long" else b
    if any(o[0] == "SP" for o in last.values()):
        losses["#SPEED (화면 속도 변화) 버림"] += sum(1 for o in last.values() if o[0] == "SP")
    return dict(notes=notes, autos=autos, lsnds=lsnds, losses=losses, pos=pos)


# ------------------------------------------------------------------ BMSON
LANE_BMSON = {"beat-5k": {8: "SC", 1: "K1", 2: "K2", 3: "K3", 4: "K4", 5: "K5"},
              "beat-7k": {8: "SC", **{i: f"K{i}" for i in range(1, 8)}},
              "beat-10k": {8: "SC1", **{i: f"K{i}" for i in range(1, 6)}, **{8 + i: f"K{5 + i}" for i in range(1, 6)}, 16: "SC2"},
              "beat-14k": {8: "SC1", **{i: f"K{i}" for i in range(1, 8)}, **{8 + i: f"K{7 + i}" for i in range(1, 8)}, 16: "SC2"},
              "popn-9k": {i: f"K{i}" for i in range(1, 10)}}


def expect_bmson(raw, chart_rel, files, folder):
    d = json.loads(raw.decode("utf-8-sig"))
    info = d.get("info") or {}
    res = info.get("resolution") or 240
    lanes = LANE_BMSON[info.get("mode_hint") or "beat-7k"]
    bpm = {Fraction(0): Fraction(str(info.get("init_bpm", 130)))}
    for e in d.get("bpm_events") or []:
        bpm[Fraction(e["y"], res)] = Fraction(str(e["bpm"]))
    stops = collections.Counter()
    for e in d.get("stop_events") or []:
        stops[Fraction(e["y"], res)] += Fraction(e["duration"], res)
    marks = sorted(set(bpm) | set(stops))
    # 표시마다 (위치, 그 위치까지의 시각(정지 전), 그 뒤 BPM) — 같은 시간 지도면 조합 사이에 결과를 공유 (검증 속도)
    sig = (tuple(sorted(bpm.items())), tuple(sorted(stops.items())))
    memo = _SEC.setdefault(sig, {})
    pre, t_acc, at, cur = [], Fraction(0), Fraction(0), abs(bpm[Fraction(0)])
    for m in marks:
        t_acc += (m - at) * 60 / cur
        at = m
        if m in bpm:
            cur = abs(bpm[m])
        pre.append((m, t_acc, cur))
        t_acc += stops[m] * 60 / cur
    mlist = [m for m, _, _ in pre]

    def sec(b):
        r = memo.get(b)
        if r is None:
            k = bisect.bisect_left(mlist, b)  # b보다 앞선 표시들
            if k == 0:
                r = b * 60 / abs(bpm[Fraction(0)])
            else:
                m, tm, c = pre[k - 1]
                r = tm + stops[m] * 60 / c + (b - m) * 60 / c
            memo[b] = r
        return r

    autos, by = [], collections.defaultdict(list)
    meta = {}
    for ch in d.get("sound_channels") or []:
        rel = files.find(ch.get("name") or "", chart_rel)
        if rel and rel not in meta:
            i = sf.info(str(folder / rel))
            meta[rel] = (i.samplerate, i.frames)
        ns_ = sorted(ch.get("notes") or [], key=lambda n: n["y"])
        t0 = None
        for k, n in enumerate(ns_):
            if t0 is None or not n.get("c"):
                t0 = n["y"]
            key = None
            if rel:
                rate, frames = meta[rel]
                s0 = round((sec(Fraction(n["y"], res)) - sec(Fraction(t0, res))) * rate)
                nx = next((m for m in ns_[k + 1:] if m["y"] > n["y"]), None)
                ln = round((sec(Fraction(nx["y"], res)) - sec(Fraction(n["y"], res))) * rate) if nx and nx.get("c") else 0
                if s0 < frames:
                    key = f"{rel}@{s0}+{min(ln, frames - s0) if ln else 0}"
            x = n.get("x") or 0
            t = ns_of(sec(Fraction(n["y"], res)))
            if not x or x not in lanes:
                if key:
                    autos.append((t, key, 1.0))
                continue
            l = n.get("l") or 0
            by[(lanes[x], n["y"])].append((l, key))
    notes, pos = [], {}
    for (lane, y), lst in sorted(by.items(), key=lambda kv: (kv[0][0], kv[0][1])):
        l = max(a for a, _ in lst)
        t = ns_of(sec(Fraction(y, res)))
        ks = tuple(sorted(k for _, k in lst if k))
        notes.append((lane, t, ns_of(sec(Fraction(y + l, res))) if l else t, "long" if l else "tap", ks, 1.0 if ks else None))
        pos[(lane, t)] = Fraction(y, res)
    return dict(notes=notes, autos=autos, lsnds=[], losses=collections.Counter(), pos=pos)


# ------------------------------------------------------------------ 묶음 쪽
def load_dump(trk):
    out = subprocess.run([TRACKC, "dump", trk], capture_output=True, text=True, encoding="utf-8", check=True).stdout
    charts, c = {}, None
    for line in out.splitlines():
        f = line.split(" ")
        if f[0] == "chart":
            c = charts[f[1]] = dict(variants=[], conds={}, sounds={}, autos=[], notes=[], lsnds=[], scroll=[], losses=collections.Counter())
        elif f[0] == "variant":
            c["variants"].append(f[2])
        elif f[0] == "cond":
            c["conds"][int(f[1])] = [tuple(map(int, x.split(":"))) for x in f[2:]]
        elif f[0] == "sound":
            c["sounds"][int(f[1])] = dict(start=int(f[2]), length=int(f[3]), gain=float(f[4]), name=" ".join(f[7:]))
        elif f[0] == "loss":
            k, v = line[5:].rsplit(": ", 1)
            c["losses"][k] = int(v)
        elif f[0] == "auto":
            c["autos"].append((int(f[1]), " ".join(f[2:-2]), f32(f[-2]), int(f[-1])))
        elif f[0] == "note":
            c["notes"].append([f[1], int(f[2]), int(f[3]), ["tap", "long", "mine"][int(f[4])], int(f[5]), []])
        elif f[0] == "nsnd":
            c["notes"][-1][5].append((" ".join(f[3:-1]), f32(f[-1])))
        elif f[0] == "lsnd":
            c["lsnds"].append((f[1], int(f[2]), " ".join(f[3:-1]), int(f[-1])))
        elif f[0] == "scroll":
            c["scroll"].append((int(f[1]), float(f[2]), float(f[3])))
    return charts


def got_for(c, vals):
    """묶음 차분 하나를 선택 조합 vals(변형 번호 0부터 → 값)로 거른다."""
    def on(ci):
        return all(vals.get(int(c["variants"][v][1:]) - 1, 1) == ch + 1 for v, ch in c["conds"].get(ci, []))
    notes, pos = [], {}
    for lane, t, end, kind, ci, snds in c["notes"]:
        if on(ci):
            ks = tuple(sorted(n for n, _ in snds))
            gs = {g for _, g in snds}
            notes.append((lane, t, end, kind, ks, gs.pop() if len(gs) == 1 else (None if not gs else tuple(sorted(gs)))))
    autos = [(t, n, g) for t, n, g, ci in c["autos"] if on(ci)]
    lsnds = [(l, t, n) for l, t, n, ci in c["lsnds"] if on(ci)]
    return dict(notes=notes, autos=autos, lsnds=lsnds)


def scroll_at(sc, t):
    p = max((x for x in sc if x[0] <= t), key=lambda x: x[0], default=sc[0])
    return p[1] + p[2] * (t - p[0]) / NS


def scroll_tol(sc, t, p):
    """허용 오차: 1e-6 박(상대) + 그 자리 최대 속도 × 1 ns. 노트·스크롤 점 시각이 ns 정수라서 생기는 몫 —
    초당 수십만 박의 순간 이동 기믹에서는 1 ns가 1e-4 박을 넘는다(화면으로는 0.01 px 미만)."""
    near = [abs(x[2]) for x in sc if abs(x[0] - t) <= 1]
    near.append(abs(max((x for x in sc if x[0] <= t), key=lambda x: x[0], default=sc[0])[2]))
    return 1e-6 * max(1, abs(p)) + max(near) * 1.0 / NS


# ------------------------------------------------------------------ 소리 PCM
def pcm_check(src_folder, auth, sounds_used):
    """원본(기준 디코더) ↔ 변환 파일(엔진 디코더) 비교. sounds_used: 소리 ID → (작성 파일, gain)."""
    by_file = {}
    for sid, (path, gain) in sounds_used.items():
        rel = sid.rsplit("@", 1)[0]
        by_file.setdefault((rel, path), gain)
    worst, bad, n_lossy = 0.0, [], 0
    tmp = tempfile.mkdtemp()
    for (rel, path), gain in sorted(by_file.items()):
        conv = auth / path
        r = subprocess.run([TRACKC, "decode", str(conv), os.path.join(tmp, "x.raw")], capture_output=True, text=True, encoding="utf-8")
        if r.returncode:
            bad.append((rel, "엔진 디코딩 실패 " + r.stderr.strip()[:80]))
            continue
        ch = int(re.search(r" ch (\d+)", r.stdout).group(1))
        ext = conv.suffix.lower()
        info = sf.info(str(conv))
        is_float = ext == ".ogg"
        raw = np.fromfile(os.path.join(tmp, "x.raw"), dtype="<f4" if is_float else "<i4").reshape(-1, ch)
        src = src_folder / rel
        lossless = ext in (".ogg", ".wav") or (ext == ".flac" and (src.read_bytes()[:4] == b"fLaC" or _pcm_wav(src)))
        if lossless and not is_float:
            ref, _ = sf.read(str(src), dtype="int32", always_2d=True)
            bits = {"PCM_16": 16, "PCM_24": 24, "PCM_S8": 8, "PCM_U8": 8, "MS_ADPCM": 16}[info.subtype]
            ok = ref.shape == raw.shape and np.array_equal(ref >> (32 - bits), raw)
            if not ok:
                bad.append((rel, f"무손실 PCM 다름 (모양 {ref.shape} / {raw.shape})"))
        elif lossless:
            ref, _ = sf.read(str(src), dtype="float32", always_2d=True)
            e = float(np.abs(ref - raw).max()) if ref.shape == raw.shape else float("inf")
            if e > 1e-6:
                bad.append((rel, f"Vorbis 차 {e:.2e}"))
        else:
            n_lossy += 1
            try:
                ref, _ = sf.read(str(src), dtype="float64", always_2d=True)
            except RuntimeError:
                ref = None
            if ref is None or ref.shape[1] > 2:
                ff = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(src), "-f", "f64le", "-acodec", "pcm_f64le", "-ac", str(ch), "-"],
                                    capture_output=True, check=True).stdout
                ref = np.frombuffer(ff, dtype="<f8").reshape(-1, ch)
            g = float(gain or 1)
            got = raw.astype(np.float64) / 2**23 * g
            e = float(np.abs(ref - got).max()) if ref.shape == got.shape else float("inf")
            worst = max(worst, e)
            if e > g * 2**-23 * 1.01:
                bad.append((rel, f"손실 경로 오차 {e:.2e} > FLAC24 양자화 × gain"))
    return len(by_file), n_lossy, worst, bad


def _pcm_wav(p):
    try:
        return sf.info(str(p)).subtype in ("PCM_16", "PCM_24", "PCM_U8", "PCM_S8")
    except RuntimeError:
        return False


# ------------------------------------------------------------------ 대조
def compare(e, g):
    diffs = []
    for k in ("notes", "autos", "lsnds"):
        a, b = collections.Counter(map(tuple, e[k])), collections.Counter(map(tuple, g[k]))
        if a != b:
            diffs.append((k, sorted((a - b).elements())[:3], sorted((b - a).elements())[:3]))
    return diffs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("auth")
    ap.add_argument("trk")
    ap.add_argument("--combos", type=int, default=64)
    ap.add_argument("--no-pcm", action="store_true")
    a = ap.parse_args()
    src, auth = pathlib.Path(a.src), pathlib.Path(a.auth)
    status = json.loads((auth / "status.json").read_text(encoding="utf-8"))
    dump = load_dump(a.trk)
    files = Files(src)
    bad, seen_charts = 0, set()
    charts_src = sorted(p.relative_to(src).as_posix() for p in src.rglob("*") if p.is_file() and p.suffix.lower() in {".bms", ".bme", ".bml", ".pms", ".bmson"})
    for rel in charts_src:
        st = status.get(rel)
        if st is None:
            print(f"{rel}: status.json에 없음 (누락)")
            bad += 1
            continue
        if st["status"] in ("unsupported", "failed"):
            print(f"{rel}: {st['status']} — {st['reason'][:100]}")
            if st["status"] == "failed":
                bad += 1
            continue
        raw = (src / rel).read_bytes()
        for cid in st["charts"]:
            seen_charts.add(cid)
            c = dump.get(cid)
            if c is None:
                print(f"{cid}: 묶음에 없음")
                bad += 1
                continue
            if rel.lower().endswith(".bmson"):
                combos, mk = [{}], lambda vals: expect_bmson(raw, rel, files, src)
            else:
                text = text_of(raw)
                layout = layout_of(src / rel, text)
                _, sizes = run(text, lambda k, n: 1)
                fixed = dict((int(x[1:].split("=")[0]) - 1, int(x.split("=")[1])) for x in cid.split("~")[1:])
                free = [k for k in range(len(sizes)) if k not in fixed]
                total = 1
                for k in free:
                    total *= sizes[k]
                if total <= a.combos:
                    combos = [dict(fixed)]
                    for k in free:
                        combos = [{**cb, k: v} for cb in combos for v in range(1, sizes[k] + 1)]
                else:
                    rnd = random.Random(1)
                    combos = [{**fixed, **{k: 1 for k in free}}, {**fixed, **{k: sizes[k] for k in free}}]
                    combos += [{**fixed, **{k: rnd.randint(1, sizes[k]) for k in free}} for _ in range(a.combos - 2)]
                mk = lambda vals, text=text, layout=layout: expect_bms(text, rel, files, layout, lambda k, n: vals.get(k, 1))
            fails, lo_bad, sc_bad = 0, [], 0
            for vals in combos:
                e, g = mk(vals), got_for(c, vals)
                d = compare(e, g)
                if d:
                    fails += 1
                    if fails == 1:
                        print(f"   {cid} 조합 {vals if len(vals) < 6 else '…'} 차이:", d)
                for k, v in e["losses"].items():
                    if c["losses"].get(k, 0) < v or (not c["variants"] and c["losses"].get(k, 0) != v):
                        lo_bad.append((k, v, c["losses"].get(k, 0)))
                for (lane, t), p in e["pos"].items() if vals is combos[0] else []:
                    q = scroll_at(c["scroll"], t)
                    if abs(q - float(p)) > scroll_tol(c["scroll"], t, float(p)):
                        sc_bad += 1
            ok = not fails and not lo_bad and not sc_bad
            bad += not ok
            n = len(mk(combos[0])["notes"])
            print(f"{cid:34} 조합 {len(combos):4}  노트 {n:5}  손실 {dict(c['losses']) or '-'}  "
                  f"{'1:1 일치' if ok else f'다름 (조합 {fails}, 손실 {lo_bad[:2]}, 스크롤 {sc_bad})'}")
    extra = set(dump) - seen_charts
    if extra:
        print("status.json에 없는 묶음 차분:", sorted(extra))
        bad += 1
    if not a.no_pcm:
        tt = tomllib.loads((auth / "track.toml").read_text(encoding="utf-8"))
        used = {s["id"]: (s["file"], Fraction(s["gain"]) if "gain" in s else None) for s in tt["sound"]}
        nf, nl, worst, pb = pcm_check(src, auth, used)
        print(f"소리 파일 {nf} (손실 경로 {nl}, 최대 오차 {worst:.2e}): {'모두 일치' if not pb else f'불일치 {len(pb)} {pb[:3]}'}")
        bad += bool(pb)
    print("결과:", "모두 일치" if not bad else f"불일치 {bad}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
