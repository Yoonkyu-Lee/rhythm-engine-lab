"""IIDX .1 채보: 헤더 표 + 한 난이도의 이벤트 앞부분 + type 집계."""
import sys, struct, collections

raw = open(sys.argv[1], "rb").read()
slot = int(sys.argv[2]) if len(sys.argv) > 2 else 2
names = ["SPH", "SPN", "SPA", "SPB", "SPL", "?5", "DPH", "DPN", "DPA", "DPB", "DPL", "?11"]  # 커뮤니티 순서(추정)
print("size", len(raw))
for i in range(12):
    off, ln = struct.unpack_from("<II", raw, i * 8)
    print(f"slot{i:2d} {names[i]:4s} off=0x{off:06X} len=0x{ln:05X} events={ln // 8}")
off, ln = struct.unpack_from("<II", raw, slot * 8)
ev = [struct.unpack_from("<IBBH", raw, off + k * 8) for k in range(ln // 8)]
print(f"\n--- slot {slot} first 30 events (time u32, type u8, param u8, value u16) ---")
for t, ty, pa, va in ev[:30]:
    print(f"{t:8d}  type={ty:2d} param={pa:3d} value={va:5d}")
print("\n--- type counts ---")
c = collections.Counter(e[1] for e in ev)
print(dict(sorted(c.items())))
print("last 3:", ev[-3:])
