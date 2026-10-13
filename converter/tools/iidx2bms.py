r"""IIDX 곡 데이터(.1 채보 + 소리 묶음) → BMS 변환기.

python -I tools\iidx2bms.py 33131 01002          # 곡 ID 지정
python -I tools\iidx2bms.py --all                # 전곡
python -I tools\iidx2bms.py 33131 --no-audio     # 채보만 (빠른 검증)

입력: 추출본 D:\Arcade\LDJ-010-2026081900\extracted (패치 -p0 적용 후 상태)
      + 곡 DB music_data.bin / music_omni.bin (차분별 소리 묶음 접미사 +0x684)
출력: out\<곡ID> <제목>\  (BMS 파일, 소리, 미리듣기, report.json)

근거 (확인한 사실, IIDX_vs_BMS.md / IIDX_timing_policy.md / tools\iidx_survey.py):
  - .1 칸 0..4 = SP H,N,A,B,L / 6..10 = DP H,N,A,B,L
  - 이벤트 8B (u32 ms, u8 type, u8 param, u16 value), 시각 0x7FFFFFFF = 끝 표식
  - type 0/1 노트 (1P/2P, param 0..6 건반, 7 스크래치, 107 특수 스크래치), value = 롱노트 길이(ms)
  - type 2/3 레인 소리칸 교체, type 7 자동재생, 소리 번호는 1부터 (0 = 소리 없음)
  - type 4 템포 = value/param (param 0 → value 그대로, 추정), type 5 박자 value/param
  - type 8 판정 창 (s8 - 1, 1/60초 단위), type 12 소절선 (param 0 = 1P), type 16 노트 수
  - 소리 묶음 = 곡ID + 접미사 (곡 DB +0x684, 순서 SP B,N,H,A,L, DP B,N,H,A,L; '0' = 없음)
"""
import argparse, bisect, json, math, os, pathlib, shutil, struct, subprocess, sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(r"D:\Arcade\LDJ-010-2026081900")
EXTRACTED = ROOT / "extracted"
DB_FILES = [ROOT / "contents/data/info/1/music_data.bin", ROOT / "contents/data/info/1/music_omni.bin"]
SONGS_JSON = EXTRACTED / "system/database/songs.json"
OUT_DEFAULT = pathlib.Path(__file__).resolve().parent.parent / "out"

# .1 칸 → (스타일, 난이도 문자, 곡 DB 난이도 인덱스 0..9 = SP B,N,H,A,L, DP B,N,H,A,L)
SLOTS = {0: ("SP", "H", 2), 1: ("SP", "N", 1), 2: ("SP", "A", 3), 3: ("SP", "B", 0), 4: ("SP", "L", 4),
         6: ("DP", "H", 7), 7: ("DP", "N", 6), 8: ("DP", "A", 8), 9: ("DP", "B", 5), 10: ("DP", "L", 9)}
DIFF_NAME = {"B": "BEGINNER", "N": "NORMAL", "H": "HYPER", "A": "ANOTHER", "L": "LEGGENDARIA"}
DIFF_NUM = {"B": 1, "N": 2, "H": 3, "A": 4, "L": 5}

# 레인 → BMS 채널 2번째 글자 (1P: 1x, 2P: 2x). 건반 1..5 → 1..5, 6,7 → 8,9, 스크래치 → 6
LANE_CH = {0: "1", 1: "2", 2: "3", 3: "4", 4: "5", 5: "8", 6: "9", 7: "6"}
SIDE = {0: "1", 1: "2"}          # 노트 type 0 = 1P, 1 = 2P
NOTE_BASE = {"tap": "", "ln": "5", "inv": "3"}  # 채널 첫 글자: 1P/2P 노트 1/2, LN 5/6, 투명 3/4

END = 0x7FFFFFFF
B36 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
B62 = B36 + "abcdefghijklmnopqrstuvwxyz"
BEAT_DENS = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 192]  # 박당 분할 후보 (192 = 768분음표)
LINE_DEN_CAP = 9600


# ------------------------------------------------------------------ 곡 DB
def load_music_db():
    db = {}
    for f in DB_FILES:
        d = f.read_bytes()
        _, count, isz = struct.unpack_from("<III", d, 4)
        base = 0x10 + 4 * isz
        for k in range(count):
            e = d[base + k * 2040: base + (k + 1) * 2040]
            sid = struct.unpack_from("<I", e, 0x67C)[0]
            db[sid] = {"suffix": e[0x684:0x68E].decode("latin1")}
    meta = {s["id"]: s for s in json.loads(SONGS_JSON.read_text(encoding="utf-8"))}
    for sid, s in meta.items():
        s.update(db.get(sid, {"suffix": "0" * 10}))
    return meta


# ------------------------------------------------------------------ 채보
def read_chart(path):
    d = path.read_bytes()
    slots = {}
    for i in range(12):
        off, ln = struct.unpack_from("<II", d, i * 8)
        if ln and off + ln <= len(d) and i in SLOTS:
            ev = [struct.unpack_from("<IBBH", d, off + k * 8) for k in range(ln // 8)]
            ev = [e for e in ev if e[0] != END]
            # 시각순 안정 정렬 (원본에 역순 이벤트가 드물게 있음)
            slots[i] = sorted(ev, key=lambda e: e[0])
    return slots


def bpm_of(param, value):
    return Fraction(value, param if param else 1)


# ------------------------------------------------------------------ 시간 모델
class Timing:
    """원래 ms 시각 → 악보 위치(박, Fraction) 복원. 소절선·템포 지점을 격자에 맞춰 고정.

    raw 박 = 구간 기준 박 + (t - 구간 기준 ms) × BPM / 60000  (원래 시각으로 계산)
    격자 박 = 소절 시작 + 가장 단순한 분수 (원래 ms와의 오차 ≤ tol)
    모델 시각 = BMS 플레이어가 격자 박에서 계산할 시각 (오차 측정용)
    """

    def __init__(self, events, tol_ms, report):
        self.tol = tol_ms
        self.rep = report
        # DP 채보는 같은 템포·박자 이벤트가 1P/2P용으로 중복될 수 있음 → 시각별로 하나만 (마지막 값)
        tempos = sorted({t: (t, bpm_of(pa, va)) for (t, ty, pa, va) in events if ty == 4}.values())
        meters = sorted({t: (t, va, pa) for (t, ty, pa, va) in events if ty == 5 and va > 0}.values())
        if any(ty == 5 and va == 0 for (t, ty, pa, va) in events):
            report["counts"]["meter_zero_ignored"] += 1
        bars = sorted({t for (t, ty, pa, va) in events if ty == 12 and pa == 0})
        if not tempos:
            raise ValueError("no tempo event")
        if tempos[0][0] != 0:
            report["warnings"].append(f"first tempo at {tempos[0][0]} ms, used as initial")
        tempos = self.effective_tempos(tempos, meters, bars, report)
        self.initial_bpm = tempos[0][1]
        # 모델 구간: (원래 ms 기준, 격자 박 기준, 모델 ms 기준, bpm). 검색용 정렬 키는 따로 (이진 탐색)
        self.segs = [(0, Fraction(0), Fraction(0), self.initial_bpm)]
        self.seg_t = [0]
        self.seg_b = [Fraction(0)]
        self.measure_starts = [Fraction(0)]
        self.meter_at_measure = {}
        self.tempo_points = []  # (격자 박, bpm) 0 제외
        points = [(t, 0, b) for (t, b) in tempos[1:]] + [(t, 1, None) for t in bars if t > 0]
        points += [(t, 2, (va, pa)) for (t, va, pa) in meters]
        points.sort(key=lambda p: (p[0], p[1]))
        cur_meter = (4, 4)
        mstart = Fraction(0)
        for (t, kind, data) in points:
            if kind == 2:
                cur_meter = data
                self.meter_at_measure[len(self.measure_starts) - 1] = data
                continue
            raw = self.raw_beat(t)
            if kind == 1:  # 소절선
                raw_len = raw - mstart
                num, den = cur_meter
                want = Fraction(4 * num, den if den else 4)
                seg_bpm = self.seg_for_raw_t(t)[3]
                if abs(float(raw_len - want)) * 60000 / float(seg_bpm) <= 2.0:
                    length = want
                else:
                    length = self.snap_beats(raw_len, t, mstart, force=True)
                    report["counts"]["irregular_measure"] += 1
                if length <= 0:
                    report["warnings"].append(f"non-positive measure length at {t} ms")
                    continue
                mstart = mstart + length
                self.measure_starts.append(mstart)
            else:  # 템포 변경
                beat = self.snap(raw, t, mstart)
                model_t = self.model_time(beat)
                self.segs.append((t, beat, model_t, data))
                self.seg_t.append(t)
                self.seg_b.append(beat)
                self.tempo_points.append((beat, data))

    @staticmethod
    def effective_tempos(tempos, meters, bars, report):
        """선언 BPM 대신 소절선 간격이 암시하는 실효 BPM (옛날 곡은 표시 BPM과 실제 진행이 미세하게 다름).

        템포 구간마다 그 안의 소절선 간격(박 수는 그때 박자)으로 추정.
        채택 조건: 소절 4개 이상, 선언값과 0.3% 이내, 선언 BPM으로 예측한 소절선 오차가 1 ms를 넘고
        실효 BPM이 그 오차를 절반 이하로 줄일 때만 (변속곡의 짧은 구간 추정 잡음을 막음). 소수 4자리.
        """
        out = []
        for i, (t0, bpm) in enumerate(tempos):
            t1 = tempos[i + 1][0] if i + 1 < len(tempos) else float("inf")
            bs = [b for b in bars if t0 <= b < t1]
            if len(bs) < 5:
                out.append((t0, bpm))
                continue
            cum = [Fraction(0)]  # 첫 소절선부터 누적 박
            for a in bs[:-1]:
                num, den = (4, 4)
                for (tm_, va, pa) in meters:
                    if tm_ <= a:
                        num, den = va, pa if pa else 4
                cum.append(cum[-1] + Fraction(4 * num, den))
            span = bs[-1] - bs[0]
            eff = cum[-1] * 60000 / span

            def resid(b):
                return max(abs(bs[0] + float(c) * 60000 / float(b) - x) for c, x in zip(cum, bs))

            adj = Fraction(round(float(eff) * 10000), 10000)
            r_decl, r_eff = resid(bpm), resid(adj)
            if abs(float(eff - bpm)) / float(bpm) < 0.003 and r_decl > 1.0 and r_eff <= r_decl / 2:
                report.setdefault("bpm_adjusted", []).append([fmt_decimal(bpm), fmt_decimal(adj)])
                out.append((t0, adj))
            else:
                out.append((t0, bpm))
        return out

    # 원래 시각 기준의 raw 박
    def seg_for_raw_t(self, t):
        return self.segs[bisect.bisect_right(self.seg_t, t) - 1]

    def raw_beat(self, t):
        t0, b0, _, bpm = self.seg_for_raw_t(t)
        return b0 + Fraction(t - t0) * bpm / 60000

    def model_time(self, beat):
        _, b0, mt0, bpm = self.segs[bisect.bisect_right(self.seg_b, beat) - 1]
        return mt0 + (beat - b0) * 60000 / bpm

    def measure_of(self, beat):
        lo, hi = 0, len(self.measure_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if self.measure_starts[mid] <= beat:
                lo = mid
            else:
                hi = mid - 1
        return lo

    def snap(self, raw, t, mstart=None):
        """raw 박을 소절 시작 기준의 가장 단순한 분수로. 오차 > tol 이면 ms 해상도로."""
        if mstart is None:
            mstart = self.measure_starts[self.measure_of(raw)]
        best = None
        for den in BEAT_DENS:
            q = round((raw - mstart) * den)
            cand = mstart + Fraction(q, den)
            err = abs(float(self.model_time(cand)) - t)
            if err <= self.tol:
                return cand
            if best is None or err < best[0]:
                best = (err, cand)
        # 격자 밖: 1/1920 박 해상도
        q = round((raw - mstart) * 1920)
        cand = mstart + Fraction(q, 1920)
        self.rep["counts"]["off_grid"] += 1
        return cand

    def snap_beats(self, raw_len, t, mstart, force=False):
        for den in BEAT_DENS:
            cand = Fraction(round(raw_len * den), den)
            if cand > 0 and abs(float(self.model_time(mstart + cand)) - t) <= self.tol:
                return cand
        return Fraction(round(raw_len * 1920), 1920)


# ------------------------------------------------------------------ ID 할당
def id_str(n, base):
    digits = B62 if base == 62 else B36
    return digits[n // base] + digits[n % base]


def fmt_decimal(fr):
    """Fraction → 10진 문자열 (유한 소수면 정확히, 아니면 12자리)."""
    if fr.denominator == 1:
        return str(fr.numerator)
    d = fr.denominator
    while d % 2 == 0:
        d //= 2
    while d % 5 == 0:
        d //= 5
    if d == 1:
        s = f"{float(fr):.10f}".rstrip("0").rstrip(".")
        return s
    return f"{float(fr):.12f}".rstrip("0").rstrip(".")


# ------------------------------------------------------------------ 차분 변환
def convert_chart(events, style, diff, song, bank_files, sound_name, tol_ms, invisible_mode="all"):
    rep = {"style": style, "diff": diff, "warnings": [], "counts": Counter(), "unknown_types": Counter(),
           "time_error_ms": {}}
    tm = Timing(events, tol_ms, rep)

    regs = {0: {}, 1: {}}            # 측(type 0/1 계열) → 레인 → 소리 번호
    pending = {}                     # (측, 레인) → 노트 없이 남은 교체 (시각, 소리)
    objs = []                        # (격자 박, 채널, 종류, 소리 번호 또는 bpm)
    errors = []
    judge = []
    note_count_file = {}

    def place(t, channel, kind, val):
        raw = tm.raw_beat(t)
        beat = tm.snap(raw, t)
        errors.append(abs(float(tm.model_time(beat)) - t))
        objs.append((beat, channel, kind, val))
        return beat

    def flush_pending(side, lane, now=None):
        p = pending.pop((side, lane), None)
        if p is not None:
            t, snd = p
            if now is not None and now == t:  # 같은 시각에 다시 교체됨 → 들릴 수 없는 소리
                rep["counts"]["swap_overwritten_same_time"] += 1
                return
            ch = ("3" if side == 0 else "4") + LANE_CH[lane]
            place(t, ch, "inv", snd)
            rep["counts"]["invisible"] += 1

    taps_seen = set()
    lns = defaultdict(list)  # (측, 레인) → [시작, 끝, 소리, 특수 여부] (이어진 특수 스크래치 합치기용)

    for (t, ty, pa, va) in events:
        if ty in (0, 1):
            side = ty
            lane = pa
            special = False
            if pa >= 100:                   # 107 = 특수 스크래치 (BSS/MSS 추정)
                special, lane = True, pa % 100
                rep["counts"]["special_scratch"] += 1
            if lane not in LANE_CH:
                rep["counts"]["bad_lane"] += 1
                continue
            if invisible_mode == "all":  # 모든 교체를 투명 노트로 (빈 레인 소리를 IIDX처럼)
                flush_pending(side, lane)
            else:
                pending.pop((side, lane), None)  # 교체가 이 노트에 쓰임
            snd = regs[side].get(lane, 0)
            ch_tail = LANE_CH[lane]
            if va:  # 롱노트: 끝난 순간 바로 이어지는 특수 스크래치(MSS 추정)는 하나로 합침
                prev = lns[(side, lane)][-1] if lns[(side, lane)] else None
                if special and prev and prev[3] and abs(prev[1] - t) <= 1:
                    prev[1] = t + va
                    rep["counts"]["special_scratch_merged"] += 1
                else:
                    lns[(side, lane)].append([t, t + va, snd, special])
                    rep["counts"]["ln"] += 1
            else:
                if (side, lane, t) in taps_seen:  # 원본의 중복 노트
                    rep["counts"]["duplicate_note_dropped"] += 1
                    continue
                taps_seen.add((side, lane, t))
                place(t, SIDE[side] + ch_tail, "tap", snd)
                rep["counts"]["tap"] += 1
            if snd == 0:
                rep["counts"]["note_without_sound"] += 1
        elif ty in (2, 3):
            side = ty - 2
            if pa not in LANE_CH:
                rep["counts"][f"swap_lane_{pa}_ignored"] += 1
                continue
            flush_pending(side, pa, now=t)
            regs[side][pa] = va
            pending[(side, pa)] = (t, va)
        elif ty == 7:
            if va == 0:
                continue
            if pa:
                rep["counts"]["autoplay_pan_lost"] += 1
            place(t, "01", "bgm", va)
        elif ty == 8:
            judge.append((pa, (va & 0xFF) - 256 if va & 0x80 else va))
        elif ty == 16:
            note_count_file[pa] = va
        elif ty in (4, 5, 6, 12):
            pass
        else:
            rep["unknown_types"][ty] += 1
    for key in list(pending):
        flush_pending(*key)
    for (side, lane), items in lns.items():
        ch = ("5" if side == 0 else "6") + LANE_CH[lane]
        for (t0, t1, snd, _) in items:
            place(t0, ch, "ln", snd)
            place(t1, ch, "ln", snd)

    # 템포 변경 객체
    for beat, bpm in tm.tempo_points:
        objs.append((beat, "08", "bpm", bpm))

    # 소리 ID 할당 (차분에서 쓰는 소리만, 처음 쓰는 순서)
    used = []
    seen = set()
    for (beat, ch, kind, val) in sorted(objs, key=lambda o: o[0]):
        if kind != "bpm" and val and val not in seen:
            if val > len(bank_files):
                rep["counts"]["missing_sound"] += 1
                continue
            seen.add(val)
            used.append(val)
    base = 36 if len(used) + 1 <= 1295 else 62
    if len(used) + 1 > 3843:
        rep["warnings"].append(f"{len(used)} sounds exceed base62 capacity")
    wav_id = {v: id_str(i + 1, base) for i, v in enumerate(used)}
    silent_id = id_str(len(used) + 1, base)  # #WAV 정의가 없는 ID = 소리 없는 노트
    bpms = sorted({o[3] for o in objs if o[2] == "bpm"})
    bpm_id = {b: id_str(i + 1, 36) for i, b in enumerate(bpms)}

    def mlen_of(m):
        if m + 1 < len(tm.measure_starts):
            return tm.measure_starts[m + 1] - tm.measure_starts[m]
        num, den = (4, 4)
        for k in sorted(tm.meter_at_measure):  # 마지막 소절: 그때 유효한 박자
            if k <= m:
                num, den = tm.meter_at_measure[k]
        return Fraction(4 * num, den if den else 4)

    # 소절별 줄 구성
    lines = defaultdict(list)  # (소절, 채널) → [(분수, ID)]
    for (beat, ch, kind, val) in objs:
        m = tm.measure_of(beat)
        if m >= 1000:
            rep["counts"]["dropped_measure_over_999"] += 1
            continue
        frac = (beat - tm.measure_starts[m]) / mlen_of(m)
        if kind == "bpm":
            s = bpm_id[val]
        elif val in wav_id:
            s = wav_id[val]
        elif kind in ("bgm", "inv"):
            continue
        else:
            s = silent_id
        lines[(m, ch)].append((frac, s))

    n_measures = min(1000, max([m for (m, _) in lines] + [len(tm.measure_starts) - 2, 0]) + 1)

    body = []
    for m in range(n_measures):
        mlen = mlen_of(m)
        if mlen != 4:
            body.append(f"#{m:03d}02:{fmt_decimal(mlen / 4)}")
        for ch in sorted({c for (mm, c) in lines if mm == m}):
            items = lines[(m, ch)]
            # 같은 칸 충돌을 피하려 여러 줄로 (BGM은 원래 여러 줄 허용)
            rows = []
            for frac, s in sorted(items, key=lambda x: x[0]):
                placed = False
                for row in rows:
                    if all(f != frac for f, _ in row):
                        row.append((frac, s))
                        placed = True
                        break
                if not placed:
                    if ch != "01" and rows:
                        rep["counts"]["slot_collision"] += 1
                    rows.append([(frac, s)])
            for row in rows:
                den = 1
                for f, _ in row:
                    den = den * f.denominator // math.gcd(den, f.denominator)
                if den > LINE_DEN_CAP:
                    den = LINE_DEN_CAP
                    rep["counts"]["line_den_capped"] += 1
                cells = ["00"] * den
                for f, s in row:
                    idx = min(den - 1, round(f * den))
                    cells[idx] = s
                body.append(f"#{m:03d}{ch}:{''.join(cells)}")

    # 오차 통계
    if errors:
        es = sorted(errors)
        rep["time_error_ms"] = {"max": round(es[-1], 3), "p99": round(es[int(len(es) * 0.99)], 3),
                                "mean": round(sum(es) / len(es), 4), "n": len(es),
                                "over_tol": sum(1 for e in es if e > tol_ms)}
    notes = rep["counts"]["tap"] + rep["counts"]["ln"]
    file_notes = sum(note_count_file.values())
    if file_notes and file_notes != notes:
        rep["source_note_count_mismatch"] = [notes, file_notes]  # 원본 type16 값이 실제 노트 이벤트 수와 다름
    rep["notes"] = notes
    rep["judge_raw"] = judge[:12]
    rep["sounds_used"] = len(used)
    rep["base"] = base

    # 헤더
    title = song["title"] or song["title_ascii"]
    lvl_idx = SLOTS_DIFF_INDEX[(style, diff)]
    level = [song.get(k) for k in ("SPB", "SPN", "SPH", "SPA", "SPL", "DPB", "DPN", "DPH", "DPA", "DPL")][lvl_idx]
    hdr = [
        "*---------------------- converted by iidx2bms (tools/iidx2bms.py)",
        f"* source: IIDX {song['id']:05d} slot {style}{diff}, bank {sound_name}",
        f"* judge windows (type 8, s8 raw): {judge[:6]}",
        f"* lost: autoplay pan x{rep['counts']['autoplay_pan_lost']}, special scratch x{rep['counts']['special_scratch']} (as LN/tap)",
        "",
        "*---------------------- HEADER FIELD",
        f"#PLAYER {1 if style == 'SP' else 3}",
        f"#GENRE {song['genre']}",
        f"#TITLE {title}",
        f"#SUBTITLE [{style} {DIFF_NAME[diff]}]",
        f"#ARTIST {song['artist']}",
        f"#BPM {fmt_decimal(tm.initial_bpm)}",
        f"#PLAYLEVEL {level}",
        f"#DIFFICULTY {DIFF_NUM[diff]}",
        "#RANK 2",
        f"#TOTAL {fmt_decimal(Fraction(round(7.605 * notes / (0.01 * notes + 6.5) * 100), 100)) if notes else 100}",
        "#LNTYPE 1",
        "#LNMODE 2",
    ]
    if base == 62:
        hdr.append("#BASE 62")
    return rep, hdr, wav_id, bpm_id, body


SLOTS_DIFF_INDEX = {(st, df): idx for (st, df, idx) in SLOTS.values()}


# ------------------------------------------------------------------ 소리
def find_bank(audio_dir, sid, suffix_char):
    name = f"{sid:05d}" if suffix_char == "0" else f"{sid:05d}{suffix_char}"
    p = audio_dir / name
    if p.is_dir():
        return name, p, False
    fallback = audio_dir / f"{sid:05d}"
    if fallback.is_dir():
        return fallback.name, fallback, True
    dirs = [d for d in audio_dir.iterdir() if d.is_dir()] if audio_dir.exists() else []
    return (dirs[0].name, dirs[0], True) if dirs else (None, None, True)


def bank_listing(bank_dir):
    files = {}
    for f in bank_dir.iterdir():
        if f.stem.isdigit():
            files[int(f.stem)] = f
    return [files.get(i) for i in range(1, max(files) + 1)] if files else []


def copy_file(src, dst):
    """WMA 외의 파일(WAV, 그림, 영상)은 원본 그대로. 임시 이름으로 쓴 뒤 이름 변경 (중단 시 반쪽 파일 방지)."""
    if dst.exists():
        return "exists"
    tmp = dst.with_name(dst.name + ".part")
    shutil.copyfile(src, tmp)
    os.replace(tmp, dst)
    return "copied"


WMA_BATCH = 40  # ffmpeg 한 번에 처리할 WMA 수 (Windows 명령줄 길이 32K 안쪽)


def convert_wma_batch(pairs):
    """WMA 여러 개 → WAV(16비트 PCM) 여러 개를 ffmpeg 한 번으로 (파일마다 프로세스를 띄우는 것보다 약 8배 빠름).
    FLAC이 아닌 이유: Qwilight가 FLAC 일부(특히 긴 반주)를 재생하지 못함 (28098 실험: FLAC·FLAC lv0 실패, WAV 정상)."""
    pairs = [(s, d) for s, d in pairs if not d.exists()]
    if not pairs:
        return ["exists"]
    cmd = ["ffmpeg", "-nostdin", "-loglevel", "error", "-y"]
    for s, _ in pairs:
        cmd += ["-i", str(s)]
    for i, (_, d) in enumerate(pairs):
        cmd += ["-map", f"{i}:a", "-c:a", "pcm_s16le", "-f", "wav", str(d.with_name(d.name + ".part"))]
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode:
        for _, d in pairs:
            d.with_name(d.name + ".part").unlink(missing_ok=True)
        if len(pairs) > 1:  # 묶음 실패(예: 동시 실행이 많아 메모리 부족) → 파일 하나씩 다시
            out = []
            for p in pairs:
                out += convert_wma_batch([p])
            return out
        return ["ffmpeg_error: " + r.stderr.decode("utf-8", "replace")[:200]]
    for _, d in pairs:
        os.replace(d.with_name(d.name + ".part"), d)
    return ["wma_to_wav"] * len(pairs)


MOVIE_DIR = ROOT / "contents/data/movie"


def find_movie(song):
    name = (song.get("bga") or "").strip()
    for ext in (".mp4", ".wmv"):
        p = MOVIE_DIR / f"{name}{ext}"
        if name and p.exists():
            return p
    return None


def safe_name(s):
    s = "".join("_" if c in '\\/:*?"<>|' else c for c in s)
    return s.rstrip(" .") or "_"  # Windows는 끝 공백·마침표를 허용하지 않음


# ------------------------------------------------------------------ 곡 변환
def convert_song(sid, meta, out_root, tol_ms, do_audio, jobs, include_hidden=False, invisible_mode="all", bga=True,
                 hardlink=False):
    song = meta[sid]
    src = EXTRACTED / song["dir"]
    chart_path = src / "chart" / f"{sid:05d}.1"
    title = song["title"] or song["title_ascii"]
    # 출력은 SP 모음 / DP 모음 두 폴더 (플레이어가 한 곡 안의 차분을 레벨순으로 섞어 보이지 않게)
    name = safe_name(f"{sid:05d} {title}")
    dsts = {st: out_root.with_name(f"{out_root.name} {st}") / name for st in ("SP", "DP")}  # 예: "IIDX converted SP"
    report = {"id": sid, "title": title, "charts": {}, "audio": Counter(), "warnings": []}
    slots = read_chart(chart_path)
    audio_dir = src / "audio"
    needed = {"SP": {}, "DP": {}}  # 스타일 → 출력 파일명 → 원본 경로

    for slot, events in sorted(slots.items()):
        style, diff, db_idx = SLOTS[slot]
        if not any(e[1] in (0, 1) for e in events):
            continue
        level = song.get(f"{style}{diff}") or 0
        if level == 0 and not include_hidden:  # 곡 DB 레벨 0 = 게임에서 고를 수 없는 숨은 차분
            report["hidden_charts_skipped"] = report.get("hidden_charts_skipped", []) + [f"{style}{diff}"]
            continue
        sfx = song["suffix"][db_idx] if len(song["suffix"]) > db_idx else "0"
        bank_name, bank_dir, fell_back = find_bank(audio_dir, sid, sfx)
        files = bank_listing(bank_dir) if bank_dir else []
        # 증거 기반 보정: 채보가 쓰는 최대 소리 번호를 담지 못하는 묶음이면, 같은 곡의 다른 묶음 중
        # 충분히 큰 가장 작은 것을 고름 (옴니믹스 전용 곡의 곡 DB 접미사가 불완전한 경우)
        max_snd = max((va for (t, ty, pa, va) in events if ty in (2, 3, 7)), default=0)
        chosen_by_count = None
        if max_snd > len(files) and audio_dir.exists():
            cands = sorted((len(bank_listing(b)), b.name, b) for b in audio_dir.iterdir() if b.is_dir())
            fit = [c for c in cands if c[0] >= max_snd]
            if fit:
                chosen_by_count = (bank_name, fit[0][1])
                bank_name, bank_dir = fit[0][1], fit[0][2]
                files = bank_listing(bank_dir)
        ext = lambda p: ".wav"  # 원본 WAV(ADPCM)는 그대로 복사, WMA는 PCM WAV로 디코딩
        rep, hdr, wav_id, bpm_id, body = convert_chart(events, style, diff, song, files, bank_name, tol_ms, invisible_mode)
        if fell_back:
            rep["warnings"].append(f"bank for suffix '{sfx}' missing, used {bank_name}")
        if chosen_by_count:
            rep["warnings"].append(f"bank by sound count: suffix gave {chosen_by_count[0]}, used {chosen_by_count[1]}")
        if rep["counts"].get("missing_sound"):
            rep["warnings"].append(f"chart references sound {max_snd} but bank has {len(files)} (source data)")
        wav_lines = []
        for v, i in sorted(wav_id.items(), key=lambda x: x[1]):
            f = files[v - 1]
            if f is None:
                continue
            out_name = f"{bank_name}_{v:04d}{ext(f)}"
            needed[style][out_name] = f
            wav_lines.append(f"#WAV{i} {out_name}")
        bpm_lines = [f"#BPM{i} {fmt_decimal(b)}" for b, i in sorted(bpm_id.items(), key=lambda x: x[1])]
        extra = []
        prev = src / "preview" / "preview_01.wav"
        if prev.exists():
            extra.append("#PREVIEW preview.wav")
            needed[style]["preview.wav"] = prev
        tc = sorted((src / "title_card").glob("*.png")) if (src / "title_card").exists() else []
        if tc:
            extra.append("#STAGEFILE stagefile.png")
            needed[style]["stagefile.png"] = tc[0]
        movie = find_movie(song) if bga else None
        if movie:  # 곡 DB의 BGA 이름 → 원본 data/movie. 0소절 시작(0 ms)에서 재생 (IIDX 시작 시점은 미확인)
            extra.append(f"#BMP01 bga{movie.suffix.lower()}")
            needed[style][f"bga{movie.suffix.lower()}"] = movie
            body = ["#00004:01"] + body
            rep["bga"] = movie.name
        text = "\n".join(hdr + extra + wav_lines + bpm_lines + ["", "*---------------------- MAIN DATA FIELD"] + body) + "\n"
        # 인코딩: Shift-JIS(cp932)로 표현되면 cp932 (BMS 전통, 판별 오류 없음),
        # 아니면 BOM 붙은 UTF-8 (플레이어 자동 판별이 UTF-8을 Shift-JIS로 오인하는 문제 방지)
        crlf = text.replace("\n", "\r\n")
        try:
            data, rep["encoding"] = crlf.encode("cp932"), "cp932"
        except UnicodeEncodeError:
            data, rep["encoding"] = crlf.encode("utf-8-sig"), "utf-8-sig"
        dsts[style].mkdir(parents=True, exist_ok=True)
        (dsts[style] / f"{sid:05d}_{style}{diff}.bme").write_bytes(data)
        rep["counts"] = dict(rep["counts"])
        rep["unknown_types"] = dict(rep["unknown_types"])
        report["charts"][f"{style}{diff}"] = rep

    if do_audio:
        # 파일 하나는 한 번만 변환하고, 다른 스타일 폴더에는 복사 (--hardlink 이면 하드 링크, 실패 시 복사)
        first = {}
        for style in ("SP", "DP"):
            for out_name, srcf in needed[style].items():
                first.setdefault(out_name, (style, srcf))
        wma = [(srcf, dsts[st] / name) for name, (st, srcf) in first.items() if srcf.suffix.lower() == ".wma"]
        rest = [(srcf, dsts[st] / name) for name, (st, srcf) in first.items() if srcf.suffix.lower() != ".wma"]
        batches = [wma[i:i + WMA_BATCH] for i in range(0, len(wma), WMA_BATCH)]
        results = []
        with ThreadPoolExecutor(max_workers=jobs) as ex:
            for r in ex.map(convert_wma_batch, batches):
                results += r
            results += list(ex.map(lambda p: copy_file(*p), rest))
        for r in results:
            report["audio"][r if not r.startswith("ffmpeg_error") else "ffmpeg_error"] += 1
            if r.startswith("ffmpeg_error"):
                report["warnings"].append(r)
        for style in ("SP", "DP"):
            for out_name in needed[style]:
                owner = first[out_name][0]
                target = dsts[style] / out_name
                if owner == style or target.exists() or not (dsts[owner] / out_name).exists():
                    continue
                if hardlink:
                    try:
                        os.link(dsts[owner] / out_name, target)
                        report["audio"]["hardlinked"] += 1
                        continue
                    except OSError:
                        pass
                copy_file(dsts[owner] / out_name, target)
                report["audio"]["copied_to_other_style"] += 1
    report["audio"] = dict(report["audio"])
    for style, d in dsts.items():
        if d.exists():
            sub = dict(report, charts={k: v for k, v in report["charts"].items() if k.startswith(style)})
            (d / "report.json").write_text(json.dumps(sub, ensure_ascii=False, indent=1), encoding="utf-8")
    return report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ids", nargs="*", type=int)
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--out", type=pathlib.Path, default=OUT_DEFAULT)
    ap.add_argument("--tol", type=float, default=1.5, help="격자 맞춤 허용 오차 (ms)")
    ap.add_argument("--no-audio", action="store_true")
    ap.add_argument("--invisible", choices=["all", "unused"], default="all",
                    help="투명 노트: all = 레인 소리 교체마다, unused = 노트에 안 쓰인 교체만")
    ap.add_argument("--hardlink", action="store_true", help="SP/DP 폴더 사이 공유 파일을 복사 대신 하드 링크")
    ap.add_argument("--no-bga", action="store_true", help="BGA 영상 복사·참조 안 함")
    ap.add_argument("--include-hidden", action="store_true", help="곡 DB 레벨 0인 숨은 차분도 변환")
    ap.add_argument("--jobs", type=int, default=4, help="곡 하나 안에서 동시에 돌리는 ffmpeg 수")
    ap.add_argument("--procs", type=int, default=max(1, (os.cpu_count() or 2) - 2), help="동시에 변환하는 곡 수")
    a = ap.parse_args()
    meta = load_music_db()
    ids = [i for i in (sorted(meta) if a.all else a.ids) if i in meta]
    for i in (a.ids or []):
        if i not in meta:
            print(f"{i:05d}: not in DB")
    summary = []
    work = [(sid, a.out, a.tol, not a.no_audio, a.jobs, a.include_hidden, a.invisible, not a.no_bga, a.hardlink)
            for sid in ids]
    if a.procs > 1 and len(ids) > 1:
        from concurrent.futures import ProcessPoolExecutor
        with ProcessPoolExecutor(max_workers=a.procs) as ex:
            results = ex.map(_worker, work, chunksize=4)
            for line, row in results:
                print(line, flush=True)
                summary.append(row)
    else:
        for w in work:
            line, row = _worker(w)
            print(line, flush=True)
            summary.append(row)
    if len(ids) > 1:
        sp_dir = a.out.with_name(f"{a.out.name} SP")
        sp_dir.mkdir(parents=True, exist_ok=True)
        (sp_dir / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")


_META = None


def _worker(args):
    """곡 하나 변환 (프로세스 풀에서 실행). 실패해도 전체를 멈추지 않음."""
    global _META
    sid, out, tol, do_audio, jobs, include_hidden, invisible_mode, bga, hardlink = args
    if _META is None:
        _META = load_music_db()
    try:
        r = convert_song(sid, _META, out, tol, do_audio, jobs, include_hidden, invisible_mode, bga, hardlink)
    except Exception as e:  # noqa: BLE001
        return f"{sid:05d}: FAILED {type(e).__name__}: {e}", {"id": sid, "error": f"{type(e).__name__}: {e}"}
    worst = max((c.get("time_error_ms", {}).get("max", 0) for c in r["charts"].values()), default=0)
    warns = sum(len(c["warnings"]) for c in r["charts"].values()) + len(r["warnings"])
    line = f"{sid:05d} {r['title'][:30]:30s} charts={len(r['charts'])} max_err={worst}ms warnings={warns} audio={r['audio']}"
    return line, {"id": sid, "charts": len(r["charts"]), "max_err_ms": worst, "warnings": warns}


if __name__ == "__main__":
    main()
