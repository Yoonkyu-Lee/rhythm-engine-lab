r"""IIDX 데이터 전수 조사: 변환기 설계에 필요한 사실 확인.

python -I tools\iidx_survey.py

확인 항목
  1. music_data.bin 항목 +0x684 의 차분별 소리 묶음 접미사 (게임 코드에서 확인한 필드)
  2. .1 칸 순서 ↔ 곡 DB 난이도 순서 대응 (type16 노트 수와 레벨로)
  3. 소리 번호 범위 ↔ 소리 묶음 파일 수 (0부터/1부터)
  4. 이벤트 종류·파라미터·값 범위, 롱노트(value>0), 레인 범위
"""
import struct, pathlib, collections, json, sys

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(r"D:\Arcade\LDJ-010-2026081900")
SOUND = ROOT / "contents" / "data" / "sound"
EXTRACTED = ROOT / "extracted" / "songs"
DB_FILES = [ROOT / "contents/data/info/1/music_data.bin", ROOT / "contents/data/info/1/music_omni.bin"]


def load_music(path):
    d = path.read_bytes()
    ver, count, isz = struct.unpack_from("<III", d, 4)
    base = 0x10 + 4 * isz
    out = {}
    for k in range(count):
        e = d[base + k * 2040: base + (k + 1) * 2040]
        sid = struct.unpack_from("<I", e, 0x67C)[0]
        out[sid] = {
            "title": e[0:0x100].decode("utf-16le", "replace").split("\0")[0],
            "levels": list(e[0x3EC:0x3F6]),
            "suffix": e[0x684:0x68E].decode("latin1"),
            "bga": e[0x690:0x6A0].split(b"\0")[0].decode("latin1"),
        }
    return out


def read_chart(p):
    d = p.read_bytes()
    slots = {}
    for i in range(12):
        off, ln = struct.unpack_from("<II", d, i * 8)
        if ln and off + ln <= len(d):
            slots[i] = [struct.unpack_from("<IBBH", d, off + k * 8) for k in range(ln // 8)]
    return slots


def main():
    db = {}
    for f in DB_FILES:
        db.update(load_music(f))  # omni 가 기본판을 포함 (나중 값 우선)
    print("songs in DB:", len(db))
    suf = collections.Counter(s["suffix"] for s in db.values())
    print("suffix arrays (top):", suf.most_common(12))

    # 추출본 차트·오디오 폴더 찾기
    chart_files = {p.stem: p for p in EXTRACTED.rglob("*.1") if p.is_file() and p.parent.name == "chart"}
    print("charts found:", len(chart_files))

    types = collections.Counter()
    tparam = collections.defaultdict(collections.Counter)
    tempo_vals = collections.Counter()
    meter_vals = collections.Counter()
    ln_vals = collections.Counter()
    unsorted = 0
    slot_vs_level = collections.Counter()      # (slot, 레벨 순위 맞는 난이도) 대응 검사
    snd_rel = collections.Counter()            # 소리 번호 최댓값 - 파일 수
    snd_zero = 0
    snd_examples = []
    end_after_last = collections.Counter()

    for sid_s, p in sorted(chart_files.items()):
        sid = int(sid_s)
        slots = read_chart(p)
        song = db.get(sid)
        audio_dir = p.parent.parent / "audio"
        banks = {b.name: len(list(b.glob("*.*"))) for b in audio_dir.iterdir() if b.is_dir()} if audio_dir.exists() else {}
        for slot, ev in slots.items():
            last_t = -1
            notes = 0
            maxsnd = 0
            for (t, ty, pa, va) in ev:
                if t < last_t and t != 0x7FFFFFFF:
                    unsorted += 1
                last_t = t if t != 0x7FFFFFFF else last_t
                types[ty] += 1
                tparam[ty][pa] += 1
                if ty == 4:
                    tempo_vals["zero" if va == 0 else ("param0" if pa == 0 else "ok")] += 1
                if ty == 5:
                    meter_vals[(va, pa)] += 1
                if ty in (0, 1) and t != 0x7FFFFFFF:
                    notes += 1
                    if va:
                        ln_vals["ln"] += 1
                    else:
                        ln_vals["tap"] += 1
                if ty in (2, 3, 7):
                    maxsnd = max(maxsnd, va)
                    if va == 0:
                        snd_zero += 1
            # type16 노트 수
            n16 = sum(va for (t, ty, pa, va) in ev if ty == 16)
            # 소리 묶음: 접미사 배열로 선택 (slot → 난이도 인덱스는 아래 가설로)
            if song and banks:
                # 가설: .1 칸 0,1,2,3,4 = H,N,A,B,L / 6..10 = DP H,N,A,B,L ; 곡DB 순서 B,N,H,A,L
                hyp = {0: 2, 1: 1, 2: 3, 3: 0, 4: 4, 6: 7, 7: 6, 8: 8, 9: 5, 10: 9}
                di = hyp.get(slot)
                if di is not None:
                    lvl = song["levels"][di]
                    slot_vs_level[("level>0" if lvl else "level=0", "notes>0" if n16 else "notes=0")] += 1
                    c = song["suffix"][di] if len(song["suffix"]) > di else "0"
                    bank = sid_s if c == "0" else sid_s + c
                    if bank in banks and maxsnd:
                        snd_rel[maxsnd - banks[bank]] += 1
                        if len(snd_examples) < 8 and maxsnd - banks[bank] not in (0,):
                            snd_examples.append((sid_s, slot, bank, maxsnd, banks[bank]))
                    elif maxsnd:
                        snd_rel["bank_missing"] += 1
                        if len(snd_examples) < 16:
                            snd_examples.append((sid_s, slot, "missing:" + bank, sorted(banks)))

    print("event types:", dict(sorted(types.items())))
    for ty in sorted(tparam):
        ps = tparam[ty]
        print(f"  type {ty:2d} params: {dict(sorted(ps.items())) if len(ps) < 20 else (min(ps), max(ps), len(ps))}")
    print("tempo:", dict(tempo_vals))
    print("meter (value,param):", meter_vals.most_common(10))
    print("notes tap/ln:", dict(ln_vals))
    print("unsorted events:", unsorted)
    print("slot→difficulty hypothesis check (level vs notes):", dict(slot_vs_level))
    print("max sound value - bank file count:", sorted(snd_rel.items(), key=lambda x: -x[1])[:10])
    print("sound value 0 occurrences:", snd_zero)
    print("examples:", snd_examples[:16])


main()
