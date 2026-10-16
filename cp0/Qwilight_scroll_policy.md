# Qwilight의 노트 노출 시간과 변속 처리

> 작성일 2026-10-16. 대상: 이 PC의 Qwilight 1.17.13 (`Qwilight.dll`, SHA-256 `b936e76a…0a06ac6e`). 짝 문서: `Qwilight_timing_policy.md`, `Qwilight_settings.md`.
> 표기: **(확인)** = 설치본 CIL·스킨 파일에서 직접 확인, **(추정)** = 정황으로 붙인 해석, **(계산)** = 확인한 식에 숫자를 넣은 값.
> 기호:
> - `BPMeff(t)`: 시각 t의 실효 BPM. BPM × SCROLL이고, STOP 중에는 0입니다.
> - `MV`: 스크롤 속도 설정값 `MultiplierValue`
> - `BPMstd`: 기준 BPM
> - `JMP`: 판정선의 화면 y 좌표
> - `Veil`: 레인 커버 높이(`VeilDrawingHeight`)

---

## 0. 결론

### 질문 1. 노트 노출 시간의 원리 (확인)

**배속과 논리 거리.**
- 실제 배속: `Multiplier = MV / (|BPMstd| × AudioMultiplier)` (`ModeComponent.get_Multiplier`)
- 논리 거리: 1 ms에 `BPMeff / 1250`(= 4/4 마디당 192단위)

**화면 y 공식.**

  `y(t) = JMP − Multiplier × D(t → 노트 시각) + Multiplier × NoteWait × (노트 시각의 BPMeff/1250)`

  여기서 `D(a → b) = ∫ₐᵇ BPMeff(τ)/1250 dτ`이고, τ는 곡 시계(ms) 기준입니다.

**보이는 시간.**
- Qwilight가 직접 계산해 "ms (frame IIDX)"로 보여 주는 값(`DefaultCompute.GetIIDXMultiplierMillis`):

  `노출 시간 = (JMP − Veil) / (Multiplier × AudioMultiplier × BPMstd/1250) = 1250 × (JMP − Veil) / MV  [ms]`

- 변속이 없으면(BPMeff = BPMstd) 실제 노출 시간과 정확히 같습니다.
- BPM이 BPMstd와 다른 구간에서는 `× BPMstd / BPMeff`가 됩니다(계산).

**기준 BPM** (`BPMStandardVarietyValueV2`, 기본 0).
- 0 = 가장 빈번한 BPM(재생 시간 가중, `Computing.BPM`)
- 1 = 시작 BPM(`#BPM`)
- 2 = 최저
- 3 = 최고
- 최저·최고·최빈 모두 BPM × SCROLL 값으로 셉니다.

**`#BPM`의 쓰임.** 시작 BPM(`LevyingBPM`)으로 쓰입니다. 첫 변화 전까지의 시각·스크롤 계산, 곡 시작 전 3000 ms 준비 구간의 높이, 기준 BPM 옵션 1에 들어갑니다.

### 질문 2. 레인 커버와 좌표 (확인)
- 좌표계는 스킨의 기본 화면(16:9 기준 1280 × 720)입니다. 레인은 화면 위(y = 0)에서 판정선 `JMP`까지입니다.
- **레인 커버는 위쪽 하나뿐입니다**(`VeilDrawingHeight`, 단위는 같은 화면 px, 기본 0). 아래쪽 커버(리프트)는 따로 없습니다.
- 판정선을 옮기는 설정은 스킨별 `JudgmentMainPosition1V2`("판정선 위치")입니다. 스킨 Lua에 `judgmentMainPositionV3`로 전달됩니다.
  - 기본 스킨: `JMP = 504 + V3`
  - CR_minimal: `JMP = 520 + V3`
  - 이 PC의 CR_minimal 값은 200이라 `JMP = 720`(화면 맨 아래)입니다.
- 이 설정은 **그리는 위치만** 옮기고 판정 시각은 바꾸지 않습니다. 판정은 `LoopingCounter − note.Wait`로 하기 때문입니다. 다만 레인 길이가 바뀌므로 노출 시간이 바뀝니다.
- `NoteWait`("노트 레이턴시")는 노트 y에 `NoteWait × 노트 BPMeff/1250 × Multiplier`를 더합니다. 즉 "노트를 NoteWait ms만큼 늦게 그리는 효과"이고, 변속 구간에서는 노트마다 이동량이 다릅니다.

### 질문 3. 변속 처리 (확인)
- **위치**: 스크롤 위치는 BPM을 시간에 대해 적분합니다(`Utility.GetDistance`). 노트의 위치는 마디 위치에서 정해집니다(`192 × 마디 길이 × SCROLL × sign(BPM)` 단위).
- **BPM 변화**: 시각에 따라 이동 속도가 바뀝니다.
  - 음수 BPM은 `LogicalYMeter = 192 × sign(BPM)`, `MillisMeter = 240000/|BPM|`이라 시간은 앞으로 가고 **스크롤은 거꾸로** 갑니다.
- **#STOP**: 런타임 BPM 표에 "그 시각 BPM 0 → `240000/|BPM| × stop/192` ms 뒤 원래 BPM"으로 넣습니다. 그래서 정지 중 스크롤이 완전히 멈춥니다. 음수 STOP은 "NegativeStopFault"로 거부합니다.
- **#SCROLL (`#SCROLLxx` + 채널 `SC`)**: 지원합니다. 그 시각부터 다음 SCROLL 전까지 BPM 항목에 곱합니다(누적이 아니라 구간 값). 노트 위치에도 곱해집니다.
- **#SPEED (채널 `SP`)**: 인식하지 않습니다. 해석기에 문자열이 없습니다.
- **BMSON**: `bpm_events`, `stop_events`(`duration`은 펄스, `/ (resolution × 4)` 마디), `info.init_bpm`만 읽습니다. 스크롤 이벤트 필드는 없습니다.
- **BPM 고정(`BPMModeValue = 1`)**:
  - BPM 표(STOP·SCROLL 포함)를 통째로 비웁니다.
  - 시작 BPM을 "가장 빈번한 BPM"으로 바꾸고, 모든 노트 위치를 `시각 × BPM/1250`으로 다시 계산합니다.
  - 그 결과 스크롤이 시간에 정비례합니다. 판정 시각(`Wait`)은 그대로입니다.

### 질문 4. Aleph-0 (레벨 19, `[clover]LeaF_Aleph0\aery hard.bms`) (계산)
- **변속 수단**:
  - BPM 변화 171회: `#BPMxx` 정의 149개, 0.2441 ~ 65535.9999. 그중 256 ~ 400 사이 145개는 단계적 가속입니다.
  - **STOP 2,553회**: 그중 1/192 마디짜리 짧은 정지가 2,547회입니다.
  - 마디 길이 변경 276회
  - SCROLL·SPEED 없음
- **이 PC 설정에서의 노출 시간**: MV 1800, 기준 BPM 250, CR_minimal JMP 720으로 보면 다음과 같습니다.
  - BPM 250에서 **500 ms**, 400에서 312 ms, 35에서 3.6 s
  - 65535.9999에서는 1.9 ms(사실상 순간 이동)
  - STOP 중에는 정지
  - 화면에 보이는 마디 수는 BPM과 무관하게 항상 약 0.52마디(4/4 기준)입니다.

---

## 1. 범위와 방법

| 대상 | 방법 |
|---|---|
| 좌표·스크롤 | `Component.SetBPM`, `Utility.GetDistance`, `BaseNote.InitY/Move/GetY`, 채보 루프(`<Compute>b__1`)의 이동량 계산, `DefaultCompute.GetIIDXMultiplierMillis` |
| 변속 해석 | `BMSCompiler/<CompileImpl>d__21`(헤더·위치→시각 적분), `d__22`(채널 03/08/09/SC 처리), `BMSONCompiler/<CompileImpl>d__8`, `BaseCompiler/<PostCompile>d__45`(대표·최저·최고 BPM), `d__47`(BPM·STOP·SCROLL 병합, `SetWaitLogicalYMap`, BPM 고정) |
| 기준 BPM | `Computing.get_BPMStandard` |
| 판정선 | `ComputedState.SetValue`(Lua 전역 전달), 기본 스킨 `Assets\UI\Default\Default.{yaml,lua}`, 사용자 스킨 `SavesDir\UI\CR_minimal\CR_minimal.{yaml,lua}`(읽기만) |
| 표본 | `D:\Games\BMS\BMS-AERY\[clover]LeaF_Aleph0\aery hard.bms`를 scratchpad의 해석 스크립트로 읽어 BPM·STOP·SCROLL 집계와 시각 적분(Qwilight와 같은 식) |

- 도구는 앞 문서들과 같습니다(rea 관리 코드 디코더로 푼 CIL).
- 정적 분석만 했고, 게임 실행·파일 수정은 하지 않았습니다.

---

## 2. 질문 1 — 노트 노출 시간의 원리

### 2-1. BPM 하나에 대한 단위 (확인: `Component.SetBPM`)

```csharp
LogicalYMeter  = 192 × sign(BPM);           // 4/4 마디 하나의 논리 길이
MillisMeter    = 240000 / |BPM|;            // 4/4 마디 하나의 시간 (ms)
LogicalYMillis = LogicalYMeter / MillisMeter = BPM / 1250;   // 1 ms당 논리 거리 (계산)
LogicalYLoopUnit = MillisLoopUnit × LogicalYMillis;
```

### 2-2. 노트 위치 공식 (확인)

1. **컴파일**: BMS는 위치(마디)마다 논리 Y를 쌓고(`_bmsPositionLogicalYMap`), 노트에 다음 값을 줍니다.
   - `LogicalY = LevyingHeight + 위치 논리 Y`
   - `LevyingHeight = 720 − LogicalYMillis(#BPM) × 3000` (`Component..ctor`)
   - 위치 논리 Y는 `−Σ Δ위치 × 마디 길이 × SCROLL × 192·sign(BPM)`입니다(`d__21` `IL_15ac`~). STOP은 위치가 아니라 시간만 더하므로 논리 Y에는 영향이 없습니다.
2. **곡 시작**: `LoopingCounter = −3000` ms에서 출발합니다(`<Compute>b__1` `IL_0812`). 누적 이동량 `loc11 = 0`입니다.
3. **매 반복**: `distance = GetDistance(BPM 표, 직전 LoopingCounter, 현재 LoopingCounter)`입니다. 이 값은 그 사이 BPM 변화마다 `LogicalYMillis(구간 BPM) × Δt`를 더한 것입니다(BPM 적분).
   - 노트가 판정 5초 전 범위에 들어오면 `_y = LogicalY − loc11`로 초기화합니다(`InitY`).
   - 그 뒤 반복마다 `_y += distance`, `loc11 −= distance`입니다(`Move`).
4. **그리기**: `y = (_y + NoteWait × note.LogicalYMillis − 720) × Multiplier + JMP` (`BaseNote.GetY`).

1 ~ 3을 합치면 `_y(t) = 720 − D(t → note.Wait)`가 됩니다(계산: −3000 ~ 0 ms 구간 거리가 `LogicalYMillis(#BPM) × 3000`이라 `LevyingHeight` 항과 상쇄). 따라서:

```text
y(t) = JMP − Multiplier × D(t → 노트 시각) + Multiplier × NoteWait × BPMeff(노트)/1250
D(a → b) = ∫ₐᵇ BPMeff(τ)/1250 dτ        (τ: 곡 시계 ms, STOP 구간은 BPMeff = 0)
```

- y는 아래로 갈수록 커집니다. 노트는 판정선보다 `Multiplier × D` 위에 그려집니다. 업스크롤(`IsPUMPMode`)에서는 방향이 바뀝니다(추정: 그리기에서 뒤집음, 식은 미추적).

### 2-3. 배속 (확인: `ModeComponent.get_Multiplier`)

```text
Multiplier = MV / (|BPMstd| × AudioMultiplier)      (MV = MultiplierValue, 기본 1000)
화면 속도(px/실제 ms) = Multiplier × BPMeff/1250 × AudioMultiplier = MV × BPMeff / (|BPMstd| × 1250)
```

- 곡 시계는 실제 시간의 `AudioMultiplier`배로 흐릅니다. 그래서 음악 속도를 바꿔도 BPMstd 구간의 화면 속도는 `MV/1250` px/ms로 일정합니다. MV는 "BPM × 배속"에 해당하는 목표값입니다. (계산)

### 2-4. 기준 BPM (확인: `Computing.get_BPMStandard`, `PostCompile d__45`)

| `BPMStandardVarietyValueV2` | 화면 이름 | 값 |
|---|---|---|
| **0 (기본)** | 가장 빈번한 BPM | `Computing.BPM`: (BPM × SCROLL) 값별로 재생 시간을 합해 가장 긴 값. 동률·비정상 값(100001 초과) 처리 분기가 있음(세부는 추정) |
| 1 | 시작 BPM | `LevyingBPM` = BMS `#BPM` / BMSON `init_bpm` |
| 2 | 가장 낮은 BPM | `min(PositionMultipliedBPMMap 값, #BPM)` |
| 3 | 가장 높은 BPM | `max(…)` |

`Qwilight_settings.md` 5-5에는 이 값을 "0 시작, 1 최빈, 2 최고, 3 최저"로 적었는데 틀렸습니다. 이 표가 맞습니다(`get_BPMStandard`의 switch 순서: 0 → `BPM`, 1 → `LevyingBPM`, 2 → `LowestBPM`, 3 → `HighestBPM`).

### 2-5. 노출 시간 (확인 + 계산)

Qwilight가 게임 모드 창에 보여 주는 값은 `DefaultCompute.GetIIDXMultiplierMillis`이고, 표시 형식은 "x ms (y frame IIDX)"이고, 식 안의 상수 6과 10으로 보아 y = x × 6/10입니다(추정).

```text
노출 시간 = max(0, (JMP − Veil) / (Multiplier × AudioMultiplier × BPMstd/1250))
         = 1250 × (JMP − Veil) / MV      [ms, 기준 BPM 구간]
실제 노출 시간(BPM이 b로 일정한 구간) = 1250 × (JMP − Veil) / MV × BPMstd / b   (계산)
변속 구간: 노트가 화면에 들어오는 시각 t_in은 D(t_in → 노트 시각) = (JMP − Veil)/Multiplier 를 만족하는 시각   (계산)
```

| 예 (Veil 0) | JMP | MV | 노출 시간(기준 BPM) |
|---|---|---|---|
| 기본 스킨, V3 = 0 | 504 | 1000(기본) | 630 ms |
| 기본 스킨, V3 = 0 | 504 | 1800 | 350 ms |
| CR_minimal, V3 = 200 (이 PC) | 720 | 1800 (이 PC) | **500 ms** |

(계산)

---

## 3. 질문 2 — 레인 커버와 좌표

### 3-1. 좌표계와 레인 (확인)
- 스킨 Lua의 `DefaultHeight(1.0)`은 16:9에서 720(16:10은 800)이고, `DefaultLength(1.0)`은 1280입니다(`Default.lua`). 창 크기에 맞춰 늘립니다.
- 레인 영역(`mainHeight`)은 기본 스킨에서 `JudgmentMainPosition(0)`, CR_minimal에서 `JMP()`입니다. 즉 **y = 0에서 판정선까지**입니다.
- 노출 시간 식도 `JMP − Veil`만 쓰므로 레인 위 끝은 0입니다.

### 3-2. 판정선 위치 (확인)

| 스킨 | 정의 | 이 PC의 V3 | JMP |
|---|---|---|---|
| 기본(`Default.lua`) | `JudgmentMainPosition(e) = DefaultHeight(1.0) − 76 − 20 − 120 + judgmentMainPositionV3 + e` | 0 | 504 (16:9) |
| CR_minimal(`CR_minimal.lua`) | `JMP() = 520 + judgmentMainPositionV3` | 200 | **720** |
| CR_minimal_DJMAX | 같은 Lua(사본) | 200 | 720 |

- `judgmentMainPositionV3`는 스킨별 설정 `UIConfigure.JudgmentMainPosition1V2`("판정선 위치")의 값입니다. Qwilight는 이 값을 Lua 전역 세 가지로 넘깁니다(`ComputedState.SetValue`).
  - `judgmentMainPosition` = 값 + 600
  - `judgmentMainPositionV2` = 1 + 값/600
  - `judgmentMainPositionV3` = 값
- **판정 시각은 바뀌지 않습니다**. 판정은 `InputNote.Judge`의 `(LoopingCounter − Wait)`이고, 판정선 y는 `GetY`의 더하기 항일 뿐입니다. 판정선을 내리면(값 증가) 레인이 길어지고, 같은 MV에서 노출 시간이 늘어납니다.

### 3-3. 레인 커버 (확인)

| 설정 | 단위 | 기본 | 동작 |
|---|---|---|---|
| `VeilDrawingHeight` "레인 커버 높이" | 화면 px(위에서부터) | 0 | 노출 길이에서 빠짐(`GetIIDXMultiplierMillis`) |
| `FlowVeilDrawing` "레인 커버 슬라이드" | bool | true | 플레이 중 커버를 움직이거나 접을 수 있게 함(`MovableValue` 애니메이션) |
| `VeilDrawingFileItems` | 이미지 | — | 커버 그림 |

아래쪽 커버(리프트) 설정은 없습니다. 판정선 위치가 그 역할을 겸합니다.

### 3-4. `NoteWait`와의 관계 (확인 + 계산)
- `GetY`에 `NoteWait × note.LogicalYMillis × Multiplier`가 더해집니다.
  - `note.LogicalYMillis`는 노트 시각 직전의 실효 BPM/1250입니다. SCROLL이 곱해지고, STOP 중이면 0입니다(`PostCompile SetLogicalYMillis|20`).
- 따라서 `NoteWait`는 "노트를 NoteWait ms 전의 위치에 그린다"(양수면 아래로 내려 그림)는 뜻입니다. 판정선 위치를 옮기는 것과 비슷하지만 노트마다 BPM에 비례합니다.
  - BPM 400 노트는 250 노트보다 1.6배 더 움직입니다.
  - STOP 시점 노트는 거의 움직이지 않습니다.

---

## 4. 질문 3 — 변속 처리

### 4-1. 해석 단계: 위치 → 시각 (확인: `BMSCompiler d__21` `IL_157c` ~ `IL_168e`)

모든 이벤트 위치 p(마디 단위, 오름차순)에 대해 다음을 차례로 합니다.

```csharp
logicalY -= (p − prevP) × scroll × meter[(int)prevP] × LogicalYMeter(현재 BPM);   // 위치 논리 Y
wait     += MillisMeter(현재 BPM) × meter[(int)prevP] × (p − prevP);              // 시각 (ms)
if (BPM 변화 at p)  { SetBPM(bpm); PositionMultipliedBPMMap[p] = bpm; }
if (STOP at p)      { wait += MillisMeter × stop / 192; }                           // 정지 시간만 더함
if (SCROLL at p)    { scroll = s; PositionMultipliedBPMMap[p] = BPM × s; }
```

| 수단 | 채널·정의 | 처리 |
|---|---|---|
| BPM(정수) | 채널 03 (16진) | `PositionBPMMap[p] = hex` |
| BPM(확장) | 채널 08 + `#BPMxx`(`#EXBPMxx`는 문자열 없음) | `bmsIDBPMMap` 조회 |
| STOP | 채널 09 + `#STOPxx` | 1/192 마디(4/4) 단위. **음수면 예외 `NegativeStopFault`** |
| SCROLL | 채널 `SC` + `#SCROLLxx` | 배율 |
| SPEED | — | **미지원**(문자열 없음) |
| 마디 길이 | 채널 02 | 시각·논리 Y 모두에 곱함 |
| 시작 BPM | `#BPM` | `LevyingBPM` |

BMSON(`BMSONCompiler d__8`):
- `bpm_events[{y, bpm}]`, `stop_events[{y, duration}]`(같은 y는 합산), `info.init_bpm`, `info.resolution`(마디 = resolution × 4 펄스)를 읽습니다.
- `scroll_events` 필드는 클래스에 없습니다.
- 시각 = Σ `MillisMeter × Δy / res`, 정지 = `MillisMeter × duration / res`입니다.

### 4-2. 런타임 BPM 표 만들기 (확인: `PostCompile d__47` `IL_0fb1` ~ `IL_125e`)

1. 해석기의 `WaitBPMMap`(시각 → BPM)을 복사합니다.
2. **STOP**: 각 (시각 w, 길이 s 마디)에 대해 다음을 넣습니다.
   - `w` 직전의 BPM을 b라 하면 `표[w] = 0`, `표[w + MillisMeter(b) × s] = b`
   - 즉 정지 = BPM 0 구간이고, `LogicalYMillis(0) = 0`이라 이동량이 0입니다.
3. **SCROLL**: `WaitMultiplierMap`을 **내림차순**으로 돌며(비교자 `(a, b) => b.CompareTo(a)`) 다음을 합니다.
   - `표[w] = w 직전 BPM`
   - `w ≤ 키 < 다음 SCROLL 시각`인 모든 항목에 배율을 곱합니다(`b__31`).
   - 그래서 SCROLL 값은 구간마다 독립된 배율이고, 겹쳐 곱해지지 않습니다.
4. `SetWaitLogicalYMap`: −3000 ms부터 곡 길이 + 2000 ms까지 1 ms 간격(그리고 노트·소리·BGA 시각)으로 시각 → 논리 Y 표를 만듭니다.
   - `Y(t) = Y(직전) − GetDistance(직전 → t)`
   - 이 표는 역재생·BPM 고정 등으로 노트 위치를 다시 매길 때 씁니다.

### 4-3. 스크롤의 성질 (확인 + 계산)

| 성질 | 결과 |
|---|---|
| 위치와 BPM | 노트 사이 화면 간격 = `Multiplier × 192 × Δ마디 × 마디 길이 × SCROLL`. **BPM과 무관**하고, BPM은 그 간격을 지나가는 속도만 바꿉니다 |
| BPM 적분 | 이동량 = Σ `BPMeff/1250 × Δt` (`GetDistance`). 루프 반복(기본 1 ms, 이 PC 0.125 ms)마다 적분 |
| STOP 중 | BPMeff = 0 → 화면 정지. 노트 판정 시각에는 정지 시간이 이미 포함됨 |
| 음수 BPM | `sign(BPM)`으로 논리 거리가 음수 → 역스크롤. 시간은 `|BPM|`로 앞으로 감. 기준 BPM에는 `|·|` 적용 |
| BPM 0 (채보에 직접) | `MillisMeter = ∞` → 위치 → 시각 적분이 무한대가 됨(추정: 사실상 금지, 별도 검사는 미확인) |
| 극단 BPM | 그대로 적용. 65535 BPM은 기준 250에 비해 262배 빠름 |

### 4-4. BPM 고정 `BPMModeValue = 1` (확인: `PostCompile` `IL_21a9` ~ `IL_2248`)

```csharp
WaitBPMMap.Clear();                         // BPM 변화, STOP의 0, SCROLL 배율 모두 사라짐
LevyingBPM = Computing.BPM;                 // 가장 빈번한 BPM
ComponentValue = new Component(LevyingBPM, …);
foreach (note) {
    note.LogicalY   = LevyingHeight − note.Wait × LogicalYMillis;   // 시각에 정비례
    note.LongHeight = note.LongWait × LogicalYMillis;
}
```

- 노트 판정 시각(`Wait`)과 소리 시각은 그대로입니다. 화면 위치만 "시간 × 일정 속도"가 됩니다. 그래서 STOP 구간에도 노트가 계속 내려오고, SCROLL·역스크롤도 사라집니다.
- 역방향 플레이(`WaveMode`)는 BPM 표를 `Length − t`로 뒤집은 뒤 `SetWaitLogicalYMap`으로 노트 위치를 다시 매깁니다(`IL_2443` ~).

---

## 5. 질문 4 — Aleph-0 (레벨 19)

### 5-1. 파일 (확인)
- 경로: `D:\Games\BMS\BMS-AERY\[clover]LeaF_Aleph0\aery hard.bms`
- 헤더: `#TITLE Aleph-0`, `#SUBTITLE [5KEY AERY]`, `#PLAYLEVEL 19`, `#BPM 250`, `#RANK 3`, `#TOTAL 600`, `#LNTYPE 1`
- 같은 폴더의 `_aleph[5knosv].bms`는 `[5KEY AERY No SV]`, 레벨 17로, 변속을 뺀 판입니다.

### 5-2. 변속 수단 집계 (계산: scratchpad 해석 스크립트, Qwilight와 같은 적분식)

| 항목 | 값 |
|---|---|
| 노트 오브젝트(채널 1x/5x) | 1,537 |
| 마디 | 332 (0 ~ 331), 마디 길이 변경 276개 (0.03125 ~ 2.0) |
| BPM 변화 | **171회** (채널 03: 16, 채널 08: 155). `#BPMxx` 정의 149개 |
| BPM 범위 | **0.2441 ~ 65535.9999**. 정의 중 145개가 256 ~ 400 사이(1씩 오르는 단계적 가속), 극단값 0.2441·0.52·65535.9999 |
| 1000 이상 / 1 미만 | 7회 / 3회 |
| STOP | **2,553회** (`#STOPxx` 정의 7개). 1/192 마디 정지가 BPM 400 구간에서 1,536회(각 3.1 ms), BPM 250 구간에서 1,011회(각 5.0 ms). 긴 정지 6회: 445 ~ 7,680 ms |
| 정지 합계 | 약 21.8 s (곡 약 141 s 중) |
| SCROLL / SPEED | 없음 / 없음 |
| BPMeff별 재생 시간 상위 | 250: 62.1 s, 정지(0): 21.8 s, 35: 13.7 s, 0.98: 7.7 s, 0.24: 5.6 s, 400: 1.2 s |

### 5-3. Qwilight가 그리는 모습 (계산)

전제는 이 PC 설정입니다: MV 1800, 기준 BPM 옵션 0(가장 빈번한 BPM = 250. 시작 BPM도 250이라 옵션 0과 1이 같음), 5+1K 스킨 CR_minimal(JMP 720), Veil 0.

| 구간 | BPMeff | 화면 속도 (px/ms) | 레인(720 px)을 지나는 시간 |
|---|---|---|---|
| 기본 구간 | 250 | 1.44 | **500 ms** |
| 가속 끝 | 400 | 2.30 | 312 ms |
| 감속 | 35 | 0.20 | 3.6 s |
| 거의 정지 | 0.52 / 0.2441 | 0.003 / 0.0014 | 240 s / 512 s (화면상 정지) |
| 순간 이동 | 65535.9999 | 377 | 1.9 ms (한두 반복 만에 통과) |
| STOP | 0 | 0 | 정지 |

- `Multiplier = 1800 / 250 = 7.2`이므로 4/4 한 마디 = 192 × 7.2 = 1,382 px입니다. **화면에 보이는 마디 수는 BPM과 무관하게 720/1382 ≈ 0.52마디**이고, BPM은 그 0.52마디가 지나가는 속도만 정합니다.
- 1/192 마디 정지가 노트 사이사이에 수천 번 들어 있습니다. 그래서 BPM 250·400 구간의 스크롤은 "움직임 ↔ 3 ~ 5 ms 정지"를 반복하는 끊김 연출이 됩니다.
  - 루프 간격(0.125 ms) 단위로 적분하므로 이 짧은 정지도 그대로 반영됩니다.
  - 화면은 그리기 프레임(240 fps 상한이면 4.2 ms 간격)으로 샘플링되므로, 실제로는 미세한 떨림으로 보일 것입니다(추정).
- 65535.9999 구간은 노트 묶음이 한 반복 사이에 판정선 근처까지 순간 이동합니다. 0.24 ~ 0.52 구간은 노트가 화면에 멈춘 것처럼 보이다가 다음 BPM 변화에서 다시 움직입니다.
- 기준 BPM 옵션을 3(최고)으로 두면 `Multiplier = 1800/65536`이 되어, 250 구간 노출 시간이 131 s(사실상 노트가 거의 안 움직임)가 됩니다. 옵션 2(최저)면 0.49 ms가 됩니다. 이 곡에서는 0이나 1이 정상입니다. (계산)
- BPM 고정을 켜면 모든 STOP·BPM 연출이 사라집니다. 노트가 시간에 정비례해 BPM 250 속도로 내려오고, 판정 시각은 그대로입니다.

---

## 6. 확인하지 못한 것

| 항목 | 상태 |
|---|---|
| 업스크롤(`IsPUMPMode`)에서 y를 뒤집는 정확한 식 | 그리기 쪽 미추적 |
| 노트가 그려지는 범위 판정(`BaseNote.IsVisible`) | 추상 메서드라 하위 클래스별 구현 미확인. 레인 위 끝을 0으로 본 것은 노출 시간 식과 스킨 `mainHeight`에 근거 |
| "가장 빈번한 BPM"의 동률·100001 초과 값 처리 세부 | 분기만 확인, 의미는 추정 |
| `Computing.BPM`이 Aleph-0에서 실제로 250인지 | 같은 방식의 계산으로 250이 압도적(62 s)이라 추정 |
| 채보에 BPM 0이 직접 쓰였을 때의 처리 | 미확인(무한대 시간) |
| `#EXBPMxx`, BMSON `scroll_events`·`speed_events` | 해석기에 문자열·필드가 없어 미지원으로 판단(확인), 다른 이름으로 처리하는 경로는 없다고 봄(추정) |
| 실제 화면 모습 | 정적 분석이라 관찰하지 않음 |
