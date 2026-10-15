"""Converter paths anchored to the tools directory, plus external data overrides."""
import os
from pathlib import Path


CONVERTER_ROOT = Path(__file__).resolve().parent.parent
IIDX_ROOT = Path(os.environ.get("RHYTHM_IIDX_ROOT", r"D:\Arcade\LDJ-010-2026081900")).expanduser().resolve()
BMS_LIBRARY_BASE = Path(os.environ.get("RHYTHM_BMS_LIBRARY_BASE", r"D:\Games\BMS\IIDX converted")).expanduser().resolve()
BMS_TABLES = Path(os.environ.get("RHYTHM_BMS_TABLES", r"D:\Games\BMS\IIDX tables")).expanduser().resolve()
