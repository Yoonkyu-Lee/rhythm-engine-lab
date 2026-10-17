"""BMS/BMSON 폴더 → Track 작성 형식 (CP3 3-2, BMS_컨버터.md B-1: 폴더 하나 = Track 하나).

    python -X utf8 cp3/converters/bms2track.py <BMS 폴더>... [--out cp3/content/bms] [--ids cp3/content/bms/ids.json]

폴더 사이 같은 곡 합치기·편입(B-2·B-3)은 이 단계에 없다. 폴더 안 차분은 같은 원점(shift 0).

채보 (BMS 텍스트: .bms .bme .bml .pms, 그리고 .bmson)
- 인코딩: UTF-8(BOM 포함) → cp932 → cp949 순으로 판별해 보고서에. 판별 실패면 cp932로 대체 문자.
- 레이아웃: 쓰인 채널로 고른다. .pms → "9K"(11–15, 22–25). 2P 채널이 있으면 DP("14+2" 또는 18·19·28·29가 없으면 "10+2"),
  아니면 SP("7+1", 18·19가 없으면 "5+1"). 1P 16·2P 26 = 스크래치, 17·27(페달)은 버리고 보고.
- 시각: 박 단위 "마디:박"(마디 길이 02는 meter로), BPM(03 16진, 08 확장), STOP(09, 값/48 박 = 1/192 마디).
  음수 BPM → 시간은 절댓값, 화면은 명시 스크롤의 음수 속도. 0 BPM → 표현할 수 없어 그 차분은 미지원(보고서).
  #SCROLL(채널 SC) → 명시 스크롤(속도 = |BPM| × SCROLL × 부호). #SPEED(SP)는 버리고 보고.
- 노트: 1x/2x 보이는 노트, 3x/4x 투명 노트 → 빈 레인 소리 바꾸기, 5x/6x LNTYPE 1 롱노트(짝), LNOBJ 끝 표시, Dx/Ex 지뢰(피해량은 규칙 기본).
  01 = 자동재생. 04·06·07(BGA)은 CP4 — 보고만.
- #RANDOM/#SETRANDOM + #IF/#ELSEIF/#ELSE/#ENDIF(+ #ENDRANDOM): 변형 r1, r2…(선택지 "1"…"n"), 블록 안 이벤트는 when 조건,
  중첩은 AND. 시간 지도를 바꾸는 줄(02·03·08·09·SC·#BPMxx 등)이 조건 블록 안에 있으면 그 차분은 미지원(보고서, 명세 2-4).
  #SWITCH/#CASE는 미지원.
- 기준 스크롤 = 가장 오래 유지된 BPM(시간 기준, 같으면 큰 값). 후보(시작·최빈·최고·최저)는 보고서에.
소리
- #WAVxx 이름 → 폴더의 실제 파일(대소문자 무시, 확장자 대체 .ogg → .wav → .flac → .mp3). 없으면 소리 없는 노트(보고).
- 같은 파일을 가리키는 #WAV 여럿 → 소리 ID 하나(파일 상대 경로). 재발음은 끊음(retrigger = "cut", LR2·beatoraja).
- Ogg Vorbis·FLAC·MS-ADPCM WAV는 그대로, PCM WAV → FLAC(무손실), 실수 WAV → FLAC24(손실), 그 밖(MP3·Opus·IMA ADPCM 등) →
  ffmpeg → FLAC16(손실), 3채널 이상 → 스테레오 다운믹스(ffmpeg 기본 행렬 ⚑, 손실). 손실 경로는 보고서에.
- #VOLWAV는 모든 이벤트 이득으로.
출력: <out>/<폴더 이름>/ (track.toml, charts/*.toml, audio/, report.md, expected.json — check_bms.py가 대조).
"""
import argparse, collections, hashlib, json, os, pathlib, re, subprocess, sys, uuid
from fractions import Fraction as F

import numpy as np
import soundfile as sf

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXTS = {".bms", ".bme", ".bml", ".pms"}
SND_ORDER = [".ogg", ".wav", ".flac", ".mp3", ".m4a", ".opus"]
TIMING_CH = {"02", "03", "08", "09", "SC", "SP"}

LAYOUTS = {
    "5+1": [("SC", "scratch")] + [(f"K{i}", "key") for i in range(1, 6)],
    "7+1": [("SC", "scratch")] + [(f"K{i}", "key") for i in range(1, 8)],
    "10+2": [("SC1", "scratch")] + [(f"K{i}", "key") for i in range(1, 11)] + [("SC2", "scratch")],
    "14+2": [("SC1", "scratch")] + [(f"K{i}", "key") for i in range(1, 15)] + [("SC2", "scratch")],
    "9K": [(f"K{i}", "key") for i in range(1, 10)],
}
GROUPS = {
    "5+1": [("main", [f"K{i}" for i in range(1, 6)])],
    "7+1": [("main", [f"K{i}" for i in range(1, 8)])],
    "10+2": [("1p", [f"K{i}" for i in range(1, 6)]), ("2p", [f"K{i}" for i in range(6, 11)])],
    "14+2": [("1p", [f"K{i}" for i in range(1, 8)]), ("2p", [f"K{i}" for i in range(8, 15)])],
    "9K": [("main", [f"K{i}" for i in range(1, 10)])],
}
KEY_TAIL = {"1": 1, "2": 2, "3": 3, "4": 4, "5": 5, "8": 6, "9": 7}  # 1P/2P 채널 둘째 자리 → 건반 번호(7키 기준)


def lane_of(layout, side, tail):
    """(레이아웃, 1P=1/2P=2, 채널 둘째 자리) → 레인 id 또는 None."""
    if layout == "9K":
        m = {(1, "1"): 1, (1, "2"): 2, (1, "3"): 3, (1, "4"): 4, (1, "5"): 5, (2, "2"): 6, (2, "3"): 7, (2, "4"): 8, (2, "5"): 9}
        k = m.get((side, tail))
        return f"K{k}" if k else None
    if tail == "6":
        if layout in ("5+1", "7+1"):
            return "SC" if side == 1 else None
        return "SC1" if side == 1 else "SC2"
    k = KEY_TAIL.get(tail)
    if not k:
        return None
    if layout in ("5+1", "7+1"):
        if side != 1 or (layout == "5+1" and k > 5):
            return None
        return f"K{k}"
    if layout == "10+2" and k > 5:
        return None
    return f"K{k}" if side == 1 else f"K{k + (7 if layout == '14+2' else 5)}"


def q(x):
    f = F(x)
    return str(f.numerator) if f.denominator == 1 else f"{f.numerator}/{f.denominator}"


def tq(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ") + '"'


def safe(s):
    return re.sub(r'[\\/:*?"<>|]', "_", s).strip().rstrip(".") or "_"


def decode_text(b):
    for enc in ("utf-8-sig", "cp932", "cp949"):
        try:
            return b.decode(enc), enc
        except UnicodeDecodeError:
            pass
    return b.decode("cp932", errors="replace"), "cp932(대체 문자)"


RX_DATA = re.compile(r"#(\d{3})([0-9A-Za-z]{2}):(.*)$")
RX_KV = re.compile(r"#([A-Za-z][A-Za-z0-9]*)(?:[\s:]+(.*))?$")


class Unsupported(Exception):
    pass


def parse_bms(raw, rep):
    """BMS 텍스트 → (머리, 정의, 데이터 줄 [(마디, 채널, 값, 조건)], 변형 [(id, n)])."""
    text, enc = decode_text(raw)
    rep[f"인코딩 {enc}"] += 1
    head, wavs, bpms, stops, scrolls, data = {}, {}, {}, {}, {}, []
    variants = []
    stack = []  # [("rand", 변형 번호, 값)|("if", 변형 번호, 선택, 이미 맞음)]
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("#"):
            continue
        u = line.upper()
        if u.startswith("#SETRANDOM") or u.startswith("#RANDOM"):
            try:
                n = int(u.split()[1]) if len(u.split()) > 1 else 0
            except ValueError:
                n = 0
            if n < 1:
                raise Unsupported(f"#RANDOM 값 오류: {line}")
            variants.append((f"r{len(variants) + 1}", n))
            stack.append(["rand", len(variants) - 1])
            continue
        if u.startswith("#IF"):
            vi = next((s[1] for s in reversed(stack) if s[0] == "rand"), None)
            if vi is None:
                raise Unsupported("#IF가 #RANDOM 밖에 있음")
            try:
                ch = int(u.split()[1])
            except (ValueError, IndexError):
                raise Unsupported(f"#IF 값 오류: {line}")
            stack.append(["if", vi, {ch}])
            continue
        if u.startswith("#ELSEIF"):
            if not stack or stack[-1][0] != "if":
                raise Unsupported("#ELSEIF 짝 없음")
            try:
                ch = int(u.split()[1])
            except (ValueError, IndexError):
                raise Unsupported(f"#ELSEIF 값 오류: {line}")
            stack[-1][2] = {ch}
            continue
        if u.startswith("#ELSE"):
            if not stack or stack[-1][0] != "if":
                raise Unsupported("#ELSE 짝 없음")
            vi = stack[-1][1]
            used = stack[-1][2]
            stack[-1][2] = set(range(1, variants[vi][1] + 1)) - used
            continue
        if u.startswith("#ENDIF") or u.startswith("#END IF"):
            if stack and stack[-1][0] == "if":
                stack.pop()
            continue
        if u.startswith("#ENDRANDOM"):
            while stack and stack[-1][0] == "if":
                stack.pop()
            if stack:
                stack.pop()
            continue
        if u.startswith("#SWITCH") or u.startswith("#CASE") or u.startswith("#SETSWITCH"):
            raise Unsupported("#SWITCH/#CASE 미지원")
        conds = tuple((s[1], frozenset(s[2])) for s in stack if s[0] == "if")
        m = RX_DATA.match(line)
        if m:
            meas, ch, val = int(m.group(1)), m.group(2).upper(), m.group(3).strip()
            if conds and ch in TIMING_CH:
                raise Unsupported(f"시간 지도를 바꾸는 줄이 #IF 안에 있음 (채널 {ch}) — 명세 2-4")
            data.append((meas, ch, val, conds))
            continue
        m = RX_KV.match(line)
        if not m:
            continue
        k, v = m.group(1).upper(), (m.group(2) or "").strip()
        if conds and (k.startswith("BPM") or k.startswith("STOP") or k.startswith("SCROLL")):
            raise Unsupported("#IF 안의 BPM·STOP·SCROLL 정의 — 명세 2-4")
        if conds and k.startswith("WAV"):
            rep["#IF 안의 #WAV (전역으로 처리)"] += 1
        if k.startswith("WAV") and len(k) == 5:
            wavs[k[3:]] = v
        elif k.startswith("BPM") and len(k) == 5:
            bpms[k[3:]] = v
        elif k.startswith("STOP") and len(k) == 6:
            stops[k[4:]] = v
        elif k.startswith("SCROLL") and len(k) == 8:
            scrolls[k[6:]] = v
        else:
            head.setdefault(k, v)
    return head, wavs, bpms, stops, scrolls, data, variants, enc


def fdec(s):
    """BMS 수 → 정확한 유리수 (지수 표기 허용)."""
    s = s.strip()
    try:
        return F(s)
    except ValueError:
        return F(float(s))


def chart_from_bms(path, raw, rep):
    head, wavs, bpms, stops, scrolls, data, variants, enc = parse_bms(raw, rep)
    base62 = head.get("BASE", "36").strip() == "62"
    norm = (lambda v: v) if base62 else (lambda v: v.upper())
    lnobj = norm(head.get("LNOBJ", "")) if head.get("LNOBJ") else ""
    lntype = (head.get("LNTYPE", "1") or "1").strip()
    if lntype not in ("1", ""):
        raise Unsupported(f"LNTYPE {lntype} 미지원")
    try:
        bpm0 = fdec(head.get("BPM", "130") or "130")
    except Exception:
        bpm0 = F(130)
    mlen = {}
    for meas, ch, v, _ in data:
        if ch == "02":
            mlen[meas] = fdec(v)
    # 쓰인 채널로 레이아웃
    used = set()
    for meas, ch, v, _ in data:
        if len(ch) == 2 and ch[0] in "123456" and any(v[i:i + 2] != "00" for i in range(0, len(v) - 1, 2)):
            used.add(ch)
    two_p = any(c[0] in "246" and c[1] in "123456789" for c in used)
    if path.suffix.lower() == ".pms":
        layout = "9K"
    elif two_p:
        layout = "14+2" if any(c[1] in "89" for c in used) else "10+2"
    else:
        layout = "7+1" if any(c[1] in "89" for c in used) else "5+1"

    def objs(meas, payload):
        n = len(payload) // 2
        for i in range(n):
            v = norm(payload[2 * i:2 * i + 2])
            if v != "00":
                yield meas, F(i, n), v

    def pos_str(meas, frac):
        L = 4 * mlen.get(meas, F(1))
        return f"{meas}:{q(L * frac)}"

    def pos_beat(meas, frac, mstart):
        return mstart[meas] + 4 * mlen.get(meas, F(1)) * frac

    last = max([m for m, _, _, _ in data], default=0)
    mstart = [F(0)]
    for k in range(last + 2):
        mstart.append(mstart[-1] + 4 * mlen.get(k, F(1)))
    # 템포·정지·스크롤 변화 (조건 없음 보장됨)
    tempo, stop_list, scroll_ch = {}, [], {}
    tempo[F(0)] = bpm0
    for meas, ch, v, _ in data:
        if ch == "03":
            for m_, fr, x in objs(meas, v):
                try:
                    tempo[pos_beat(m_, fr, mstart)] = F(int(x, 16))
                except ValueError:
                    rep["03 채널 값 오류"] += 1
        elif ch == "08":
            for m_, fr, x in objs(meas, v):
                if x in bpms:
                    tempo[pos_beat(m_, fr, mstart)] = fdec(bpms[x])
                else:
                    rep["없는 #BPMxx"] += 1
        elif ch == "09":
            for m_, fr, x in objs(meas, v):
                if x in stops:
                    stop_list.append((pos_beat(m_, fr, mstart), fdec(stops[x]) / 48))
                else:
                    rep["없는 #STOPxx"] += 1
        elif ch == "SC":
            for m_, fr, x in objs(meas, v):
                if x in scrolls:
                    scroll_ch[pos_beat(m_, fr, mstart)] = fdec(scrolls[x])
        elif ch == "SP":
            rep["#SPEED 버림"] += 1
    if any(b == 0 for b in tempo.values()):
        raise Unsupported("0 BPM은 표현할 수 없음")
    neg = any(b < 0 for b in tempo.values())
    # 노트·소리
    autos, notes, lsnds = [], [], []  # (위치 문자열, 박, wav, 조건, ...)
    ln_open = {}
    lane_ev = collections.defaultdict(list)
    for meas, ch, v, cd in data:
        if ch == "01":
            for m_, fr, x in objs(meas, v):
                autos.append((pos_str(m_, fr), pos_beat(m_, fr, mstart), x, cd))
        elif len(ch) == 2 and ch[0] in "123456DE" and ch[1] in "123456789":
            if ch[1] == "7":
                rep["페달(17·27) 버림"] += 1
                continue
            side = 1 if ch[0] in "135D" else 2
            lane = lane_of(layout, side, ch[1])
            if lane is None:
                rep[f"레이아웃 {layout} 밖 채널 {ch} 버림"] += 1
                continue
            kind = {"1": "note", "2": "note", "3": "inv", "4": "inv", "5": "ln", "6": "ln", "D": "mine", "E": "mine"}[ch[0]]
            for m_, fr, x in objs(meas, v):
                lane_ev[lane].append((pos_beat(m_, fr, mstart), pos_str(m_, fr), kind, x, cd))
        elif ch in ("04", "06", "07", "0A"):
            rep["BGA 채널 (CP4)"] += 1
    for lane, evs in lane_ev.items():
        evs.sort(key=lambda e: e[0])
        last_note = None
        ln_start = None
        for beat, ps, kind, x, cd in evs:
            if kind == "inv":
                lsnds.append((ps, beat, lane, x, cd))
            elif kind == "mine":
                notes.append(dict(at=ps, beat=beat, lane=lane, kind="mine", end=None, wav=None, cond=cd))
            elif kind == "ln":
                if ln_start is None:
                    ln_start = (ps, beat, x, cd)
                else:
                    notes.append(dict(at=ln_start[0], beat=ln_start[1], lane=lane, kind="long", end=ps, end_beat=beat, wav=ln_start[2], cond=ln_start[3]))
                    ln_start = None
            else:
                if lnobj and x == lnobj:  # LNOBJ 끝 표시: 앞 탭 노트를 롱노트로 (끝 표시 자체는 소리 없음)
                    if last_note is not None and last_note["kind"] == "tap":
                        last_note["kind"], last_note["end"], last_note["end_beat"] = "long", ps, beat
                        last_note = None
                    else:
                        rep["앞 노트 없는 LNOBJ 끝 표시 버림"] += 1
                    continue
                n = dict(at=ps, beat=beat, lane=lane, kind="tap", end=None, wav=x, cond=cd)
                notes.append(n)
                last_note = n
        if ln_start is not None:
            rep["짝 없는 롱노트 시작 → 탭"] += 1
            notes.append(dict(at=ln_start[0], beat=ln_start[1], lane=lane, kind="tap", end=None, wav=ln_start[2], cond=ln_start[3]))
    return dict(head=head, wavs=wavs, layout=layout, mlen=mlen, last=last, tempo=tempo, stops=stop_list, scroll=scroll_ch, neg=neg,
                autos=autos, notes=notes, lsnds=lsnds, variants=variants, enc=enc, mstart=mstart)


class Resolver:
    def __init__(self, folder):
        self.exact, self.stem = {}, collections.defaultdict(list)
        for dp, _, fs in os.walk(folder):
            for f in fs:
                rel = os.path.relpath(os.path.join(dp, f), folder).replace("\\", "/")
                r = rel.lower()
                self.exact[r] = rel
                s, e = os.path.splitext(r)
                if e in SND_ORDER:
                    self.stem[s].append((SND_ORDER.index(e), rel))

    def __call__(self, name, chart_dir=""):
        n = name.replace("\\", "/").strip().lower()
        base = (chart_dir.lower() + "/") if chart_dir else ""
        for cand in (base + n, n):
            if cand in self.exact and os.path.splitext(cand)[1] in SND_ORDER:
                return self.exact[cand]
            s = os.path.splitext(cand)[0]
            if s in self.stem:
                return sorted(self.stem[s])[0][1]
        return None


def put_audio(src, dst_dir, rel, rep):
    """원본 소리 → 작성 형식 audio/ 아래 파일 (상대 경로)."""
    b = src.read_bytes()
    stem = safe(os.path.splitext(rel)[0].replace("/", "__"))
    keep = None
    if b[:4] == b"OggS" and b"\x01vorbis" in b[:64]:
        keep = ".ogg"
    elif b[:4] == b"fLaC":
        keep = ".flac"
    elif b[:4] == b"RIFF" and b[8:12] == b"WAVE":
        i = b.find(b"fmt ")
        tag = int.from_bytes(b[i + 8:i + 10], "little") if i >= 0 else -1
        ch = int.from_bytes(b[i + 10:i + 12], "little") if i >= 0 else 0
        bits = int.from_bytes(b[i + 22:i + 24], "little") if i >= 0 else 0
        if tag == 2 and ch <= 2:
            keep = ".wav"
        elif tag in (1, 0xFFFE) and ch <= 2 and bits in (8, 16, 24):
            dst = dst_dir / (stem + ".flac")
            if not dst.exists():
                a, rate = sf.read(str(src), dtype="int32", always_2d=True)
                sf.write(str(dst), a, rate, format="FLAC", subtype={8: "PCM_S8", 16: "PCM_16", 24: "PCM_24"}[bits])
            rep["PCM WAV → FLAC (무손실)"] += 1
            return dst
        elif tag == 3 and ch <= 2:  # 실수 WAV → FLAC24
            dst = dst_dir / (stem + ".flac")
            if not dst.exists():
                a, rate = sf.read(str(src), dtype="float64", always_2d=True)
                pk = float(np.abs(a).max()) if a.size else 0
                if pk > 1:
                    a = a / pk
                    rep["실수 WAV 넘침 감쇠 (이득 손실 ⚑)"] += 1
                sf.write(str(dst), a, rate, format="FLAC", subtype="PCM_24")
            rep["실수 WAV → FLAC24 (손실)"] += 1
            return dst
    if keep:
        dst = dst_dir / (stem + keep)
        if not dst.exists():
            dst.write_bytes(b)
        rep[f"그대로 {keep}"] += 1
        return dst
    # 그 밖: ffmpeg → FLAC16 (손실), 3채널 이상은 스테레오로
    dst = dst_dir / (stem + ".flac")
    if not dst.exists():
        r = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(src), "-ac", "2", "-c:a", "flac", "-sample_fmt", "s16", str(dst)],
                           capture_output=True)
        if r.returncode:
            raise Unsupported(f"소리 변환 실패 {rel}: {r.stderr.decode('utf-8', 'replace')[:100]}")
    rep["기타 → FLAC16 (손실)"] += 1
    return dst


def ref_scroll(tempo_pts, beat_end, rep):
    """가장 오래(시간) 유지된 |BPM| — 같으면 큰 값. 후보를 보고서에."""
    pts = sorted(tempo_pts.items())
    dur = collections.Counter()
    for i, (b, bpm) in enumerate(pts):
        nb = pts[i + 1][0] if i + 1 < len(pts) else max(beat_end, b)
        dur[abs(bpm)] += (nb - b) * 60 / abs(bpm)
    best = max(dur.items(), key=lambda kv: (kv[1], kv[0]))[0]
    vals = [abs(v) for _, v in pts]
    rep["_ref"] = f"시작 {float(abs(pts[0][1])):g}, 최빈 {float(best):g}, 최고 {float(max(vals)):g}, 최저 {float(min(vals)):g}"
    return best


def convert_folder(folder, out_root, ids):
    folder = pathlib.Path(folder)
    files = sorted(p for p in folder.rglob("*") if p.suffix.lower() in EXTS | {".bmson"})
    if not files:
        return None
    out = pathlib.Path(out_root) / safe(folder.name)
    for sub in ("charts", "audio"):
        (out / sub).mkdir(parents=True, exist_ok=True)
    for old in (out / "charts").glob("*.toml"):
        old.unlink()
    rep = collections.Counter()
    res = Resolver(folder)
    sounds = {}  # 파일 상대 경로 → 작성 형식 상대 경로
    docs, expected, layouts, titles, artists, sources = [], {}, set(), collections.Counter(), collections.Counter(), []
    for p in files:
        raw = p.read_bytes()
        crep = collections.Counter()
        try:
            if p.suffix.lower() == ".bmson":
                raise Unsupported("BMSON은 B-1 다음 단계 (BMSON 2개, 2차 연구 Q9)")
            c = chart_from_bms(p, raw, crep)
        except Unsupported as e:
            rep[f"미지원 차분: {e}"] += 1
            continue
        if not c["notes"]:
            rep["노트 없는 차분 건너뜀"] += 1
            continue
        sources.append((p.relative_to(folder).as_posix(), hashlib.sha256(raw).hexdigest()))
        head = c["head"]
        titles[head.get("TITLE", "")] += 1
        artists[head.get("ARTIST", "")] += 1
        chart_dir = p.parent.relative_to(folder).as_posix()
        chart_dir = "" if chart_dir == "." else chart_dir
        volwav = F(int(head["VOLWAV"]), 100) if head.get("VOLWAV", "").strip().isdigit() else F(1)

        def snd(x):
            name = c["wavs"].get(x)
            if not name:
                crep["정의 없는 #WAV"] += 1
                return None
            rel = res(name, chart_dir)
            if not rel:
                rep["파일 없는 소리"] += 1
                return None
            if rel not in sounds:
                sounds[rel] = put_audio(folder / rel, out / "audio", rel, rep).relative_to(out).as_posix()
            return rel

        cid = safe(re.sub(r"\s+", "_", p.stem.lower()))[:60]
        while any(cid == d[0] for d in docs):
            cid += "_"
        layouts.add(c["layout"])
        beat_end = c["mstart"][-1]
        refs = ref_scroll(c["tempo"], beat_end, crep)
        rep[f"기준 스크롤 후보 ({cid}): {crep.pop('_ref')}"] += 1
        tempo_pts = sorted(c["tempo"].items())
        # 마디 길이 02는 그 마디에만 → 다음 마디는 다시 4박
        mdict = {0: F(4)}
        for m, L in sorted(c["mlen"].items()):
            mdict[m] = 4 * L
            if m + 1 not in c["mlen"]:
                mdict.setdefault(m + 1, F(4))
        meter_lines = [f'{{ measure = {m}, length = "{q(L)}" }}' for m, L in sorted(mdict.items())]
        variants = c["variants"]

        def when(cd):
            terms = []
            for vi, chs in cd:
                if len(chs) != 1:
                    # #ELSE가 여러 선택을 덮으면 when 하나로 못 적는다 → 선택마다 이벤트를 복제 (AND 조건은 같은 변형에 한 번)
                    raise Unsupported("#ELSE가 여러 선택을 덮음 (복제 미구현)")
                terms.append(f"r{vi + 1}={next(iter(chs))}")
            if not terms:
                return []
            return [f"when = {tq(terms[0])}"] if len(terms) == 1 else ["when = [" + ", ".join(tq(t) for t in terms) + "]"]

        lines = [f"# {p.relative_to(folder).as_posix()} (인코딩 {c['enc']})", f"id = {tq(cid)}",
                 f"name = {tq(head.get('SUBTITLE') or p.stem)}", f"level = {tq(head.get('PLAYLEVEL', ''))}", f'layout = "{c["layout"]}"',
                 'shift = "0"', f'reference_scroll = "{q(refs)}"', "", "[rules]", 'long_note = "ln"', 'empty_lane = "previous_note"', "",
                 "[timing]", 'unit = "beat"', "tempo = [ " + ", ".join(f'{{ at = "{q(b)}", bpm = "{q(abs(v))}" }}' for b, v in tempo_pts) + " ]",
                 "meter = [ " + ", ".join(meter_lines) + " ]",
                 "stops = [ " + ", ".join(f'{{ at = "{q(b)}", beats = "{q(d)}" }}' for b, d in sorted(c["stops"])) + " ]", ""]
        if c["neg"] or c["scroll"]:
            # 명시 스크롤: 템포·SCROLL 변화마다 속도 = |BPM| × SCROLL × 부호
            keys = sorted(set(c["tempo"]) | set(c["scroll"]))
            cur_b, cur_s, pts = None, F(1), []
            for b in keys:
                if b in c["tempo"]:
                    cur_b = c["tempo"][b]
                if b in c["scroll"]:
                    cur_s = c["scroll"][b]
                pts.append(f'{{ at = "{q(b)}", speed = "{q(cur_b * cur_s)}" }}')
            lines += ["[scroll]", "points = [ " + ", ".join(pts) + " ]", ""]
            rep["명시 스크롤 (음수 BPM·#SCROLL)"] += 1
        for vi, (vid, n) in enumerate(variants):
            lines += ["[[variant]]", f'id = "{vid}"', "choices = [" + ", ".join(f'"{k}"' for k in range(1, n + 1)) + "]", ""]
        try:
            exp_s, exp_n = [], []
            for at, beat, x, cd in c["autos"]:
                s = snd(x)
                if s:
                    lines += ["[[event]]", f'at = "{at}"', f"sound = {tq(s)}"] + ([f'gain = "{q(volwav)}"'] if volwav != 1 else []) + when(cd) + [""]
                    exp_s.append((str(beat), s, [[vi, sorted(chs)] for vi, chs in cd]))
            lane_order = [l for l, _ in LAYOUTS[c["layout"]]]
            # 같은 레인 같은 위치 중복 노트: 하나만 (조건이 같을 때)
            seen = set()
            for n in sorted(c["notes"], key=lambda n: (lane_order.index(n["lane"]), n["beat"])):
                key = (n["lane"], n["beat"], tuple(n["cond"]))
                if key in seen:
                    rep["같은 위치 중복 노트 버림"] += 1
                    continue
                seen.add(key)
                s = snd(n["wav"]) if n["wav"] else None
                kind = n["kind"]
                lines += ["[[note]]", f'at = "{n["at"]}"', f'lane = "{n["lane"]}"', f'kind = "{kind}"']
                if kind == "long":
                    lines.append(f'end = "{n["end"]}"')
                if s:
                    lines.append(f"sounds = [{tq(s)}]" if volwav == 1 else f'sounds = [{{ sound = {tq(s)}, gain = "{q(volwav)}" }}]')
                lines += when(n["cond"]) + [""]
                exp_n.append((n["lane"], str(n["beat"]), str(n.get("end_beat", n["beat"])), kind, [[vi, sorted(chs)] for vi, chs in n["cond"]]))
                if s:
                    exp_s.append((str(n["beat"]), s, [[vi, sorted(chs)] for vi, chs in n["cond"]]))
            for at, beat, lane, x, cd in sorted(c["lsnds"], key=lambda l: (lane_order.index(l[2]), l[1])):
                s = snd(x)
                if s:
                    lines += ["[[lane_sound]]", f'at = "{at}"', f'lane = "{lane}"', f"sound = {tq(s)}"] + when(cd) + [""]
        except Unsupported as e:
            rep[f"미지원 차분: {e}"] += 1
            continue
        docs.append((cid, "\n".join(lines)))
        expected[cid] = {"tempo": [[str(b), str(v)] for b, v in tempo_pts], "stops": [[str(b), str(d)] for b, d in sorted(c["stops"])],
                         "sounds": exp_s, "notes": exp_n}
        rep.update(crep)
    if not docs:
        return None
    for cid, doc in docs:
        (out / "charts" / f"{cid}.toml").write_text(doc, encoding="utf-8")
    key = folder.name
    if key not in ids:
        ids[key] = str(uuid.uuid7())
    title = titles.most_common(1)[0][0] or folder.name
    tl = [f"# BMS 폴더 {folder.name} — bms2track 변환 (B-1: 폴더 하나 = Track 하나)", 'format = "1.0"', f"id = {tq(ids[key])}", "revision = 1", "",
          "[info]", f"title = {tq(title)}", f"title_sort = {tq(title.lower())}", f"artist = {tq(artists.most_common(1)[0][0])}", 'genre = ""',
          f'tags = ["bms", "folder:{folder.name.replace(chr(34), "")}"]', "", "[origin]", f"anchor_chart = {tq(docs[0][0])}", ""]
    for name in sorted(layouts):
        tl += ["[[layout]]", f'id = "{name}"', "lanes = [" + ", ".join(f'{{ id = "{l}", role = "{r}" }}' for l, r in LAYOUTS[name]) + "]",
               "groups = [" + ", ".join(f'{{ id = "{g}", lanes = [' + ", ".join(f'"{x}"' for x in ls) + "] }" for g, ls in GROUPS[name]) + "]", ""]
    for rel in sorted(sounds):
        tl += ["[[sound]]", f"id = {tq(rel)}", f"file = {tq(sounds[rel])}", 'retrigger = "cut"', ""]
    tl += ["[source]", 'format = "bms"', 'converter = "bms2track 0.1"',
           "files = [" + ", ".join(f'{{ path = {tq(pth)}, hash = "sha256:{h}" }}' for pth, h in sources) + "]", ""]
    (out / "track.toml").write_text("\n".join(tl), encoding="utf-8")
    (out / "expected.json").write_text(json.dumps(expected, ensure_ascii=False), encoding="utf-8")
    (out / "report.md").write_text(f"# 변환 보고서: BMS {folder.name}\n\n- 차분 {len(docs)}: " + ", ".join(d[0] for d in docs) +
                                   f"\n- 소리 {len(sounds)}개\n" + "".join(f"- {k}: {v}\n" for k, v in sorted(rep.items())), encoding="utf-8")
    return out, len(docs), len(sounds), rep


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("folders", nargs="+")
    ap.add_argument("--out", default=str(ROOT / "cp3" / "content" / "bms"))
    a = ap.parse_args()
    out_root = pathlib.Path(a.out)
    out_root.mkdir(parents=True, exist_ok=True)
    idp = out_root / "ids.json"
    ids = json.loads(idp.read_text(encoding="utf-8")) if idp.exists() else {}
    for f in a.folders:
        before = len(ids)
        try:
            r = convert_folder(f, out_root, ids)
        except Exception as e:  # noqa: BLE001
            print(f"{f}: 실패 {e!r}", flush=True)
            continue
        if r is None:
            print(f"{f}: 차분 없음", flush=True)
            continue
        out, nc, ns, rep = r
        print(f"{out.name}: 차분 {nc}, 소리 {ns}, " + ", ".join(f"{k} {v}" for k, v in sorted(rep.items()) if not k.startswith("기준 스크롤")), flush=True)
        if len(ids) != before:
            idp.write_text(json.dumps(ids, indent=1, ensure_ascii=False), encoding="utf-8")


if __name__ == "__main__":
    main()
