"""BMS/BMSON 폴더 → Track 작성 형식 (CP3 3-2, BMS_컨버터.md B-1: 폴더 하나 = Track 하나). 0.2 = Codex 3-2 리뷰 반영.

    python -I -X utf8 cp3/converters/bms2track.py <BMS 폴더>... [--out cp3/content/bms]

폴더 사이 같은 곡 합치기·편입(B-2·B-3)은 이 단계에 없다. 폴더 안 차분은 같은 원점(shift 0).

해석 순서 (리뷰 3-2 #1·#2). BMS는 "#RANDOM 값을 먼저 정하고 그 결과를 평범한 BMS로 읽는" 형식이므로 그대로 따른다.
  1) 줄마다 조건(변형 → 허용 선택지 집합)을 붙여 읽는다. #SETRANDOM은 고정값(변형 아님), #ELSEIF·#ELSE는 앞 갈래를 뺀 나머지.
  2) **선택 조합마다** 평범한 BMS로 푼다: 정의·머리, 같은 위치 덮어쓰기, 롱노트 짝, LNOBJ가 모두 그 조합 안에서 정해진다.
  3) 같은 결과 객체를 모아 "그 객체가 나오는 조합 집합"을 when 조건(AND)으로 적는다. 한 AND로 안 되면 겹치지 않는 AND 여럿으로 복제.
  시간 지도·롱노트 규칙이 조합마다 다르면 명세 2-4대로 조합마다 차분을 따로 만든다(`<id>~r1=2~r2=1`, 16개까지). 조합 1024개 초과는 미지원.
BMS 채보 (.bms .bme .bml .pms)
- 인코딩: UTF-8(BOM) → cp932 → cp949 순으로 판별(보고서). 실패면 cp932 대체 문자.
- #BASE 62면 객체 ID 대소문자를 구분한다(명령 이름과 ID를 따로 읽음, 리뷰 #12). 그 밖은 ID를 대문자로.
- 레이아웃: 보이는 노트·롱노트·지뢰 채널로 고른다(투명 노트·페달 제외, 리뷰 #8). .pms → "9K"(11–15, 22–25). 2P 채널이 있으면 DP
  ("14+2", x8·x9가 없으면 "10+2"), 아니면 SP("7+1", 18·19가 없으면 "5+1"). x6 = 스크래치.
- 시각: 박 단위 "마디:박"(02 = 마디 길이), BPM(03 16진, 08 = #BPMxx·#EXBPMxx), STOP(09, 값/48 박 = 1/192 마디).
  음수 BPM → 시간은 절댓값, 화면은 명시 스크롤의 음수 속도. 0 BPM·0 이하 마디 길이 → 미지원. #SCROLL(SC) → 명시 스크롤.
  #SPEED(SP)는 표현하지 않음 → 차분 losses.
- 같은 채널·같은 위치(01 제외)는 **뒤에 적힌 정의가 덮어쓴다**(BMS 관례, 01 BGM은 겹쳐 쌓임). 다른 채널끼리 같은 레인·위치
  (보이는·롱노트·지뢰)는 뒤 정의 하나만 남긴다(보고서).
- 노트: 1x/2x 보이는 노트, 3x/4x 투명 노트 → 빈 레인 소리, 5x/6x LNTYPE 1 롱노트(채널마다 차례로 짝), LNOBJ(그 레인의 바로 앞 노트를
  롱노트로), Dx/Ex 지뢰(피해량은 CP3 규칙 기본 — 원작 값은 쓰지 않음, 보고서). 롱노트 규칙: #LNMODE 1 ln, 2 cn, 3 hcn (없으면 ln).
- 플레이 손실 → 차분 `losses`(불완전 변환, 리뷰 #4·#13): 롱노트 안에서 시작하는 노트, 페달(17·27), 레이아웃 밖 채널 — 소리는
  자동재생으로 옮겨 발음은 보존하고 판정 노트만 잃는다. 롱노트 안·레인 밖 지뢰는 버린다. #SPEED.
- 01 = 자동재생. BGA(04·06·07·0A)는 CP4 — 보고만. #SWITCH 계열·LNTYPE 2는 미지원.
BMSON (.bmson 1.0)
- resolution(박당 펄스, 기본 240), lines → 마디선(bars), bpm_events, stop_events(펄스), scroll_events(beatoraja 확장 → 명시 스크롤),
  sound_channels, mine_channels·key_channels(확장: 지뢰·투명 노트). mode_hint: beat-5k/7k/10k/14k, popn-9k.
  x = 레인(0·없음 = 자동재생; beat 모드는 1–7 건반, 8 스크래치, 2P는 +8 ⚑ beatoraja 대응), l = 롱노트 길이(info.ln_type 1 ln·2 cn·3 hcn).
- c(이어 재생)는 beatoraja와 같게: 채널마다 c = false 노트(또는 채널 첫 노트)에서 소리를 처음부터 내고, c = true 노트는 그 뒤 흐른
  시간만큼 뒤에서 시작한다. 다음 노트가 c = true면 이 노트 소리는 그 시각까지만(소리 `length`), 아니면 끝까지.
소리
- 논리 소리 ID = "<원본 파일 상대 경로>@<WAV ID>" — 같은 파일을 가리키는 #WAV 여럿은 다른 소리(README 4-1, 리뷰 #3). 재발음은 끊음
  (retrigger = "cut", LR2·beatoraja는 #WAV 번호마다 한 소리). BMSON은 "<경로>@<start>+<length>"(샘플).
- #WAV 이름 → 폴더의 실제 파일(대소문자 무시, 확장자 대체 .ogg → .wav → .flac → .mp3 → .m4a → .opus). 없으면 소리 없는 노트(보고).
- 출력 파일 이름 = 원본 경로 + 원본 바이트·변환기 판의 해시 → 이름 충돌과 낡은 파일 재사용이 없다(리뷰 #4).
- Ogg Vorbis·FLAC·MS-ADPCM WAV(2채널 이하)는 바이트 그대로, PCM WAV(8·16·24비트) → FLAC(무손실). 그 밖(실수·32비트 WAV, MP3, Opus,
  IMA ADPCM, 3채널 이상 = ffmpeg 기본 행렬로 스테레오 ⚑)은 실수로 디코딩 → ±1을 넘으면 최댓값으로 나눠 FLAC24 + 소리 gain = 최댓값.
  손실 경로는 되돌린 값 기준 최대·RMS 오차를 보고서에(리뷰 #7).
- #VOLWAV는 모든 이벤트 이득으로.
출력: <out>/<폴더 이름>-<원본 경로 해시 6자>/ — track.toml, charts/*.toml, audio/, report.md, status.json(원본 차분 파일마다
ok·incomplete·unsupported·failed). 차분 문서는 다 만든 뒤 한 번에 바꾼다(도중에 실패하면 이전 결과 그대로, 리뷰 #14).
"""
import argparse, collections, hashlib, itertools, json, math, os, pathlib, re, shutil, subprocess, uuid
from fractions import Fraction as F

import numpy as np
import soundfile as sf

CONVERTER = "bms2track 0.2.1"
ROOT = pathlib.Path(__file__).resolve().parents[2]
EXTS = {".bms", ".bme", ".bml", ".pms"}
SND_ORDER = [".ogg", ".wav", ".flac", ".mp3", ".m4a", ".opus"]
TIMING_CH = {"02", "03", "08", "09", "SC", "SP"}
BGA_CH = {"04", "06", "07", "0A"}
MAX_COMBOS = 1024
MAX_SPLIT = 16
SELFCHECK = 0  # --selfcheck N: 조합 N개 이하인 #RANDOM 차분은 국소 풀이를 전체 펼침과 대조

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
BMSON_MODES = {"beat-5k": "5+1", "beat-7k": "7+1", "beat-10k": "10+2", "beat-14k": "14+2", "popn-9k": "9K"}


class Unsupported(Exception):
    pass


def lane_of(layout, side, tail):
    """(레이아웃, 1P=1/2P=2, 채널 둘째 자리) → 레인 id 또는 None (페달 x7은 늘 None)."""
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


def bmson_lane(layout, x):
    if layout == "9K":
        return f"K{x}" if 1 <= x <= 9 else None
    side, k = (1, x) if x <= 8 else (2, x - 8)
    if k == 8:
        return lane_of(layout, side, "6")
    return lane_of(layout, side, {1: "1", 2: "2", 3: "3", 4: "4", 5: "5", 6: "8", 7: "9"}.get(k, "?"))


def q(x):
    f = F(x)
    return str(f.numerator) if f.denominator == 1 else f"{f.numerator}/{f.denominator}"


def tq(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ") + '"'


def safe(s):
    return re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", s).strip().rstrip(".") or "_"


def decode_text(b):
    for enc in ("utf-8-sig", "cp932", "cp949"):
        try:
            return b.decode(enc), enc
        except UnicodeDecodeError:
            pass
    return b.decode("cp932", errors="replace"), "cp932(대체 문자)"


def fnum(s, what):
    """BMS 수 → 정확한 유리수 (지수 표기 허용). 실패면 위치를 단 Unsupported."""
    s = (s or "").strip()
    try:
        return F(s)
    except (ValueError, ZeroDivisionError):
        try:
            v = float(s)
        except ValueError:
            raise Unsupported(f"{what}: 수가 아님 {s[:20]!r}") from None
        if not math.isfinite(v):
            raise Unsupported(f"{what}: 수가 아님 {s[:20]!r}")
        return F(v)


# ------------------------------------------------------------------ 1) 조건 붙여 읽기
RX_CTL = re.compile(r"#(SETRANDOM|RANDOM|ENDRANDOM|ELSEIF|ELSE|ENDIF|END\s*IF|IFEND|IF|SETSWITCH|SWITCH|CASE|SKIP|DEF|ENDSW)(?![0-9A-Za-z_])\s*(.*)$", re.I)
RX_DATA = re.compile(r"#(\d{3})([0-9A-Za-z]{2}):(.*)$")
RX_DEF = re.compile(r"#(WAV|BMP|EXBPM|BPM|STOP|SCROLL|SPEED|EXWAV)([0-9A-Za-z]{2})(?:\s+(.*))?$", re.I)
RX_KV = re.compile(r"#([A-Za-z][A-Za-z0-9]*)(?:[\s:]+(.*))?$")


def parse_bms(raw, rep):
    """BMS 텍스트 → dict(items=[(줄, 종류, a, b, c, 조건)], sizes=[변형마다 선택지 수], norm, enc).
    조건 = ((변형 번호, 허용 선택지 frozenset), ...). 정적으로 거짓인 갈래(#SETRANDOM)의 줄은 버린다."""
    text, enc = decode_text(raw)
    sizes, items, stack = [], [], []

    def branch(f, ks):  # ks = 이 갈래의 선택지 집합 (None = #ELSE: 나머지 전부)
        r = f["rand"]
        if r["vi"] is None:  # 고정값(#SETRANDOM) 또는 죽은 갈래 안의 #RANDOM
            hit = r["fixed"] is not None and not f["matched"] and (ks is None or r["fixed"] in ks)
            f["static"], f["matched"] = hit, f["matched"] or hit
        else:
            full = frozenset(range(1, sizes[r["vi"]] + 1))
            f["cur"] = full - f["used"] if ks is None else frozenset(ks) & full
            f["used"] = set(full) if ks is None else f["used"] | set(ks)

    for ln, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line.startswith("#"):
            continue
        dead = any(f.get("static") is False for f in stack)
        m = RX_CTL.match(line)
        if m:
            cmd = re.sub(r"\s+", "", m.group(1).upper())
            arg = m.group(2).split()
            num = int(arg[0]) if arg and re.fullmatch(r"[+-]?\d+", arg[0]) else None
            if cmd in ("SWITCH", "SETSWITCH", "CASE", "SKIP", "DEF", "ENDSW"):
                raise Unsupported(f"{ln}줄: #{cmd} (#SWITCH 계열) 미지원")
            if cmd in ("RANDOM", "SETRANDOM"):
                if num is None or not 1 <= num <= 65535:
                    raise Unsupported(f"{ln}줄: #{cmd} 값 오류")
                if dead:
                    stack.append({"kind": "rand", "vi": None, "fixed": None})
                elif cmd == "SETRANDOM":
                    stack.append({"kind": "rand", "vi": None, "fixed": num})
                    rep["#SETRANDOM (고정값)"] += 1
                else:
                    sizes.append(num)
                    stack.append({"kind": "rand", "vi": len(sizes) - 1, "fixed": None})
            elif cmd == "IF":
                r = next((f for f in reversed(stack) if f["kind"] == "rand"), None)
                if r is None:  # 값이 없으니 어느 갈래도 맞지 않는다 (LR2 확장 영역의 BGA 정의 등) → 건너뜀
                    r = {"kind": "rand", "vi": None, "fixed": None}
                    rep["#RANDOM 밖 #IF 갈래 건너뜀"] += 1
                if num is None:
                    raise Unsupported(f"{ln}줄: #IF 값 오류")
                f = {"kind": "if", "rand": r, "used": set(), "matched": False}
                branch(f, {num})
                stack.append(f)
            elif cmd in ("ELSEIF", "ELSE"):
                if not stack or stack[-1]["kind"] != "if":
                    raise Unsupported(f"{ln}줄: #{cmd} 짝 없음")
                if cmd == "ELSEIF" and num is None:
                    raise Unsupported(f"{ln}줄: #ELSEIF 값 오류")
                f = stack[-1]
                branch(f, None if cmd == "ELSE" else ({num} - f["used"]) if f["rand"]["vi"] is not None else {num})
                if f["rand"]["vi"] is not None and cmd == "ELSEIF":
                    f["used"].add(num)
            elif cmd in ("ENDIF", "IFEND"):
                if stack and stack[-1]["kind"] == "if":
                    stack.pop()
                else:
                    rep["짝 없는 #ENDIF 무시"] += 1
            elif cmd == "ENDRANDOM":
                while stack and stack[-1]["kind"] == "if":
                    stack.pop()
                    rep["#ENDIF 없이 #ENDRANDOM"] += 1
                if stack:
                    stack.pop()
            continue
        if dead:
            continue
        guard = {}
        for f in stack:
            if f["kind"] == "if" and "cur" in f:
                vi = f["rand"]["vi"]
                guard[vi] = guard.get(vi, f["cur"]) & f["cur"]
        if any(not s for s in guard.values()):
            continue  # 어느 조합에서도 읽히지 않는 갈래
        guard = tuple(sorted(guard.items()))
        m = RX_DATA.match(line)
        if m:
            items.append((ln, "data", int(m.group(1)), m.group(2).upper(), re.sub(r"\s+", "", m.group(3)), guard))
            continue
        m = RX_DEF.match(line)
        if m:
            t = m.group(1).upper()
            if t == "EXWAV":
                rep["#EXWAV 미지원 (정의 무시)"] += 1
                continue
            items.append((ln, "def", "BPM" if t == "EXBPM" else t, m.group(2), (m.group(3) or "").strip(), guard))
            continue
        m = RX_KV.match(line)
        if m:
            items.append((ln, "head", m.group(1).upper(), (m.group(2) or "").strip(), None, guard))
    bases = [(it[3], it[5]) for it in items if it[1] == "head" and it[2] == "BASE"]
    if any(g for _, g in bases):
        raise Unsupported("#BASE가 #IF 안에 있음")
    base62 = bool(bases) and bases[-1][0].strip() == "62"
    if base62:
        rep["#BASE 62"] += 1
    norm = (lambda v: v) if base62 else (lambda v: v.upper())
    # 레이아웃용 채널: 보이는·롱노트·지뢰 (투명·페달 제외), 모든 갈래
    used = set()
    for it in items:
        if it[1] == "data":
            ch, pl = it[3], it[4]
            if len(ch) == 2 and ch[0] in "1256DE" and ch[1] in "12345689" and any(pl[i:i + 2] != "00" for i in range(0, len(pl) - 1, 2)):
                used.add({"1": "1", "5": "1", "D": "1", "2": "2", "6": "2", "E": "2"}[ch[0]] + ch[1])
    return dict(items=items, sizes=sizes, norm=norm, enc=enc, used=used)


def pick_layout(path, used):
    if path.suffix.lower() == ".pms":
        return "9K"
    big = any(c[1] in "89" for c in used)
    if any(c[0] == "2" for c in used):
        return "14+2" if big else "10+2"
    return "7+1" if big else "5+1"


# ------------------------------------------------------------------ 2) 평범한 BMS로 풀기
NOTE_CH = re.compile(r"[123456DE][1-9]$")


def chart_ctx(head, defs, lines, norm):
    """머리·정의·데이터 줄 [(줄, 마디, 채널, 값)] → 차분 공통 값 (규칙·이득·마디·시간 지도). 시간 줄은 조건이 없어야 한다."""
    losses = collections.Counter()
    lnobj = norm(head.get("LNOBJ", "").strip())
    lntype = head.get("LNTYPE", "1").strip() or "1"
    if lntype != "1":
        raise Unsupported(f"LNTYPE {lntype} 미지원")
    lnm = head.get("LNMODE", "").strip()
    if lnm and lnm not in ("1", "2", "3"):
        raise Unsupported(f"#LNMODE {lnm} 미지원")
    rule = {"": "ln", "1": "ln", "2": "cn", "3": "hcn"}[lnm]
    bpm0 = fnum(head.get("BPM", "130") or "130", "#BPM")
    vw = head.get("VOLWAV", "").strip()
    gain = F(int(vw), 100) if vw.isdigit() else F(1)
    mlen = {}
    for ln, meas, ch, pl in lines:
        if ch == "02":
            mlen[meas] = fnum(pl, f"{ln}줄 마디 길이")
            if mlen[meas] <= 0:
                raise Unsupported(f"{ln}줄: 마디 길이 ≤ 0")
    last = max((meas for _, meas, _, _ in lines), default=0)
    mstart = [F(0)]
    for k in range(last + 2):
        mstart.append(mstart[-1] + 4 * mlen.get(k, F(1)))
    # 시간 채널: 같은 채널·위치는 뒤 정의, 같은 위치의 03·08은 파일 순서상 뒤가 이김
    slots = {}
    for ln, meas, ch, pl in lines:
        if ch not in TIMING_CH or ch == "02":
            continue
        n = len(pl) // 2
        for i in range(n):
            v = norm(pl[2 * i:2 * i + 2])
            if v != "00":
                slots[(ch, mstart[meas] + 4 * mlen.get(meas, F(1)) * F(i, n))] = ((ln, i), v)
    tempo, stops, scroll = {F(0): bpm0}, [], {}
    for (ch, beat), (order, v) in sorted(slots.items(), key=lambda kv: (kv[0][1], kv[1][0])):
        if ch == "03":
            try:
                tempo[beat] = F(int(v, 16))
            except ValueError:
                losses["03 채널 값 오류 (무시)"] += 1
        elif ch == "08":
            if v in defs["BPM"]:
                tempo[beat] = fnum(defs["BPM"][v], f"#BPM{v}")
            else:
                losses["없는 #BPMxx (무시)"] += 1
        elif ch == "09":
            if v in defs["STOP"]:
                d = fnum(defs["STOP"][v], f"#STOP{v}") / 48
                if d < 0:
                    raise Unsupported(f"#STOP{v} 음수")
                if d:
                    stops.append((beat, d))
            else:
                losses["없는 #STOPxx (무시)"] += 1
        elif ch == "SC":
            if v in defs["SCROLL"]:
                scroll[beat] = fnum(defs["SCROLL"][v], f"#SCROLL{v}")
        elif ch == "SP":
            losses["#SPEED (화면 속도 변화) 버림"] += 1
    if any(b == 0 for b in tempo.values()):
        raise Unsupported("0 BPM은 표현할 수 없음")
    sig = (rule, lnobj, bpm0, tuple(sorted(tempo.items())), tuple(sorted(stops)), tuple(sorted(scroll.items())), tuple(sorted(mlen.items())))
    return dict(lnobj=lnobj, rule=rule, gain=gain, mlen=mlen, mstart=mstart, tempo=tempo, stops=stops, scroll=scroll, losses=losses, sig=sig,
                head=head, bars=None)


def make_snd(defs, sound_of, rep):
    cache = {}

    def snd(v):
        if v not in cache:
            name = defs["WAV"].get(v)
            if name is None:
                rep["정의 없는 #WAV (객체 ID 수)"] += 1
                cache[v] = None
            else:
                rel = sound_of(name)
                if rel is None:
                    rep["파일 없는 소리 (#WAV 수)"] += 1
                cache[v] = f"{rel}@{v}" if rel else None
        return cache[v]
    return snd


def raw_objects(lines, ctx, norm):
    """[(줄, 마디, 채널, 값, 조건)] → 객체 [(채널, 박, 순서, ID, 조건)] (02·시간 채널 제외)."""
    out = []
    for ln, meas, ch, pl, g in lines:
        if ch in TIMING_CH:
            continue
        n = len(pl) // 2
        L = 4 * ctx["mlen"].get(meas, F(1))
        for i in range(n):
            v = norm(pl[2 * i:2 * i + 2])
            if v != "00":
                out.append((ch, ctx["mstart"][meas] + L * F(i, n), (ln, i), v, g))
    return out


def lane_pass(objs, ctx, layout, snd):
    """조건이 정해진 객체 [(채널, 박, 순서, ID)] → (결과 객체, 손실, 정보). 평범한 BMS 하나를 읽는 것과 같다."""
    losses, info = collections.Counter(), collections.Counter()
    gain, lnobj = ctx["gain"], ctx["lnobj"]
    slots, bgm = {}, []
    for ch, beat, order, v in sorted(objs, key=lambda o: o[2]):  # 파일 순서
        if ch == "01":
            bgm.append((beat, order, v))
        else:
            if (ch, beat) in slots:
                info["같은 채널·위치 → 뒤 정의가 덮어씀"] += 1
            slots[(ch, beat)] = (order, v)
    out = [("auto", beat, s, gain) for beat, _, v in sorted(bgm) if (s := snd(v))]
    per_ch = collections.defaultdict(list)
    for (ch, beat), (order, v) in slots.items():
        if ch in BGA_CH:
            info["BGA 객체 (CP4)"] += 1
        elif not NOTE_CH.match(ch):
            info[f"알 수 없는 채널 {ch}"] += 1
        else:
            per_ch[ch].append((beat, order, v))
    cand = collections.defaultdict(list)  # 레인 → [(시작, 순서, 종류, 끝, ID)]
    for ch, lst in sorted(per_ch.items()):
        lst.sort()
        side = 1 if ch[0] in "135D" else 2
        lane = lane_of(layout, side, ch[1]) if ch[1] != "7" else None
        found = []
        if ch[0] in "56":  # LNTYPE 1: 채널마다 차례로 짝
            for i in range(0, len(lst) - 1, 2):
                found.append((lst[i][0], lst[i][1], "long", lst[i + 1][0], lst[i][2]))
            if len(lst) % 2:
                info["짝 없는 롱노트 시작 → 탭"] += 1
                found.append((lst[-1][0], lst[-1][1], "tap", None, lst[-1][2]))
        elif ch[0] in "12":
            for beat, order, v in lst:
                if lnobj and v == lnobj:  # LNOBJ 끝 표시: 그 채널의 바로 앞 노트를 롱노트로 (표시 자체는 소리 없음)
                    if found and found[-1][2] == "tap":
                        b0, o0, _, _, v0 = found[-1]
                        found[-1] = (b0, o0, "long", beat, v0)
                    else:
                        info["앞 노트 없는 LNOBJ 끝 표시 버림"] += 1
                    continue
                found.append((beat, order, "tap", None, v))
        elif ch[0] in "DE":
            found = [(beat, order, "mine", None, v) for beat, order, v in lst]
            info["지뢰 (피해량 = 규칙 기본)"] += len(found)
        else:  # 3x/4x 투명 노트 → 빈 레인 소리
            if lane is None:
                info[f"레인 밖 투명 노트 버림 (채널 {ch})"] += len(lst)
            else:
                out += [("lsnd", lane, beat, s) for beat, _, v in lst if (s := snd(v))]
            continue
        if lane is None:
            what = "페달" if ch[1] == "7" else f"레이아웃 {layout} 밖 채널 {ch}"
            for beat, _, kind, _, v in found:
                if kind == "mine":
                    losses[f"{what} 지뢰 버림"] += 1
                else:
                    losses[f"{what} 노트 → 소리만 자동재생"] += 1
                    if s := snd(v):
                        out.append(("auto", beat, s, gain))
            continue
        cand[lane] += found
    for lane, lst in cand.items():
        by_pos = {}
        for x in sorted(lst, key=lambda x: (x[0], x[1])):
            if x[0] in by_pos:
                info["같은 레인·위치 다른 채널 노트 → 뒤 정의만"] += 1
            by_pos[x[0]] = x
        last_end = None
        for beat, _, kind, end, v in sorted(by_pos.values()):
            s = snd(v) if kind != "mine" else None
            if last_end is not None and beat <= last_end:
                if kind == "mine":
                    losses["롱노트 안 지뢰 버림"] += 1
                else:
                    losses["롱노트 안 노트 → 소리만 자동재생"] += 1
                    if s:
                        out.append(("auto", beat, s, gain))
                continue
            out.append(("note", lane, kind, beat, end, s, gain))
            last_end = end if kind == "long" else beat  # 컴파일러 규칙: 다음 노트는 이 노트 끝보다 뒤 (지뢰도 노트)
    return out, losses, info


def split_lines(P, combo=None):
    """조합(없으면 조건 무시하고 전부)에 맞는 머리·정의·데이터 줄."""
    norm = P["norm"]
    head, defs, lines = {}, collections.defaultdict(dict), []
    for ln, kind, a, b, c, g in P["items"]:
        if combo is not None and g and not all(combo[vi] in s for vi, s in g):
            continue
        if kind == "head":
            head[a] = b  # 뒤 정의가 이김
        elif kind == "def":
            defs[a][norm(b)] = c
        else:
            lines.append((ln, a, b, c, g if combo is None else ()))
    return head, defs, lines


def resolve_combo(P, combo, layout, sound_of, rep):
    """선택 조합 하나 → 결과 (그 조합의 평범한 BMS)."""
    head, defs, lines = split_lines(P, combo)
    ctx = chart_ctx(head, defs, [l[:4] for l in lines], P["norm"])
    snd = make_snd(defs, sound_of, rep)
    out, lo, info = lane_pass([o[:4] for o in raw_objects(lines, ctx, P["norm"])], ctx, layout, snd)
    ctx["losses"].update(lo)
    return dict(ctx, objs=out, info=info)


PLAY_HEAD = {"BPM", "LNOBJ", "LNTYPE", "LNMODE", "VOLWAV", "BASE"}


def item_ok(it):
    """조건이 붙어도 국소 풀이를 막지 않는 줄: 노트·BGM·BGA 데이터, 플레이에 쓰지 않는 정의(#BMP 등)·머리."""
    _, kind, a, b, _, _ = it
    if kind == "data":
        return b == "01" or bool(NOTE_CH.match(b)) or b in BGA_CH
    if kind == "def":
        return a not in ("WAV", "BPM", "STOP", "SCROLL")
    return a not in PLAY_HEAD


def factored_ok(P):
    """조건이 item_ok인 줄에만 있으면 조합을 다 펼치지 않고 서로 영향을 주는 객체끼리만 펼친다."""
    return all(not it[5] or item_ok(it) for it in P["items"])


def guard_conds(g, sizes):
    """줄 조건 ((변형, 선택지 집합), ...) → 겹치지 않는 AND 조건 목록."""
    vs = [(vi, sorted(s)) for vi, s in g if len(s) < sizes[vi]]
    return [dict(zip([vi for vi, _ in vs], t)) for t in itertools.product(*[s for _, s in vs])]


def resolve_factored(P, layout, sound_of, rep):
    """조건이 노트 채널에만 있는 차분: 레인마다 서로 영향을 주는 객체 묶음(같은 위치, 또는 롱노트 상태를 바꾸면 레인 전체)만
    국소 조합으로 푼다. 결과는 전체 조합을 펼쳐 푼 것(resolve_combo + merge)과 같다 (--selfcheck로 대조)."""
    sizes = P["sizes"]
    head, defs, lines = split_lines(P)
    ctx = chart_ctx(head, defs, [l[:4] for l in lines], P["norm"])
    snd = make_snd(defs, sound_of, rep)
    objs = raw_objects(lines, ctx, P["norm"])
    items, losses, info = [], collections.Counter(ctx["losses"]), collections.Counter()

    def beat_of(x):
        return x[3] if x[0] == "note" else x[1] if x[0] == "auto" else x[2]

    def run(group, V, context=(), only=None, base_lo=None):
        ls = [sizes[v] for v in V]
        n = math.prod(ls)
        if n > MAX_COMBOS:
            raise Unsupported(f"#RANDOM: 서로 영향을 주는 묶음의 선택 조합 {n}개 > {MAX_COMBOS}")
        combos = list(itertools.product(*[range(1, k + 1) for k in ls]))
        occ, order, mlo, minf = {}, [], collections.Counter(), collections.Counter()
        for ci, a in enumerate(combos):
            asg = dict(zip(V, a))
            act = [o[:4] for o in group if all(asg[vi] in s for vi, s in o[4])] + list(context)
            out, lo, inf = lane_pass(act, ctx, layout, snd)
            if only is not None:
                out = [x for x in out if beat_of(x) == only]
                lo = lo - base_lo
            seen = collections.Counter()
            for x in out:
                k = (x, seen[x])
                seen[x] += 1
                if k not in occ:
                    occ[k] = set()
                    order.append(k)
                occ[k].add(ci)
            mlo |= lo
            minf |= inf
        for k in order:
            items.append((k[0], [{V[i]: c for i, c in d.items()} for d in cover(occ[k], combos, ls)]))
        losses.update(mlo)
        info.update(minf)

    def lane_key(ch):
        side = 1 if ch[0] in "135D" else 2
        lane = lane_of(layout, side, ch[1]) if ch[1] != "7" else None
        return lane or ("채널", ch)

    groups = collections.defaultdict(list)
    for o in objs:
        if o[0] == "01":
            if o[4]:
                if s := snd(o[3]):
                    items.append((("auto", o[1], s, ctx["gain"]), guard_conds(o[4], sizes)))
            else:
                groups["01"].append(o)
        elif NOTE_CH.match(o[0]):
            groups[lane_key(o[0])].append(o)
        else:
            groups["기타"].append(o)  # BGA·알 수 없는 채널: lane_pass가 보고만 한다
    for k, L in groups.items():
        cond = [o for o in L if o[4]]
        if not cond or k == "기타":
            out, lo, inf = lane_pass([o[:4] for o in L], ctx, layout, snd)
            items += [(x, [{}]) for x in out]
            losses.update(lo)
            info.update(inf)
            continue
        # 롱노트 상태를 만드는 채널: LN 채널, LNOBJ 끝 표시가 있는 보이는 채널
        stateful = {o[0] for o in L if o[0][0] in "56" or (ctx["lnobj"] and o[0][0] in "12" and o[3] == ctx["lnobj"])}
        context = [o[:4] for o in L if not o[4] and o[0] in stateful]
        whole = any(o[0] in stateful for o in cond)
        base_lo = collections.Counter()
        if not whole:
            base_out, base_lo, _ = lane_pass(context, ctx, layout, snd)
            bounds = {b for x in base_out if x[0] == "note" and x[2] == "long" for b in (x[3], x[4])}
            whole = any(o[1] in bounds for o in cond)
        if whole:  # 레인 전체를 한 묶음으로
            run(L, sorted({vi for o in cond for vi, _ in o[4]}))
            continue
        cb = {o[1] for o in cond}
        out, lo, inf = lane_pass([o[:4] for o in L if o[1] not in cb], ctx, layout, snd)
        items += [(x, [{}]) for x in out]
        losses.update(lo)
        info.update(inf)
        for b in sorted(cb):
            grp = [o for o in L if o[1] == b and o[0] not in stateful]
            run(grp, sorted({vi for o in grp for vi, _ in o[4]}), context, b, base_lo)
    return dict(ctx, objs=None, items=items, losses=losses, info=info)


def expand(items, combo):
    """합친 결과를 조합 하나로 펼침 (자체 대조용): 객체 다중집합."""
    c = collections.Counter()
    for o, conds in items:
        c[o] += sum(1 for d in conds if all(combo[v] == ch for v, ch in d.items()))
    return +c


# ------------------------------------------------------------------ BMSON
def resolve_bmson(raw, path, folder, sound_meta, rep):
    """BMSON → 결과 dict 하나 (변형 없음). sound_meta(파일 이름) → (상대 경로, rate, frames) 또는 None."""
    try:
        d = json.loads(raw.decode("utf-8-sig"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise Unsupported(f"BMSON JSON 오류: {e}") from None
    info = d.get("info") or {}
    mode = info.get("mode_hint") or "beat-7k"
    layout = BMSON_MODES.get(mode)
    if not layout:
        raise Unsupported(f"BMSON mode_hint {mode} 미지원")
    res = info.get("resolution") or 240
    if not isinstance(res, int) or res <= 0:
        raise Unsupported("BMSON resolution 오류")
    rule = {None: "ln", 0: "ln", 1: "ln", 2: "cn", 3: "hcn"}.get(info.get("ln_type"))
    if rule is None:
        raise Unsupported(f"BMSON ln_type {info.get('ln_type')} 미지원")
    beat = lambda y: F(int(y), res)
    bpm0 = fnum(str(info.get("init_bpm", 130)), "init_bpm")
    tempo = {F(0): bpm0}
    for e in sorted(d.get("bpm_events") or [], key=lambda e: e["y"]):
        tempo[beat(e["y"])] = fnum(str(e["bpm"]), "bpm_events")
    if any(b == 0 for b in tempo.values()):
        raise Unsupported("0 BPM은 표현할 수 없음")
    stops = [(beat(e["y"]), beat(e["duration"])) for e in d.get("stop_events") or [] if e.get("duration")]
    if any(s < 0 for _, s in stops):
        raise Unsupported("음수 STOP")
    scroll = {beat(e["y"]): fnum(str(e["rate"]), "scroll_events") for e in d.get("scroll_events") or []}
    bars = sorted({beat(l["y"]) for l in d.get("lines") or []})
    # 박 → 초 (조각 시작·길이용; 컴파일러와 따로 계산)
    pts = sorted(tempo.items())
    stop_at = collections.Counter()
    for b, s in stops:
        stop_at[b] += s

    def sec(b):
        t, cur_b, cur_bpm = F(0), F(0), abs(pts[0][1])
        marks = sorted(set([p for p, _ in pts] + list(stop_at)))
        for m in marks:
            if m >= b:
                break
            t += (m - cur_b) * 60 / cur_bpm
            cur_b = m
            if m in tempo:
                cur_bpm = abs(tempo[m])
            t += stop_at[m] * 60 / cur_bpm
        return t + (b - cur_b) * 60 / cur_bpm

    losses = collections.Counter()
    objs, lsnd = [], []
    for ci, ch in enumerate(d.get("sound_channels") or []):
        meta = sound_meta(ch.get("name") or "")
        notes = sorted(enumerate(ch.get("notes") or []), key=lambda kv: (kv[1]["y"], kv[0]))
        restart = None
        for k, (_, n) in enumerate(notes):
            y = int(n["y"])
            if not n.get("c") or restart is None:
                restart = y
            nxt = next((m for _, m in notes[k + 1:] if int(m["y"]) > y), None)
            s = None
            if meta:
                rel, rate, frames = meta
                start = round((sec(beat(y)) - sec(beat(restart))) * rate)
                length = round((sec(beat(int(nxt["y"]))) - sec(beat(y))) * rate) if nxt is not None and nxt.get("c") else 0
                if start >= frames:
                    rep["BMSON 조각이 파일 끝 뒤 → 소리 없음"] += 1
                else:
                    length = min(length, frames - start) if length else 0
                    s = f"{rel}@{start}+{length}"
                    if n.get("c"):
                        rep["BMSON 이어 재생 노트"] += 1
            else:
                rep["파일 없는 소리"] += 1
            x = n.get("x") or 0
            l = int(n.get("l") or 0)
            if x == 0:
                if s:
                    objs.append(("auto", beat(y), s, F(1)))
                continue
            lane = bmson_lane(layout, int(x))
            if lane is None:
                losses[f"레이아웃 {layout} 밖 레인 x={x} 노트 → 소리만 자동재생"] += 1
                if s:
                    objs.append(("auto", beat(y), s, F(1)))
                continue
            if n.get("t") not in (None, 0) and {1: "ln", 2: "cn", 3: "hcn"}.get(n.get("t")) != rule:
                raise Unsupported("BMSON 노트마다 다른 롱노트 종류 미지원")
            objs.append(("pre", lane, beat(y), "long" if l > 0 else "tap", beat(y + l) if l > 0 else None, s))
    for ch in d.get("mine_channels") or []:
        for n in ch.get("notes") or []:
            lane = bmson_lane(layout, int(n.get("x") or 0))
            if lane:
                objs.append(("pre", lane, beat(n["y"]), "mine", None, None))
                rep["지뢰 (피해량 = 규칙 기본)"] += 1
    for ch in d.get("key_channels") or []:
        meta = sound_meta(ch.get("name") or "")
        for n in ch.get("notes") or []:
            lane = bmson_lane(layout, int(n.get("x") or 0))
            if lane and meta:
                lsnd.append(("lsnd", lane, beat(n["y"]), f"{meta[0]}@0+0"))
    if d.get("bga"):
        rep["BGA (CP4)"] += 1
    # 레인마다: 같은 위치는 하나(노트에 소리 여럿이 아니라 먼저 온 것 — BMSON은 같은 x·y 노트를 겹쳐 둘 수 있어 소리는 모두 남김), 롱노트 안 노트
    out = [o for o in objs if o[0] == "auto"]
    by_lane = collections.defaultdict(dict)
    for o in objs:
        if o[0] != "pre":
            continue
        _, lane, b, kind, end, s = o
        cur = by_lane[lane].get(b)
        if cur is None:
            by_lane[lane][b] = [kind, end, [s] if s else []]
        else:
            if s:
                cur[2].append(s)  # 같은 레인·위치의 다른 채널 노트 = 화음 한 노트
            if kind == "long" and (cur[0] != "long" or end > cur[1]):
                cur[0], cur[1] = "long", end
            rep["BMSON 같은 레인·위치 노트 합침 (소리 모두)"] += 1
    notes = []
    for lane, dd in by_lane.items():
        last_end = None
        for b in sorted(dd):
            kind, end, ss = dd[b]
            if last_end is not None and b <= last_end:
                if kind == "mine":
                    losses["롱노트 안 지뢰 버림"] += 1
                else:
                    losses["롱노트 안 노트 → 소리만 자동재생"] += 1
                    out += [("auto", b, s, F(1)) for s in ss]
                continue
            notes.append(("noteN", lane, kind, b, end, tuple(ss)))
            last_end = end if kind == "long" else b
    out += notes + lsnd
    sig = (rule, bpm0, tuple(sorted(tempo.items())), tuple(sorted(stops)), tuple(sorted(scroll.items())), tuple(bars))
    head = {"TITLE": info.get("title", ""), "SUBTITLE": info.get("chart_name") or info.get("subtitle", ""), "ARTIST": info.get("artist", ""),
            "PLAYLEVEL": str(info.get("level", ""))}
    return dict(sig=sig, rule=rule, tempo=tempo, stops=stops, scroll=scroll, mlen={}, mstart=None, objs=out, losses=losses, head=head,
                bars=bars, layout=layout)


# ------------------------------------------------------------------ 3) 조합 합치기
def cover(S, combos, sizes):
    """조합 번호 집합 S → 겹치지 않는 AND 조건 목록 [{변형: 선택}]. 모든 조합이면 [{}]."""
    out = []

    def rec(idx, fixed):
        total = math.prod(sizes[v] for v in range(len(sizes)) if v not in fixed)
        if len(idx) == total:
            out.append(dict(fixed))
            return
        free = [v for v in range(len(sizes)) if v not in fixed]
        # 선택지 일부에만 나오는 변형부터 가른다 (r2=1처럼 한 변형만 정하면 되는 경우를 짧게)
        v = next((v for v in free if len({combos[i][v] for i in idx}) < sizes[v]), free[0])
        by = collections.defaultdict(list)
        for i in idx:
            by[combos[i][v]].append(i)
        for c in sorted(by):
            rec(by[c], {**fixed, v: c})

    rec(sorted(S), {})
    return out


def merge(results, combos, sizes):
    """[(조합 번호, 결과)] → [(객체, [조건 dict])] (객체가 나오는 조합 집합을 조건으로)."""
    occ, order = {}, []
    for ci, r in results:
        seen = collections.Counter()
        for o in r["objs"]:
            k = (o, seen[o])
            seen[o] += 1
            if k not in occ:
                occ[k] = set()
                order.append(k)
            occ[k].add(ci)
    return [(k[0], cover(occ[k], combos, sizes)) for k in order]


# ------------------------------------------------------------------ 소리 파일
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


def ffprobe(src):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=sample_rate,channels", "-of", "csv=p=0",
                        str(src)], capture_output=True, text=True)
    try:
        rate, ch = (int(x) for x in r.stdout.strip().split(",")[:2])
    except ValueError:
        raise Unsupported(f"소리 판별 실패 {src.name}: {r.stderr.strip()[:80]}") from None
    return rate, ch


def decode_float(src, kind, rep):
    """손실 경로용: 원본 → (float64 [frames, ch≤2], rate). 3채널 이상은 ffmpeg 기본 행렬로 스테레오."""
    if kind == "float":
        a, rate = sf.read(str(src), dtype="float64", always_2d=True)
        if a.shape[1] <= 2:
            return a, rate
    rate, ch = ffprobe(src)
    out_ch = min(ch, 2)
    if ch > 2:
        rep["3채널 이상 → 스테레오 다운믹스 (ffmpeg 기본 행렬 ⚑)"] += 1
    r = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(src), "-f", "f32le", "-acodec", "pcm_f32le", "-ac", str(out_ch), "-"],
                       capture_output=True)
    if r.returncode:
        raise Unsupported(f"소리 변환 실패 {src.name}: {r.stderr.decode('utf-8', 'replace')[:100]}")
    return np.frombuffer(r.stdout, dtype="<f4").reshape(-1, out_ch).astype(np.float64), rate


def channels_of(src):
    try:
        return sf.info(str(src)).channels
    except RuntimeError:
        return 0


def sniff(b, src):
    # 엔진은 1·2채널만 그대로 받는다 → 3채널 이상 Ogg·FLAC은 보존하지 않고 다운믹스 경로로 (CP3 마감 리뷰 #5)
    if b[:4] == b"OggS":
        if b"\x01vorbis" not in b[:64]:
            return "other"
        return "ogg" if channels_of(src) <= 2 else "other"
    if b[:4] == b"fLaC":
        return "flac" if channels_of(src) <= 2 else "other"
    if b[:4] == b"RIFF" and b[8:12] == b"WAVE":
        try:
            info = sf.info(str(src))
        except RuntimeError:
            return "other"
        if info.subtype == "MS_ADPCM" and info.channels <= 2:
            return "adpcm"
        if info.subtype in ("PCM_16", "PCM_24", "PCM_U8", "PCM_S8") and info.channels <= 2:
            return "pcm"
        if info.subtype in ("FLOAT", "DOUBLE", "PCM_32"):
            return "float"
    return "other"


def put_audio(src, dst_dir, rel, rep, err):
    """원본 소리 → (출력 파일, gain 유리수 또는 None). 이름 = 원본 경로 + (변환기 판 + 원본 바이트) 해시."""
    b = src.read_bytes()
    h = hashlib.sha256(CONVERTER.encode() + b"\0" + b).hexdigest()[:12]
    stem = safe(os.path.splitext(rel)[0].replace("/", "__"))[:80] + "." + h
    kind = sniff(b, src)
    if kind in ("ogg", "flac", "adpcm"):
        dst = dst_dir / (stem + {"ogg": ".ogg", "flac": ".flac", "adpcm": ".wav"}[kind])
        if not dst.exists():
            dst.write_bytes(b)
        rep[f"그대로 ({kind})"] += 1
        return dst, None
    dst = dst_dir / (stem + ".flac")
    if kind == "pcm":
        if not dst.exists():
            info = sf.info(str(src))
            a, rate = sf.read(str(src), dtype="int32", always_2d=True)
            sf.write(str(dst), a, rate, format="FLAC", subtype={"PCM_U8": "PCM_S8"}.get(info.subtype, info.subtype))
        rep["PCM WAV → FLAC (무손실)"] += 1
        return dst, None
    x, rate = decode_float(src, kind, rep)
    peak = float(np.abs(x).max()) if x.size else 0.0
    gain = F(peak) if peak > 1 else None
    y = x / peak if gain else x
    if not dst.exists():
        # 양자화는 직접 반올림(×2²³) — libsndfile의 실수→정수 변환은 2²³−1을 곱해 전체가 미세하게 작아진다
        qy = np.round(y * 2**23)
        rep["양자화 클리핑 (+1.0 → 2²³−1) 샘플"] += int((qy > 2**23 - 1).sum())
        sf.write(str(dst), (np.clip(qy, -2**23, 2**23 - 1).astype(np.int32) << 8), rate, format="FLAC", subtype="PCM_24")
    z, _ = sf.read(str(dst), dtype="float64", always_2d=True)
    if gain:
        z = z * peak
        rep["넘침 감쇠 → 소리 gain으로 되돌림"] += 1
    e = np.abs(z - x) if z.shape == x.shape else np.array([np.inf])
    err["n"] += e.size
    err["max"] = max(err["max"], float(e.max()) if e.size else 0.0)
    err["sq"] += float((e[np.isfinite(e)] ** 2).sum())
    rep["실수 디코딩 → FLAC24 (손실)"] += 1
    return dst, gain


# ------------------------------------------------------------------ 차분 문서
def ref_scroll(tempo, beat_end):
    """가장 오래(시간) 유지된 |BPM| — 같으면 큰 값. (후보 설명 문자열도)"""
    pts = sorted(tempo.items())
    dur = collections.Counter()
    for i, (b, bpm) in enumerate(pts):
        nb = pts[i + 1][0] if i + 1 < len(pts) else max(beat_end, b)
        dur[abs(bpm)] += (nb - b) * 60 / abs(bpm)
    best = max(dur.items(), key=lambda kv: (kv[1], kv[0]))[0]
    vals = [abs(v) for _, v in pts]
    return best, f"시작 {float(abs(pts[0][1])):g}, 최빈 {float(best):g}, 최고 {float(max(vals)):g}, 최저 {float(min(vals)):g}"


def chart_doc(cid, src_rel, enc, r, layout, items, sizes_used, losses):
    """items = [(객체, [조건 dict])]. 조건 dict의 변형 번호는 원래 번호(0부터) → id r{번호+1}."""
    head = r["head"]
    mstart = r["mstart"]

    def pos(b):
        if mstart is None:
            return q(b)
        m = max(0, min(len(mstart) - 2, next((i for i in range(len(mstart) - 1) if mstart[i + 1] > b), len(mstart) - 2)))
        return f"{m}:{q(b - mstart[m])}"

    beat_end = max([o[3] if o[0] in ("note", "noteN") else o[1] if o[0] == "auto" else o[2] for o, _ in items] + [F(0)])
    refs, cands = ref_scroll(r["tempo"], beat_end)
    lines = [f"# {src_rel} (인코딩 {enc})", f"id = {tq(cid)}", f"name = {tq(head.get('SUBTITLE') or pathlib.PurePosixPath(src_rel).stem)}",
             f"level = {tq(head.get('PLAYLEVEL', ''))}", f'layout = "{layout}"', 'shift = "0"', f'reference_scroll = "{q(refs)}"   # {cands}']
    if losses:
        lines.append("losses = [" + ", ".join(tq(f"{k}: {v}") for k, v in sorted(losses.items())) + "]")
    lines += ["", "[rules]", f'long_note = "{r["rule"]}"', 'empty_lane = "previous_note"', "", "[timing]", 'unit = "beat"',
              "tempo = [ " + ", ".join(f'{{ at = "{q(b)}", bpm = "{q(abs(v))}" }}' for b, v in sorted(r["tempo"].items())) + " ]"]
    if mstart is not None:
        md = {0: F(4)}
        for m, L in sorted(r["mlen"].items()):
            md[m] = 4 * L
            if m + 1 not in r["mlen"]:
                md.setdefault(m + 1, F(4))
        lines.append("meter = [ " + ", ".join(f'{{ measure = {m}, length = "{q(L)}" }}' for m, L in sorted(md.items())) + " ]")
    else:
        lines.append('meter = [ { measure = 0, length = "4" } ]')
        lines.append("bars = [" + ", ".join(f'"{q(b)}"' for b in r["bars"]) + "]")
    lines += ["stops = [ " + ", ".join(f'{{ at = "{q(b)}", beats = "{q(d)}" }}' for b, d in sorted(r["stops"])) + " ]", ""]
    neg = any(v < 0 for v in r["tempo"].values())
    if neg or r["scroll"]:
        keys = sorted(set(r["tempo"]) | set(r["scroll"]))
        cur_b, cur_s, pts = None, F(1), []
        for b in keys:
            if b in r["tempo"]:
                cur_b = r["tempo"][b]
            if b in r["scroll"]:
                cur_s = r["scroll"][b]
            pts.append(f'{{ at = "{q(b)}", speed = "{q(cur_b * cur_s)}" }}')
        lines += ["[scroll]                             # 음수 BPM·#SCROLL: 속도 = |BPM| × SCROLL × 부호", "points = [ " + ", ".join(pts) + " ]", ""]
    for vi in sorted(sizes_used):
        lines += ["[[variant]]", f'id = "r{vi + 1}"', "choices = [" + ", ".join(f'"{k}"' for k in range(1, sizes_used[vi] + 1)) + "]", ""]

    def when(c):
        terms = [f"r{vi + 1}={ch}" for vi, ch in sorted(c.items())]
        if not terms:
            return []
        return [f"when = {tq(terms[0])}"] if len(terms) == 1 else ["when = [" + ", ".join(tq(t) for t in terms) + "]"]

    lane_order = [l for l, _ in LAYOUTS[layout]]
    key = {"auto": lambda o: (0, 0, o[1]), "note": lambda o: (1, lane_order.index(o[1]), o[3]), "noteN": lambda o: (1, lane_order.index(o[1]), o[3]),
           "lsnd": lambda o: (2, lane_order.index(o[1]), o[2])}
    used = set()
    for o, conds in sorted(items, key=lambda it: key[it[0][0]](it[0])):
        for c in conds:
            if o[0] == "auto":
                _, b, s, g = o
                lines += ["[[event]]", f'at = "{pos(b)}"', f"sound = {tq(s)}"] + ([f'gain = "{q(g)}"'] if g != 1 else []) + when(c) + [""]
                used.add(s)
            elif o[0] in ("note", "noteN"):
                if o[0] == "note":
                    _, lane, kind, b, end, s, g = o
                    ss = [s] if s else []
                else:
                    _, lane, kind, b, end, ss = o
                    g = F(1)
                lines += ["[[note]]", f'at = "{pos(b)}"', f'lane = "{lane}"', f'kind = "{kind}"'] + ([f'end = "{pos(end)}"'] if kind == "long" else [])
                if ss:
                    lines.append("sounds = [" + ", ".join(tq(s) if g == 1 else f'{{ sound = {tq(s)}, gain = "{q(g)}" }}' for s in ss) + "]")
                    used.update(ss)
                lines += when(c) + [""]
            else:
                _, lane, b, s = o
                lines += ["[[lane_sound]]", f'at = "{pos(b)}"', f'lane = "{lane}"', f"sound = {tq(s)}"] + when(c) + [""]
                used.add(s)
    return "\n".join(lines), used


# ------------------------------------------------------------------ 폴더
def convert_folder(folder, out_root, ids):
    folder = pathlib.Path(folder).resolve()
    if not folder.is_dir():
        raise FileNotFoundError(f"폴더 없음: {folder}")
    files = sorted(p for p in folder.rglob("*") if p.is_file() and p.suffix.lower() in EXTS | {".bmson"})
    if not files:
        return None
    key = str(folder).lower()
    out = pathlib.Path(out_root) / f"{safe(folder.name)}-{hashlib.sha256(key.encode()).hexdigest()[:6]}"
    (out / "audio").mkdir(parents=True, exist_ok=True)
    rep, err = collections.Counter(), {"n": 0, "max": 0.0, "sq": 0.0}
    res = Resolver(folder)
    audio = {}  # 원본 상대 경로 → (작성 형식 상대 경로, gain)
    docs, status, layouts, titles, artists, sources, all_used = [], {}, set(), collections.Counter(), collections.Counter(), [], set()

    def file_for(rel):
        if rel not in audio:
            p, g = put_audio(folder / rel, out / "audio", rel, rep, err)
            audio[rel] = (p.relative_to(out).as_posix(), g)
        return audio[rel]

    for p in files:
        src_rel = p.relative_to(folder).as_posix()
        raw = p.read_bytes()
        crep = collections.Counter()
        chart_dir = p.parent.relative_to(folder).as_posix()
        chart_dir = "" if chart_dir == "." else chart_dir
        base_id = safe(re.sub(r"\s+", "_", p.stem.lower()))[:60]
        while any(base_id == d[0].split("~")[0] for d in docs):
            base_id += "_"
        try:
            if p.suffix.lower() == ".bmson":
                def sound_meta(name):
                    rel = res(name, chart_dir)
                    if rel is None:
                        return None
                    path, _ = file_for(rel)
                    info = sf.info(str(out / path))
                    return rel, info.samplerate, info.frames
                r = resolve_bmson(raw, p, folder, sound_meta, crep)
                layout, enc = r["layout"], "UTF-8 (BMSON)"
                plan = [(base_id, r, [(o, [{}]) for o in r["objs"]], {})]
            else:
                P = parse_bms(raw, crep)
                enc = P["enc"]
                layout = pick_layout(p, P["used"])
                sizes = P["sizes"]
                n = math.prod(sizes)

                def sound_of(name):
                    rel = res(name, chart_dir)
                    if rel is not None:
                        file_for(rel)
                    return rel

                def full(PP=P):
                    nn = math.prod(PP["sizes"])
                    if nn > MAX_COMBOS:
                        raise Unsupported(f"#RANDOM 선택 조합 {nn}개 > {MAX_COMBOS} (조건이 노트 줄 밖에도 있어 묶음으로 나눌 수 없음)")
                    combos = list(itertools.product(*[range(1, k + 1) for k in PP["sizes"]]))
                    return combos, [(ci, resolve_combo(PP, c, layout, sound_of, collections.Counter())) for ci, c in enumerate(combos)]

                def factored(PP, cid):
                    r = resolve_factored(PP, layout, sound_of, crep)
                    items = r["items"]
                    nn = math.prod(PP["sizes"])
                    if SELFCHECK and PP["sizes"] and nn <= SELFCHECK:
                        combos, results = full(PP)
                        for ci, rr in results:
                            if expand(items, combos[ci]) != collections.Counter(rr["objs"]):
                                raise RuntimeError(f"자체 대조 실패: 조합 {combos[ci]}에서 국소 풀이 ≠ 전체 풀이")
                        crep[f"자체 대조 통과 (조합 {nn})"] += 1
                    crep.update(r["info"])
                    vs = {vi for _, conds in items for c in conds for vi in c}
                    return cid, r, items, {vi: sizes[vi] for vi in vs}

                results = None
                if factored_ok(P):
                    plan = [factored(P, base_id)]
                    if sizes:
                        crep[f"#RANDOM 변형 {len(sizes)}개 (조건은 노트 줄에만 → 국소 풀이)"] += 1
                elif n <= MAX_COMBOS:
                    combos, results = full()
                    if len({rr["sig"] for _, rr in results}) == 1:
                        lo, inf = collections.Counter(), collections.Counter()
                        for _, rr in results:
                            lo |= rr["losses"]
                            inf |= rr["info"]
                        crep.update(inf)
                        items = merge(results, combos, sizes)
                        vs = {vi for _, conds in items for c in conds for vi in c}
                        plan = [(base_id, dict(results[0][1], losses=lo), items, {vi: sizes[vi] for vi in vs})]
                        crep[f"#RANDOM 변형 {len(sizes)}개, 조합 {n} (전체 펼침)"] += 1
                    else:
                        results = None
                if results is None and not factored_ok(P):
                    # 시간 지도·스크롤·규칙·소리 정의를 바꾸는 변형(T)만 골라 그 조합마다 차분을 나눈다 (명세 2-4).
                    # 나머지 변형은 각 차분 안에서 when으로 남는다.
                    T = sorted({vi for it in P["items"] if it[5] and not item_ok(it) for vi, _ in it[5]})
                    m = math.prod(sizes[v] for v in T)
                    if m > MAX_SPLIT:
                        raise Unsupported(f"#RANDOM이 시간 지도·스크롤·정의를 바꾸는 변형 {len(T)}개, 조합 {m}개 > {MAX_SPLIT} (명세 2-4)")
                    plan = []
                    for asg in itertools.product(*[range(1, sizes[v] + 1) for v in T]):
                        fix = dict(zip(T, asg))
                        PP = dict(P, items=[it[:5] + (tuple((v, s) for v, s in it[5] if v not in fix),) for it in P["items"]
                                            if all(fix[v] in s for v, s in it[5] if v in fix)])
                        assert factored_ok(PP)
                        plan.append(factored(PP, base_id + "~" + "~".join(f"r{v + 1}={a}" for v, a in fix.items())))
                    crep[f"#RANDOM이 시간 지도·스크롤·정의를 바꿈 → 변형 {['r%d' % (v + 1) for v in T]} 조합마다 차분 {m}개 (명세 2-4)"] += 1
            made = []
            for cid, r, items, sizes_used in plan:
                if not any(o[0] in ("note", "noteN") and o[2] != "mine" for o, _ in items):
                    crep["노트 없는 차분 건너뜀"] += 1
                    continue
                doc, used = chart_doc(cid, src_rel, enc, r, layout, items, sizes_used, r["losses"])
                made.append((cid, doc, used, dict(r["losses"])))
                titles[r["head"].get("TITLE", "")] += 1
                artists[r["head"].get("ARTIST", "")] += 1
        except Unsupported as e:
            status[src_rel] = {"status": "unsupported", "reason": str(e)}
            rep[f"미지원 차분: {e}"] += 1
            continue
        except Exception as e:  # noqa: BLE001 — 차분 하나의 실패가 폴더 전체를 멈추지 않게, 이유를 남긴다
            status[src_rel] = {"status": "failed", "reason": repr(e)[:300]}
            rep[f"실패 차분: {type(e).__name__}"] += 1
            continue
        if not made:
            status[src_rel] = {"status": "unsupported", "reason": "노트 없음"}
            continue
        layouts.add(layout)
        sources.append((src_rel, hashlib.sha256(raw).hexdigest()))
        for cid, doc, used, lo in made:
            docs.append((cid, doc))
            all_used |= used
        lo_all = collections.Counter()
        for *_, lo in made:
            lo_all.update(lo)
        status[src_rel] = {"status": "incomplete" if lo_all else "ok", "charts": [m[0] for m in made], "losses": dict(lo_all)}
        rep.update(crep)
    if not docs:
        return None
    if key not in ids:
        ids[key] = str(uuid.uuid7())
    title = titles.most_common(1)[0][0] or folder.name
    tl = [f"# BMS 폴더 {folder.name} — {CONVERTER} (B-1: 폴더 하나 = Track 하나)", 'format = "1.0"', f"id = {tq(ids[key])}", "revision = 1", "",
          "[info]", f"title = {tq(title)}", f"title_sort = {tq(title.lower())}", f"artist = {tq(artists.most_common(1)[0][0])}", 'genre = ""',
          f"tags = [\"bms\", {tq('folder:' + folder.name)}]", "", "[origin]", f"anchor_chart = {tq(docs[0][0])}", ""]
    for name in sorted(layouts):
        tl += ["[[layout]]", f'id = "{name}"', "lanes = [" + ", ".join(f'{{ id = "{l}", role = "{r}" }}' for l, r in LAYOUTS[name]) + "]",
               "groups = [" + ", ".join(f'{{ id = "{g}", lanes = [' + ", ".join(f'"{x}"' for x in ls) + "] }" for g, ls in GROUPS[name]) + "]", ""]
    for sid in sorted(all_used):
        rel, tail = sid.rsplit("@", 1)
        path, g = audio[rel]
        tl += ["[[sound]]", f"id = {tq(sid)}", f"file = {tq(path)}"] + ([f'gain = "{q(g)}"'] if g else [])
        if "+" in tail:  # BMSON 조각
            st, ln = tail.split("+")
            tl += ([f'start = "{st}"'] if st != "0" else []) + ([f'length = "{ln}"'] if ln != "0" else [])
        tl += ['retrigger = "cut"', ""]
    tl += ["[source]", 'format = "bms"', f'converter = "{CONVERTER}"',
           "files = [" + ", ".join(f'{{ path = {tq(pth)}, hash = "sha256:{h}" }}' for pth, h in sources) + "]", ""]
    rms = (err["sq"] / err["n"]) ** 0.5 if err["n"] else 0.0
    counts = collections.Counter(s["status"] for s in status.values())
    report = [f"# 변환 보고서: BMS {folder.name}", "", f"- 변환기 {CONVERTER}", f"- 차분 {len(docs)}: " + ", ".join(d[0] for d in docs),
              "- 원본 차분 파일 " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())), f"- 소리 {len(all_used)}개 (파일 {len(audio)})"]
    if err["n"]:
        report.append(f"- 손실 경로 오차 (gain 되돌린 값 기준): 최대 {err['max']:.3e}, RMS {rms:.3e}")
    report += [f"- {k}: {v}" for k, v in sorted(rep.items())]
    lossy = [(k, s) for k, s in sorted(status.items()) if s["status"] != "ok"]
    if lossy:
        report += ["", "## 원본 차분별 (ok 아닌 것)"] + [f"- `{k}`: {s['status']} — {s.get('reason') or s.get('losses')}" for k, s in lossy]
    # 한 번에 바꾸기: 새 차분 폴더를 만든 뒤 옛 것과 맞바꾼다
    new = out / "charts.new"
    if new.exists():
        shutil.rmtree(new)
    new.mkdir()
    for cid, doc in docs:
        (new / f"{cid}.toml").write_text(doc, encoding="utf-8")
    for name, text in (("track.toml", "\n".join(tl)), ("report.md", "\n".join(report) + "\n"),
                       ("status.json", json.dumps(status, ensure_ascii=False, indent=1))):
        (out / (name + ".new")).write_text(text, encoding="utf-8")
    if (out / "charts").exists():
        shutil.rmtree(out / "charts")
    new.rename(out / "charts")
    for name in ("track.toml", "report.md", "status.json"):
        os.replace(out / (name + ".new"), out / name)
    (out / "expected.json").unlink(missing_ok=True)  # 0.1의 기대값 (검증기가 원본을 직접 읽도록 바뀜)
    keep = {pathlib.PurePosixPath(a[0]).name for a in audio.values()}
    for f in (out / "audio").iterdir():
        if f.name not in keep:
            f.unlink()
    return out, len(docs), len(all_used), rep, counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("folders", nargs="+")
    ap.add_argument("--out", default=str(ROOT / "cp3" / "content" / "bms"))
    ap.add_argument("--selfcheck", type=int, default=0)
    a = ap.parse_args()
    global SELFCHECK
    SELFCHECK = a.selfcheck
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
        out, nc, ns, rep, counts = r
        print(f"{out.name}: 차분 {nc}, 소리 {ns}, 원본 {dict(counts)}", flush=True)
        if len(ids) != before:
            idp.write_text(json.dumps(ids, indent=1, ensure_ascii=False), encoding="utf-8")


if __name__ == "__main__":
    main()
