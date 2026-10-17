# Track 형식 명세 v1.5

> 작성일 2026-10-17. CP3 3-0 산출물. 엔진의 곡 객체 **Track**의 작성 형식(텍스트), 컴파일 규칙, 실행 형식(바이너리), 묶음 파일 배치를 구현할 수 있는 수준으로 정한다.
> 근거와 결정 과정: `README.md`(2·2-1·3·3-1·4장), `BMS_컨버터.md`(1-2, 2-4), `../design/곡 데이터 형식.md`(구상). 이 문서와 그 문서들이 어긋나면 **이 문서가 맞다**(구상 문서의 "음악 레이어를 작성"·"묶이지 않은 이벤트 = 자동재생"은 화이트리스트 결정으로 바뀜).
> v1.1 (2026-10-17): 합성 예시 Track(`example/synth_track/`)을 쓰며 막힌 곳 G1~G11을 고침 — 목록은 `example/synth_track/report.md`.
> v1.2 (2026-10-17): Codex 3-0 리뷰(`reviews/2026-10-17_codex_3-0.md`) 반영 — 차분 안 소리 표로 논리 ID 보존(해시), 노트 소리 offset 반올림 규칙과 정규 발음 순서·프레임 변환(3-4), STOP + 명시 스크롤, `damage`, 바이트 단위 레코드·표 디렉터리·stride 확장, 정규 해시 바이트열, 코덱 구역·트림, 변형 조건 AND.
> v1.4 (2026-10-17): Codex 3-1 리뷰(`reviews/2026-10-17_codex_3-1.md`) 반영 — 모든 BPM 양수, STOP + 스크롤 적분 규칙, Vorbis 소리 해시를 보존 패킷으로(플랫폼 무관), 머리에 컴파일 캐시 열쇠, 읽는 쪽 의미 검사, 자원 상한, Ogg·ADPCM 검증, unpack 경로 규칙.
> v1.3 (2026-10-17): 3-1 컴파일러 구현으로 확정 — 차분 안 소리 레코드 패딩(52 → 56 B로 명세의 합산 오류 수정), CHART 밖 구역 바이트 배치(5-5), Vorbis EOS 트림·MS-ADPCM 반올림 규칙, 라이브러리 색인 형식.
> v1.5 (2026-10-17): Codex 3-2 리뷰(`reviews/2026-10-17_codex_3-2.md`)와 컨버터 셋 검증으로 확정 — 소리 `length`(BMSON 이어 재생 조각), 차분 `losses`(불완전 변환 표시)와 `LOSSES` 구역, Vorbis 앞 자르기 없음(libsndfile·ffmpeg와 같게), 음악 레이어 열쇠에 length. 부 버전 변경(옛 읽는 쪽은 새 칸·구역을 무시).
> 이름 규칙: 엔진 임시 이름(Unison)은 문서에서만 쓴다. 코드·필드·매직 넘버·확장자에는 쓰지 않는다. "Track"은 곡 객체의 이름이라 코드에 써도 된다.

---

## 0. 이 문서에서 새로 정한 것 (검토용)

README·컨버터 문서의 결정을 옮기면서 구현에 필요해 새로 정한 세부다.

| # | 정한 것 | 이유 |
|---|---|---|
| N1 | **실행 형식의 시각은 차분 자기 시각**(시간 이동을 적용하지 않음). 이동은 차분 머리에 따로 둔다 | 차분 해시가 이동을 빼고 계산되고(결정), 기록·리플레이가 차분 자기 시각(결정)이므로 그대로 맞물린다. Track 시각은 레이어 계산·차분 사이 위치 대응에만 쓴다 |
| N2 | 위치 표기는 차분마다 단위 하나: `beat`(박, `마디:박` 허용) / `ms` / `tick` | 원작 시각을 그대로 적기 위해(IIDX ms, DJMAX tick, BMS 마디) |
| N3 | 모든 수는 **문자열 유리수**(`"3"`, `"1.5"`, `"7/3"`) | 부동소수점 오차 없이 원본 보존 |
| N4 | ns 반올림은 한 번, **짝수 쪽 반올림**(round half to even) | 구상 문서 5-2의 정책을 그대로 |
| N5 | 소리 ID·차분 ID·레이아웃 ID는 **작성 형식에서 문자열**, 실행 형식에서 번호 | 편입·편집에 번호 밀림이 없게 |
| N6 | 노트는 소리를 직접 가진다(`sounds = [...]`), 자동재생은 `[[event]]` | 순수 화이트리스트: 차분이 자기 소리를 전부 적는다 |
| N7 | BMS 투명 노트는 **빈 레인 소리 바꾸기**로 옮긴다 | 소리만 바꾸고 판정이 없는 동작과 같다 |
| N8 | `#RANDOM`은 `[[variant]]` + 이벤트의 `when` 조건. 실행 형식은 조건 표시를 달고 로딩 때 거른다 | 조합 폭발 없이, 선택은 리플레이에 기록 |
| N9 | 롱노트 규칙 `ln`(끝 판정 없음) · `cn`(떼기 판정) · `hcn`(⚑ CP3 미구현) | BMS LN과 IIDX CN을 명시 규칙으로 |
| N10 | 해시: 내용 식별은 **BLAKE3-256**, 소리 동일성은 디코딩 PCM의 BLAKE3(Vorbis는 보존 패킷의 BLAKE3 — 디코더 f32가 플랫폼마다 다를 수 있어서, v1.4), 차분 해시는 `CHART` 구역 바이트(shift 0) | 빠르고 충돌 걱정 없음. CP2의 FNV-1a 64는 실행 데이터 비교용으로만 남음 |
| N11 | Track ID = Track을 처음 만들 때 만드는 **UUIDv7**, 이후 바뀌지 않음 | 편입으로 내용이 바뀌어도 같은 곡 |
| N12 | 묶음 파일 매직 `TRK\0`, 구역 64바이트 정렬, 소리 데이터 구역 4 KiB 정렬, 확장자는 임시로 `.trk` (⚑ 정식 이름 뒤) | 구역 단위 읽기·메모리 매핑 대비 |
| N13 | 묶음 안 작성 원본은 **텍스트만** 담는다. 오디오는 소리 데이터 구역에서 되살린다 | 오디오를 두 번 담지 않음. 되살린 오디오는 디코딩 해시가 같다(바이트는 다를 수 있음, 4-1 보존 계약대로) |

---

## 1. 전체 그림

```
작성 형식 (폴더, 텍스트 + 오디오)          실행 형식 (묶음 파일 하나)
<track>/                                  <track-id>.trk
├── track.toml   정보·레이아웃·소리 표       ├── 머리 + 구역 색인
├── charts/                                ├── INFO, LAYOUTS
│   ├── sc.toml  차분 (자기 이벤트 전부)     ├── CHART × n   (차분 자기 시각)
│   └── ...                                ├── SOUNDS, CODEC_TABLES, SOUND_DATA
├── audio/       Ogg Vorbis · FLAC · ADPCM ├── GRAPHICS
└── report.md    변환 보고서 (컨버터가 씀)   └── SOURCE  (작성 형식 텍스트, 플레이 때 안 읽음)
        │                                          ▲
        └──────────── 컴파일러 (버전 고정) ─────────┘
```

- 컨버터는 **작성 형식만** 만든다. 실행 형식은 컴파일러만 만든다.
- 같은 작성 형식 + 같은 컴파일러 버전 → **비트 단위로 같은 묶음**(3-1 완료 기준).

---

## 2. 작성 형식

모든 텍스트는 UTF-8, TOML 1.0. 알 수 없는 키는 **오류**(조용히 무시하지 않음).

### 2-1. 수와 위치

| 종류 | 문법 | 예 |
|---|---|---|
| 유리수 | 정수, 소수, 분수(`a/b`), 앞에 `-` 허용. 항상 문자열 | `"190"`, `"179.94"`, `"-1/3"` |
| 위치 (`unit = "beat"`) | 박 수 유리수, 또는 `"마디:박"`(박은 유리수, 마디 길이는 `meter`로) | `"33/4"`, `"12:1.5"` |
| 위치 (`unit = "ms"`) | ms 유리수 | `"2526"`, `"2526.5"` |
| 위치 (`unit = "tick"`) | 정수 tick | `"18432"` |
| 상대 위치 | 앞에 `+` (같은 단위) | `"+1/8"` |

### 2-2. `track.toml`

```toml
format = "1.0"                       # 형식 버전 (주.부)
id = "0192b7e4-5a1c-7c3e-9f00-1a2b3c4d5e6f"   # UUIDv7, 처음 만들 때 한 번
revision = 3                         # 내용이 바뀔 때마다 올림 (곡 오프셋 수정, 편입 등)

[info]
title = "BLACK SWAN"
title_sort = "black swan"           # 정렬·검색용 읽는 법
artist = "..."
genre = "..."
tags = ["djmax", "pack:v-extension"]   # 여러 값
highlight = "61000"                 # 선택: 하이라이트 시작 (Track 시각 ms). 없으면 컴파일러가 계산
[info.lang.ja]                      # 선택: 언어별 표기
title = "..."

[origin]                            # Track 시각 원점 (README 2장 결정)
anchor_chart = "8b-sc"              # 이 차분의 자기 시각 0 = Track 시각 0. 처음 만들 때 정하고 다시 바꾸지 않음

[[layout]]                          # 레이아웃 (차분이 id로 고름)
id = "8B"
lanes = [
  { id = "SL", role = "side" },
  { id = "K1", role = "key" }, { id = "K2", role = "key" }, { id = "K3", role = "key" },
  { id = "L1", role = "trigger" },
  { id = "R1", role = "trigger" },
  { id = "K4", role = "key" }, { id = "K5", role = "key" }, { id = "K6", role = "key" },
  { id = "SR", role = "side" },
]
groups = [{ id = "main", lanes = ["K1","K2","K3","K4","K5","K6"] }]
# 공간을 공유하는 레인(사이드처럼 화면에서 겹쳐 보이는 것)의 표기는 CP4 화면 배치에서

[[sound]]
id = "kick"                         # 문자열, Track 안에서 유일, 바뀌지 않음
file = "audio/kick.ogg"
gain = "1"                          # 선형 이득 (기본 1). ±1 넘침을 줄여 담은 소리는 여기서 되돌림
start = "0"                         # 시작 보정 = 앞에서 버릴 샘플 수 (원본 레이트, 기본 0). 예: WMA 변환분 3 (G8)
length = "22050"                    # 선택 (v1.5): start 뒤로 이 샘플 수만 낸다(1 이상, start + length ≤ 길이). 없으면 끝까지. BMSON 이어 재생 조각
retrigger = "overlap"               # 같은 소리 다시 울림: overlap(겹침, 기본) | cut(이전 것을 끊음)
choke = ""                          # 선택: 같은 그룹의 새 소리가 나면 이전 소리를 끊음

[[media]]                           # 그래픽 참조 (2-1). 실체는 Track 밖
hash = "blake3:9f2c..."             # 미디어 저장소의 파일 이름
kind = "video"                      # video | image_pack | thumbnail | cover
role = "bga"
start = "0"                         # 곡 시각(Track 시각 ms) 기준 시작 위치. 영상만

[source]                            # 출처 (변환으로 만들었을 때)
format = "djmax"                    # djmax | iidx | bms | bmson | manual
converter = "djmax2track 0.1.0"
files = [{ path = "...", hash = "blake3:..." }]
```

- 소리 파일 형식은 4-1의 엔진 디코더 셋만: **Ogg Vorbis, FLAC, MS-ADPCM WAV**. 그 밖은 컨버터가 바꿔 둔다(손실 경로는 보고서에 기록).
- 코덱·샘플레이트·채널·샘플 수는 적지 않는다(파일에서 읽는다). 컴파일러가 소리 표에 채운다.

### 2-3. 차분 `charts/<id>.toml`

```toml
id = "8b-sc"                        # 차분 ID, Track 안에서 유일, 바뀌지 않음 (파일 이름과 같음)
name = "SC"
level = "13"
layout = "8B"
shift = "0"                         # Track 시각 = 차분 자기 시각 + shift (ms 유리수). 편입 때 계산. 음수 허용
reference_scroll = "174"            # 기준 스크롤 속도 (박/분). 노출 시간 설정의 기준
losses = []                         # 선택 (v1.5): 변환에서 원작과 달라진 점("롱노트 안 노트 → 소리만 자동재생: 2" 식). 있으면 불완전 변환 차분. 배속 변형은 base 것을 물려받음

[rules]
long_note = "cn"                    # ln | cn | hcn(⚑)
empty_lane = "previous_note"        # 빈 레인을 치면: previous_note | next_note | none

[timing]
unit = "tick"                       # beat | ms | tick
ticks_per_beat = 192                # unit = tick일 때
tempo = [ { at = "0", bpm = "116" }, { at = "18432", bpm = "174" } ]
meter = [ { measure = 0, length = "4" } ]   # 마디 길이(박). unit = beat의 "마디:박"과 마디선에 씀
stops = [ ]                         # { at = 위치, beats = 유리수 } : 그 위치에서 박 수만큼 시간이 멈춤 (BMS STOP)
bars = [ ]                          # 선택: 마디선을 직접 적음 (IIDX처럼 원작이 따로 가진 경우). 없으면 meter로 계산

[scroll]                            # 선택. 없으면 tempo를 그대로 적분(1박 = 위치 1)
points = [ { at = "0", speed = "120" } ]   # 이 위치부터 속도(박/분 = bpm과 같은 단위, G1). 0 = 정지, 음수 = 역스크롤
# pos(박)는 선택: 없으면 앞 점에서 이어지는 위치, 적으면 그 위치로 순간 이동 (G2). 첫 점은 at = 0, pos 기본 0

[[event]]                           # 자동재생: 차분이 저절로 내는 소리 (노트에 묶이지 않은 것)
at = "0"
sound = "bgm_a"
gain = "1"                          # 선택

[[note]]
at = "1920"
lane = "K3"
kind = "tap"                        # tap | long | mine (mine은 선택 damage = 게이지 % 유리수 0~100, 없으면 규칙 기본 ⚑ — CP3 기본: BAD와 같은 감소, G4)
sounds = ["piano_c", "piano_e"]     # 이 노트를 치면 울릴 소리 (0개 이상). 화음 = 여러 개
# sounds = [{ sound = "arp_1", at = "+0" }, { sound = "arp_2", at = "+1/16" }, { sound = "x", gain = "0.8" }]

[[note]]
at = "2304"
lane = "L1"
kind = "long"
end = "2688"
sounds = ["synth_long"]

[[lane_sound]]                      # 빈 레인 소리 바꾸기: 이 위치부터 이 레인을 빈 손으로 치면 이 소리 (BMS 투명 노트, IIDX 소리 교체)
                                    # 유효: 그 레인의 다음 노트(판정 뒤에는 그 노트 소리) 또는 다음 lane_sound까지 (G5)
at = "1800"
lane = "K3"
sound = "piano_c"

[[variant]]                         # #RANDOM 대체
id = "r1"
choices = ["a", "b"]                # 균등 선택 (선택: weights = ["1","1"])
# 이벤트·노트·lane_sound에 when = "r1=a" (또는 ["r1=a", "r2=b"] = 모두 참일 때) 를 달면 그 선택일 때만 있음
```

**배속 변형 차분** (BMS `time_scale`, BMS_컨버터 1-1):

```toml
id = "7k-another-x1.05"
name = "ANOTHER (x1.05)"
level = "12"
base = "7k-another"                 # 같은 Track의 다른 차분 (배속 변형이 아닌 것)
time_scale = "21/20"                # 템포 배율 (> 0). 이 차분 시각 = base 시각 ÷ time_scale. 소리 길이는 바꾸지 않음
# 물려받음(G6): base의 레이아웃·규칙·시간 지도·스크롤·이벤트·노트·빈 레인 소리·변형. 이것들(layout 포함)을 적으면 오류.
# 적는 것: id·name·level·base·time_scale. 선택: shift(기본 base.shift ÷ time_scale), reference_scroll(기본 base × time_scale)
# 스크롤: base 스크롤 점의 시각 ÷ time_scale, 속도 × time_scale (위치는 그대로)
```

### 2-4. 작성 형식 규칙

- **화이트리스트**: 차분이 내는 소리는 그 차분의 `event`·`note.sounds`·`lane_sound`에 있는 것뿐이다. 다른 차분을 참조하지 않는다(배속 변형의 `base`만 예외).
- 같은 레인의 노트는 시간순이고 겹치지 않는다(롱노트 끝 이후에 다음 노트). 겹침은 **변형 선택 조합마다**, 그리고 **ns로 반올림한 뒤에도** 검사한다 — 같은 자리의 변형 대안 노트는 겹침이 아님 (G3).
- `when`: 문자열 `"변형id=선택"` 하나, 또는 그런 문자열의 목록(**모두 참일 때만**, AND). `event`·`note`·`lane_sound`에만 단다.
  - BMS `#RANDOM`이 시간 지도나 스크롤(BPM·STOP·SCROLL)을 바꾸는 경우는 `when`으로 표현하지 않는다. 컨버터가 **선택 조합마다 차분을 따로 만든다**(`<id>~r1=a` 식, 조합이 16개를 넘으면 변환하지 않고 보고서에 "미지원"으로 남김).
- `unit`이 beat·tick이면 tempo는 위치 0에서 시작해야 한다 (G10). 0보다 앞의 위치(음수)는 첫 tempo를 뒤로 늘여 계산한다.
- `at`·`end`는 그 차분의 `unit`. `shift`는 항상 ms.
- ID(소리·차분·레이아웃·레인·묶음·변형·choke)는 각자 범위 안에서 유일해야 하고, 알 수 없는 키는 어디서든 오류.
- 차분 해시에 들어가는 것과 아닌 것은 3-5.

---

## 3. 컴파일 규칙

### 3-1. 시각 계산 (차분 자기 시각)

1. 위치 → 박: `beat`는 그대로(`마디:박`은 meter의 마디 길이 합 + 박), `tick`은 tick ÷ ticks_per_beat.
2. 박 → 초: tempo 구간마다 `Δ박 × 60 / bpm`을 **유리수로** 누적. stop은 그 위치에서 `beats × 60 / 그때 bpm`초를 더한다. **정지 위치와 같은 위치의 이벤트는 정지 전**에 울리고, 그 뒤 이벤트부터 밀린다 (BMS 동작, G9). **모든 bpm은 양수여야 한다**(0·음수는 컴파일 오류 — BMS의 0 BPM은 컨버터가 STOP으로, 음수 BPM 역스크롤은 `[scroll]`로 옮긴다, v1.4). 같은 위치의 정지는 박 수를 더해 하나로 합친다.
3. `unit = "ms"`는 ms ÷ 1000 = 초(tempo는 시간 계산에 안 쓰고 스크롤·마디선용).
4. 배속 변형: base의 초 ÷ time_scale.
5. 초 → ns: × 10⁹ 을 **한 번** 짝수 쪽 반올림 → i64. 결과가 i64 범위를 벗어나면 오류.
6. **노트 소리의 offset**: 각 소리의 절대 자기 시각(노트 위치 + 상대 위치)을 5번으로 **한 번** 반올림한 ns에서, 노트의 반올림된 ns를 뺀 정수. (`round(소리) − round(노트)`. `round(소리 − 노트)`가 아님 — 그래야 같은 소리를 자동재생으로 적었을 때와 ns가 같다.)
7. Track 시각(레이어 계산용) = 자기 시각 ns + shift ns(shift ms를 5번처럼 한 번 반올림).

### 3-2. 스크롤 지도
- `[scroll]`이 없으면 tempo를 그대로 쓴다: **위치 = 박 수**, 속도 = 그 구간의 bpm ÷ 60 (박/초). 실행 형식 ScrollPoint = (시각 ns, 위치 박, 속도 박/초).
- `[scroll]`이 있으면 그 점들을 쓴다: 작성 형식의 speed(박/분) ÷ 60 = 실행 형식 박/초, pos 없는 점은 앞 점에서 이어 계산. 컨버터가 IIDX 정수 반올림 BPM, DJMAX 곡 정보 BPM 비례, BMS SCROLL·음수 BPM을 계산해 적는다(README 3-1).
- **STOP은 어느 경우든 컴파일러가 스크롤에 정지 구간을 넣는다**: 정지 시작 시각에 (위치 그대로, 속도 0), 정지 끝 시각에 (같은 위치, 그때의 속도) 점을 끼운다. 명시 스크롤의 점들은 박 위치로 시각을 정하므로 정지 뒤 점은 이미 밀려 있다. (STOP + 명시 스크롤을 같이 쓸 수 있게 — 리뷰 3-0 #3.)
- **위치 적분 (v1.4, 리뷰 3-1 #6)**: `pos`가 없는 점의 위치 = 앞 기준점 위치 + 속도 × **이동 시간**(정지 구간을 뺀 시간). 점과 정지 시작·끝을 한 시간순 목록으로 처리하고, 같은 시각이면 정지 끝 → 점 → 정지 시작 순. 같은 시각에 점이 여럿 나오면 마지막 것만 남긴다(위치는 같다). `[scroll]`의 `points`는 비어 있으면 오류.
- 화면 속도는 엔진이 `reference_scroll`(박/분)과 사용자의 노출 시간으로 정한다(README 3-1). 차분 데이터의 위치·속도에는 들어가지 않는다.

### 3-3. 검사 (실패하면 컴파일 오류)
- 알 수 없는 키, 중복 ID, 자료형 오류.
- 없는 소리·레인·레이아웃·차분·변형·변형 선택·choke 참조(`event`·`note`·`lane_sound`·`when` 모두).
- 같은 레인 노트 겹침(변형 조합마다, ns 반올림 뒤), 롱노트 `end_ns ≤ at_ns`, `time_scale ≤ 0`, 배속 변형의 base가 또 배속 변형, 배속 변형에 물려받는 키를 적음.
- 소리 파일이 엔진 디코더 셋(Ogg Vorbis, FLAC, MS-ADPCM WAV) 밖이거나 디코딩 실패, 샘플 수 0.
- 이득이 음수이거나 f32로 바꾸면 유한하지 않음, `damage`가 0~100 밖, Track id가 UUIDv7(ASCII 16진수, 하이픈 위치, 버전 7, RFC 변형) 형식이 아님.
- 자원 상한 (v1.4): 차분 자기 시각 ±24시간, 마디선 1,000,000개, 변형 65,535개·선택지 65,535개, 레인 64개. 같은 레인 겹침은 변형 조합을 펼치지 않고 **두 노트의 조건이 동시에 참일 수 있는지**로 판단한다.
- 경고(보고서): 차분 사이에 같지 않은 레이어 이벤트 수(README 2장의 "모든 난이도 음악 같음" 검사), 1 ms 안에서 어긋난 같은 소리.

### 3-4. 동시 발음 순서와 프레임 변환 (PERFECT 키음 = 반주)
- **정규 발음 순서**: 차분의 모든 소리 시작을 (ns, 종류, 순번)으로 정렬한다. 종류: 자동재생 0, 노트 소리 1. 순번: 자동재생은 작성 순서, 노트 소리는 레인 선언 순서 → 그 노트의 `sounds` 순서. CP2 믹서(같은 프레임이면 자동재생 먼저)와 같은 규칙이다.
- 같은 소리 끊기(`retrigger = "cut"`)·choke는 이 순서로 판단한다. 정답 렌더도 이 순서로 섞는다(덧셈 순서 고정).
- **프레임 변환은 최종 ns 하나에 한 번**: 자동재생은 `원점 + ns_to_frame(t)`, 플레이어 키음은 노트 프레임과 offset 프레임을 따로 반올림해 더하지 않고 **(도장 시각 + D + offset) ns 하나를 한 번** 프레임으로 바꾼다(CP2 `ns_to_frame`, 반 프레임 이하 대칭 반올림). 그래서 PERFECT(도장 = 노트 시각)일 때 아르페지오 소리도 같은 ns의 자동재생과 같은 프레임에 놓인다.

### 3-5. 해시 (모두 BLAKE3-256)
| 해시 | 계산 | 넣지 않는 것 |
|---|---|---|
| **소리 해시** | 정수 코덱(FLAC·MS-ADPCM): `"SND1"` ‖ rate u32 ‖ channels u16 ‖ frames u64 ‖ 기준 디코더 출력 PCM(i32 LE, 채널 끼움, 코덱 트림 적용 후·`start` 적용 전). **Vorbis (v1.4)**: `"SNDV"` ‖ rate u32 ‖ channels u16 ‖ frames u64 ‖ skip_front u32 ‖ 코덱 표 길이 u64 ‖ 코덱 표 ‖ 데이터 — 보존한 패킷 자체. Vorbis 디코더 출력은 f32 계산이 플랫폼마다 다를 수 있어(lewton은 표를 실행 중 sin/cos로 만든다) 해시에 넣지 않는다(리뷰 3-1 #11) | 파일 바이트, 컨테이너, 주석 |
| **차분 해시** | 실행 형식 `CHART` 구역의 바이트 전체, 단 **shift 칸을 0으로 두고** 계산(4장). 구역 안 소리는 **차분 안 소리 표**(차분이 쓰는 논리 소리마다 한 줄: 소리 해시·start·gain·retrigger·choke·length)의 번호로 참조한다. 같은 내용의 다른 논리 ID는 다른 줄이라 구분이 남는다(리뷰 3-0 #1). | shift, 이름·레벨(`CHARTS` 구역에 있음), Track 정보, 다른 차분, Track 전역 소리 번호 |
| **Track 묶음 해시** | 묶음 파일 전체, 머리의 묶음 해시 칸을 0으로 두고 계산 | — |

- 차분 안 소리 표의 순서 = 3-4 정규 발음 순서에서 처음 나오는 순서(그다음 `lane_sound`에만 쓰인 소리를 레인 선언·시각 순으로). 그래서 같은 작성 내용이면 Track에 다른 소리가 더해져도(편입) 바이트가 같다.
- 컴파일러는 결정적이므로 "구역 바이트 = 정규 인코딩"이다. 바이트를 바꾸는 변경(필드 추가 포함)은 6장 버전 규칙을 따른다.
- **기준 디코더** = 엔진 내장 디코더. 이름·버전을 `SOUNDS` 구역 머리에 기록한다. 같은 입력에서 PCM이 달라지는 디코더 교체는 주 버전 변경이다.
- 기록·리플레이·랭킹은 **차분 해시**에 묶인다. 이동을 고치거나 편입·Track 합치기를 해도 차분 해시는 그대로다. 리플레이 안의 시각은 차분 자기 시각(ns).

### 3-6. 음악 레이어 (계산만, 작성하지 않음)
- 모든 차분(배속 변형은 펼친 뒤)의 소리 시작을 Track 시각으로 모아, 같은 (Track 시각 ns, 소리 해시, start·length(v1.5), 최종 이득)을 하나로 센다. 최종 이득 = 소리 표 gain × 이벤트(또는 노트 소리) gain, 유리수로 비교 (G7).
- 차분 사이 공유 수는 **차분 ID 집합**으로 센다. 변형이 있는 차분은 선택 조합별 비교와 전체 합집합을 따로 낸다.
- 쓰는 곳: 변환 보고서, 차분 사이 비교, 선곡 미리보기에서 차분을 바꿀 때 위치 대응. 플레이에는 쓰지 않는다.

### 3-7. 컴파일러가 계산해 담는 것
- 소리 표(코덱·레이트·채널·샘플 수·코덱 트림·데이터 위치), 코덱 표, 차분별 노트 수·레인별 밀도·보이스 예산(최대 동시 발음 + 여유), 소리별 첫 사용 시각(디코딩 순서, README 4-3), 차분별 구간 통계(250 ms 칸)·하이라이트(작성자 지정 우선), 소리 메모리 형식 판단 재료(디코딩 결과가 16비트 정수인지).

---

## 4. 실행 형식 (`CHART` 구역)

- 모든 수는 little-endian. 패딩·예약 칸은 **0**. 표는 구역 시작 기준 오프셋, 8바이트 정렬. 시각은 **차분 자기 시각** i64 ns.
- 머리 64 B + 표 디렉터리(표마다 12 B: `offset u32, count u32, stride u32`). 읽는 쪽은 **stride로 레코드를 건너뛴다**(부 버전이 레코드 끝에 필드를 더해도 옛 읽는 쪽이 읽음).

**머리 (64 B)**
| 오프셋 | 필드 |
|---|---|
| 0 | magic `"CHRT"` |
| 4 | minor u16, header_size u16 (= 64), table_count u16, _pad u16 |
| 12 | _pad u32 |
| 16 | shift i64 (ns) — **차분 해시 계산 때 0** |
| 24 | end i64 (마지막 이벤트·노트 끝) |
| 32 | reference_scroll f64 (박/분) |
| 40 | lanes u16, long_note u8 (0 ln, 1 cn, 2 hcn), empty_lane u8 (0 previous_note, 1 next_note, 2 none), voice_budget u32 |
| 48 | _pad 16 B |

**표 (디렉터리 순서 고정)**
| # | 표 | 레코드 | stride |
|---|---|---|---|
| 0 | 차분 안 소리 | `{ sound_hash [32], start u32, gain f32, retrigger u8 (0 overlap, 1 cut), _pad [3], choke u32 (0 없음, 그 밖 = 차분 안 choke 번호 + 1), length u32 (0 = 끝까지, v1.5 — 옛 예약 칸), _pad [4] }` | 56 |
| 1 | 레인 | `{ id_off u32, id_len u32 (문자열 표), role u8 (0 key, 1 scratch, 2 side, 3 trigger, 4 pedal, 255 기타), _pad [3], note_start u32, note_count u32, lsnd_start u32, lsnd_count u32, _pad u32 }` | 32 |
| 2 | 묶음 | `{ id_off u32, id_len u32, lanes u64 (레인 비트, 64레인까지) }` | 16 |
| 3 | 자동재생 | `{ t i64, sound u32, gain f32, cond u32, _pad u32 }` — (t, 작성 순서) 정렬 | 24 |
| 4 | 노트 (레인 표의 구간) | `{ t i64, end i64, ns_start u32, ns_len u16, kind u8 (0 tap, 1 long, 2 mine), _pad u8, cond u32, damage i32 (게이지 %의 1/1000, −1 = 규칙 기본) }` — 레인마다 t 정렬 | 32 |
| 5 | 노트 소리 | `{ offset i64 (3-1의 6번), sound u32, gain f32 }` — 노트의 `sounds` 순서 | 16 |
| 6 | 빈 레인 소리 (레인 표의 구간) | `{ t i64, sound u32, cond u32 }` | 16 |
| 7 | 스크롤 | `{ t i64, pos f64 (박), speed f64 (박/초) }` — STOP 구간 포함(3-2) | 24 |
| 8 | 마디선 | `{ t i64 }` | 8 |
| 9 | 변형 | `{ id_off u32, id_len u32, choices u16, _pad u16, weight_start u32 }` | 16 |
| 10 | 가중치 | `{ w u32 }` (선택지마다, 기본 1) | 4 |
| 11 | 조건 | `{ term_start u32, term_count u32 }` — 0번은 빈 조건(항상) | 8 |
| 12 | 조건 항 | `{ variant u16, choice u16 }` — 조건 안에서 variant 순 정렬, AND | 4 |
| 13 | 문자열 | UTF-8 바이트 | 1 |

- `cond`: 조건 표 번호. 로딩 때 선택된 변형으로 걸러 조건 없는 배열을 만든다(엔진 루프에는 조건 검사가 없다). 선택은 리플레이에 기록.
- 소리 참조(`sound`)는 모두 표 0(차분 안 소리) 번호. 차분 안 번호 → Track 소리 번호 대응은 `CHARTS` 구역에 둔다(해시 밖).
- 실수 칸(gain, pos, speed, reference_scroll)은 작성 형식의 유리수를 가장 가까운 f32/f64로(IEEE 짝수 반올림) 한 번 바꾼 값.
- **읽는 쪽 의미 검사 (v1.4, 리뷰 3-1 #2)**: 디렉터리의 표마다 stride ≥ 이 명세의 크기, offset + count × stride ≤ 구역 길이(넘침 검사). 소리·조건·노트 소리 번호가 범위 안, enum 값(종류·규칙·역할)이 정의 안, 묶음 비트가 레인 수 안, 실수가 유한하고 이득 ≥ 0, 시간순 정렬, 롱노트만 end > t, 조건 0번은 빈 조건. 엔진·도구는 이 검사를 통과한 차분만 쓴다(`pack::validate_chart`).
- CP2 `.rtc`와의 차이: 차분 안 소리 표, 레인·묶음, 노트 `kind`·`cond`·`damage`, 노트 소리 `offset`, 빈 레인 소리, 마디선, 변형·조건. 소리 PCM은 차분 밖(소리 구역).

---

## 5. 묶음 파일

### 5-1. 머리와 구역 색인
```
offset 0    magic "TRK\0" (4) | major u16 | minor u16 | section_count u32 | header_size u32 (= 128)
offset 16   Track ID [16] (UUID) | revision u32 | _pad u32
offset 40   compiler [32] (UTF-8, 0 채움)
offset 72   bundle_hash [32] (BLAKE3, 계산할 때 이 칸은 0) | source_key [24] (컴파일 캐시 열쇠, v1.4)
offset 128  구역 색인 × n: { kind u32, flags u32, offset u64, length u64, hash [32] (그 구역 바이트의 BLAKE3) } = 56 B
```
- 구역은 64바이트 정렬(사이는 0), `SOUND_DATA`는 4 KiB 정렬. 구역 순서 = 아래 표 순서.
- **컴파일 캐시 (v1.4)**: `source_key` = BLAKE3(`"TRKC"` ‖ 컴파일러 식별자 ‖ 기준 디코더 식별자 ‖ 입력 파일마다 (경로, 길이, 바이트))의 앞 24바이트. 입력 = `track.toml`, `charts/*.toml`(이름순), `report.md`(있으면), `track.toml`이 참조하는 소리 파일. 출력 묶음의 열쇠가 같으면 다시 컴파일하지 않는다(`trackc compile`, `--force`로 무시).
- 읽는 쪽은 색인·구역 범위를 넘침 검사 후 확인하고, 개수 필드는 할당 전에 남은 바이트로 검사한다.
- 읽는 쪽은 아는 kind만 읽고 모르는 kind는 건너뛴다. 주 버전이 다르면 읽지 않는다.

### 5-2. 구역

| kind | 이름 | 내용 | 읽는 때 |
|---|---|---|---|
| 1 | `INFO` | 정보·태그·하이라이트·출처 요약 (작은 고정 레코드 + 문자열 표) | 색인 갱신 |
| 2 | `LAYOUTS` | 레이아웃 표 (`track.toml`의 `[[layout]]`) | 곡 로딩 |
| 3 | `CHARTS` | 차분마다 `{ id, name, level, 차분 해시 [32], CHART 구역 번호, 차분 안 소리 → Track 소리 번호 배열, base 차분 번호(배속 변형이면), time_scale(분자 i64·분모 i64) }` | 곡 로딩 |
| 4 | `CHART` × n | 4장. 배속 변형도 **펼친 구역**을 따로 둔다(엔진은 펼침 계산을 하지 않음) | 고른 차분만 |
| 5 | `SOUND_IDS` | Track 논리 소리 표: `{ data u32 (SOUNDS 번호), start u32, gain f32, retrigger u8, _pad [3], choke u32, id_off u32, id_len u32 }` | 곡 로딩 |
| 6 | `SOUNDS` | 머리: 기준 디코더 이름·버전 문자열. 소리 데이터마다 `{ sound_hash [32], codec u8 (1 Vorbis, 2 FLAC, 3 MS-ADPCM), channels u8, int16 u8 (디코딩 결과가 16비트 정수면 1), _pad u8, rate u32, frames u64, skip_front u32, codec_table u32, data_offset u64, data_len u64 }` — 같은 소리 해시는 한 줄 | 곡 로딩 |
| 7 | `CODEC_TABLES` | Vorbis: identification 머리 + setup 머리 (comment 머리는 버림, 곡 안에서 같으면 한 번). FLAC: STREAMINFO 34 B. MS-ADPCM: WAVEFORMATEX(+ block_align, samples_per_block, 계수 표) | 곡 로딩 |
| 8 | `SOUND_DATA` | Vorbis: `{ packet_count u32, packet_len u32 × n }` + 패킷들. FLAC: 프레임열. MS-ADPCM: 블록열. 빈틈없이 | 곡 로딩 (한 번에 읽고 병렬 디코딩) |
| 9 | `GRAPHICS` | 그래픽 참조 표 (`[[media]]`) | 곡 로딩 |
| 10 | `STATS` | 차분별 구간 통계·하이라이트 (색인 갱신용 사본) | 색인 갱신 |
| 12 | `LOSSES` | v1.5, 손실이 있는 차분이 있을 때만: 차분마다 `{ id, 손실 문자열들 }` — 선택 화면의 "불완전 변환" 표시용 | 곡 목록·선택 |
| 11 | `SOURCE` | 작성 형식 텍스트(`track.toml`, `charts/*.toml`, `report.md`)를 `{ path_len u32, path, data_len u64, data }`로 이어 붙임. **경로 규칙 (v1.4)**: 상대 경로, 구성 요소는 일반 이름만(`..`·루트·드라이브·`:`·역슬래시 금지). 풀기 도구는 전부 검사한 뒤 쓰고, 실제 경로가 출력 폴더 밖이거나 링크면 거부 | 플레이 때 안 읽음 |

**코덱 트림과 길이** (원본 보존 계약, README 4-1):
- 디코딩 결과 = 디코더 출력에서 앞 `skip_front` 샘플을 버리고 `frames` 샘플만 취한 것. Ogg Vorbis는 원본 Ogg의 granule 위치(마지막 페이지의 총 길이)로 `frames`를 정한다(v1.5부터 `skip_front`는 늘 0, 칸은 유지). FLAC·MS-ADPCM은 `skip_front = 0`, `frames` = 원본 총 샘플 수(ADPCM 마지막 블록의 남는 샘플은 버림).
- 작성 형식의 `start`(시작 보정)는 그 위에 적용한다(재생할 때 앞에서 버림).
- 풀기 도구는 `SOURCE` 텍스트와 `SOUND_DATA`에서 작성 형식 폴더를 되살린다. 되살린 오디오는 **소리 해시가 같다**(바이트는 다를 수 있음: Ogg 페이지 구조·comment 머리는 다시 만듦).

### 5-3. 라이브러리 쪽 경로 (README 2-1·4장)
```
<library>/
├── index.bin            라이브러리 색인 (곡마다 INFO·STATS 사본 + 묶음 경로·해시)
├── tracks/<Track ID>.trk
├── media/<blake3 hex>.<원래 확장자>     미디어 저장소 (영상), 내용 해시가 이름
├── art/                 선곡 화면 자원 묶음 (썸네일·곡 그림), 형식은 CP4
└── decisions/           BMS 편입 판단 기록 (BMS_컨버터 2-3)
```

### 5-4. 시험 벡터
- 예시 Track(`example/synth_track/`)의 묶음 해시·구역 해시·차분 해시·소리 해시를 `example/golden/golden.txt`(`trackc info` 출력)에 둔다. `cargo test`(`crates/track/tests/golden.rs`)가 결정성(두 번 컴파일 = 같은 바이트)·정답 해시·차분 왕복을 확인한다. 형식이나 컴파일러를 의도적으로 바꾸면 golden.txt를 함께 갱신한다.

### 5-5. 구역 바이트 배치 (v1.3, 3-1 컴파일러 `cp3/crates/track/src/pack.rs`와 같음)

표기: `str` = u32 길이 + UTF-8, `none` 시각 = i64::MIN. 모든 수 little-endian.

| 구역 | 바이트 |
|---|---|
| `INFO` | str title, str title_sort, str artist, str genre, u32 n + str × n (tags), i64 highlight (Track 시각 ns), u32 n + (str 언어, str × 4: title·title_sort·artist·genre) × n, str source_format, str converter, u32 n + (str path, str hash) × n |
| `LAYOUTS` | u32 n, 레이아웃마다 { str id, u16 레인 수, (str id, u8 role) × 레인, u16 묶음 수, (str id, u64 레인 비트) × 묶음 } |
| `CHARTS` | u32 n, 차분마다 { str id, str name, str level, [32] 차분 해시, u32 CHART 구역 순번, u32 base (u32::MAX = 없음), i64 time_scale 분자, i64 분모, i64 shift ns, u32 m, u32 × m (차분 안 소리 → Track 소리 번호) } |
| `SOUND_IDS` | u32 n, 논리 소리마다 { u32 data(SOUNDS 번호), u32 start, f32 gain, u8 retrigger, [3] 0, u32 choke, str id }, u32 k + str × k (choke 이름) |
| `SOUNDS` | str 기준 디코더 이름·버전, u32 n, 소리 데이터마다 72 B { [32] 소리 해시, u8 codec, u8 channels, u8 int16, u8 0, u32 rate, u64 frames, u32 skip_front, u32 codec_table 번호, u64 data 오프셋(SOUND_DATA 기준), u64 data 길이 } |
| `CODEC_TABLES` | u32 n, (u32 길이 + 바이트) × n. 같은 바이트의 표는 한 번(곡 안 Vorbis 설정 머리 공유). Vorbis 표 = (u32 길이 + identification 머리) ‖ (u32 길이 + setup 머리) |
| `SOUND_DATA` | 소리 데이터를 SOUNDS 순서로 빈틈없이. Vorbis 데이터 = u32 패킷 수 n ‖ u32 × n (패킷 길이) ‖ 패킷들 |
| `GRAPHICS` | u32 n, (str hash, str kind, str role, i64 start) × n |
| `STATS` | u32 n, 차분마다 { str id, i64 칸 길이 ns (250 ms), u32 노트 수(지뢰 제외, 변형은 첫 선택지), u32 최대 NPS(이동 1초 창), i64 하이라이트(차분 자기 시각 ns: Track 하이라이트 − shift, 없으면 노트가 가장 많은 10초 창의 시작, 같으면 앞), u32 칸 수, u16 × 칸 } |
| `SOURCE` | u32 n, (str 상대 경로, u64 길이, 바이트) × n — `track.toml`, `charts/*.toml`(이름순), `report.md`(있으면) |
| `LOSSES` | u32 n, 차분마다 { str id, u32 m, str × m } (손실이 있는 차분만, 하나도 없으면 구역 자체가 없음) |

**코덱 세부 (기준 디코더)**
- **Vorbis 트림 (v1.5)**: **앞은 자르지 않는다**(`skip_front` = 0). 첫 오디오 페이지의 granule이 그 페이지까지 디코딩한 샘플 수보다 작은 스트림(명세 A.2의 앞 버림 표시)도 libsndfile·ffmpeg는 처음부터 내고 끝을 마지막 granule에 맞춘다 — BMS 실측 35개에서 둘 다 그랬고, 원작 재생기와 같은 소리가 기준이므로 따른다(v1.4까지는 앞을 버려 이런 키음이 약 10 ms 늦었다). 샘플 0의 granule = max(0, 첫 오디오 granule − 그 페이지까지 디코딩한 샘플 수)(중간부터 자른 스트림만 양수), 단 첫 오디오 페이지가 곧 마지막 페이지(EOS)면 0. `frames` = 마지막 granule − 샘플 0의 granule (디코딩 길이를 넘지 않게). 연결(chained)·다중 스트림 Ogg는 거부.
- **FLAC**: STREAMINFO의 총 샘플 수가 `frames`(0이면 디코딩 길이). 샘플은 원래 정수 값.
- **Ogg 검증 (v1.4)**: 버전 0, 페이지 순번 0부터 1씩, CRC, BOS는 첫 페이지만, 이어짐 표시와 패킷 상태 일치, EOS 뒤 데이터 없음. 되살리기는 패킷마다 페이지를 새로 시작하고 255 세그먼트를 넘는 패킷은 이어지는 페이지로 나눈다.
- **MS-ADPCM**: 예측값 = (s1·c1 + s2·c2) / 256 **0 쪽 버림**(Microsoft 명세, Windows ACM·ffmpeg와 같음), delta = max(16, ADAPT[n]·delta / 256), 블록 머리는 sample2 → sample1 순으로 출력, 니블은 높은 쪽 먼저(스테레오는 높은 = 왼쪽). `frames` = `fact` 청크 값(없으면 디코딩 길이) — 마지막 블록을 블록 끝까지 채운 디코더(ffmpeg·libsndfile)보다 짧을 수 있다. `fact`가 디코딩 길이보다 크면 손상으로 거부. 중간 연산은 64비트, delta는 [16, i32::MAX / 768]로 자른다(ffmpeg와 같은 상한, v1.4). fmt는 22바이트 이상, 계수 256개 이하.
- 3-1 대조(예시 Track 소리 10개): FLAC·MS-ADPCM은 libsndfile·ffmpeg와 비트까지 같음, Vorbis(lewton)는 libvorbis와 최대 차 3×10⁻⁷.

**라이브러리 색인 `index.bin`**: `"TIDX"` ‖ major u16 ‖ minor u16 ‖ u32 n ‖ Track마다 { [16] Track ID, u32 revision, [32] 묶음 해시, u64 묶음 크기, str 상대 경로(`tracks/…`), u32 길이 + INFO 구역 바이트, u32 길이 + STATS 구역 바이트 } — 경로 순.

---

## 6. 버전 규칙
- **부 버전**: 새 구역 kind 추가, 표 디렉터리 끝에 새 표 추가, 레코드 **끝**에 필드 추가(stride 증가)만. 옛 읽는 쪽은 모르는 구역·표·꼬리 필드를 건너뛴다. 검사 도구(`trackc verify`)는 해시·구조·의미 검사를 모든 부 버전에 하고, 읽고 다시 쓴 바이트 비교는 자기와 같은 부 버전에만 한다.
- **주 버전**: 그 밖의 모든 바이트 변경(필드 순서·의미·enum 값 변경, 해시 규칙 변경, PCM이 달라지는 기준 디코더 교체). 주 버전이 오르면 옛 묶음은 다시 컴파일한다(작성 원본이 묶음에 있으므로 가능). 차분 해시가 바뀌므로 기록은 (옛 해시 → 새 해시) 대응표로 잇는다.
- Track `revision`은 내용이 바뀔 때마다 오른다. 차분 해시가 같은 차분의 기록은 revision이 달라도 그대로 유효하다.

---

## 7. 3-0 합성 예시 Track이 덮어야 하는 칸 (README 7장 체크리스트 ↔ 이 문서)

| 체크리스트 | 이 문서 |
|---|---|
| 시각: 박+템포, ms, tick / 템포 변화·정지 / 스크롤 따로 / 기준 스크롤 | 2-1, 2-3 `[timing]`·`[scroll]`, 3-1, 3-2 |
| 레이아웃 둘 (8B 모양, 7+1 모양) | 2-2 `[[layout]]` |
| 레인별 롱노트, 소리 여럿 노트 | 2-3 `[[note]]` long·`sounds` |
| 차분 셋 이상 화이트리스트, 음수 shift, time_scale, #RANDOM | 2-3, `[[variant]]` |
| 소리 표: 시작 보정·이득·같은 내용 다른 ID | 2-2 `[[sound]]` start·gain·retrigger, 5-2 |
| 그래픽 참조·정보·태그·하이라이트 | 2-2 `[[media]]`·`[info]` |
| 빈 레인 소리 바꾸기 | 2-3 `[[lane_sound]]` |
