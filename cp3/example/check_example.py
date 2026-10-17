"""합성 예시 Track을 Track 형식 명세 v1(cp3/Track_형식.md) 2·3장대로 읽어 검사한다.

컴파일러가 아니라 명세의 검산기다: 참조 검사, 위치 → 차분 자기 시각 ns(유리수 누적, 한 번 짝수 반올림),
Track 시각, 음악 레이어(같은 (Track 시각, 소리 해시, start, gain)은 하나), 변형별 레인 겹침, 소리 해시(바이트·디코딩).

    python -X utf8 cp3/example/check_example.py [Track 폴더]
"""
import collections
import hashlib
import pathlib
import sys
import tomllib
from fractions import Fraction as F

import numpy as np
import soundfile as sf

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent / "synth_track")
problems = []


def bad(msg):
    problems.append(msg)


def rat(s):
    if not isinstance(s, str):
        bad(f"수는 문자열이어야 함: {s!r}")
        s = str(s)
    return F(s.strip())  # Fraction은 "1.5", "7/3", "-2" 를 정확히 읽는다


def half_even(x: F) -> int:
    n, r = divmod(x.numerator, x.denominator)
    twice = 2 * r
    if twice > x.denominator or (twice == x.denominator and n % 2 == 1):
        n += 1
    return n


class Timing:
    """차분 하나의 위치 → 초 (유리수)."""

    def __init__(self, t):
        self.unit = t.get("unit")
        if self.unit not in ("beat", "ms", "tick"):
            bad(f"unit 오류: {self.unit}")
        self.tpb = t.get("ticks_per_beat")
        self.meter = sorted((m["measure"], rat(m["length"])) for m in t.get("meter", []))
        self.tempo = sorted((self.beat(x["at"]), rat(x["bpm"])) for x in t.get("tempo", []))
        self.stops = sorted((self.beat(x["at"]), rat(x["beats"])) for x in t.get("stops", []))
        if self.unit != "ms" and (not self.tempo or self.tempo[0][0] != 0):
            bad("tempo는 위치 0에서 시작해야 함")

    def measure_start(self, m):
        """마디 m의 시작 박."""
        if not self.meter:
            bad("'마디:박' 표기에 meter가 없음")
            return F(0)
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
        if self.unit == "ms":
            return None
        if ":" in pos:
            m, b = pos.split(":")
            return self.measure_start(int(m)) + rat(b)
        return rat(pos)

    def seconds(self, pos):
        if self.unit == "ms":
            return rat(pos) / 1000
        b = self.beat(pos)
        sec = F(0)
        for i, (tb, bpm) in enumerate(self.tempo):
            end = self.tempo[i + 1][0] if i + 1 < len(self.tempo) else None
            if b <= tb:
                break
            seg = (min(b, end) if end is not None else b) - tb
            if bpm <= 0:
                bad(f"bpm ≤ 0은 시간 계산에 쓰지 않음 (명세 3-1): {bpm}")
            else:
                sec += seg * 60 / bpm
        for sb, beats in self.stops:  # 정지 위치와 같은 위치의 이벤트는 정지 전에 울림
            if b > sb:
                sec += beats * 60 / self.bpm_at(sb)
        return sec

    def bpm_at(self, b):
        cur = self.tempo[0][1]
        for tb, bpm in self.tempo:
            if tb <= b:
                cur = bpm
        return cur

    def rel(self, base_pos, rel_pos):
        """'+x' 상대 위치 → 절대 위치 문자열과 같은 단위."""
        if self.unit == "beat" and ":" in base_pos:
            return str(self.beat(base_pos) + rat(rel_pos[1:]))
        return str(rat(base_pos) + rat(rel_pos[1:]))


def main():
    tr = tomllib.loads((ROOT / "track.toml").read_text(encoding="utf-8"))
    layouts = {l["id"]: l for l in tr["layout"]}
    sounds = {s["id"]: s for s in tr["sound"]}
    # 소리 해시: 바이트 / 디코딩 PCM (명세는 BLAKE3, 여기서는 표준 라이브러리 blake2b로 대신)
    byte_h, pcm_h, info = {}, {}, {}
    for sid, s in sounds.items():
        p = ROOT / s["file"]
        byte_h[sid] = hashlib.blake2b(p.read_bytes(), digest_size=16).hexdigest()
        a, rate = sf.read(p, dtype="float32", always_2d=True)
        pcm_h[sid] = hashlib.blake2b(np.ascontiguousarray(a).tobytes() + f"{rate}/{a.shape[1]}".encode(), digest_size=16).hexdigest()
        info[sid] = (rate, a.shape[1], len(a), sf.info(p).format, sf.info(p).subtype)
        for k in s:
            if k not in ("id", "file", "gain", "start", "retrigger", "choke"):
                bad(f"소리 {sid}: 알 수 없는 키 {k}")

    charts = {}
    for p in sorted((ROOT / "charts").glob("*.toml")):
        c = tomllib.loads(p.read_text(encoding="utf-8"))
        if c["id"] != p.stem:
            bad(f"차분 id ≠ 파일 이름: {c['id']} / {p.stem}")
        charts[c["id"]] = c

    anchor = tr["origin"]["anchor_chart"]
    if anchor not in charts:
        bad(f"원점 차분 없음: {anchor}")

    out = {}  # 차분 id → [(자기 ns, 종류, 레인, 소리, when)]
    order = sorted(charts, key=lambda k: "base" in charts[k])  # 배속 변형은 나중에
    for cid in order:
        c = charts[cid]
        if "base" in c:
            b = charts.get(c["base"])
            if b is None or "base" in b:
                bad(f"{cid}: base가 없거나 또 배속 변형")
                continue
            sc = rat(c["time_scale"])
            out_exact[cid] = [(t / sc, *rest[:3], rest[3] if len(rest) > 3 else None) for t, *rest in out_exact[c["base"]]]
            out[cid] = [(half_even(e[0]), *e[1:5]) for e in out_exact[cid]]
            continue
        if c["layout"] not in layouts:
            bad(f"{cid}: 없는 레이아웃 {c['layout']}")
            continue
        lanes = {l["id"] for l in layouts[c["layout"]]["lanes"]}
        tm = Timing(c["timing"])
        variants = {v["id"]: v["choices"] for v in c.get("variant", [])}
        ev = []

        def at_ns(pos):
            return tm.seconds(pos) * 10**9

        def check_when(w):
            if w is None:
                return
            vid, ch = w.split("=")
            if ch not in variants.get(vid, []):
                bad(f"{cid}: 없는 변형 선택 {w}")

        for e in c.get("event", []):
            if e["sound"] not in sounds:
                bad(f"{cid}: 없는 소리 {e['sound']}")
            check_when(e.get("when"))
            ev.append((at_ns(e["at"]), "auto", None, e["sound"], e.get("when")))
        for n in c.get("note", []):
            if n["lane"] not in lanes:
                bad(f"{cid}: 레이아웃에 없는 레인 {n['lane']}")
            check_when(n.get("when"))
            t0 = at_ns(n["at"])
            if n["kind"] == "long" and at_ns(n["end"]) <= t0:
                bad(f"{cid}: 롱노트 끝 ≤ 시작 at {n['at']}")
            ev.append((t0, n["kind"], n["lane"], None, n.get("when"), at_ns(n["end"]) if "end" in n else t0))
            for s in n.get("sounds", []):
                s = {"sound": s} if isinstance(s, str) else s
                if s["sound"] not in sounds:
                    bad(f"{cid}: 없는 소리 {s['sound']}")
                ts = at_ns(tm.rel(n["at"], s["at"])) if "at" in s else t0
                ev.append((ts, "note_sound", n["lane"], s["sound"], n.get("when")))
        for ls in c.get("lane_sound", []):
            ev.append((at_ns(ls["at"]), "lane_sound", ls["lane"], ls["sound"], ls.get("when")))
        # 변형 선택별 같은 레인 겹침
        combos = [{}]
        for vid, chs in variants.items():
            combos = [dict(cb, **{vid: ch}) for cb in combos for ch in chs]
        for cb in combos:
            per = collections.defaultdict(list)
            for e in ev:
                if e[1] in ("tap", "long", "mine") and (e[4] is None or cb.get(e[4].split("=")[0]) == e[4].split("=")[1]):
                    per[e[2]].append((e[0], e[5]))
            for lane, L in per.items():
                L.sort()
                for (a0, a1), (b0, _) in zip(L, L[1:]):
                    if b0 <= a1:
                        bad(f"{cid} {cb}: 레인 {lane} 노트 겹침")
        out_exact[cid] = ev
        out[cid] = [(half_even(e[0]), *e[1:5]) for e in ev]

    # 보고
    print("== 소리")
    for sid in sounds:
        r, ch, n, fmt, sub = info[sid]
        print(f"  {sid:12} {fmt}/{sub:10} {r:6} Hz {ch}ch {n:7} 샘플  바이트 {byte_h[sid][:8]}  PCM {pcm_h[sid][:8]}")
    dup = collections.defaultdict(list)
    for sid in sounds:
        dup[pcm_h[sid]].append(sid)
    for v in dup.values():
        if len(v) > 1:
            print(f"  같은 내용 다른 ID → 데이터 한 번: {v}")

    print("== 차분 (자기 시각 ns → Track 시각 ns)")
    layer = collections.Counter()
    negative = []
    for cid in charts:
        c = charts[cid]
        shift_ns = half_even(rat(c.get("shift", "0")) * 10**6)
        evs = out.get(cid, [])
        sound_ev = [e for e in evs if e[3] is not None and e[1] != "lane_sound"]
        notes = [e for e in evs if e[1] in ("tap", "long", "mine")]
        print(f"  {cid:12} 소리 발음 {len(sound_ev):3}  노트 {len(notes):3}  shift {shift_ns:+,} ns  자기 시각 범위 {min(e[0] for e in evs):,} ~ {max(e[0] for e in evs):,}")
        seen = set()
        for t, kind, lane, snd, when in sound_ev:
            lk = (t + shift_ns, pcm_h[snd], sounds[snd].get("start", "0"), sounds[snd].get("gain", "1"))  # G7
            key = (*lk, when)
            if key in seen:
                continue
            seen.add(key)
            layer[lk] += 1
            if t + shift_ns < 0:
                negative.append((cid, t + shift_ns, snd))
    shared = sum(1 for v in layer.values() if v > 1)
    print(f"== 음악 레이어: 이벤트 {len(layer)}개, 둘 이상 차분이 공유 {shared}개")
    for k, v in sorted(layer.items()):
        if v > 1:
            name = next(s for s in sounds if pcm_h[s] == k[1] and sounds[s].get("start", "0") == k[2])
            print(f"  Track {k[0]:>13,} ns  {name:10} 차분 {v}개")
    print("== 음수 Track 시각:", negative)

    # 정확성 확인: tick 1728 (8b-a) == 13000/3 ms (7k-c)
    a = [e[0] for e in out["8b-a"] if e[3] == "snare" and e[1] == "auto"]
    c = [e[0] for e in out["7k-c"] if e[3] == "snare" and e[1] == "auto"]
    print("== tick 1728 = 13000/3 ms ?", a, c, "같음" if a == c else "다름")
    print("== 문제", len(problems))
    for p in problems:
        print("  -", p)
    return 1 if problems else 0


out_exact = {}
if __name__ == "__main__":
    sys.exit(main())
