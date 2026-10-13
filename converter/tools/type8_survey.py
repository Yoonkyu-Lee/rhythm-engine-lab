"""모든 .1 채보의 type 8 이벤트 값 분포 조사.

python -I tools\\type8_survey.py <extracted\\songs 경로>
"""
import sys, struct, pathlib, collections

root = pathlib.Path(sys.argv[1])
combos = collections.Counter()      # 6개 값 묶음 (s8로 해석)
raw_hi = collections.Counter()      # value 상위 바이트
times = collections.Counter()       # type 8 이 놓인 시각 (0 인지)
counts = collections.Counter()      # 칸 하나당 type 8 개수
by_slot = collections.defaultdict(collections.Counter)
examples = {}
files = [p for p in root.rglob("*.1") if p.is_file() and p.parent.name == "chart"]
for p in files:
    raw = p.read_bytes()
    for slot in range(12):
        off, ln = struct.unpack_from("<II", raw, slot * 8)
        if not ln or off + ln > len(raw):
            continue
        vals = {}
        n8 = 0
        for k in range(ln // 8):
            t, ty, pa, va = struct.unpack_from("<IBBH", raw, off + k * 8)
            if ty == 8:
                n8 += 1
                times["t=0" if t == 0 else "t>0"] += 1
                raw_hi[va >> 8] += 1
                vals[pa] = va
        counts[n8] += 1
        if vals:
            key = tuple((v & 0xFF) - 256 if v & 0x80 else v for _, v in sorted(vals.items()))
            combos[key] += 1
            by_slot[slot][key] += 1
            examples.setdefault(key, f"{p.stem} slot{slot}")

print("files", len(files))
print("type8 per chart:", dict(counts))
print("time:", dict(times), " value high byte:", dict(raw_hi))
print("\ntop combos (param0..5, low byte as s8):")
for k, n in combos.most_common(15):
    print(f"{n:6d}  {k}  e.g. {examples[k]}")
print("distinct combos:", len(combos))
