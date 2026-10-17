"""analyze_sync.py 결과(report_sync.json) 여러 개 → 마크다운 표 하나.
python -I -X utf8 cp3/tools/sync_table.py <runs 폴더...>"""
import json, pathlib, sys

sys.stdout.reconfigure(encoding="utf-8")


def rng(d):
    return "-" if not d else f"{d['min']:+.2f}~{d['max']:+.2f}"


print("| 실행 | 레인 | 경로 | D ms | 건너뜀 | PGREAT | 키음 vs 반주 (중앙, 범위 ms) | 잔차 범위 ms | 불일치 | 키음 누락 · 늦음 | 도장 − 누름 (중앙, 1 ms 넘음, 짝 없음) | 시간 이동 (바뀐 창 / 창, 상관 최소) |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|")
for root in sys.argv[1:]:
    for p in sorted(pathlib.Path(root).glob("*/report_sync.json")):
        r = json.loads(p.read_text(encoding="utf-8"))
        a = r["all"]
        kv = a["ks_vs_long"] or a["ks_vs_new"]
        res = a["residual_long"] or a["residual"]
        st = a["stamp_vs_press"]
        g = r.get("integrity")
        steps = sorted({s["shift_ms"] for s in g["shifts"]}) if g else []
        print(f"| {r['run'].rsplit('-', 1)[0]} | {r['lanes']} | {r['path']} | {r['d_ms']} | {r['skips']} | {a['perfect']} | "
              f"{kv.get('p50', 0):+.2f}, {rng(kv)} | {rng(res)} | {a['mismatch_long_1ms'] if a['residual_long'] else a['mismatch_1ms']} | "
              f"{a['keysound_missing']} · {a['perfect_late_keysound']} | "
              f"{(st.get('p50', 0) if st else 0):+.2f}, {a['stamp_off_1ms']}, {r['stamp_unpaired'] + r['bot_not_sent']} | "
              f"{(str(g['shifted']) + ' / ' + str(g['windows'] * 2) + ', ' + str(g['corr_min'])) if g else '-'} |")
