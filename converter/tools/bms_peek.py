"""BMS 파일 하나를 열어 바이트 수준 정보 + 헤더/채널 구조 요약."""
import sys, re, collections, pathlib

p = pathlib.Path(sys.argv[1])
raw = p.read_bytes()
print("file:", p.name, "size:", len(raw))
print("first 64 bytes hex:", raw[:64].hex(" "))
bom = raw[:3] == b"\xef\xbb\xbf"
print("UTF-8 BOM:", bom)
for enc in ("utf-8", "cp932", "cp949"):
    try:
        raw.decode(enc); print("decodes as", enc); break
    except UnicodeDecodeError:
        print("not", enc)
text = raw.decode(enc, errors="replace")
lines = text.splitlines()
print("line endings CRLF:", b"\r\n" in raw, "lines:", len(lines))

heads, chans, other = [], collections.Counter(), []
defs = collections.Counter()
for ln in lines:
    m = re.match(r"#(\d{3})([0-9A-Za-z]{2}):(.*)", ln)
    if m:
        chans[m.group(2).upper()] += 1
        continue
    m = re.match(r"#(WAV|BMP|BPM|STOP|EXBPM)([0-9A-Za-z]{2})\s", ln, re.I)
    if m:
        defs[m.group(1).upper()] += 1
        if defs[m.group(1).upper()] <= 3:
            heads.append(ln)
        continue
    if ln.startswith("#"):
        heads.append(ln)

print("\n--- header (non-channel) lines ---")
for h in heads[:40]:
    print(h)
print("\n--- definition counts ---", dict(defs))
print("--- channel usage (line count) ---")
for c, n in sorted(chans.items()):
    print(c, n)
# print a few measure lines
print("\n--- sample measure lines ---")
shown = 0
for ln in lines:
    if re.match(r"#00[0-9]..:", ln) or re.match(r"#01[0-9]..:", ln):
        print(ln[:120]); shown += 1
        if shown > 25: break
