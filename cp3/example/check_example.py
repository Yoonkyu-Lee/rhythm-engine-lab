"""합성 예시 Track을 Track 형식 명세 v1.2(cp3/Track_형식.md) 2·3장대로 읽어 검산한다.

컴파일러가 아니라 명세의 검산기다(실행 형식 바이너리·묶음은 3-1 컴파일러에서). 하는 일:
- 스키마: 모든 표의 허용 키·중복 ID·참조(소리·레인·레이아웃·변형·when·choke)·범위
- 시각: 위치 → 차분 자기 시각 ns (유리수 누적, 한 번 짝수 반올림), 노트 소리 offset = round(소리) − round(노트)
- 배속 변형 펼치기(물려받기, shift·reference_scroll 기본값, 물려받는 키를 적으면 오류)
- 레인 겹침·롱노트 길이 (변형 선택 조합마다, ns 반올림 뒤)
- 스크롤 지도(명시·기본, STOP 구간 삽입), 마디선
- 정규 발음 순서로 같은 소리 끊기·choke 모의 실행
- 음악 레이어 (Track 시각, 소리 해시, start, 최종 이득), 공유는 차분 ID 집합으로
- 예시의 기대값(아래 EXPECT)이 다르면 실패

    python -X utf8 cp3/example/check_example.py [Track 폴더]
종료 코드: 문제가 하나라도 있으면 1.
"""
import collections
import hashlib
import itertools
import pathlib
import struct
import sys
import tomllib
from fractions import Fraction as F

import numpy as np
import soundfile as sf

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent / "synth_track")
I64 = (-(2**63), 2**63 - 1)
problems = []


def bad(msg):
    problems.append(msg)


# ---------------------------------------------------------------- 스키마

KEYS = {
    "track": {"format", "id", "revision", "info", "origin", "layout", "sound", "media", "source"},
    "info": {"title", "title_sort", "artist", "genre", "tags", "highlight", "lang"},
    "lang": {"title", "title_sort", "artist", "genre"},
    "origin": {"anchor_chart"},
    "layout": {"id", "lanes", "groups"},
    "lane": {"id", "role"},
    "group": {"id", "lanes"},
    "sound": {"id", "file", "gain", "start", "length", "retrigger", "choke"},
    "media": {"hash", "kind", "role", "start"},
    "source": {"format", "converter", "files"},
    "chart": {"id", "name", "level", "layout", "shift", "reference_scroll", "rules", "timing", "scroll",
              "event", "note", "lane_sound", "variant", "losses"},
    "variant_chart": {"id", "name", "level", "base", "time_scale", "shift", "reference_scroll"},
    "rules": {"long_note", "empty_lane"},
    "timing": {"unit", "ticks_per_beat", "tempo", "meter", "stops", "bars"},
    "tempo": {"at", "bpm"}, "meter": {"measure", "length"}, "stop": {"at", "beats"},
    "scroll": {"points"}, "point": {"at", "pos", "speed"},
    "event": {"at", "sound", "gain", "when"},
    "note": {"at", "lane", "kind", "end", "sounds", "damage", "when"},
    "note_sound": {"sound", "at", "gain"},
    "lane_sound": {"at", "lane", "sound", "when"},
    "variant": {"id", "choices", "weights"},
}
ROLES = {"key", "scratch", "side", "trigger", "pedal"}
CODECS = {("OGG", "VORBIS"), ("FLAC", "PCM_16"), ("FLAC", "PCM_24"), ("FLAC", "PCM_S8"), ("WAV", "MS_ADPCM")}


def keys(where, d, kind):
    for k in d:
        if k not in KEYS[kind]:
            bad(f"{where}: 알 수 없는 키 '{k}'")


def unique(where, items):
    seen = set()
    for x in items:
        if x in seen:
            bad(f"{where}: 중복 ID '{x}'")
        seen.add(x)
    return seen


def rat(s, where="", lo=None, hi=None):
    if not isinstance(s, str):
        bad(f"{where}: 수는 문자열이어야 함: {s!r}")
        s = str(s)
    try:
        v = F(s.strip())
    except ValueError:
        bad(f"{where}: 유리수가 아님: {s!r}")
        return F(0)
    if (lo is not None and v < lo) or (hi is not None and v > hi):
        bad(f"{where}: 범위 밖 {s} (허용 {lo}~{hi})")
    return v


def half_even(x: F) -> int:
    n, r = divmod(x.numerator, x.denominator)
    if 2 * r > x.denominator or (2 * r == x.denominator and n % 2 == 1):
        n += 1
    if not I64[0] <= n <= I64[1]:
        bad(f"i64 범위 밖: {n}")
    return n


def conds(when):
    """when → frozenset((변형, 선택))."""
    if when is None:
        return frozenset()
    return frozenset(tuple(w.split("=", 1)) for w in ([when] if isinstance(when, str) else when))


# ---------------------------------------------------------------- 시각

class Timing:
    """차분 하나의 위치 → 초 (유리수). 명세 3-1."""

    def __init__(self, t, where):
        keys(where, t, "timing")
        self.unit = t.get("unit")
        if self.unit not in ("beat", "ms", "tick"):
            bad(f"{where}: unit 오류 {self.unit}")
        self.tpb = t.get("ticks_per_beat")
        if self.unit == "tick" and not (isinstance(self.tpb, int) and self.tpb > 0):
            bad(f"{where}: ticks_per_beat 필요")
        for m in t.get("meter", []):
            keys(where, m, "meter")
        self.meter = sorted((m["measure"], rat(m["length"], where, lo=F(1, 1000))) for m in t.get("meter", []))
        for x in t.get("tempo", []):
            keys(where, x, "tempo")
        self.tempo = sorted((self.beat(x["at"]), rat(x["bpm"], where)) for x in t.get("tempo", [])) if self.unit != "ms" else \
            sorted((rat(x["at"], where), rat(x["bpm"], where)) for x in t.get("tempo", []))
        for x in t.get("stops", []):
            keys(where, x, "stop")
        self.stops = sorted((self.beat(x["at"]), rat(x["beats"], where, lo=F(0))) for x in t.get("stops", []))
        if self.unit != "ms" and (not self.tempo or self.tempo[0][0] != 0):
            bad(f"{where}: tempo는 위치 0에서 시작해야 함 (G10)")
        self.bars_explicit = [rat(b, where) for b in t.get("bars", [])]

    def measure_start(self, m):
        total, length, cur = F(0), self.meter[0][1], 0
        for mm, ln in self.meter:
            if mm > m:
                break
            total += (mm - cur) * length
            cur, length = mm, ln
        return total + (m - cur) * length

    def beat(self, pos):
        if self.unit == "tick":
            return F(int(pos)) / self.tpb
        if ":" in pos:
            if not self.meter:
                bad("'마디:박' 표기에 meter가 없음")
                return F(0)
            m, b = pos.split(":")
            return self.measure_start(int(m)) + rat(b)
        return rat(pos)

    def bpm_at(self, b):
        cur = self.tempo[0][1]
        for tb, bpm in self.tempo:
            if tb <= b:
                cur = bpm
        return cur

    def sec_of_beat(self, b):
        sec = F(0)
        if b < 0:  # 0보다 앞: 첫 tempo를 뒤로 늘임
            return b * 60 / self.tempo[0][1]
        for i, (tb, bpm) in enumerate(self.tempo):
            end = self.tempo[i + 1][0] if i + 1 < len(self.tempo) else None
            if b <= tb:
                break
            if bpm <= 0:
                bad(f"bpm ≤ 0은 시간 계산에 쓰지 않음 (명세 3-1): {bpm}")
                continue
            sec += ((min(b, end) if end is not None else b) - tb) * 60 / bpm
        for sb, beats in self.stops:  # 정지 위치와 같은 위치의 이벤트는 정지 전 (G9)
            if b > sb:
                sec += beats * 60 / self.bpm_at(sb)
        return sec

    def seconds(self, pos):
        if self.unit == "ms":
            return rat(pos) / 1000
        return self.sec_of_beat(self.beat(pos))

    def plus(self, base_pos, rel):
        if not rel.startswith("+"):
            bad(f"상대 위치는 '+'로 시작: {rel}")
        if self.unit == "ms":
            return str(rat(base_pos) + rat(rel[1:]))
        if self.unit == "tick":
            return str(int(base_pos) + int(rel[1:]))
        return str(self.beat(base_pos) + rat(rel[1:]))

    def bpm_at_ms(self, ms):
        cur = self.tempo[0][1] if self.tempo else F(120)
        for t, bpm in self.tempo:
            if t <= ms:
                cur = bpm
        return cur


def ns(sec):
    return half_even(sec * 10**9)


# ---------------------------------------------------------------- 메인

def main():
    tr = tomllib.loads((ROOT / "track.toml").read_text(encoding="utf-8"))
    keys("track.toml", tr, "track")
    keys("info", tr.get("info", {}), "info")
    for lang in tr.get("info", {}).get("lang", {}).values():
        keys("info.lang", lang, "lang")
    keys("origin", tr.get("origin", {}), "origin")
    layouts = {}
    unique("layout", [l["id"] for l in tr["layout"]])
    for l in tr["layout"]:
        keys(f"layout {l['id']}", l, "layout")
        ids = unique(f"layout {l['id']} 레인", [x["id"] for x in l["lanes"]])
        for x in l["lanes"]:
            keys(f"layout {l['id']} 레인", x, "lane")
            if x["role"] not in ROLES:
                bad(f"layout {l['id']}: 알 수 없는 역할 {x['role']}")
        for g in l.get("groups", []):
            keys(f"layout {l['id']} 묶음", g, "group")
            for ln in g["lanes"]:
                if ln not in ids:
                    bad(f"layout {l['id']} 묶음 {g['id']}: 없는 레인 {ln}")
        if len(l["lanes"]) > 64:
            bad(f"layout {l['id']}: 레인 64개 초과")
        layouts[l["id"]] = [x["id"] for x in l["lanes"]]
    unique("sound", [s["id"] for s in tr["sound"]])
    sounds = {s["id"]: s for s in tr["sound"]}
    for m in tr.get("media", []):
        keys("media", m, "media")
    keys("source", tr.get("source", {}), "source")

    # 소리 해시 (명세 3-5: "SND1" ‖ rate ‖ channels ‖ frames ‖ PCM. 정수 코덱은 i32, Vorbis는 f32)
    # 명세는 BLAKE3, 여기서는 표준 라이브러리 blake2b로 대신한다(값 비교만 하므로 같은 역할).
    pcm_h, info = {}, {}
    for sid, s in sounds.items():
        keys(f"소리 {sid}", s, "sound")
        rat(s.get("gain", "1"), f"소리 {sid} gain", lo=F(0))
        if s.get("retrigger", "overlap") not in ("overlap", "cut"):
            bad(f"소리 {sid}: retrigger 값 오류")
        p = ROOT / s["file"]
        fi = sf.info(p)
        if (fi.format, fi.subtype) not in CODECS:
            bad(f"소리 {sid}: 엔진 디코더 셋 밖 {fi.format}/{fi.subtype}")
        if fi.subtype == "VORBIS":
            a, _ = sf.read(p, dtype="float32", always_2d=True)
            body = a.astype("<f4").tobytes()
        else:
            a, _ = sf.read(p, dtype="int32", always_2d=True)
            shift = {"PCM_16": 16, "MS_ADPCM": 16, "PCM_24": 8, "PCM_S8": 24}[fi.subtype]
            body = (a >> shift).astype("<i4").tobytes()
        if len(a) == 0:
            bad(f"소리 {sid}: 샘플 수 0")
        head = b"SND1" + struct.pack("<IHQ", fi.samplerate, fi.channels, len(a))
        pcm_h[sid] = hashlib.blake2b(head + body, digest_size=16).hexdigest()
        info[sid] = (fi.samplerate, fi.channels, len(a), fi.format, fi.subtype)

    raw = {}
    for p in sorted((ROOT / "charts").glob("*.toml")):
        c = tomllib.loads(p.read_text(encoding="utf-8"))
        if c.get("id") != p.stem:
            bad(f"차분 id ≠ 파일 이름: {c.get('id')} / {p.stem}")
        raw[c["id"]] = c
    if tr["origin"]["anchor_chart"] not in raw:
        bad(f"원점 차분 없음: {tr['origin']['anchor_chart']}")

    charts = {}  # id → 펼친 차분
    for cid, c in raw.items():
        if "base" in c:
            continue
        charts[cid] = compile_chart(cid, c, layouts, sounds)
    for cid, c in raw.items():
        if "base" not in c:
            continue
        keys(f"{cid}", c, "variant_chart")
        b = raw.get(c["base"])
        if b is None or "base" in b:
            bad(f"{cid}: base가 없거나 또 배속 변형")
            continue
        sc = rat(c["time_scale"], f"{cid} time_scale")
        if sc <= 0:
            bad(f"{cid}: time_scale ≤ 0")
            continue
        base = charts[c["base"]]
        v = dict(base)
        v["id"] = cid
        v["shift"] = rat(c["shift"], cid) if "shift" in c else base["shift"] / sc
        v["reference_scroll"] = rat(c["reference_scroll"], cid) if "reference_scroll" in c else base["reference_scroll"] * sc
        v["starts"] = [dict(e, t=e["t"] / sc) for e in base["starts"]]
        v["notes"] = [dict(n, t=n["t"] / sc, end=n["end"] / sc) for n in base["notes"]]
        v["lane_sounds"] = [dict(x, t=x["t"] / sc) for x in base["lane_sounds"]]
        v["scroll"] = [(t / sc, pos, spd * sc) for t, pos, spd in base["scroll"]]
        v["bars"] = [t / sc for t in base["bars"]]
        charts[cid] = v

    report = {}
    for cid, c in charts.items():
        report[cid] = finish_chart(c, sounds)

    # ---------------- 출력과 기대값
    print("== 소리")
    for sid in sounds:
        r, ch, n, fmt, sub = info[sid]
        print(f"  {sid:12} {fmt}/{sub:10} {r:6} Hz {ch}ch {n:7} 샘플  PCM {pcm_h[sid][:8]}")
    by_pcm = collections.defaultdict(list)
    for sid in sounds:
        by_pcm[pcm_h[sid]].append(sid)
    dup = [v for v in by_pcm.values() if len(v) > 1]
    print("  같은 내용 다른 ID (데이터 한 번, ID는 따로):", dup)

    print("== 차분")
    layer = collections.defaultdict(set)
    negative = []
    for cid, c in charts.items():
        r = report[cid]
        sh = ns(c["shift"] / 1000)
        print(f"  {cid:12} 발음 {len(c['starts']):3}  노트 {len(c['notes']):3}  shift {sh:+,} ns  "
              f"변형 조합 {r['combos']}  끊김 cut {r['cut']} · choke {r['choke']}  스크롤 점 {len(r['scroll'])}  마디선 {len(r['bars'])}")
        for e in c["starts"]:
            t = ns(e["t"]) + sh
            g = rat(sounds[e["sound"]].get("gain", "1")) * e["gain"]
            layer[(t, pcm_h[e["sound"]], (rat(sounds[e["sound"]].get("start", "0")), rat(sounds[e["sound"]].get("length", "0"))), g)].add(cid)
            if t < 0:
                negative.append((cid, t, e["sound"]))
    shared = {k: v for k, v in layer.items() if len(v) > 1}
    print(f"== 음악 레이어: 이벤트 {len(layer)}개, 둘 이상 차분이 공유 {len(shared)}개")
    for k, v in sorted(shared.items()):
        name = next(s for s in sounds if pcm_h[s] == k[1] and (rat(sounds[s].get("start", "0")), rat(sounds[s].get("length", "0"))) == k[2])
        print(f"  Track {k[0]:>13,} ns  {name:10} 이득 {k[3]}  {sorted(v)}")
    print("== 음수 Track 시각:", sorted(negative))

    # 예시의 기대값 (명세대로 계산한 정답; 다르면 실패)
    def first(cid, sound, kind="auto"):
        return [ns(e["t"]) for e in charts[cid]["starts"] if e["sound"] == sound and e["kind"] == kind]
    def EXPECT():
      try:
        return [
        ("tick 1728 = 13000/3 ms", first("8b-a", "snare")[-1], first("7k-c", "snare")[-1]),
        ("snare 정답 ns", first("8b-a", "snare")[-1], 4_333_333_333),
        ("배속 변형 shift 기본값 = −500 ÷ 1.25", ns(charts["7k-b-x1.25"]["shift"] / 1000), -400_000_000),
        ("반주 stem은 4차분 모두 Track 0에서 하나",
         sorted(next(v for k, v in layer.items() if k[0] == 0 and k[1] == pcm_h["bass_long"])), sorted(charts)),
        ("음수 Track 시각", sorted(negative), [("7k-b", -500_000_000, "kick"), ("7k-b-x1.25", -400_000_000, "kick")]),
        ("8b-a choke 1회 (hat_closed가 울리는 hat_open을 끊음)", report["8b-a"]["choke"], 1),
        ("7k-c cut 1회 (piano_c_alt 재발음, 같은 PCM 다른 ID piano_c는 안 끊음)", report["7k-c"]["cut"], 1),
        ("7k-b 스크롤에 STOP 구간 삽입 (속도 0 점)", any(s == 0 for _, _, s in report["7k-b"]["scroll"]), True),
        ("7k-b 13박 정지 뒤 14박 노트 = 6.5초", ns(next(n["t"] for n in charts["7k-b"]["notes"] if n["lane"] == "K5")), 6_500_000_000),
        ("8b-a 아르페지오 offset = 1/4박(120 BPM) = 125 ms", report["8b-a"]["arp_offsets"], [125_000_000]),
        ("7k-b 변형 조합 4개 (r1 × r2)", report["7k-b"]["combos"], 4),
        ("bass_slice: bass_long과 같은 PCM, start 1초·length 0.5초인 별도 레이어 항목 (v1.5)",
         any(k[1] == pcm_h["bass_long"] and k[2] == (44100, 22050) for k in layer), True),
        ]
      except Exception as ex:  # 앞의 오류로 차분·소리가 빠지면 기대값을 셀 수 없음
        bad(f"기대값을 계산할 수 없음: {type(ex).__name__} {ex}")
        return []
    print("== 기대값")
    for name, got, want in EXPECT():
        ok = got == want
        print(f"  {'OK ' if ok else 'BAD'} {name}: {got}")
        if not ok:
            bad(f"기대값 다름: {name}: {got} ≠ {want}")
    print("== 문제", len(problems))
    for p in problems:
        print("  -", p)
    return 1 if problems else 0


def compile_chart(cid, c, layouts, sounds):
    keys(cid, c, "chart")
    keys(f"{cid} rules", c.get("rules", {}), "rules")
    if c.get("rules", {}).get("long_note", "cn") not in ("ln", "cn", "hcn"):
        bad(f"{cid}: long_note 값 오류")
    if c.get("rules", {}).get("empty_lane", "previous_note") not in ("previous_note", "next_note", "none"):
        bad(f"{cid}: empty_lane 값 오류")
    if c.get("layout") not in layouts:
        bad(f"{cid}: 없는 레이아웃 {c.get('layout')}")
        return {"id": cid, "starts": [], "notes": [], "lane_sounds": [], "scroll": [], "bars": [], "shift": F(0),
                "reference_scroll": F(1), "variants": {}, "lanes": []}
    lanes = layouts[c["layout"]]
    tm = Timing(c["timing"], f"{cid} timing")
    variants = {}
    unique(f"{cid} 변형", [v["id"] for v in c.get("variant", [])])
    for v in c.get("variant", []):
        keys(f"{cid} 변형", v, "variant")
        unique(f"{cid} 변형 {v['id']} 선택", v["choices"])
        if "weights" in v and len(v["weights"]) != len(v["choices"]):
            bad(f"{cid} 변형 {v['id']}: weights 수 ≠ choices 수")
        variants[v["id"]] = v["choices"]

    def check_when(w):
        for vid, ch in conds(w):
            if ch not in variants.get(vid, []):
                bad(f"{cid}: 없는 변형 선택 {vid}={ch}")

    def snd(sid, where):
        if sid not in sounds:
            bad(f"{cid} {where}: 없는 소리 {sid}")
            return False
        return True

    starts, notes, lsnds = [], [], []
    for i, e in enumerate(c.get("event", [])):
        keys(f"{cid} event", e, "event")
        check_when(e.get("when"))
        if not snd(e["sound"], "event"):
            continue
        starts.append(dict(t=tm.seconds(e["at"]), kind="auto", seq=(0, i), sound=e["sound"],
                           gain=rat(e.get("gain", "1"), f"{cid} event gain", lo=F(0)), cond=conds(e.get("when"))))
    for i, n in enumerate(c.get("note", [])):
        keys(f"{cid} note", n, "note")
        if n["lane"] not in lanes:
            bad(f"{cid}: 레이아웃에 없는 레인 {n['lane']}")
            continue
        if n["kind"] not in ("tap", "long", "mine"):
            bad(f"{cid}: 노트 종류 오류 {n['kind']}")
        if n["kind"] == "long" and "end" not in n:
            bad(f"{cid}: 롱노트에 end 없음")
        if "damage" in n:
            if n["kind"] != "mine":
                bad(f"{cid}: damage는 지뢰에만")
            rat(n["damage"], f"{cid} damage", lo=F(0), hi=F(100))
        check_when(n.get("when"))
        t = tm.seconds(n["at"])
        end = tm.seconds(n["end"]) if "end" in n else t
        nd = dict(t=t, end=end, lane=n["lane"], kind=n["kind"], cond=conds(n.get("when")), at=n["at"])
        notes.append(nd)
        for j, s in enumerate(n.get("sounds", [])):
            s = {"sound": s} if isinstance(s, str) else s
            keys(f"{cid} note sound", s, "note_sound")
            if not snd(s["sound"], "note sound"):
                continue
            ts = tm.seconds(tm.plus(n["at"], s["at"])) if "at" in s else t
            starts.append(dict(t=ts, kind="note", seq=(1, lanes.index(n["lane"]), i, j), sound=s["sound"],
                               gain=rat(s.get("gain", "1"), f"{cid} note gain", lo=F(0)), cond=nd["cond"], note_t=t))
    for ls in c.get("lane_sound", []):
        keys(f"{cid} lane_sound", ls, "lane_sound")
        snd(ls["sound"], "lane_sound")
        if ls["lane"] not in lanes:
            bad(f"{cid} lane_sound: 레이아웃에 없는 레인 {ls['lane']}")
        check_when(ls.get("when"))
        lsnds.append(dict(t=tm.seconds(ls["at"]), lane=ls["lane"], sound=ls["sound"], cond=conds(ls.get("when"))))

    # 스크롤 (명세 3-2)
    pts = []
    if "scroll" in c:
        keys(f"{cid} scroll", c["scroll"], "scroll")
        for i, p in enumerate(c["scroll"]["points"]):
            keys(f"{cid} scroll point", p, "point")
            t = tm.seconds(p["at"])
            if i == 0 and t != 0:
                bad(f"{cid}: 첫 스크롤 점은 at = 0")
            pos = rat(p["pos"]) if "pos" in p else (F(0) if i == 0 else None)
            pts.append((t, pos, rat(p["speed"], f"{cid} scroll speed") / 60))
    elif tm.unit != "ms":
        for tb, bpm in tm.tempo:
            t = tm.sec_of_beat(tb)
            pts.append((t, tb, bpm / 60))
    # 위치 없는 점은 이동 시간(정지 구간 제외)만 적분, 정지 시작 (위치, 0)·끝 (같은 위치, 그때 속도) — 명세 3-2
    items = [(t_, 1, ("pt", pos, s)) for t_, pos, s in pts]
    if tm.unit != "ms":
        for sb, beats in tm.stops:
            t0 = tm.sec_of_beat(sb); d = beats * 60 / tm.bpm_at(sb)
            items += [(t0, 2, ("start",)), (t0 + d, 0, ("end",))]
    items.sort(key=lambda x: (x[0], x[1]))
    out, t_ref, pos_ref, v, in_stop = [], F(0), F(0), F(0), False
    for t_, _, it in items:
        here = pos_ref if in_stop else pos_ref + v * (t_ - t_ref)
        if it[0] == "pt":
            v = it[2]
            pnt = it[1] if it[1] is not None else here
            out.append((t_, pnt, F(0) if in_stop else v)); pos_ref = pnt
        elif it[0] == "start":
            out.append((t_, here, F(0))); pos_ref = here; in_stop = True
        else:
            in_stop = False; out.append((t_, pos_ref, v))
        t_ref = t_
    pts = []
    for x in out:
        if pts and pts[-1][0] == x[0]:
            pts.pop()
        pts.append(x)
    for a, b in zip(pts, pts[1:]):
        if b[0] < a[0]:
            bad(f"{cid}: 스크롤 점 시간순 아님")
    # 마디선
    if tm.bars_explicit:
        bars = [b / 1000 for b in tm.bars_explicit] if tm.unit == "ms" else [tm.seconds(str(b)) for b in tm.bars_explicit]
    elif tm.meter and tm.unit != "ms":
        last_t = max([s["t"] for s in starts] + [n["end"] for n in notes] + [F(0)])
        bars, m = [], 0
        while tm.sec_of_beat(tm.measure_start(m)) <= last_t:
            bars.append(tm.sec_of_beat(tm.measure_start(m)))
            m += 1
    else:
        bars = []
    return dict(id=cid, starts=starts, notes=notes, lane_sounds=lsnds, scroll=pts, bars=bars,
                shift=rat(c.get("shift", "0"), f"{cid} shift"), reference_scroll=rat(c["reference_scroll"], cid),
                variants=variants, lanes=lanes)


def finish_chart(c, sounds):
    """ns 반올림 뒤 검사 + 정규 순서 끊기·choke 모의 + offset."""
    cid = c["id"]
    combos = [dict(zip(c["variants"], ch)) for ch in itertools.product(*c["variants"].values())] or [{}]
    on = lambda cond, cb: all(cb.get(v) == ch for v, ch in cond)
    for cb in combos:
        per = collections.defaultdict(list)
        for n in c["notes"]:
            if on(n["cond"], cb):
                per[n["lane"]].append((ns(n["t"]), ns(n["end"]), n["kind"]))
        for lane, L in per.items():
            L.sort()
            for t0, t1, k in L:
                if k == "long" and t1 <= t0:
                    bad(f"{cid} {cb}: 레인 {lane} 롱노트 끝 ≤ 시작 (ns)")
            for (a0, a1, _), (b0, _, _) in zip(L, L[1:]):
                if b0 <= a1:
                    bad(f"{cid} {cb}: 레인 {lane} 노트 겹침 (ns 반올림 뒤)")
    # offset = round(소리) − round(노트)
    arp = [ns(e["t"]) - ns(e["note_t"]) for e in c["starts"] if e["kind"] == "note" and e["t"] != e["note_t"]]
    # 정규 발음 순서로 끊기·choke (모든 조합 합산)
    cut = choke = 0
    for cb in combos:
        live = []  # (끝 ns, 소리 id)
        order = sorted((e for e in c["starts"] if on(e["cond"], cb)), key=lambda e: (ns(e["t"]), e["seq"]))
        for e in order:
            t = ns(e["t"])
            s = sounds[e["sound"]]
            live = [(end, sid) for end, sid in live if end > t]
            if s.get("retrigger") == "cut" and any(sid == e["sound"] for _, sid in live):
                cut += 1
                live = [(end, sid) for end, sid in live if sid != e["sound"]]
            ch = s.get("choke")
            if ch and any(sounds[sid].get("choke") == ch and sid != e["sound"] for _, sid in live):
                choke += 1
                live = [(end, sid) for end, sid in live if sounds[sid].get("choke") != ch]
            fi = sf.info(ROOT / s["file"])
            n_fr = int(s["length"]) if "length" in s else fi.frames - int(s.get("start", "0"))  # v1.5 length
            live.append((t + n_fr * 10**9 // fi.samplerate, e["sound"]))
    scroll = [(ns(t), float(p), float(s)) for t, p, s in c["scroll"]]
    return dict(combos=len(combos), cut=cut, choke=choke, scroll=scroll, bars=[ns(b) for b in c["bars"]], arp_offsets=arp)


if __name__ == "__main__":
    sys.exit(main())
