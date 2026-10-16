"""IIDX 채보 하나 → 1건반 실행 데이터 (시험 콘텐츠 어댑터, 로컬 전용 데이터 사용). 원본 .1 채보와 원본 음원 묶음에서 직접 만든다.

python -I -X utf8 cp2/tools/adapt_iidx.py <곡 ID> [--chart SPH] [--lane (노트가 가장 많은 레인)] [--out cp2/content/<이름>]

- 입력: 추출본(cp0/bms/converter/tools/_paths.py의 IIDX_ROOT\\extracted). .1 채보 읽기·곡 DB·음원 묶음 고르기만 iidx2bms에서 빌려 쓴다
  (BMS를 만들지 않는다. 시각은 .1의 ms 정수 그대로 × 1,000,000 ns).
- 건반: 고른 차분의 1P 레인(0..6 건반, 7 스크래치) 하나 → 레인 0. 노트의 소리 = 그 순간 그 레인의 소리칸(type 2 교체). 롱노트 = value ms.
- 자동재생: type 7 전부 + 나머지 레인의 노트 소리(그 레인의 그 순간 소리칸). 노트 없는 교체(투명)는 소리를 내지 않는다.
  같은 시각 이벤트는 파일 순서대로 처리한다 (교체가 앞이면 그 노트에 적용).
- 소리: .2dx에서 나온 WAV(MS-ADPCM)는 libsndfile, .s3p에서 나온 WMA는 ffmpeg로 디코딩. 44.1 kHz만. 이득 1
  (묶음 항목 헤더의 볼륨 추정값은 확인되지 않아 쓰지 않음). type 7의 param(팬 추정)도 쓰지 않음.
- 스크롤: type 4 템포(value / param)로 박 위치. 기준 BPM = 첫 템포 (곡 DB에 BPM이 없음).
출력: chart.rtc, names.json (소리 번호 → 묶음 파일), expected.json (모든 발음: 시각 ns, 소리, 이득, 출처).
"""
import argparse, json, pathlib, subprocess, sys, warnings
from collections import Counter
from fractions import Fraction

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "cp0" / "bms" / "converter" / "tools"))
import rtc  # noqa: E402

NS = rtc.NS
MS = 1_000_000
CHART_SLOT = {"SPN": 1, "SPH": 0, "SPA": 2, "SPB": 3, "SPL": 4}


def decode(path):
    if path.suffix.lower() == ".wav":
        import soundfile as sf
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            data, rate = sf.read(str(path), dtype="float32", always_2d=True)
    else:
        probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=sample_rate,channels",
                                "-of", "csv=p=0", str(path)], capture_output=True, text=True, check=True).stdout.strip().split(",")
        rate, ch = int(probe[0]), int(probe[1])
        raw = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(path), "-f", "f32le", "-acodec", "pcm_f32le", "-"],
                             capture_output=True, check=True).stdout
        data = np.frombuffer(raw, dtype="<f4").reshape(-1, ch)
    if rate != 44100:
        raise SystemExit(f"{path.name}: {rate} Hz (44.1 kHz만 지원)")
    if data.shape[1] == 1:
        data = np.repeat(data, 2, 1)
    return np.ascontiguousarray(data[:, :2], dtype=np.float32)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("id", type=int)
    ap.add_argument("--chart", default="SPH", choices=sorted(CHART_SLOT))
    ap.add_argument("--lane", type=int, default=-1, help="1P 레인 0..6 건반, 7 스크래치 (기본: 노트가 가장 많은 레인)")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    sys.argv = sys.argv[:1]
    import iidx2bms as C  # .1 읽기·곡 DB·묶음 고르기만 빌려 쓴다

    meta = C.load_music_db()
    song = meta[a.id]
    src = C.EXTRACTED / song["dir"]
    slot = CHART_SLOT[a.chart]
    events = C.read_chart(src / "chart" / f"{a.id:05d}.1")[slot]
    _, _, db_idx = C.SLOTS[slot]
    sfx = song["suffix"][db_idx] if len(song["suffix"]) > db_idx else "0"
    bank_name, bank_dir, fell_back = C.find_bank(src / "audio", a.id, sfx)
    files = C.bank_listing(bank_dir)
    rep = Counter()

    # 1P 노트만 (SP 차분). 특수 스크래치(107) = 스크래치 레인
    def lane_of(pa):
        return pa % 100 if pa >= 100 else pa

    per_lane = Counter(lane_of(pa) for (t, ty, pa, va) in events if ty == 0 and lane_of(pa) <= 7)
    key = a.lane if a.lane >= 0 else per_lane.most_common(1)[0][0]

    sound_ix, clips, order = {}, [], []

    def sound(v):
        if v == 0:
            rep["note_without_sound"] += 1
            return None
        f = files[v - 1] if v <= len(files) else None
        if f is None:
            rep["missing_sound"] += 1
            return None
        if v not in sound_ix:
            sound_ix[v] = len(clips)
            clips.append(decode(f))
            order.append(f"{bank_name}/{f.name}")
        return sound_ix[v]

    regs, auto, notes, note_sounds, seen = {}, [], [], [], set()
    for (t, ty, pa, va) in events:
        if ty == 0:
            lane = lane_of(pa)
            if lane > 7:
                rep["bad_lane"] += 1
                continue
            if not va:
                if (lane, t) in seen:  # 원본의 중복 노트 (iidx2bms와 같은 처리)
                    rep["duplicate_note_dropped"] += 1
                    continue
                seen.add((lane, t))
            s = sound(regs.get(lane, 0))
            if lane == key:
                start = len(note_sounds)
                if s is not None:
                    note_sounds.append((s, 1.0))
                notes.append((t * MS, (t + va) * MS if va else t * MS, start, len(note_sounds) - start))
            elif s is not None:
                auto.append((t * MS, s, 1.0))
        elif ty == 2:
            regs[pa] = va
        elif ty == 7 and va:
            if pa:
                rep["autoplay_param_ignored"] += 1
            s = sound(va)
            if s is not None:
                auto.append((t * MS, s, 1.0))
        elif ty in (1, 3):
            rep["2p_event_ignored"] += 1
    auto.sort(key=lambda e: e[0])
    bad = [i for i in range(1, len(notes)) if notes[i][0] <= notes[i - 1][1]]
    if bad:
        raise SystemExit(f"레인 안 노트 겹침 {len(bad)}개 (--lane을 바꾸세요)")

    # 스크롤: 템포 (ms 정수 시각, BPM = value / param)
    tempos = sorted({t: C.bpm_of(pa, va) for (t, ty, pa, va) in events if ty == 4}.items())
    if not tempos or tempos[0][0] != 0:
        tempos.insert(0, (0, tempos[0][1]))
    scroll, pos = [], Fraction(0)
    for i, (t, bpm) in enumerate(tempos):
        if i:
            pos += Fraction(t - tempos[i - 1][0]) * tempos[i - 1][1] / 60000
        scroll.append((t * MS, float(pos), float(bpm) / 60))

    pcm, sounds = rtc.bank(clips, 44100)
    last = max(t for (t, ty, pa, va) in events)
    end = max([e[0] for e in auto] + [n[1] for n in notes] + [last * MS])
    title = f"IIDX {a.id:05d} {song['title_ascii'] or song['title']} {a.chart} lane {key}"
    chart = dict(title=title, rate=44100, channels=2, pcm=pcm, sounds=sounds, auto=auto, note_sounds=note_sounds,
                 lanes=[notes], scroll=scroll, reference_bpm=float(tempos[0][1]), end=end)
    out = pathlib.Path(a.out or HERE.parent / "content" / f"iidx_{a.id:05d}_{a.chart.lower()}_l{key}")
    out.mkdir(parents=True, exist_ok=True)
    data = rtc.encode(chart)
    assert rtc.encode(rtc.decode(data)) == data
    (out / "chart.rtc").write_bytes(data)
    (out / "names.json").write_text(json.dumps(order, ensure_ascii=False), encoding="utf-8")
    exp = [(t, s, g, "auto") for t, s, g in auto]
    for t, _, s0, n in notes:
        exp += [(t, s, g, "note") for s, g in note_sounds[s0:s0 + n]]
    exp.sort()
    (out / "expected.json").write_text(json.dumps({"rate": 44100, "events": exp}), encoding="utf-8")
    print(f"{out / 'chart.rtc'}: {len(data) / 1e6:.1f} MB, 묶음 {bank_name}{' (대체)' if fell_back else ''}, 소리 {len(sounds)}, "
          f"자동 {len(auto)}, 노트 {len(notes)} (롱 {sum(n[1] > n[0] for n in notes)}), 레인 {key} (레인별 {dict(sorted(per_lane.items()))}), "
          f"끝 {end / NS:.1f}초, BPM {[float(b) for _, b in tempos][:6]}, 기타 {dict(rep)}")


if __name__ == "__main__":
    main()
