"""Read-only CP0 script, import, document-link and data-path checks. No game/HTTP access."""
import argparse
import ast
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from urllib.parse import unquote


REPO = next(p for p in Path(__file__).resolve().parents if (p / "cp0/README.md").is_file())
CP0 = REPO / "cp0"
CONVERTER_TOOLS = CP0 / "bms/converter/tools"
SCRIPT_DIRS = (CP0 / "iidx/tools", CP0 / "bms/tools", CONVERTER_TOOLS, CP0 / "djmax/work")
sys.path.insert(0, str(CONVERTER_TOOLS))
sys.path.insert(0, str(CP0 / "djmax/work/pylibs"))
sys.path.insert(0, str(CP0 / "djmax/work/plotlibs"))
from _paths import BMS_LIBRARY_BASE, BMS_TABLES, CONVERTER_ROOT, IIDX_ROOT


def audit(include_data=True):
    errors = []
    scripts = sorted(p for folder in SCRIPT_DIRS for p in folder.glob("*.py"))
    modules = set()
    for path in scripts:
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        except (SyntaxError, UnicodeError) as error:
            errors.append(f"syntax: {path.relative_to(REPO)}: {error}")
            continue
        for node in ast.walk(tree):
            imported = []
            if isinstance(node, ast.Import):
                imported = [name.name.split(".")[0] for name in node.names]
            elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
                imported = [node.module.split(".")[0]]
            for name in imported:
                if name in sys.stdlib_module_names or (path.parent / f"{name}.py").is_file():
                    continue
                modules.add(name)
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                value = node.value.replace("\\", "/")
                if re.search(r"(?<![/\w])(?:cp0/(?:tools|third_party)/|converter/(?:tools|out|system_bgm)/)", value):
                    errors.append(f"old workspace path: {path.relative_to(REPO)}:{node.lineno}")
    missing_modules = sorted(name for name in modules if importlib.util.find_spec(name) is None)
    errors.extend(f"missing Python dependency: {name}" for name in missing_modules)
    executables = {name: shutil.which(name) for name in ("ffmpeg", "ffprobe", "git", "powershell")}
    errors.extend(f"missing executable: {name}" for name, location in executables.items() if not location)

    documents = [REPO / "README.md", *CP0.glob("*.md")]
    for document in documents:
        for target in re.findall(r"\]\(([^)]+)\)", document.read_text(encoding="utf-8")):
            if target.startswith(("http://", "https://", "app://", "#")):
                continue
            target = unquote(target.strip("<>").split("#", 1)[0])
            resolved = document.parent / target
            private_link = (
                resolved.resolve().is_relative_to(CP0 / "djmax")
                or resolved.resolve().is_relative_to(CP0 / "bms/third_party")
                or resolved.name.startswith(("DJMAX_", "IIDX_"))
            )
            if not resolved.exists() and (include_data or not private_link):
                errors.append(f"missing link: {document.relative_to(REPO)} -> {target}")

    ignored_paths = ["cp0/bms/converter/out SP/probe.bme", "cp0/bms/converter/out DP/probe.bme",
                     "cp0/bms/converter/out_charts SP/probe.bme", "cp0/bms/converter/system_bgm/probe.wav",
                     "cp0/bms/converter/converted example/probe.bme", "cp0/bms/converter/logs/probe.log",
                     "cp0/DJMAX_analysis.md", "cp0/djmax/collected/state.json",
                     "cp0/iidx/tools/re_disasm_index.py", "cp0/bms/third_party/beatoraja/probe"]
    if executables["git"]:
        # NUL framing avoids Windows CRLF translation and Git's quoted path output.
        ignored = subprocess.run(["git", "check-ignore", "-z", "--stdin"],
                                 input=("\0".join(ignored_paths) + "\0").encode("utf-8"),
                                 cwd=REPO, capture_output=True, timeout=10)
        found = set(ignored.stdout.decode("utf-8").split("\0"))
        errors.extend(f"Git exclusion missing: {path}" for path in ignored_paths if path not in found)

    paths = {}
    if include_data:
        paths = {
            "converter": CONVERTER_ROOT,
            "converter_output_parent": CONVERTER_ROOT,
            "iidx_source": IIDX_ROOT,
            "iidx_extracted_songs": IIDX_ROOT / "extracted/songs",
            "iidx_music_db": IIDX_ROOT / "contents/data/info/1/music_data.bin",
            "iidx_omni_db": IIDX_ROOT / "contents/data/info/1/music_omni.bin",
            "iidx_songs_json": IIDX_ROOT / "extracted/system/database/songs.json",
            "system_sound_map": IIDX_ROOT / "experiement/_swap/sound_map_ko.csv",
            "bms_library_sp": BMS_LIBRARY_BASE.with_name(BMS_LIBRARY_BASE.name + " SP"),
            "bms_library_dp": BMS_LIBRARY_BASE.with_name(BMS_LIBRARY_BASE.name + " DP"),
            "bms_tables": BMS_TABLES,
            "djmax_packs": CP0 / "djmax/DJMAX RESPECT V_Data/StreamingAssets/Packs",
            "djmax_snapshot": CP0 / "djmax/work/GameAssembly.dll",
            "djmax_decomp": CP0 / "djmax/work/decomp.jsonl",
            "djmax_inventory": CP0 / "djmax/collected/inventory.json",
            "djmax_state": CP0 / "djmax/collected/state.json",
            "djmax_chart_bundles": CP0 / "djmax/collected/charts",
            "djmax_pack_items": CP0 / "djmax/collected/packs",
            "djmax_native": CP0 / "djmax/collected/native",
            "djmax_tocs": CP0 / "djmax/collected/metadata/tocs",
            "djmax_d2_audio": CP0 / "djmax/work/songs/D2/audio",
            "djmax_d2_charts": CP0 / "djmax/work/songs/D2/charts",
            "djmax_keys_cache": CP0 / "djmax/work/runtime/pack_keys.json",
            "djmax_toc_cache": CP0 / "djmax/work/runtime/pack_toc.json",
        }
        errors.extend(f"missing data: {name} -> {path}" for name, path in paths.items() if not path.exists())
    return {"scripts": len(scripts), "documents": len(documents), "data_paths": len(paths),
            "ignored_paths": len(ignored_paths),
            "dependencies": sorted(modules), "errors": errors, "complete": not errors,
            "converter_output": str(CONVERTER_ROOT / "out"), "iidx_source": str(IIDX_ROOT),
            "bms_library_base": str(BMS_LIBRARY_BASE), "bms_tables": str(BMS_TABLES)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-data", action="store_true", help="Check scripts/imports without private game datasets")
    args = parser.parse_args()
    report = audit(not args.skip_data)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["complete"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
