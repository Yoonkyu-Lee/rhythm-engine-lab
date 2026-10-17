# Track 형식 명세 v1

> 작성일 2026-10-17. CP3 3-0 산출물. 엔진의 곡 객체 **Track**의 작성 형식(텍스트), 컴파일 규칙, 실행 형식(바이너리), 묶음 파일 배치를 구현할 수 있는 수준으로 정한다.
> 근거와 결정 과정: `README.md`(2·2-1·3·3-1·4장), `BMS_컨버터.md`(1-2, 2-4), `../design/곡 데이터 형식.md`(구상). 이 문서와 그 문서들이 어긋나면 **이 문서가 맞다**(구상 문서의 "음악 레이어를 작성"·"묶이지 않은 이벤트 = 자동재생"은 화이트리스트 결정으로 바뀜).
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
| N10 | 해시: 내용 식별은 **BLAKE3-256**, 소리 동일성은 디코딩 PCM의 BLAKE3 | 빠르고 충돌 걱정 없음. CP2의 FNV-1a 64는 실행 데이터 비교용으로만 남음 |
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
start = "0"                         # 시작 보정 (원본 레이트의 샘플 수, 기본 0). 예: WMA 변환분 3
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
points = [ { at = "0", pos = "0", speed = "1" } ]   # 이 위치부터 속도(기준 박/박). 0 = 정지, 음수 = 역스크롤, pos 불연속 = 순간 이동

[[event]]                           # 자동재생: 차분이 저절로 내는 소리 (노트에 묶이지 않은 것)
at = "0"
sound = "bgm_a"
gain = "1"                          # 선택

[[note]]
at = "1920"
lane = "K3"
kind = "tap"                        # tap | long | mine
sounds = ["piano_c", "piano_e"]     # 이 노트를 치면 울릴 소리 (0개 이상). 화음 = 여러 개
# sounds = [{ sound = "arp_1", at = "+0" }, { sound = "arp_2", at = "+1/16" }, { sound = "x", gain = "0.8" }]

[[note]]
at = "2304"
lane = "L1"
kind = "long"
end = "2688"
sounds = ["synth_long"]

[[lane_sound]]                      # 빈 레인 소리 바꾸기: 이 위치부터 이 레인을 빈 손으로 치면 이 소리 (BMS 투명 노트, IIDX 소리 교체)
at = "1800"
lane = "K3"
sound = "piano_c"

[[variant]]                         # #RANDOM 대체
id = "r1"
choices = ["a", "b"]                # 균등 선택 (선택: weights = ["1","1"])
# 이벤트·노트·lane_sound에 when = "r1=a" 를 달면 그 선택일 때만 있음
```

**배속 변형 차분** (BMS `time_scale`, BMS_컨버터 1-1):

```toml
id = "7k-another-x1.05"
name = "ANOTHER (x1.05)"
layout = "7+1"
base = "7k-another"                 # 같은 Track의 다른 차분
time_scale = "21/20"                # 템포 배율. 이 차분 시각 = base 시각 ÷ time_scale. 소리 길이는 바꾸지 않음
shift = "0"
# 이벤트·노트를 적지 않는다. reference_scroll을 적지 않으면 base 값 × time_scale
```

### 2-4. 작성 형식 규칙

- **화이트리스트**: 차분이 내는 소리는 그 차분의 `event`·`note.sounds`·`lane_sound`에 있는 것뿐이다. 다른 차분을 참조하지 않는다(배속 변형의 `base`만 예외).
- 같은 레인의 노트는 시간순이고 겹치지 않는다(롱노트 끝 이후에 다음 노트).
- `at`·`end`는 그 차분의 `unit`. `shift`는 항상 ms.
- 차분 해시에 들어가는 것과 아닌 것은 3-4.

---

## 3. 컴파일 규칙

### 3-1. 시각 계산 (차분 자기 시각)

1. 위치 → 박: `beat`는 그대로(`마디:박`은 meter의 마디 길이 합 + 박), `tick`은 tick ÷ ticks_per_beat.
2. 박 → 초: tempo 구간마다 `Δ박 × 60 / bpm`을 **유리수로** 누적. stop은 그 위치에서 `beats × 60 / 그때 bpm`초를 더한다. bpm 0·음수는 시간 계산에 쓰지 않는다(시간이 멈추거나 거꾸로 가지 않음 — BMS 음수 BPM 역스크롤은 `[scroll]`로 표현, 컨버터가 적음).
3. `unit = "ms"`는 ms ÷ 1000 = 초(tempo는 시간 계산에 안 쓰고 스크롤·마디선용).
4. 배속 변형: base의 초 ÷ time_scale.
5. 초 → ns: × 10⁹ 을 **한 번** 짝수 쪽 반올림 → i64.
6. Track 시각(레이어 계산용) = 자기 시각 + shift(ms → ns, 같은 방식으로 한 번 반올림한 값을 더함).

### 3-2. 스크롤 지도
- `[scroll]`이 없으면 tempo를 그대로 쓴다: **위치 = 박 수**, 속도 = 그 구간의 bpm ÷ 60 (박/초), stop은 속도 0 구간. 실행 형식 ScrollPoint = (시각 ns, 위치 박, 속도 박/초).
- 화면 속도는 엔진이 `reference_scroll`(박/분)과 사용자의 노출 시간으로 정한다(README 3-1). 차분 데이터에는 들어가지 않는다.
- `[scroll]`이 있으면 그것만 쓴다(컨버터가 IIDX 정수 반올림 BPM, DJMAX 곡 정보 BPM 비례, BMS SCROLL·음수 BPM을 계산해 적음, README 3-1).

### 3-3. 검사 (실패하면 컴파일 오류)
- 없는 소리·레인·레이아웃·차분 참조, 같은 레인 노트 겹침, 롱노트 `end ≤ at`, `time_scale ≤ 0`, 배속 변형의 base가 또 배속 변형.
- 소리 파일이 엔진 디코더 셋 밖이거나 디코딩 실패, 샘플 수 0.
- 이득이 음수·유한하지 않음.
- 경고(보고서): 차분 사이에 같은 (Track 시각, 소리)가 아닌 이벤트 수(README 2장의 "모든 난이도 음악 같음" 검사), 1 ms 안에서 어긋난 같은 소리.

### 3-4. 해시
| 해시 | 대상 | 넣지 않는 것 |
|---|---|---|
| **소리 해시** | 기준 디코더로 디코딩한 PCM(원본 레이트·채널, `start` 적용 전) + 샘플레이트 + 채널 | 파일 바이트, 컨테이너 |
| **차분 해시** | 실행 형식 차분 구역의 정규 인코딩: 레이아웃(레인 id·역할·묶음), 규칙, 자기 시각의 모든 이벤트·노트·lane_sound·변형, 스크롤 지도, 마디선, reference_scroll. 소리는 번호 대신 **소리 해시 + gain + start + retrigger + choke** | **shift**, 이름·레벨, Track 정보, 다른 차분 |
| **Track 묶음 해시** | 묶음 파일 전체(머리의 해시 칸 제외) | — |

- 기록·리플레이·랭킹은 **차분 해시**에 묶인다. 이동을 고치거나 편입·Track 합치기를 해도 차분 해시는 그대로다.
- 리플레이 안의 시각은 차분 자기 시각(ns).

### 3-5. 음악 레이어 (계산만, 작성하지 않음)
- 모든 차분(배속 변형은 펼친 뒤)의 소리 발음을 Track 시각으로 모은 다중집합에서, 같은 (Track 시각 ns, 소리 해시)를 하나로 센 것.
- 쓰는 곳: 변환 보고서, 차분 사이 비교, 선곡 미리보기에서 차분을 바꿀 때 위치 대응. 플레이에는 쓰지 않는다(차분은 자기 배열만 읽는다).

### 3-6. 컴파일러가 계산해 담는 것
- 소리 표(코덱·레이트·채널·샘플 수·데이터 위치), Vorbis 설정 머리 공유 표, 차분별 노트 수·레인별 밀도·보이스 예산(최대 동시 발음 + 여유), 소리별 첫 사용 시각(디코딩 순서, README 4-3), 차분별 구간 통계(250 ms 칸)·하이라이트(작성자 지정 우선), 소리 메모리 형식(16비트 / f32 판단 재료: 원본이 16비트 정수인지).

---

## 4. 실행 형식 (차분 구역)

모든 정수는 little-endian. 시각은 **차분 자기 시각** i64 ns. 배열은 시간순(같은 시각이면 작성 순서).

| 표 | 레코드 | 크기 |
|---|---|---|
| 머리 | 레이아웃 번호 u16, 레인 수 u16, 규칙(long_note u8, empty_lane u8), 변형 수 u16, shift i64(ns), reference_scroll f64(박/분), end i64, 보이스 예산 u32, 각 표의 위치·개수 | 고정 |
| `auto` | `{ t: i64, sound: u32, gain: f32, when: u32 }` | 24 B |
| `note` (레인마다 한 배열) | `{ t: i64, end: i64, sounds_start: u32, sounds_len: u16, kind: u8, _pad: u8, when: u32, _pad: u32 }` | 32 B |
| `note_sound` | `{ offset: i64 (노트 시각 기준 ns), sound: u32, gain: f32 }` | 16 B |
| `lane_sound` (레인마다) | `{ t: i64, sound: u32, when: u32 }` | 16 B |
| `scroll` | `{ t: i64, pos: f64 (박), speed: f64 (박/초) }` | 24 B |
| `bar` | `{ t: i64 }` | 8 B |
| `variant` | `{ choices: u16, _pad: u16, weights_start: u32 }` + 가중치 표 | — |

- `when`: 변형 조건 비트(0 = 항상). 로딩 때 선택된 변형으로 걸러 `when`이 없는 배열을 만든다(엔진 루프에는 조건 검사가 없다).
- CP2 `.rtc`와의 차이: 노트에 `kind`·`when`, 노트 소리에 `offset`(아르페지오), 빈 레인 소리 표, 마디선, 변형. 소리 뱅크(PCM)는 차분 밖(소리 구역)으로 나간다.

---

## 5. 묶음 파일

### 5-1. 머리와 구역 색인
```
offset 0   magic "TRK\0" (4) | format major u16 | minor u16 | 구역 수 u32 | 머리 크기 u32
           Track ID (16, UUID) | revision u32 | 컴파일러 버전 (32, UTF-8 0 채움)
           묶음 해시 (32, BLAKE3 — 이 칸은 0으로 두고 계산)
그 뒤      구역 색인 × n: { kind u32, flags u32, offset u64, length u64, hash [32] }
```
- 구역은 64바이트 정렬, `SOUND_DATA`는 4 KiB 정렬.
- 읽는 쪽은 아는 구역만 읽고 모르는 구역은 건너뛴다(부 버전이 오르며 구역이 늘 수 있음). 주 버전이 다르면 읽지 않는다.

### 5-2. 구역

| kind | 내용 | 읽는 때 |
|---|---|---|
| `INFO` | 정보·태그·하이라이트·출처 요약 (작은 고정 레코드 + 문자열 표) | 색인 갱신 |
| `LAYOUTS` | 레이아웃 표 | 곡 로딩 |
| `CHARTS` | 차분 목록: id·이름·레벨·차분 해시·차분 구역 위치 | 곡 로딩 |
| `CHART` × n | 4장 | 고른 차분만 |
| `SOUNDS` | 소리 표: `{ 소리 해시 [32], codec u8, channels u8, 16bit_source u8, retrigger u8, rate u32, frames u64, start u32, gain f32, choke u32, data_offset u64, data_len u64, codec_table u32 }` | 곡 로딩 |
| `CODEC_TABLES` | Vorbis 설정 머리(곡 안 공유), 그 밖 코덱 표 | 곡 로딩 |
| `SOUND_DATA` | Vorbis 패킷열 · FLAC 프레임열 · MS-ADPCM 블록열, 빈틈없이. 같은 소리 해시는 데이터 한 번(소리 ID는 따로, README 4-1) | 곡 로딩 (한 번에 읽고 병렬 디코딩) |
| `GRAPHICS` | 그래픽 참조 표 (`[[media]]`) | 곡 로딩 |
| `STATS` | 차분별 구간 통계·하이라이트 (색인 갱신용 사본) | 색인 갱신 |
| `SOURCE` | 작성 형식 텍스트(`track.toml`, `charts/*.toml`, `report.md`)를 경로·길이·바이트로 이어 붙임 | 플레이 때 안 읽음. 풀기 도구가 오디오는 `SOUND_DATA`에서 되살림 |

### 5-3. 라이브러리 쪽 경로 (README 2-1·4장)
```
<library>/
├── index.bin            라이브러리 색인 (곡마다 INFO·STATS 사본 + 묶음 경로·해시)
├── tracks/<Track ID>.trk
├── media/<blake3 hex>.<원래 확장자>     미디어 저장소 (영상), 내용 해시가 이름
├── art/                 선곡 화면 자원 묶음 (썸네일·곡 그림), 형식은 CP4
└── decisions/           BMS 편입 판단 기록 (BMS_컨버터 2-3)
```

---

## 6. 버전 규칙
- `format` 주 버전이 오르면 옛 묶음은 다시 컴파일한다(작성 원본이 묶음에 있으므로 가능).
- 부 버전은 구역·필드 추가만. 옛 읽는 쪽은 모르는 구역을 건너뛴다.
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
