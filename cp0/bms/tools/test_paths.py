"""Relocation regressions using temporary inputs and an unrelated working directory."""
import array
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
import wave


REPO = next(p for p in Path(__file__).resolve().parents if (p / "cp0/README.md").is_file())
TOOLS = REPO / "cp0/bms/converter/tools"


class RelocationChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rhythm_paths_")
        self.scratch = Path(self.temp.name).resolve()
        self.cwd = self.scratch / "unrelated"
        self.cwd.mkdir()

    def tearDown(self):
        # Check the exact task-owned temporary directory before recursive cleanup.
        self.assertEqual(self.scratch.parent, Path(tempfile.gettempdir()).resolve())
        self.assertTrue(self.scratch.name.startswith("rhythm_paths_"))
        self.temp.cleanup()

    def run_python(self, *arguments, env=None):
        result = subprocess.run([sys.executable, "-I", "-X", "utf8", *map(str, arguments)],
                                cwd=self.cwd, env=env, capture_output=True, text=True,
                                encoding="utf-8", timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def test_converter_paths_and_external_overrides_survive_other_cwd(self):
        env = os.environ.copy()
        source = self.scratch / "iidx_source"
        library = self.scratch / "converted"
        tables = self.scratch / "tables"
        env.update(RHYTHM_IIDX_ROOT=str(source), RHYTHM_BMS_LIBRARY_BASE=str(library),
                   RHYTHM_BMS_TABLES=str(tables))
        code = (
            "import json,runpy,sys; m=runpy.run_path(sys.argv[1]); "
            "p=sys.modules['_paths']; print(json.dumps({"
            "'source':str(m['ROOT']),'extracted':str(m['EXTRACTED']),"
            "'movie':str(m['MOVIE_DIR']),'out':str(m['OUT_DEFAULT']),"
            "'library':str(p.BMS_LIBRARY_BASE),'tables':str(p.BMS_TABLES)}))"
        )
        values = json.loads(self.run_python("-c", code, TOOLS / "iidx2bms.py", env=env))
        self.assertEqual(Path(values["source"]), source)
        self.assertEqual(Path(values["extracted"]), source / "extracted")
        self.assertEqual(Path(values["movie"]), source / "contents/data/movie")
        self.assertEqual(Path(values["out"]), TOOLS.parent / "out")
        self.assertEqual(Path(values["library"]), library)
        self.assertEqual(Path(values["tables"]), tables)
        self.assertFalse((self.cwd / "out").exists())

    def test_renderer_finds_audio_beside_chart_and_comparison_imports_sibling(self):
        song = self.scratch / "song"
        song.mkdir()
        with wave.open(str(song / "tone.wav"), "wb") as output:
            output.setparams((2, 2, 8000, 200, "NONE", "not compressed"))
            output.writeframes(struct.pack("<hh", 1000, -1000) * 200)
        chart = song / "fixture.bme"
        chart.write_text("#BPM 120\n#WAV01 tone.wav\n#00011:01\n#00101:01\n", encoding="utf-8")
        rendered = self.scratch / "rendered.wav"
        log = self.run_python(TOOLS / "bms_render.py", chart, rendered)
        self.assertIn("missing 0", log)
        with wave.open(str(rendered), "rb") as source:
            self.assertEqual(source.getframerate(), 44100)
            samples = array.array("h", source.readframes(source.getnframes()))
        self.assertGreater(max(abs(x) for x in samples), 500)
        comparison = self.run_python(TOOLS / "compare_bms.py", chart, chart)
        self.assertIn("A 1 B 1", comparison)

    def test_tables_use_explicit_library_output_and_songs_json(self):
        base = self.scratch / "converted"
        content = b"#TITLE fixture\n#ARTIST test\n#BPM 120\n#00011:01\n"
        for style in ("SP", "DP"):
            folder = base.with_name(base.name + " " + style) / "33131 fixture"
            folder.mkdir(parents=True)
            (folder / f"33131_{style}A.bme").write_bytes(content)
        songs = self.scratch / "songs.json"
        songs.write_text(json.dumps([{"id": 33131, "version": "fixture", "SPA": 12, "DPA": 12}]), encoding="utf-8")
        output = self.scratch / "tables"
        self.run_python(TOOLS / "make_tables.py", base, output, "--songs-json", songs)
        for style in ("sp", "dp"):
            data = json.loads((output / style / "data.json").read_text(encoding="utf-8"))
            self.assertEqual(len(data), 1)
            self.assertEqual(data[0]["sha256"], hashlib.sha256(content).hexdigest())

    def test_system_bgm_root_override_exports_only_temporary_data(self):
        source = self.scratch / "source"
        sounds = source / "extracted/system/sound"
        sounds.mkdir(parents=True)
        name = "syssd_bgm_opening"
        (sounds / (name + ".wav")).write_bytes(b"fixture-audio")
        mapping = source / "experiement/_swap/sound_map_ko.csv"
        mapping.parent.mkdir(parents=True)
        mapping.write_text("file,group_ko\n" + name + ",opening\n", encoding="utf-8")
        output = self.scratch / "system_bgm"
        self.run_python(TOOLS / "export_system_bgm.py", output, "--root", source)
        files = list(output.rglob("*.wav"))
        self.assertEqual(len(files), 1)
        self.assertEqual(files[0].read_bytes(), b"fixture-audio")

    def test_iidx_survey_root_override_uses_temporary_databases(self):
        source = self.scratch / "iidx"
        database = source / "contents/data/info/1"
        database.mkdir(parents=True)
        for name in ("music_data.bin", "music_omni.bin"):
            (database / name).write_bytes(b"TEST" + struct.pack("<III", 0, 0, 0))
        (source / "extracted/songs").mkdir(parents=True)
        log = self.run_python(REPO / "cp0/iidx/tools/iidx_survey.py", "--root", source)
        self.assertIn("songs in DB: 0", log)
        self.assertIn("charts found: 0", log)


if __name__ == "__main__":
    unittest.main(verbosity=2)
