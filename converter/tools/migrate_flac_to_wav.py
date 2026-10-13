r"""이미 변환한 라이브러리의 FLAC 키음을 WAV(16비트 PCM)로 바꾸고 채보의 #WAV 참조를 고침.

python -I tools\migrate_flac_to_wav.py [변환 출력 기준 경로] [--procs N]
  기본: D:\Games\BMS\IIDX converted (→ "... SP", "... DP")

이유: Qwilight가 FLAC 일부(특히 긴 반주)를 재생하지 못함. iidx2bms.py는 이제 WMA를 바로 WAV로 내보내므로
     새로 변환하면 이 스크립트는 필요 없음. FLAC은 무손실이라 FLAC→WAV 결과는 WMA→WAV와 표본 단위로 같음.
순서 (곡마다): WAV를 .part로 쓴 뒤 이름 변경 → 채보 참조 교체 → FLAC 삭제. 중간에 끊겨도 다시 돌리면 이어짐.
※ 채보 바이트가 바뀌므로 끝나면 tools\make_tables.py 로 난이도 테이블을 다시 만들 것.
"""
import argparse, os, pathlib, re, subprocess, sys
from concurrent.futures import ProcessPoolExecutor, as_completed

sys.stdout.reconfigure(encoding="utf-8")
BATCH = 40  # ffmpeg 한 번에 처리할 파일 수 (명령줄 길이 제한 안쪽)
WAV_REF = re.compile(rb"^(#WAV\w\w [^\r\n]*?)\.flac(\r?)$", re.M | re.I)


def decode(pairs):
    cmd = ["ffmpeg", "-nostdin", "-loglevel", "error", "-y"]
    for s, _ in pairs:
        cmd += ["-i", str(s)]
    for i, (_, d) in enumerate(pairs):
        cmd += ["-map", f"{i}:a", "-c:a", "pcm_s16le", "-f", "wav", str(d.with_name(d.name + ".part"))]
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode:
        for _, d in pairs:
            d.with_name(d.name + ".part").unlink(missing_ok=True)
        if len(pairs) > 1:
            for p in pairs:
                decode([p])
            return
        raise RuntimeError(f"{pairs[0][0]}: {r.stderr.decode('utf-8', 'replace')[:200]}")
    for _, d in pairs:
        os.replace(d.with_name(d.name + ".part"), d)


def migrate_song(song_dir):
    song_dir = pathlib.Path(song_dir)
    flacs = sorted(song_dir.glob("*.flac"))
    todo = [(f, f.with_suffix(".wav")) for f in flacs if not f.with_suffix(".wav").exists()]
    for k in range(0, len(todo), BATCH):
        decode(todo[k:k + BATCH])
    charts = 0
    for bme in song_dir.glob("*.bm[es]"):
        raw = bme.read_bytes()
        new = WAV_REF.sub(rb"\1.wav\2", raw)  # ASCII 부분만 바꾸므로 cp932·UTF-8 모두 안전
        if new != raw:
            tmp = bme.with_name(bme.name + ".part")
            tmp.write_bytes(new)
            os.replace(tmp, bme)
            charts += 1
    for bme in song_dir.glob("*.bm[es]"):  # 참조가 남아 있으면 FLAC을 지우지 않음
        if b".flac" in bme.read_bytes().lower():
            return song_dir.name, len(todo), charts, "flac reference remains"
    for f in flacs:
        f.unlink()
    return song_dir.name, len(todo), charts, "ok"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("base", nargs="?", default=r"D:\Games\BMS\IIDX converted")
    ap.add_argument("--procs", type=int, default=max(1, (os.cpu_count() or 4) - 2))
    a = ap.parse_args()
    base = pathlib.Path(a.base)
    dirs = [d for st in ("SP", "DP") for d in sorted(base.with_name(f"{base.name} {st}").iterdir())
            if d.is_dir() and any(d.glob("*.flac"))]
    print(f"{len(dirs)} song folders with FLAC", flush=True)
    done = files = 0
    bad = []
    with ProcessPoolExecutor(a.procs) as ex:
        futs = {ex.submit(migrate_song, d): d for d in dirs}
        for fu in as_completed(futs):
            done += 1
            try:
                name, n, charts, status = fu.result()
                files += n
                if status != "ok":
                    bad.append((name, status))
            except Exception as e:
                bad.append((futs[fu].name, repr(e)[:200]))
            if done % 50 == 0 or done == len(dirs):
                print(f"[{done}/{len(dirs)}] {files} files", flush=True)
    print(f"done: {done} folders, {files} files, problems {len(bad)}")
    for b in bad:
        print("  !", *b)


if __name__ == "__main__":
    main()
