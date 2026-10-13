"""BMS 전체 통계: 인코딩, 채널 사용, 헤더 명령 빈도."""
import sys, re, collections, pathlib

root = pathlib.Path(sys.argv[1])
files = [p for p in root.rglob("*") if p.suffix.lower() in (".bms", ".bme", ".bml", ".pms", ".bmx")]
enc_c, ch_c, cmd_c, wav_n, poly = collections.Counter(), collections.Counter(), collections.Counter(), [], []
lntype = collections.Counter()
ext_c = collections.Counter(p.suffix.lower() for p in files)
for p in files:
    raw = p.read_bytes()
    for enc in ("utf-8", "cp932", "cp949"):
        try:
            text = raw.decode(enc); break
        except UnicodeDecodeError:
            continue
    else:
        enc = "other"; text = raw.decode("cp932", errors="replace")
    enc_c[enc + ("+BOM" if raw[:3] == b"\xef\xbb\xbf" else "")] += 1
    seen_ch, seen_cmd = set(), set()
    nwav = 0
    bgm_lines = collections.Counter()
    for ln in text.splitlines():
        ln = ln.strip()
        m = re.match(r"#(\d{3})([0-9A-Za-z]{2}):", ln)
        if m:
            ch = m.group(2).upper(); seen_ch.add(ch)
            if ch == "01": bgm_lines[m.group(1)] += 1
            continue
        m = re.match(r"#([A-Za-z]+)", ln)
        if m:
            cmd = m.group(1).upper()
            cmd = re.sub(r"(WAV|BMP|BPM|STOP|EXBPM|BGA|LNOBJ)$", r"\1", cmd)
            if re.match(r"(WAV|BMP|BPM|STOP|EXBPM)[0-9A-Z]{0,2}$", cmd + "", re.I):
                cmd = re.match(r"(WAV|BMP|BPM|STOP|EXBPM)", cmd).group(1)
            seen_cmd.add(cmd)
            if re.match(r"#WAV[0-9A-Za-z]{2}\s", ln, re.I): nwav += 1
            if cmd == "LNTYPE": lntype[ln.split()[-1] if len(ln.split()) > 1 else "?"] += 1
    ch_c.update(seen_ch); cmd_c.update(seen_cmd); wav_n.append(nwav)
    poly.append(max(bgm_lines.values()) if bgm_lines else 0)

n = len(files)
print("files:", n, dict(ext_c))
print("encoding:", dict(enc_c))
print("#WAV per chart: min %d median %d max %d" % (min(wav_n), sorted(wav_n)[n // 2], max(wav_n)))
print("max #xxx01 lines in one measure (BGM polyphony): median %d max %d" % (sorted(poly)[n // 2], max(poly)))
print("LNTYPE values:", dict(lntype))
print("\nchannels (files using):")
print(" ".join(f"{c}:{k}" for c, k in sorted(ch_c.items())))
print("\nheader commands (files using):")
print(" ".join(f"{c}:{k}" for c, k in cmd_c.most_common(60)))
