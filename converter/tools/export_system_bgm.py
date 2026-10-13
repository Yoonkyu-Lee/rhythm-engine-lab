r"""IIDX 시스템 BGM(타이틀·선곡·리절트 등)만 골라 용도별 폴더로 정리.

python -I tools\export_system_bgm.py [출력 폴더]

입력: 추출본 system\sound (33 세트), legacy_32\sound (32 세트)  — .sd9 에서 32바이트 헤더만 뗀 WAV
설명: experiement\_swap\sound_map_ko.csv (sys_soundtable.def 번역)
대상: syssd_bgm_* (BGM), syssd_result_* (리절트 징글·관객 환호). 네비 음성·계절 음성·효과음은 제외.
"""
import csv, pathlib, re, shutil, sys

sys.stdout.reconfigure(encoding="utf-8")
ROOT = pathlib.Path(r"D:\Arcade\LDJ-010-2026081900")
SETS = {"33 Sparkle Shower": ROOT / "extracted/system/sound", "32 Pinky Crush": ROOT / "extracted/legacy_32/sound"}
MAP = ROOT / "experiement/_swap/sound_map_ko.csv"
OUT = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(__file__).resolve().parent.parent / "system_bgm"

CATEGORIES = [  # (폴더, 이름 규칙) — 위에서부터 처음 맞는 것
    ("1 타이틀·대기·데모", r"^syssd_bgm_(opening|cardout|um)"),
    ("2 선곡", r"^syssd_bgm_(default|season)"),
    ("3 리절트", r"^syssd_(result|bgm_result)"),
    ("4 아레나·BPL", r"^syssd_bgm_(arena|bpl)"),
    ("5 스텝업", r"^syssd_bgm_stepup"),
    ("6 이벤트", r"^syssd_bgm_event"),
    ("7 엑스트라 스테이지", r"^syssd_bgm_extrastage"),
    ("8 기타", r"^syssd_bgm_"),
]


def safe(s):
    return "".join("_" if c in '\\/:*?"<>|' else c for c in s).strip()


def main():
    desc = {r["file"]: r["group_ko"] for r in csv.DictReader(open(MAP, encoding="utf-8-sig"))}
    total = 0
    for set_name, src in SETS.items():
        if not src.exists():
            continue
        for f in sorted(src.rglob("*.wav")):
            stem = f.stem
            if not re.match(r"^syssd_(bgm_|result_)", stem):
                continue
            cat = next(c for c, rx in CATEGORIES if re.match(rx, stem)) if any(
                re.match(rx, stem) for _, rx in CATEGORIES) else "8 기타"
            name = f"{safe(desc[stem])} ({stem}).wav" if stem in desc else f"{stem}.wav"
            dst = OUT / set_name / cat / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, dst)
            total += 1
    print(f"exported {total} files → {OUT}")
    for d in sorted(OUT.glob("*/*")):
        print(f"  {d.relative_to(OUT)}: {len(list(d.iterdir()))}")


main()
