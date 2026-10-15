r"""변환한 IIDX BMS로 "작품별" BMS 난이도 테이블 생성 (SP, DP 각각).

python -I cp0\bms\converter\tools\make_tables.py [변환 출력 기준 경로] [테이블 출력 폴더]
  기본: D:\Games\BMS\IIDX converted (→ "... SP", "... DP"),  D:\Games\BMS\IIDX tables

형식: 표준 BMS 테이블 (beatoraja/LR2/Qwilight 공통)
  index.html  <meta name="bmstable" content="header.json">
  header.json {name, symbol, data_url, level_order, mode}
  data.json   [{md5, sha256, level, title, artist, comment}]
레벨 = 작품 ("01 1st style" ~ "33 Sparkle Shower"). 플레이어는 MD5로 내 폴더의 BMS 파일을 찾음.
※ BMS 파일을 다시 변환하면 해시가 바뀌므로 테이블도 다시 만들어야 함.
"""
import argparse, hashlib, json, pathlib, sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from _paths import BMS_LIBRARY_BASE, BMS_TABLES, IIDX_ROOT

sys.stdout.reconfigure(encoding="utf-8")
SONGS_DEFAULT = IIDX_ROOT / "extracted/system/database/songs.json"
DIFF = {"B": "BEGINNER", "N": "NORMAL", "H": "HYPER", "A": "ANOTHER", "L": "LEGGENDARIA"}
LEVEL_KEY = {"B": "B", "N": "N", "H": "H", "A": "A", "L": "L"}


def header_value(text, key):
    for ln in text.splitlines():
        if ln.upper().startswith(f"#{key} "):
            return ln[len(key) + 2:].strip()
    return ""


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", nargs="?", type=pathlib.Path, default=BMS_LIBRARY_BASE)
    parser.add_argument("out", nargs="?", type=pathlib.Path, default=BMS_TABLES)
    parser.add_argument("--songs-json", type=pathlib.Path, default=SONGS_DEFAULT)
    args = parser.parse_args(argv)
    base = args.base.expanduser().resolve()
    out = args.out.expanduser().resolve()
    songs = {s["id"]: s for s in json.loads(args.songs_json.read_text(encoding="utf-8"))}
    for style, mode, symbol in (("SP", "beat-7k", "Ⅱ"), ("DP", "beat-14k", "Ⅱ")):
        root = base.with_name(f"{base.name} {style}")
        versions = {}
        data = []
        for bme in sorted(root.glob("*/*.bme")):
            raw = bme.read_bytes()
            text = raw.decode("utf-8-sig") if raw[:3] == b"\xef\xbb\xbf" else raw.decode("cp932")
            sid = int(bme.stem.split("_")[0])
            diff = bme.stem.split("_")[1][2]
            song = songs.get(sid, {})
            v = sid // 1000
            vname = f"{v:02d} {song.get('version', '?')}"
            versions[v] = vname
            iidx_level = song.get(f"{style}{diff}", "")
            data.append({
                "md5": hashlib.md5(raw).hexdigest(),
                "sha256": hashlib.sha256(raw).hexdigest(),
                "level": vname,
                "title": f"{header_value(text, 'TITLE')} {header_value(text, 'SUBTITLE')}".strip(),
                "artist": header_value(text, "ARTIST"),
                "comment": f"{style}{diff} ☆{iidx_level}" + (" [omni]" if song.get("omnimix_only") else ""),
            })
        name = f"IIDX {style} 작품별"
        header = {"name": name, "symbol": symbol, "data_url": "data.json",
                  "level_order": [versions[k] for k in sorted(versions)], "mode": mode}
        d = out / style.lower()
        d.mkdir(parents=True, exist_ok=True)
        (d / "header.json").write_text(json.dumps(header, ensure_ascii=False, indent=1), encoding="utf-8")
        (d / "data.json").write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        (d / "index.html").write_text(
            '<!doctype html><html><head><meta charset="utf-8">'
            '<meta name="bmstable" content="header.json">'
            f"<title>{name}</title></head><body>{name}: {len(data)} charts</body></html>\n", encoding="utf-8")
        print(f"{name}: {len(data)} charts, {len(versions)} levels → {d}")


if __name__ == "__main__":
    main()
