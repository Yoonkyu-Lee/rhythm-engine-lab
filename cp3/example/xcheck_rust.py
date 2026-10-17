# Rust 컴파일러(trackc dump) ↔ Python 검산기(check_example.py)의 계산을 차분마다 대조
# 사용: trackc compile cp3/example/synth_track x.trk && trackc dump x.trk > dump.txt && python -X utf8 cp3/example/xcheck_rust.py dump.txt
import sys, collections, tomllib, pathlib
from fractions import Fraction as F
sys.path.insert(0, "cp3/example"); import check_example as ce
root = ce.ROOT = pathlib.Path("cp3/example/synth_track")
tr = tomllib.loads((root / "track.toml").read_text(encoding="utf-8"))
layouts = {l["id"]: [x["id"] for x in l["lanes"]] for l in tr["layout"]}
sounds = {s["id"]: s for s in tr["sound"]}
raw = {p.stem: tomllib.loads(p.read_text(encoding="utf-8")) for p in (root / "charts").glob("*.toml")}
py = {}
for cid, c in raw.items():
    if "base" not in c:
        py[cid] = ce.compile_chart(cid, c, layouts, sounds)
for cid, c in raw.items():
    if "base" in c:
        b = py[c["base"]]; sc = F(c["time_scale"])
        py[cid] = dict(b, starts=[dict(e, t=e["t"] / sc) for e in b["starts"]], notes=[dict(n, t=n["t"]/sc, end=n["end"]/sc) for n in b["notes"]],
                       lane_sounds=[dict(x, t=x["t"]/sc) for x in b["lane_sounds"]], scroll=[(t/sc, p, s*sc) for t,p,s in b["scroll"]], bars=[t/sc for t in b["bars"]])
rust = collections.defaultdict(lambda: collections.defaultdict(list)); cur = None
for line in open(sys.argv[1], encoding="utf-8"):
    f = line.split()
    if f[0] == "chart": cur = f[1]; continue
    rust[cur][f[0]].append(f[1:])
bad = 0
for cid in sorted(py):
    p, r = py[cid], rust[cid]
    ps = sorted((ce.ns(e["t"]), e["sound"]) for e in p["starts"])
    rs = sorted([(int(a[0]), a[1]) for a in r["auto"]] + [(int(a[1]), a[2]) for a in r["nsnd"]])
    pn = sorted((n["lane"], ce.ns(n["t"]), ce.ns(n["end"])) for n in p["notes"])
    rn = sorted((a[0], int(a[1]), int(a[2])) for a in r["note"])
    pl = sorted((x["lane"], ce.ns(x["t"]), x["sound"]) for x in p["lane_sounds"])
    rl = sorted((a[0], int(a[1]), a[2]) for a in r["lsnd"])
    pb = sorted(ce.ns(t) for t in p["bars"]); rb = sorted(int(a[0]) for a in r["bar"])
    psc = [(ce.ns(t), float(pp)) for t, pp, s in p["scroll"]]; rsc = [(int(a[0]), float(a[1])) for a in r["scroll"]]
    ok = [ps == rs, pn == rn, pl == rl, pb == rb, sorted(psc) == sorted(rsc)]
    print(f"{cid:12} 소리 시작 {len(rs):3} {'같음' if ok[0] else '다름'} | 노트 {len(rn)} {'같음' if ok[1] else '다름'} | 빈 레인 소리 {'같음' if ok[2] else '다름'} | 마디선 {'같음' if ok[3] else '다름'} | 스크롤 {'같음' if ok[4] else '다름'}")
    if not all(ok):
        bad += 1
        if not ok[4]: print("   py", sorted(psc)); print("   rs", sorted(rsc))
        if not ok[0]: print("   차이", sorted(set(ps) ^ set(rs))[:6])
sys.exit(1 if bad else 0)
