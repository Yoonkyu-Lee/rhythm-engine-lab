# Qwilight 게임 설정 전수 목록

> 작성일 2026-10-16. 대상: 이 PC에 설치된 Qwilight 1.17.13 (`D:\Games\Steam\steamapps\common\Qwilight\Qwilight.dll`, SHA-256 `b936e76a…0a06ac6e`).
> 짝 문서: `Qwilight_timing_policy.md`(시계·오디오·입력·판정·키음 구동 방식).
> 표기: **(확인)** = 설치본 CIL·리소스에서 직접 확인, **(추정)** = 이름·정황으로 붙인 해석.
> 분류 꼬리표:
> - **[백엔드]**: 타이밍·오디오·입력·판정·점수·게이지·채보 변형처럼 게임 로직이나 소리에 영향을 주는 설정
> - **[프론트]**: 화면·그리기·스킨·연출·UI·선곡 화면에만 영향을 주는 설정
> - **[운영]**: 파일·네트워크·계정·기록·도구처럼 플레이와 무관한 설정
>
> 목적: 우리 엔진은 Qwilight의 설정 원리를 따라 하지 않습니다. 다만 "어떤 설정이 있고 각각 무엇을 바꾸는가"를 빠짐없이 기록해, 게임 디자인 관점의 자유도를 참고합니다.

---

## 0. 요약

### 0-1. 개수

| 저장소 | 범위 | 설정 수 | [백엔드] | [프론트] | [운영] |
|---|---|---|---|---|---|
| `Configure.json` 최상위 (`Configure`) | 사용자 전역 | 227 | 62 | 138 | 27 |
| `ModeComponentValue` (`ModeComponent`) | 플레이 옵션(전역, 프리셋 8칸) | 25 | 21 | 4 | 0 |
| `UIConfigureValuesV2[스킨]` (`UIConfigure`) | 인게임 스킨별 | 15 | 0 | 15 | 0 |
| `BaseUIConfigureValues[테마]` (`BaseUIConfigure`) | 테마 스킨별 | 3 | 0 | 3 | 0 |
| `AudioConfigureValues[장치]` (`AudioConfigure`) | 출력 장치별 | 2 | 2 | 0 | 0 |
| `DB.db` `wait`·`format`·`favorite_entry` 표 | 곡(차분)별 | 5 | 1 | 2 | 2 |
| **합계** | | **277** | **86** | **162** | **29** |

- `Configure`의 227개는 사용자 `Configure.json`의 최상위 키 수와 같습니다. 모두 `Configure` 클래스의 읽기·쓰기 속성입니다. (확인)
  - 그중 `ModeComponentValue`, `ModeComponentBundles`, `UIConfigureValuesV2`, `BaseUIConfigureValues`, `AudioConfigureValues`는 다른 저장소를 담는 그릇입니다. 그릇 자체도 1개씩 셌습니다.
- 읽기 전용(계산) 속성 95개와, 저장되지 않는 실행 상태 속성 4개는 세지 않았습니다(9장).
- 분류는 "그 값을 읽는 코드가 무엇을 하는가"로 정했습니다. 경계에 있는 항목은 표에 이유를 적었습니다.
  - 예: 미스 효과음은 소리지만 연출이므로 [프론트]입니다.
  - 예: BPM 고정은 플레이 옵션이지만 스크롤만 바꾸므로 [프론트]입니다.

### 0-2. 게임 디자인 관점에서 눈에 띄는 자유도

1. **음악 속도와 피치 보정을 분리**: `AudioMultiplier`(0.5 ~ 2.0배)는 곡 시계와 소리를 함께 빠르게 합니다. `AudioMultiplierAtone`("피치 보정")을 켜면 FMOD 피치 시프트 DSP로 음높이를 되돌립니다. (확인)
2. **화면 속도는 "BPM × 배속" 목표값으로 지정**: `MultiplierValue`를 기준 BPM으로 나눠 실제 배속을 구하므로, 곡이 바뀌어도 노트가 내려오는 속도가 일정합니다. 기준 BPM은 최빈(기본)·시작·최저·최고 중에서 고를 수 있습니다(`BPMStandardVarietyValueV2`). (확인)
3. **판정 창과 게이지를 숫자로 직접 정의**: 판정은 `FavorJudgments`(6등급 × 빠름/늦음 ms), 게이지는 `FavorHitPoints`(6등급 × 토탈 비례/절대 증감)로 정하고, 이름 붙은 프리셋으로 저장합니다. 기본 프리셋으로 LR2 EZ/NM 값이 들어 있습니다. (확인)
4. **게이지 자동 전환(GAS)과 무한 모드**:
   - `GASLevel`(1 ~ 4)은 어느 게이지까지 GAS를 허용할지 정합니다(Yell ≥ 1, 하드코어 ≥ 2, 매우 어려움 ≥ 3, 어려움 ≥ 4). 게임 안내 문구에 따르면 게이지가 0이 되면 끝나지 않고 다음 게이지로 내려갑니다. (확인: 조건식·안내 문구 / 추정: 내부 동시 계산)
   - `IsInfiniteMode`를 켜면 게이지가 0이 되어도 곡이 끝나지 않습니다. (확인)
5. **채보 변형이 로드 단계의 정식 기능**: 랜덤 9종, 키 수 강제 변환 30종("자연스러운" 간격 유지형 포함), 숏↔롱 노트화(간격 범위 지정), BGM 소리를 노트로 추가(비율·간격), 중앙 라인 제거, 1P→2P 복사·대칭, 역방향 플레이. 모두 `BaseCompiler.PostCompile`에서 채보를 바꿉니다. (확인)
6. **키음 정책 3축을 사용자에게 개방**:
   - 빈 누름 소리: DJMAX식(다음 노트) / IIDX식(마지막 판정 노트)
   - 같은 소리 재트리거: 끊기(중복 키음 방지) / 겹치기
   - 키음 자체: 누를 때 냄 / 채보 시각에 자동 재생(장치별 `HandleInputAudio`)
   - "범용 키음"(모든 누름에 고정 타격음 추가)
   (확인)
7. **간접 미스와 롱노트 끝 판정 강제**: 빈 누름에 POOR를 주는 `LowestJudgmentConditionMode`, 롱노트를 뗄 때의 판정을 넣거나 빼는 `LongNoteMode`가 있습니다. 같은 채보를 다른 규칙으로 칠 수 있습니다. (확인)
8. **오프셋을 세 층으로 분리**: 오디오(곡별 + 장치별), BGA(곡별 + 전역), 노트 표시(스킨별, 곡 끝 자동 보정 가능). 판정 오프셋은 없습니다(`Qwilight_timing_policy.md` 8장). (확인)
9. **루프 주기와 실행 환경까지 노출**: 판정 루프 주기(`LoopUnit`, "폴링 레이트"), 프로세스 우선순위, 플레이 중 GC 억제 메모리 크기, 오디오 출력 API와 버퍼 크기, NVIDIA Reflex·AMD Anti-Lag·DirectComposition 부스트·삼중 버퍼링·프레임 제한까지 설정 창에 있습니다. (확인)
10. **스킨이 자기 옵션을 정의**: 스킨 YAML의 `configure-N`(최대 16개, 한국어 이름 포함)이 설정 창에 그대로 나타나고, 값은 스킨별로 저장됩니다(`UIConfiguresV2`). 화면 요소 14종은 스킨과 따로 켜고 끕니다(`UIPipeline*`). (확인)
11. **연출 피드백의 세밀한 조건**: "틀릴 때" 미스 사운드·미스 레이어·판정 애니메이션을 각각 항상/오토 제외/끄기로 고르고, 반응할 최소 콤보 수와 레이어 지속 시간도 정합니다. 컨트롤러 진동(누를 때/틀릴 때)과 키보드·마우스 RGB 조명 8개 제조사도 지원합니다. (확인)

---

## 1. 범위와 방법

| 무엇 | 어떻게 |
|---|---|
| 설정 속성 목록 | `Configure`, `ModeComponent`, `UIConfigure`, `BaseUIConfigure`, `AudioConfigure`, `WaitEntity`의 `get_`/`set_` 메서드 쌍. rea 관리 코드 디코더로 푼 CIL(`Qwilight_timing_policy.md` 1-2와 같은 방법)에서 뽑음 |
| 저장되는 키 | 사용자 `SavesDir\Configure.json`(읽기만)의 키와 대조. 곡별 저장은 `SavesDir\DB.db`의 SQLite 스키마(읽기 전용으로 열어 표 정의만 읽음) |
| 기본값 | `Configure.Validate(bool)`의 `set_X` 직전 상수(새 설정이면 모든 분기를 탐), `ModeComponent..ctor()` 등 생성자의 필드 초기값, `Configure.InitEqualizers` |
| 화면 이름 | 설정 창 WPF BAML(`Qwilight.g.resources`에서 꺼냄)의 "언어 키 ↔ 바인딩 속성" 순서 + `Assets\Language.json`(ko-KR/en-US) |
| 무엇을 바꾸나 | 각 속성 게터를 부르는 메서드 전수 교차참조(설정 창 코드는 제외). 주요 항목은 해당 메서드 CIL을 읽음 |
| 열거값 | 메타데이터 Constant 표에서 실제 숫자 값, `LanguageSystem.Init`에서 값 ↔ 표시 이름 |

- 게임을 실행하지 않았고, 파일을 바꾸지 않았습니다.
- 화면 이름은 BAML에서 바인딩 바로 앞에 나온 언어 키로 짝지었으므로, 일부는 짝이 어긋날 수 있습니다. 표에는 사람이 확인한 이름을 적었습니다.
- "사용자 값"은 이 PC의 `Configure.json` 값입니다(참고용).

---

## 2. 설정 저장소 지도 (확인)

| 저장소 | 파일·위치 | 키 | 언제 읽나 |
|---|---|---|---|
| 사용자 전역 | `SavesDir\Configure.json` 최상위 | `Configure`의 속성 이름 | 시작 시 `Configure.Load` → `Validate`(버전별 마이그레이션 포함) |
| 플레이 옵션 | 같은 파일 `ModeComponentValue`, 프리셋 8칸 `ModeComponentBundles[0..7]` | `ModeComponent` 속성 | 선곡·플레이 시작. 리플레이(`Comment`)에도 함께 기록 |
| 인게임 스킨별 | 같은 파일 `UIConfigureValuesV2["스킨\\스킨.yaml"]` | `UIConfigure` 속성 | 스킨 로드 |
| 테마 스킨별 | 같은 파일 `BaseUIConfigureValues["@테마\\@테마.yaml"]` | `BaseUIConfigure` 속성 | 테마 로드 |
| 출력 장치별 | 같은 파일 `AudioConfigureValues["장치 이름"]` | `AudioConfigure` 속성 | 장치 선택(`AudioSystem.OnAudioValueChanged`) |
| 곡별 | `SavesDir\DB.db` 표 `wait`(`Audio_Wait`, `Media_Wait`, `Media`), `format`(`Format`), `favorite_entry` | 차분 해시(`Note_ID`) | 차분 선택(`BaseNoteFile.SetConfigure`, `SetData`) |
| 입력 배치 | `Configure.json`의 `DefaultInputBundlesV6`(키보드), `DInputBundlesV4`, `XInputBundlesV4`, `WGIBundlesV3`, `MIDIBundlesV4` | 입력 모드 × 레인 × 슬롯 | 입력 처리 |
| 스킨 정의 옵션 | 스킨 YAML `configure-N`, `configure-comment-N`(이름) | 값은 `UIConfiguresV2[N]`/`UIConfigures[N]`에 저장 | 스킨 Lua·YAML 계산 |

- 곡별 표 중 `handled`(클리어 램프), `comment`(리플레이), `date`(플레이 날짜), `event_note`·`event_note_data`(코스), `entry`(목록 위치)는 설정이 아니라 기록이므로 세지 않았습니다.
- `SavesDir\#.json`은 온라인 코멘트 캐시라 설정이 아닙니다.

---

## 3. Qwilight 설정 화면의 구조 (확인: BAML)

```text
게임 설정 (ConfigureWindow, 탭 5개 — 아래 순서)
├─ 플레이 설정 (ComputingConfigureWindow, 하위 탭 3개)
│   ├─ 게임 모드 (ModeComponentWindow): 스크롤 속도, 음악 속도(+피치 보정), 모드 13종 버튼,
│   │                                    랜덤 시드(+고정), 중앙 라인 제거, 1P→2P 복사, 업스크롤, 프리셋 8칸 저장
│   │     └─ 보조 창: 게임 모드 설정(무한 모드 + 모드 13종), 사용자 지정 판정, 사용자 지정 게이지,
│   │                 자연스러운 강제 nK 간격, 롱 노트화 간격, 노트 추가 비율·간격
│   ├─ 스킨 설정 (ComputingUIConfigureWindow): 테마 스킨(+스킨 정의 옵션), 인게임 스킨(+스킨 정의 옵션, 입력 모드별 스킨),
│   │                                           색(NPS·마디선·라인 구분선), 스킨 세부 설정(화면 요소 14종 켜기·끄기와 각 요소 옵션)
│   └─ 비주얼 설정 (ComputingGUIConfigureWindow): 불투명도 5종, 위치·크기 8종(노트 레이턴시 포함), 레인 커버,
│                                                 블리딩, 틀릴 때 반응 3종
├─ 키보드 설정 (InputConfigureWindow): 키 배치 창, Windows 키 비활성화, 조명 설정(8개 제조사),
│                                       컨트롤러 설정(입력 모드·진동·턴테이블 민감도), MIDI 설정
├─ 그래픽 설정 (GUIConfigureWindow): 폰트, 화면 모드·창 크기, 수직 동기화, 삼중 버퍼링, Reflex/Anti-Lag, 프레임 제한,
│                                     DirectComposition 부스트, BGA 설정, 범용 이미지, BGA 레이턴시,
│                                     오디오 비주얼라이저, 웹캠
├─ 오디오 설정 (AudioConfigureWindow): 음량 4종, 키음(장치별), 중복 키음 방지, 범용 키음, 선곡 BGM,
│                                       백그라운드 오디오, DSP 효과 5종(+이퀄라이저 창), 출력 장비,
│                                       오디오 레이턴시(곡별·전역), 버퍼 크기, 출력 API, 키음 출력 방식, 음성 대화
└─ 일반 설정 (DefaultConfigureWindow): 미리듣기·시작 지점, 기본곡·추가 스킨 받기, 성능 설정
                                       (메모리 예약·폴링 레이트·프로세스 우선순위), 데이터베이스 초기화 7종,
                                       고급 설정(개발자 모드 등), 엔진 정보
계정 설정 (TwilightConfigureWindow, 별도 창): 대화방 자동 입장 2종, 권한·알림 6종(서버 쪽 값)
선곡 화면 쪽 창: 필터(WantWindow), 난이도 테이블(LevelWindow), 기본 폴더(ModifyDefaultEntryWindow), 선곡 BGM, 레인 커버 이미지
```

- **배치 원칙**: 탭이 "기능 영역(플레이/입력/그래픽/오디오/일반)"으로 나뉘고, 플레이 탭 안에서 다시 "규칙(게임 모드) / 스킨 / 비주얼"로 나뉩니다.
  - 그래서 [백엔드] 설정이 여러 탭에 흩어져 있습니다. 판정 루프 주기는 일반 → 성능에, 오디오 오프셋은 오디오 탭에, 판정 규칙은 플레이 → 게임 모드에 있습니다.
  - 노트 표시 오프셋("노트 레이턴시")은 비주얼 설정에 있습니다.
- 게임 모드 13종은 아이콘 버튼으로 보이고, 누르면 "게임 모드 설정" 보조 창이 열립니다. 세부 수치(사용자 지정 판정 등)는 다시 그 아래 보조 창에 있습니다.
- 화면에 보이지 않는 설정도 있습니다. 예: `GASLevel`(게임 모드 창의 게이지 버튼 묶음으로만 쓰임), 창 위치, 마지막 선택 상태 등. 표에는 "화면 이름 없음"으로 적었습니다.

---

## 4. 플레이 옵션 — `ModeComponent` (25개)

`Configure.ModeComponentValue`에 저장되고, 프리셋 8칸(`ModeComponentBundles`)에 통째로 저장·불러오기할 수 있습니다. 리플레이 기록(`comment` 표)에도 이 값들이 들어갑니다. (확인)

| 속성 | 화면 이름 (ko / en) | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `MultiplierValue` | 스크롤 속도 / Scroll Speed | double, 1000 (1800) | 실제 배속 = `MultiplierValue / (|기준 BPM| × 음악 속도)`(`ModeComponent.get_Multiplier`). 노트가 화면을 지나는 속도만 바꿈 | [프론트] | 확인 |
| `AudioMultiplier` | 음악 속도 / Music speed | double, 1 (범위 0.5 ~ 2.0) | 곡 시계 진행 속도(`LoopingCounter += step × AudioMultiplier`), FMOD 채널 피치. 판정 오차는 이 값으로 나눔 | [백엔드] | 확인 |
| `Salt` | 랜덤 시드 / Random seed | int, 시작 시각(`Environment.TickCount`) | 랜덤 계열 변형의 난수 시드(`BaseCompiler.PostCompile`) | [백엔드] | 확인 |
| `AutoModeValue` | 사용 안 함 / 자동 턴 테이블 | 0 Default, 1 Autoable (0) | 턴테이블 레인을 입력 대상에서 빼고 자동 판정(`DefaultCompute.IsSuitableAsInput`, `IsSuitableAsAutoJudge`) | [백엔드] | 확인 |
| `NoteSaltModeValue` | 미러, 슈퍼 랜덤, 랜덤, 마디 랜덤, 하프 랜덤, 하프 마디 랜덤, 미러 하프 랜덤, 미러 하프 마디 랜덤, 로테이션 랜덤 | 0,1,2,4,11,13,14,15,16,17 (0) | 레인 배치 변형(`PostCompile`) | [백엔드] | 확인 |
| `FaintNoteModeValue` | 페이드 아웃 / 점멸하는 노트 / 점멸하는 기어 | 0 ~ 3 (0) | 노트·기어 그리기 투명도(`BaseNote.GetFaint`) | [프론트] | 확인 |
| `JudgmentModeValue` | 매우 쉬운 ~ 매우 어려운 판정 난이도, 사용자 지정 | 0 쉬움, 1 기본, 2 어려움, 3 매우 쉬움, 4 매우 어려움, 5 사용자 지정 (기본 1, 이 PC 2) | 판정 창 표 선택(`Component.GetJudgmentMillis`의 `JudgmentMap[날짜, 모드, 등급, 방향]`)과 점수 배율(`DefaultCompute.SetStandMultiplier`) | [백엔드] | 확인 |
| `HitPointsModeValue` | 쉬운·기본·어려운·하드코어·매우 쉬운·매우 어려운·사용자 지정·단위인정·Yell 전용 라이프 게이지 | 0 ~ 8 (생성자 기본 8 = Yell 전용, 이 PC 5) | 게이지 규칙. 단위인정(7)이면 컴파일 때 `Computing.HitPointsValue`도 바꿈(`PostCompile`, 수치 의미는 미확인) | [백엔드] | 확인 |
| `NoteMobilityModeValue` | 4D 노트 / 더욱 압축되는 노트 / 4D 라인 / 압축되는 노트 | 0,1,3,4,5 (0) | 노트 이동 속도 곡선(`BaseNote.GetMultiplierAsNoteMobility`) | [프론트] | 확인 |
| `LongNoteModeValue` | 강제 롱 노트 떼는 판정 제거 / 추가 | 0, 1 Auto, 2 Input (0) | 롱노트 끝 판정 유무(`LongNote.Judge`, `SilentJudge`) | [백엔드] | 확인 |
| `InputFavorModeValue` | 강제 4K ~ 48+4K, 자연스러운 강제 nK(Beta), 자연스러운 강제 DJMAX 4/5/6/8B | 0, 4 ~ 33 (0) | 키 수 변환(`PostCompile`). "자연스러운"은 `InputFavorLabelledMillis` 이상 간격 유지 | [백엔드] | 확인 |
| `NoteModifyModeValue` | 숏 노트화 / 롱 노트화 | 0 ~ 2 (0) | 노트 종류 변환(`PostCompile`) | [백엔드] | 확인 |
| `BPMModeValue` | BPM 고정 | 0, 1 Not (0) | 컴파일 때 BPM 변화 표를 비우고 대표 BPM으로 노트 위치를 다시 계산 → 스크롤만 일정해짐 | [프론트] | 확인(표 비움) / 추정(판정 시각 불변) |
| `WaveModeValue` | 역방향 플레이 | 0, 1 Counter (0) | 채보를 뒤집고(`PostCompile`) 모든 소리를 거꾸로 재생(`IsCounterWave` → `setFrequency` 음수) | [백엔드] | 확인 |
| `SetNoteModeValue` | 노트 추가 / 빈 라인에 노트 추가 | 0, 1 Put, 3 VoidPut (0) | BGM 소리를 노트로 추가(`PostCompile`, 비율 `SetNotePut`, 간격 `SetNotePutMillis`) | [백엔드] | 확인 |
| `LowestJudgmentConditionModeValue` | 간접 미스 추가 | 0, 1 Wrong (0) | 창 밖 빈 누름에 POOR(소리 없음) | [백엔드] | 확인 |
| `WipeCenterNotes` | 중앙 라인 제거 | bool (false) | 가운데 레인 노트 제거(`PostCompile`) | [백엔드] | 확인 |
| `PutCopyNotesValueV2` | 1P를 2P로 복사 | 0 Default, 1 Copy, 2 P1Symmetric, 3 P2Symmetric (0) | DP에서 한쪽 채보를 복사·대칭(`PostCompile`) | [백엔드] | 확인 |
| `FavorJudgments` | 사용자 지정 판정 (보조 창) | double[6][2], 기본 [−1, 1] × 6 | 판정 모드가 사용자 지정일 때 등급별 [빠른 경계, 늦은 경계] ms | [백엔드] | 확인 |
| `FavorHitPoints` | 사용자 지정 게이지 (보조 창) | double[6][2], 기본 0 | 게이지 모드가 사용자 지정일 때 등급별 [토탈 비례 수치, 절대 수치] | [백엔드] | 확인 |
| `InputFavorLabelledMillis` | 노트 간격 (자연스러운 강제 nK) | double (0) | 변환 뒤 같은 레인 노트 사이 최소 간격 | [백엔드] | 확인 |
| `LowestLongNoteModify` | 시작 간격 (롱 노트화) | double, 100 | 롱노트와 다음 노트 간격의 하한 | [백엔드] | 확인 |
| `HighestLongNoteModify` | 끝 간격 (롱 노트화) | double, 100 | 간격의 상한 | [백엔드] | 확인 |
| `SetNotePut` | 노트 비율 (노트 추가) | double, 1 | BGM 중 노트로 바꿀 비율 | [백엔드] | 확인 |
| `SetNotePutMillis` | 노트 간격 (노트 추가) | double (0) | 추가 노트의 최소 간격 | [백엔드] | 확인 |

- `IsPUMPMode`(업스크롤)는 이 창에 있지만 스킨별 저장소(`UIConfigure`)에 들어갑니다(6장).
- `JudgmentMap`의 실제 ms 표는 `Component`의 정적 데이터이고, 이번에는 수치까지 풀지 않았습니다(10장).

---

## 5. 사용자 전역 — `Configure` (227개)

표의 "기본값"은 새 설정을 만들 때 `Validate`가 넣는 값입니다. "—"는 타입 기본값(0, false, 빈 값)이거나 다른 곳에서 정해지는 값입니다.

### 5-1. 타이밍·성능 (4)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `LoopUnit` | 폴링 레이트 / Polling rate | int Hz, 1000 (8000) | 채보 루프와 패드 폴링 루프의 주기 = 1000/값 ms(`Component..ctor`, `HandlingController.HandleLooping`). 리플레이에도 기록 | [백엔드] | 확인 |
| `ExeClass` | 프로세스 우선 순위 | `ProcessPriorityClass`, 32 Normal (128 High) | 프로세스 우선순위(`Configure.Load`에서 적용) | [백엔드] | 확인 |
| `LazyGCV2` | 인게임 메모리 미리 예약 | long MB, `Utility.GetLazyGC()` (8192) | 플레이 중 `GC.TryStartNoGCRegion(값 × 10⁶)` 크기 | [백엔드] | 확인 |
| `AutoLazyGC` | (위의 "자동") | bool, true (false) | 플레이 진입마다 `GetLazyGC`로 크기를 다시 정함 | [백엔드] | 확인 |

### 5-2. 오디오 (31)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `AudioVariety` | 출력 API / Output API | `FMOD.OUTPUTTYPE`, 6 WASAPI (7 ASIO) | FMOD `setOutput` | [백엔드] | 확인 |
| `AudioDataLength` | 버퍼 크기 / Buffer size | uint 샘플, 256 (64) | FMOD `setDSPBufferSize(값, 2)` | [백엔드] | 확인 |
| `LastASIOAudioValueID` | — | int (0) | 마지막으로 고른 ASIO 장치 번호 기억 | [백엔드] | 확인 |
| `LastWASAPIAudioValueID` | — | int (0) | 마지막으로 고른 WASAPI 장치 번호 기억 | [백엔드] | 확인 |
| `AudioWait` | 오디오 레이턴시 / Audio latency | double ms, 0, ±1000 클램프 | **곡별 값의 현재 사본**(DB `wait.Audio_Wait`에서 불러와 저장). 배경음 레인 발동 시각만 옮김 | [백엔드] | 확인 |
| `TotalAudioVolume` | 모든음 / All sounds | 0 ~ 1, 0.4 (0.75) | 마스터 채널 그룹 음량 | [백엔드] | 확인 |
| `MainAudioVolume` | BGM | 0 ~ 1, 0.65 (0.9) | 채널 그룹 0(배경음) 음량 | [백엔드] | 확인 |
| `InputAudioVolume` | 키음 / Keysound | 0 ~ 1, 0.65 (1) | 채널 그룹 1(키음·범용 키음) 음량 | [백엔드] | 확인 |
| `SEAudioVolume` | SE | 0 ~ 1, 0.75 (0.5) | 채널 그룹 2(UI 효과음) 음량 | [백엔드] | 확인 |
| `LostPointAudio` | 백그라운드 오디오 | bool, true | 끄면 창이 포커스를 잃었을 때 음량 0(`AudioSystem.HandleSystem`) | [백엔드] | 확인 |
| `StopLastEqualAudio` | 중복 키음 방지 / Prevent duplicate keysound | bool, true | 같은 `#WAV` ID를 다시 울릴 때 앞 채널을 끊음(배경음·키음 공통) | [백엔드] | 확인 |
| `InputAudioVarietyValue` | 키음 출력 방식 | 0 DJMAX, 1 IIDX (기본 1, 이 PC 0) | 빈 누름 소리: 레인의 다음 노트 / 마지막 판정 노트 | [백엔드] | 확인 |
| `BanalAudio` | 범용 키음 / Universal Keysound | bool, false | 모든 누름에 지정 소리를 추가 재생(`HandleBanalAudio`) | [백엔드] | 확인 |
| `BanalAudioFileItem` | (범용 키음 파일) | 경로, `UI\Default.wav` | 범용 키음 소리 파일 | [백엔드] | 확인 |
| `AudioConfigureValues` | — | 장치 이름 → `AudioConfigure` | 장치별 저장소 그릇(7장) | [백엔드] | 확인 |
| `AudioMultiplierAtone` | 피치 보정 / Pitch calibration | bool, false | 음악 속도를 바꿀 때 FMOD 피치 시프트 DSP(타입 13)로 음높이 유지 | [백엔드] | 확인 |
| `Equalizer` | 이퀄라이저 | bool, false | 5밴드 EQ DSP 켜기 | [백엔드] | 확인 |
| `Equalizer0`~`Equalizer4` (5개) | 이퀄라이저 밴드 이득 | double dB, 0 (`InitEqualizers`) | 각 밴드 이득 | [백엔드] | 확인 |
| `EqualizerHz0`~`EqualizerHz4` (5개) | 이퀄라이저 밴드 주파수 | Hz, 90 / 500 / 3000 / 5000 / 8000 | 각 밴드 중심 주파수 | [백엔드] | 확인 |
| `Tube` | 컴프레서 / Compressor | bool, false | DSP 추가(`AudioSystem.SetTube`) | [백엔드] | 확인 |
| `SFX` | 리버브 / Reverb | bool, false | DSP 추가(`SetSFX`) | [백엔드] | 확인 |
| `Flange` | 플랜지 / Flange | bool, false | DSP 추가(`SetFlange`) | [백엔드] | 확인 |
| `Averager` | 노멀라이저 / Normalizer | bool, false | DSP 추가(`SetAverager`) | [백엔드] | 확인 |

(`Equalizer0~4`, `EqualizerHz0~4`는 표에서 한 줄로 묶었지만 개수에는 10개로 셌습니다.)

### 5-3. 입력 (11)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `DefaultInputBundlesV6` | 키보드 설정(키 배치 창) | 입력 모드 × 레인 × 슬롯의 가상 키 | 키보드 → 레인 대응(`DefaultControllerSystem` `HandleInput`) | [백엔드] | 확인 |
| `DInputBundlesV4` | 컨트롤러 키 배치 | DirectInput 버튼·축 | DirectInput → 레인 | [백엔드] | 확인 |
| `XInputBundlesV4` | 〃 | XInput 버튼·트리거 | XInput → 레인 | [백엔드] | 확인 |
| `WGIBundlesV3` | 〃 | Windows.Gaming.Input | WGI → 레인 | [백엔드] | 확인 |
| `MIDIBundlesV4` | MIDI 키 배치 | MIDI 음·값 | MIDI → 레인(`MIDISystem.Handle`) | [백엔드] | 확인 |
| `ControllerVariety` | 입력 모드 / Input mode | 0 DInput, 1 XInput, 2 WGI (0) | 어떤 패드 API를 폴링할지(`ControllerSystem.HandleSystem`) | [백엔드] | 확인 |
| `DInputControllerVarietyValue` | 컨트롤러 모드 | 0 BMS, 1 IIDX (기본 0, 이 PC 1) | DirectInput 턴테이블 해석 방식 | [백엔드] | 확인 |
| `DInputXyzSensitivityV2` | 턴 테이블 민감도 | int, 100 | 축 → 턴테이블 입력 문턱 | [백엔드] | 확인 |
| `DInputIIDXSensitivity` | 턴 테이블 정지 속도 | double, 142.86 | IIDX 모드에서 회전이 멈췄다고 볼 시간 | [백엔드] | 확인(사용처) / 추정(단위) |
| `MIDIPBCSensitivity` | 피치 설정 민감도 | double, 100 | MIDI 피치 벤드 → 입력 문턱 | [백엔드] | 확인 |
| `IsXwindow` | Windows™ 키 비활성화 | bool, false | 보조 프로그램(`Xwindow.exe`)으로 Windows 키를 막음 | [백엔드] | 추정(이름·창 위치) |

### 5-4. 판정·게이지·채보 변형 보조 (16)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `ModeComponentValue` | (게임 모드 탭 전체) | `ModeComponent` | 현재 플레이 옵션(4장) | [백엔드] | 확인 |
| `ModeComponentBundles` | 프리셋 저장 8칸 | `ModeComponentBundle[8]` (이름 + 값) | 플레이 옵션 프리셋. 키 입력으로 바로 불러옴(`OnWMInput`) | [백엔드] | 확인 |
| `FavorJudgments` | 사용자 지정 판정 프리셋 | 이름 → double[6][2]. 기본 LR2 EZ/NM 등 | 사용자 지정 판정 창 프리셋 목록 | [백엔드] | 확인 |
| `FavorHitPoints` | 사용자 지정 게이지 프리셋 | 이름 → double[6][2]. 기본 LR2 EZ/NM 등 | 사용자 지정 게이지 프리셋 목록 | [백엔드] | 확인 |
| `GASLevel` | (화면 이름 없음, 경고 문구 `GASWarning`) | int 1 ~ 4, 4 | 동시에 계산할 게이지 수. Yell(≥1), 하드코어(≥2), 매우 어려움(≥3), 어려움(≥4)일 때 GAS 가능(`ModeComponent.get_CanGAS`). 게이지가 0이 되면 다음 게이지로 내려감 | [백엔드] | 확인 |
| `IsInfiniteMode` | 무한 모드 / Infinite mode | bool, false | 게이지 0(F)이어도 곡을 계속(`DefaultCompute.OnHaveGotF`가 `!IsInfiniteMode` 반환) | [백엔드] | 확인 |
| `SetSalt` | 고정 / Pin (랜덤 시드) | bool, false | 랜덤 시드를 바꾸지 않고 고정(`ModeComponent.CopyAs`) | [백엔드] | 확인 |
| `SaltAuto` | (선곡 화면 아이콘) | bool, false | 오토 플레이(미리보기)가 끝날 때 시드 자동 변경 | [백엔드] | 추정 |
| `AutoInputFavorLabelledMillis` | 노트 간격 "자동" | bool, false | `InputFavorLabelledMillis`를 곡 BPM으로 자동 계산(`ModeComponent.SetAutoInputFavorLabelledMillis`) | [백엔드] | 확인 |
| `AutoInputFavorLabelledMillisValue` | (분할 수) | double, 16 | 자동 계산 식의 분모(16 → 16분음 간격) | [백엔드] | 확인(식 형태는 아래 `SetNotePut`과 같다고 추정) |
| `AutoLowestLongNoteModify` | 시작 간격 "자동" | bool, false | 롱 노트화 하한 자동 계산 | [백엔드] | 확인 |
| `AutoLowestLongNoteModifyValue` | (분할 수) | double, 16 | 위 식의 분모 | [백엔드] | 확인 |
| `AutoHighestLongNoteModify` | 끝 간격 "자동" | bool, false | 롱 노트화 상한 자동 계산 | [백엔드] | 확인 |
| `AutoHighestLongNoteModifyValue` | (분할 수) | double, 16 | 위 식의 분모 | [백엔드] | 확인 |
| `AutoSetNotePutMillis` | 노트 간격 "자동" (노트 추가) | bool, false | `SetNotePutMillis = 240000 / (기준 BPM × 음악 속도) / 분할 수`(`SetAutoSetNotePutMillis`) | [백엔드] | 확인 |
| `AutoSetNotePutMillisValue` | (분할 수) | double, 16 | 위 식의 분할 수 | [백엔드] | 확인 |

### 5-5. 플레이 화면·연출 (70)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `UIItemValue` | 인게임 스킨 | 스킨 이름·YAML, "Default" (CR_minimal) | 플레이 화면 스킨 | [프론트] | 확인 |
| `BaseUIItemValue` | 테마 스킨 | 〃 (@CR_advanced) | 선곡 등 나머지 화면 스킨 | [프론트] | 확인 |
| `UIConfigureValuesV2` | — | 스킨 → `UIConfigure` | 인게임 스킨별 저장소 그릇(6장) | [프론트] | 확인 |
| `BaseUIConfigureValues` | — | 테마 → `BaseUIConfigure` | 테마별 저장소 그릇(6장) | [프론트] | 확인 |
| `SetUIItems` | 입력 모드별 스킨 사용 | bool[17] | 키 모드(4K ~ 48+4K)마다 다른 스킨을 쓸지 | [프론트] | 확인 |
| `UIItemsV2` | 입력 모드별 스킨 | string[17] | 그 모드에서 쓸 스킨(`DefaultCompute.SetUIItem`) | [프론트] | 확인 |
| `UIPipelineJudgmentMain` | 판정선 / Judgment line | bool, true | 그리기 켜기·끄기(`DrawingSystem` `CanPaint`) | [프론트] | 확인 |
| `UIPipelineLimiter` | 라인 구분선 | bool, false | 〃 | [프론트] | 확인 |
| `UIPipelineMainDrawingPaint` | 키빔 / Key beam | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineMainJudgmentMeter` | 라인 판정 비주얼라이저 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineHitNotePaint` | 노트 폭발 이펙트 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineJudgmentPaint` | 판정 이펙트 | bool, true | 〃 | [프론트] | 확인 |
| `UICommentNote` | 리플레이 노트 | bool, true | 비교 리플레이 노트 그리기(`CommentNote.Paint`) | [프론트] | 확인 |
| `UIPipelineJudgmentMeter` | 판정 오차 / Judgment checker | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineJudgmentVisualizer` | 판정 비주얼라이저 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineBPM` | BPM | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineJudgmentCount` | 판정 개수 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineJudgmentVSVisualizer` | 판정 개수 비율 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineHunter` | 점수 격차 / Score gap | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineNet` | 실시간 랭킹 | bool, true | 〃 | [프론트] | 확인 |
| `UIPipelineJudgmentInputVisualizer` | 실시간 NPS 그래프 | bool, true | 〃 | [프론트] | 확인 |
| `TotalLimiterVariety` | 라인 구분선 배치: 모든 라인 | bool, true | 구분선을 그릴 위치 | [프론트] | 확인 |
| `AutoableLimiterVariety` | 〃: 턴 테이블 | bool, false | 〃 | [프론트] | 확인 |
| `CenterLimiterVariety` | 〃: 중앙 | bool, false | 〃 | [프론트] | 확인 |
| `Limiter57Variety` | 〃: 5 \| 7 (24, 48 전용) | bool, true | 〃 | [프론트] | 확인 |
| `LimiterColor` | 라인 구분선 색 | Color | 구분선 색 | [프론트] | 확인 |
| `MeterNoteColor` | 마디선 / Bar line | Color | 마디선 색(`MeterNote.Paint`) | [프론트] | 확인 |
| `InputNoteCountViewColor` | NPS 그래프 색 | Color | NPS 그래프 색 | [프론트] | 확인 |
| `AutoableInputNoteCountViewColor` | 턴 테이블 NPS 그래프 색 | Color | 〃 | [프론트] | 확인 |
| `JudgmentMeterCriteria` | 판정 비주얼라이저와 리플레이 노트 출력 기준 | double ms, 0 (12.5) | \|오차\|가 이 값을 넘는 판정을 빠름/늦음으로 세어 표시(`<Compute>b__1` `JudgmentMeters`) | [프론트] | 확인 |
| `AutoJudgmentMeterMillis` | (위의 "자동") | bool, true | 기준을 판정 등급 경계에서 자동으로 가져옴(`OnJudgmentMeterMillisModified`) | [프론트] | 확인 |
| `AutoJudgmentMeterMillisItemValueV2` | (자동 기준 등급) | `Judged`, 0 Highest | 어느 등급 경계를 기준으로 할지 | [프론트] | 확인 |
| `JudgmentMeterMillis` | 판정 오차 출력 지속 시간 | ms, 1000 | 판정 오차 표시 유지 시간 | [프론트] | 추정(게터 사용처 없음, 이름) |
| `JudgmentVisualizerMillis` | 판정 비주얼라이저 출력 지속 시간 | ms, 1000 | 비주얼라이저 점 유지 시간(`JudgmentVisualizer.Set`) | [프론트] | 확인 |
| `JudgmentVSVisualizers` | 판정 개수 비율 = 체크 / 체크 안 함 | bool[6], [T,F,F,F,F,F] | 비율 계산의 분자 등급 | [프론트] | 확인 |
| `BPMVarietyValueV2` | BPM 출력 방식 | 선택지 `BPMVariety.BPM` / `AudioMultiplier` / `Multiplier` (1) | 화면 BPM 숫자 계산법(그대로 / 음악 속도 반영 / 배속 반영) | [프론트] | 확인(선택지 이름) / 추정(계산법) |
| `BPMStandardVarietyValueV2` | BPM 출력 방식(선곡) / 기준 BPM | 0 최빈(재생 시간이 가장 긴 BPM×SCROLL), 1 시작, 2 최저, 3 최고 BPM (0) — 순서 정정 2026-10-16, Qwilight_scroll_policy.md 2-4 | 기준 BPM 선택 → 스크롤 속도 환산(`Computing.get_BPMStandard` → `ModeComponent.get_Multiplier`)과 선곡 표시 | [프론트] | 확인 |
| `HunterVarietyV3Value` | 점수 격차 기준 | 1등·선두·후미·사용자 지정·내 기록 (4) | 점수 격차 비교 대상 | [프론트] | 확인 |
| `FavorHunterStand` | (사용자 지정 점수) | int (0) | 사용자 지정 비교 점수 | [프론트] | 확인 |
| `NetCommentFollow` | 실시간 랭킹 업데이트 | bool, true | 플레이 중 랭킹을 계속 받아옴 | [프론트] | 확인 |
| `NetItemCount` | 실시간 랭킹의 기록 개수 | int, 8 | 표시 개수 | [프론트] | 확인 |
| `UbuntuNetItemTarget` | 친구만 / 오프라인 (코멘트 창) | bool, false | 랭킹 대상을 친구로 한정 | [프론트] | 추정 |
| `VeilDrawingHeight` | 레인 커버 높이 | double (0) | 레인 커버 크기. 화면에 보이는 시간(녹색 숫자) 계산에도 들어감(`GetIIDXMultiplierMillis`) | [프론트] | 확인 |
| `FlowVeilDrawing` | 레인 커버 슬라이드 | bool, true | 플레이 중 레인 커버 조작 허용 | [프론트] | 확인 |
| `VeilDrawingFileItems` | 레인 커버 이미지 | 파일 목록 | 커버 이미지 | [프론트] | 확인 |
| `MainAreaFaint` | 기어 영역 불투명도 | 0 ~ 1, 1 (0.85) | 기어 배경 투명도 | [프론트] | 확인 |
| `LowHitPointsFaintUI` | 블리딩 이펙트 | bool, true | 게이지가 낮을 때 화면 붉게 | [프론트] | 확인 |
| `AutoNoteWait` | 노트 레이턴시 "자동" | bool, false | 곡이 끝날 때 판정 오차 평균을 스킨의 `NoteWait`에 더함(`SetAutoNoteWait`) | [프론트] | 확인 |
| `InputMappingValue` | 1P와 2P의 턴 테이블 방향 | 0 ~ 3, 1 | 턴테이블 레인을 왼쪽·오른쪽 어디에 둘지(레인 배치·그리기) | [프론트] | 확인(사용처) / 추정(입력 대응 불변) |
| `AlwaysNotP2Position` | DP에서 1P와 2P를 붙임 | bool, false | DP 화면 배치(`IsIn2P`, `Has2P`) | [프론트] | 확인 |
| `HandleFailedAudioV2` | 틀릴 때 미스 사운드 출력 | 0 항상, 1 오토 제외, 2 끄기 (0) | 미스 효과음 | [프론트] | 확인 |
| `HandleFailedAudioCount` | 틀릴 때 반응할 콤보 | int, 5 | 이 콤보 이상에서 끊길 때만 반응 | [프론트] | 확인 |
| `ViewFailedDrawingV2` | 틀릴 때 미스 레이어 출력 | 0 ~ 2 (0) | BMS 미스 레이어(#BMP00) 표시 | [프론트] | 확인 |
| `FailedDrawingMillis` | 미스 레이어 지속 시간 | ms, 500 | 표시 시간 | [프론트] | 확인 |
| `ViewLowestJudgmentV2` | 틀릴 때 판정 애니메이션 출력 | 0 ~ 2 (0) | POOR 판정 애니메이션 | [프론트] | 확인 |
| `DefaultSpinningModeValue` | (일시정지 메뉴) | 0 계속, 1 설정, 2 다시, 3 그만 (2) | 일시정지 메뉴의 마지막 선택 기억 | [프론트] | 확인 |
| `DefaultDrawingFileItem` | 범용 이미지 | 파일 | 곡 이미지가 없을 때 쓸 이미지 | [프론트] | 확인 |
| `VibrationModeValue` | 진동 모드 | 0 끄기, 1 누를 때, 2 틀릴 때 (1) | 패드 진동 시점 | [프론트] | 확인 |
| `Vibration0` | 왼쪽 모터 | 0 ~ 1, 1 | 진동 세기 | [프론트] | 확인 |
| `Vibration1` | 오른쪽 모터 | 0 ~ 1, 1 | 〃 | [프론트] | 확인 |
| `Vibration2` | 왼쪽 트리거 | 0 ~ 1, 1 | 〃 (WGI) | [프론트] | 확인 |
| `Vibration3` | 오른쪽 트리거 | 0 ~ 1, 1 | 〃 (WGI) | [프론트] | 확인 |
| `Aura`, `K70`, `LS`, `BW`, `CM`, `MSI`, `GS`, `Wooting` (8개) | 조명 설정 (ASUS Aura, Corsair K70, Logitech, Razer(추정), Cooler Master, MSI, SteelSeries, Wooting) | bool, false | 각 제조사 RGB 조명 연동(`*System.Toggle`). `RGBSystem.HandleSystem`이 키 배치·오디오 비주얼라이저 색을 읽어 조명을 칠함 | [프론트] | 확인(연동·읽는 값) / 추정(BW·LS 제조사) |

(조명 8개는 한 줄로 묶었지만 개수에는 8개로 셌습니다.)

### 5-6. 그래픽·창·언어 (14)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `WindowedMode` | 화면 모드 | bool, true (false) | 창 / 전체 화면 | [프론트] | 확인 |
| `WindowLengthV2` | 창 크기(가로) | int, 1280 | | [프론트] | 확인 |
| `WindowHeightV2` | 창 크기(세로) | int, 720 | | [프론트] | 확인 |
| `WindowPosition0V2` | — | int, 0 | 창 X 위치 기억 | [프론트] | 확인 |
| `WindowPosition1V2` | — | int, 0 | 창 Y 위치 기억 | [프론트] | 확인 |
| `VESAV2` | 수직 동기화 | bool, false | 그리기 V-Sync(`DrawingSystem.HandleSystem`) | [프론트] | 확인 |
| `DataCount3` | 삼중 버퍼링 | bool, false | 스왑체인 버퍼 수 | [프론트] | 확인(이름·창) |
| `NVLLModeValue` | NVIDIA Reflex (끄기 / 사용 / 사용(부스트)) | 0 ~ 2, 0 (1) | Reflex 저지연 모드(`SetNVLLConfigure`) | [프론트] | 확인 |
| `IsAMDLL` | AMD Anti-Lag | bool, false | AMD 저지연(`InitAMDLL`/`WaitAMDLL`) | [프론트] | 확인 |
| `LLFramerate` | 프레임 제한 | fps, 0 (240) | 그리기 프레임 상한 | [프론트] | 확인 |
| `AutoLLFramerate` | (프레임 제한 "자동") | bool, false (true) | 상한을 자동으로 정함 | [프론트] | 추정 |
| `VRRMode` | DirectComposition™ 부스트 | bool, false | `DCompositionBoostCompositorClock` | [프론트] | 확인(P/Invoke) |
| `FontFamily` | 폰트 | 문자열, "Century Gothic" | 게임 내 글꼴 | [프론트] | 확인 |
| `Language` | 언어 (en-US / ja-JP / ko-KR) | 문자열, OS 언어 (ko-KR) | 표시 언어, 서버 요청 언어 | [프론트] | 확인 |

### 5-7. BGA·미디어·비주얼라이저·웹캠 (26)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `LoadedMedia` | BGA 설정(BGA 불러오기) | bool, true | 곡 시작 때 BGA를 불러올지 | [프론트] | 확인 |
| `Media` | BGA 켜기 | bool | **곡별 값의 현재 사본**(DB `wait.Media`) | [프론트] | 확인 |
| `MediaWait` | BGA 레이턴시 | ms, 0, ±1000 | **곡별 값의 현재 사본**. BGA 발동 시각 | [프론트] | 확인 |
| `BanalMediaWait` | BGA 레이턴시 (전역) | ms, 0 | 곡별 값에 더해짐 | [프론트] | 확인 |
| `IsMediaFill` | BGA 출력 방식 | bool, true | 늘려 채우기 / 비율 유지 | [프론트] | 확인 |
| `MediaVarietyValue` | BGA 엔진 | 0 MF(Media Foundation), 1 FF(FFmpeg) (1) | 동영상 해독기 | [프론트] | 확인 |
| `BanalMedia` | 범용 BGA | bool, false | BGA 없는 곡에 지정 영상 | [프론트] | 확인 |
| `AlwaysBanalMedia` | 항상 | bool, false | BGA가 있어도 범용 BGA | [프론트] | 확인 |
| `BanalMediaFilePath` | (범용 BGA 파일) | 경로 | | [프론트] | 확인 |
| `BanalFailedMedia` | 범용 미스 레이어 | bool, false | 미스 레이어 없는 곡에 지정 영상 | [프론트] | 확인 |
| `AlwaysBanalFailedMedia` | 항상 | bool, false | | [프론트] | 확인 |
| `BanalFailedMediaFilePath` | (범용 미스 레이어 파일) | 경로 | | [프론트] | 확인 |
| `AudioVisualizer` | 오디오 비주얼라이저 설정 | bool, true | FMOD FFT 시각화 켜기 | [프론트] | 확인 |
| `AudioVisualizerModeValue` | 오디오 비주얼라이저 방향 | 0 ~ 3, 2 | 막대 방향 | [프론트] | 확인 |
| `AudioVisualizerCount` | 오디오 비주얼라이저 개수 | int, 80 | 막대 수 | [프론트] | 확인 |
| `AudioVisualizerMainColor` | BGM 비주얼라이저 색 | Color | | [프론트] | 확인 |
| `AudioVisualizerInputColor` | 키음 비주얼라이저 색 | Color | | [프론트] | 확인 |
| `MediaInput` | 웹캠 설정 | bool, false | 웹캠 영상 켜기 | [프론트] | 확인 |
| `MediaInputID` | (장치) | 문자열 | 웹캠 장치 | [프론트] | 확인 |
| `MediaInputQualityMap` | 화질 | 장치 → 해상도·Hz | | [프론트] | 확인 |
| `FavorMediaInput` | BGA 대신 사용자 지정 영역 | bool, false | 웹캠을 BGA 자리 대신 지정 영역에 | [프론트] | 확인 |
| `MediaInputPosition0` | (라이브 컨트롤) | double | 웹캠 영역 X | [프론트] | 확인 |
| `MediaInputPosition1` | 〃 | double | 영역 Y | [프론트] | 확인 |
| `MediaInputLength` | 〃 | double | 영역 너비 | [프론트] | 확인 |
| `MediaInputHeight` | 〃 | double | 영역 높이 | [프론트] | 확인 |
| `MediaInputFaint` | 웹캠 불투명도 | 0 ~ 1, 1 | | [프론트] | 확인 |

### 5-8. 선곡 화면·필터·라이브러리 (36)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `AutoCompute` | 미리듣기 / Preview | bool, true | 선곡 시 오토 플레이 미리보기 | [프론트] | 확인 |
| `AutoHighlight` | 미리듣기 시작 지점 | bool, true (false) | 하이라이트부터 미리듣기(`AutoCompute.OnCompiled`) | [프론트] | 확인 |
| `DefaultAudioVarietyValue` | 선곡 BGM (사용 안 함 / 스킨 / 커스텀) | 0 ~ 2, 1 (2) | 선곡 화면 음악 출처 | [프론트] | 확인 |
| `DefaultAudioFileItems` | 선곡 BGM 목록 | 파일 목록 | 커스텀 선곡 음악 | [프론트] | 확인 |
| `InputWantInputMode` | 필터: 키 모드 | bool[17] | 보일 키 모드 | [프론트] | 확인 |
| `InputWantNoteVariety` | 필터: 형식(BMS/BMSON/…/코스) | bool[6] | | [프론트] | 확인 |
| `InputWantHandled` | 필터: 클리어 상태 | bool[9] | | [프론트] | 확인 |
| `InputWantLevel` | 필터: 난이도 칸 | bool[6] | | [프론트] | 확인 |
| `WantLevelTextValue` | 필터: 난이도 범위 켜기 | bool, false (true) | | [프론트] | 확인 |
| `LowestWantLevelTextValue` | 난이도 범위 하한 | int, 1 | | [프론트] | 확인 |
| `HighestWantLevelTextValue` | 난이도 범위 상한 | int, 12 (15) | | [프론트] | 확인 |
| `WantBPM` | BPM 범위 켜기 | bool, false | | [프론트] | 확인 |
| `LowestWantBPM` | BPM 범위 하한 | int, 60 | | [프론트] | 확인 |
| `HighestWantBPM` | BPM 범위 상한 | int, 240 | | [프론트] | 확인 |
| `WantAverageInputCount` | NPS 범위 켜기 | bool, false | | [프론트] | 확인 |
| `LowestWantAverageInputCount` | NPS 범위 하한 | int | | [프론트] | 확인 |
| `HighestWantAverageInputCount` | NPS 범위 상한 | int | | [프론트] | 확인 |
| `WantHighestInputCount` | 피크 NPS 범위 켜기 | bool, false | | [프론트] | 확인 |
| `LowestWantHighestInputCount` | 피크 NPS 하한 | int, 8 | | [프론트] | 확인 |
| `HighestWantHighestInputCount` | 피크 NPS 상한 | int, 16 | | [프론트] | 확인 |
| `WantHellBPM` | 기믹 노트 파일만 보기 | bool, false | BPM 기믹 차분만 | [프론트] | 확인 |
| `FitModeValueV2` | 정렬 기준 | BPM·아티스트·길이·제목·노트 수·난이도·피크 NPS·최근 플레이·플레이 횟수·평균 NPS·경로·게이지·클리어 (4 → 이 PC 11) | 목록 정렬 | [프론트] | 확인 |
| `GroupEntry` | 이름순 정렬 (기본 폴더) | bool, true | 폴더 묶기 방식 | [프론트] | 확인(창) |
| `CommentViewTabPosition` | (코멘트 창 탭) | int | 마지막 탭 | [프론트] | 확인 |
| `HallViewTabPosition` | 명예의 전당 | int | 〃 | [프론트] | 확인 |
| `HallViewTotalTabPosition` | 전체 | int | 〃 | [프론트] | 확인 |
| `HallViewAtTabPosition` | 이번 달 | int | 〃 | [프론트] | 확인 |
| `HallViewAbilityTabPosition` | 레이팅 | int | 〃 | [프론트] | 확인 |
| `DefaultEntryItems` | 기본 폴더 목록 | 폴더 목록 | 곡 폴더 등록 | [운영] | 확인 |
| `LastDefaultEntryItem` | — | 폴더 | 마지막으로 연 폴더 | [운영] | 확인 |
| `LastEntryItemPositions` | — | 폴더 → 위치 | 폴더별 커서 위치 | [운영] | 확인 |
| `LastInputWants` | — | 폴더 → 검색어 | 폴더별 검색어 | [운영] | 확인 |
| `LastWantLevelConfigures` | — | 폴더 → 테이블 설정 | 폴더별 난이도 테이블 선택 | [운영] | 확인 |
| `LevelTargetMap` | 난이도 테이블 설정 | 이름 → URL | 등록한 난이도 테이블 주소 | [운영] | 확인 |
| `NoteFormatID` | 텍스트 인코딩 | int 코드 페이지, −1 자동 | 차분 텍스트 인코딩 전역 기본(곡별은 DB `format`) | [운영] | 확인(언어 키) / 추정(전역 기본 역할, 게터 사용처 없음) |
| `PassedTutorialIDs` | — | 집합 | 본 안내 창 | [운영] | 확인 |

### 5-9. 계정·온라인·음성 대화·기타 운영 (19)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `AvatarID` | 로그인 | 문자열 | 계정 ID | [운영] | 확인 |
| `AESAvatarCipher` | — | 문자열 | 저장된 로그인 정보(암호화) | [운영] | 확인 |
| `AllowTwilightComment` | (기록 등록) | bool, true | 플레이 결과를 서버에 보낼지(`AtQuitMode`, `HandleWarning`) | [운영] | 확인(사용처) / 추정(의미) |
| `AutoEnterDefaultSiteV2` | 일반 대화방 자동 입장 | int, 0 | | [운영] | 확인 |
| `AutoEnterCommentSiteV2` | 성과 알림방 자동 입장 | int, 0 | | [운영] | 확인 |
| `AutoGetDefaultNote` | 기본곡 받기 | bool, true | 시작 때 기본곡 내려받기 | [운영] | 확인 |
| `AutoGetDefaultUI` | 추가 스킨 받기 | bool, true | 시작 때 추가 스킨 내려받기 | [운영] | 확인 |
| `AutoGetQwilight` | — | bool, true (false) | 시작 때 새 버전 확인 | [운영] | 추정 |
| `DefaultNotesDate` | — | long | 기본곡 받은 시각 | [운영] | 확인 |
| `DefaultUIDate` | — | long | 추가 스킨 받은 시각 | [운영] | 확인 |
| `Date` | — | 버전 | 설정 파일 버전(`Validate`의 마이그레이션 기준) | [운영] | 확인 |
| `Is1st` | — | bool | 첫 실행 여부 | [운영] | 확인 |
| `BMSEditorFilePath` | — | 경로 | 외부 BMS 편집기 | [운영] | 확인 |
| `BMSONEditorFilePath` | — | 경로 | 외부 BMSON 편집기 | [운영] | 확인 |
| `AudioInput` | 음성 대화 설정 | bool, false | 음성 대화 켜기(NAudio) | [운영] | 확인 |
| `Wave` | 출력 장비 (음성 대화) | bool, true | 음성 대화 출력 켜기 | [운영] | 확인 |
| `WaveIn` | 입력 장비 (음성 대화) | bool, true | 마이크 켜기 | [운영] | 확인 |
| `AudioInputValue` | 음성 대화 감도 | double, 0 | 마이크 문턱 | [운영] | 확인 |
| `WaveFadeVolume` | 음성 대화 사용 중 게임 음량 | %, 50 | 말할 때 게임 음량 줄이기 | [운영] | 확인 |

`Configure`의 [운영] 27개 = 이 절 19개 + 5-8의 라이브러리·인코딩·안내 기록 8개입니다.

---

## 6. 스킨별 — `UIConfigure` (15) / `BaseUIConfigure` (3)

`UIConfigure`는 인게임 스킨마다 따로 저장됩니다. 위치·크기 값 일부는 키 모드별 사전(`MainPositions[모드]` 등)입니다. (확인)

| 속성 | 화면 이름 | 값·기본값 | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `UIConfiguresV2` | 스킨 정의 옵션 (스킨 YAML `configure-0..15`) | string[16] | 스킨이 정의한 선택지의 현재 값. 스킨 Lua·YAML 계산에 들어감 | [프론트] | 확인 |
| `IsPUMPMode` | 업스크롤 모드 | bool, false | 노트가 아래에서 위로 | [프론트] | 확인 |
| `MainFaintV2` | 기어 불투명도 | 0 ~ 1, 1 | | [프론트] | 확인 |
| `MediaFaintV2` | BGA 불투명도 | 0 ~ 1, 1 | | [프론트] | 확인 |
| `NetItemFaintV2` | 실시간 랭킹 불투명도 | 0 ~ 1, 1 | | [프론트] | 확인 |
| `AudioVisualizerFaintV2` | 오디오 비주얼라이저 불투명도 | 0 ~ 1, 0.1 | | [프론트] | 확인 |
| `JudgmentMainPosition1V2` | 판정선 위치 | double | 판정선 높이 | [프론트] | 확인 |
| `NoteWait` | 노트 레이턴시 / Note latency | double ms, 0 | **노트를 그리는 Y 위치만** 옮김(`BaseNote.GetY`). 판정 불변 | [프론트] | 확인 |
| `MainPositions` | 기어 위치 | 키 모드 → double | 기어 가로 위치 | [프론트] | 확인 |
| `NoteLengths` | 노트 너비 | 키 모드 → double | | [프론트] | 확인 |
| `NoteHeights` | 노트 두께 | 키 모드 → double | | [프론트] | 확인 |
| `HitNotePaintAreas` | 노트 폭발 애니메이션 크기 | 키 모드 → double | | [프론트] | 확인 |
| `BandPositionV2` | 콤보 위치 | double | | [프론트] | 확인 |
| `JudgmentPaintPosition` | 판정 애니메이션 위치 | double | | [프론트] | 확인 |
| `JudgmentVisualizerPosition` | 판정 비주얼라이저 위치 | double | | [프론트] | 확인 |

`BaseUIConfigure`(테마별):

| 속성 | 화면 이름 | 값·기본값 | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `UIConfigures` | 테마 정의 옵션 (`@테마.yaml` `configure-N`: 화면 비율, 선곡 배경 등) | string[16] | 테마가 정의한 선택지 | [프론트] | 확인 |
| `DefaultMediaFaint` | 기본 BGA 불투명도 | 0 ~ 1, 0.25 | 선곡 화면 배경 BGA | [프론트] | 확인 |
| `DefaultAudioVisualizerFaint` | 기본 오디오 비주얼라이저 불투명도 | 0 ~ 1, 0.1 | | [프론트] | 확인 |

예: 기본 스킨 `Default.yaml`은 화면 비율(16:9/16:10/21:9/32:9), 노트 모양(막대/서클), 턴테이블(턴테이블/사이드 트랙), 중앙 노트 색을 스킨 옵션으로 정의합니다. (확인)

---

## 7. 출력 장치별 — `AudioConfigure` (2)

`Configure.AudioConfigureValues["장치 이름"]`에 저장되고, 장치를 바꾸면 그 장치의 값이 적용됩니다. (확인)

| 속성 | 화면 이름 | 값·기본값 (이 PC) | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `AudioWait` (게터 `Configure.BanalAudioWait`) | 오디오 레이턴시 (전역) | ms, 0 | 곡별 `AudioWait`에 더해져 배경음 발동 시각을 옮김 | [백엔드] | 확인 |
| `HandleInputAudio` (게터 `Configure.HandleInputAudio`) | 키음 / Keysound | bool, true | 끄면 누를 때 키음을 내지 않고, 노트 소리를 채보 시각에 자동 재생 | [백엔드] | 확인 |

---

## 8. 곡(차분)별 — `DB.db` (5)

| 표.열 | 화면에서 | 값·기본값 | 무엇을 바꾸나 | 분류 | 근거 |
|---|---|---|---|---|---|
| `wait.Audio_Wait` | 오디오 레이턴시 (곡 선택 상태에서) | ms, NULL(없음 = 0) | 그 차분의 배경음 발동 오프셋(`DB.SetWait`/`GetWait`) | [백엔드] | 확인 |
| `wait.Media_Wait` | BGA 레이턴시 | ms, 0 | 그 차분의 BGA 오프셋 | [프론트] | 확인 |
| `wait.Media` | BGA 켜기 | 0/1, 1 | 그 차분의 BGA 표시 | [프론트] | 확인 |
| `format.Format` | 텍스트 인코딩 | 코드 페이지(65001/932/936/949), −1 자동 | 그 차분을 읽을 때의 인코딩 | [운영] | 확인 |
| `favorite_entry.Favorite_Entry` | 즐겨찾기 폴더 | 이름 | 차분을 즐겨찾기 폴더에 넣음 | [운영] | 확인 |

데이터베이스 초기화 버튼 7종(일반 설정 → 데이터베이스 설정): 컴파일 캐시, 즐겨찾기, 기록, 레이턴시(`InitWait`), BGA(`InitMedia`), 색, 전체. 이것들은 설정이 아니라 동작입니다. (확인)

---

## 9. 세지 않은 것

| 항목 | 이유 |
|---|---|
| `Configure`의 읽기 전용 속성 95개 (`…Paint`, `…Text`, `IsControllerVarietyDInput` 등) | 화면 표시용 계산값 |
| `InputWant`(검색어), `WantLevelSystem`, `LastWantLevelName`, `LastWantLevelIDs` | 쓰기 가능하지만 `Configure.json`에 저장되지 않는 실행 중 상태(추정: 직렬화 제외) |
| `D2D…Color`, `…Paints`, `MeterNoteAverageColor`, `ConfigureFault` | 색 설정에서 만든 그리기 캐시, 오류 문구 |
| `ModeComponent`의 `CanModifyMultiplier`, `CanModifyAudioMultiplier`, `Lowest/HighestAudioMultiplier`, `SentMultiplier`, `ComputingValue`, `HandlingHitPointsModeValue` | 실행 중 제한·상태값(JSON에 저장되지 않음) |
| 계정 설정 창의 권한·알림 6종 | 서버 쪽 값(로컬 `Configure`에 없음) |
| 개발자 모드(`TelnetSystem`), 성능 정보 보기 | 로컬 설정 파일에 저장되지 않는 실행 상태(추정) |

---

## 10. 확인하지 못한 것

| 항목 | 상태 |
|---|---|
| 판정 난이도별 실제 판정 창 ms(`Component.JudgmentMap`) | 정적 배열 데이터. 이번에는 수치를 풀지 않음 |
| 게이지 모드별 증감 수치 | 같은 이유로 미추출 |
| `DInputIIDXSensitivity`의 단위, `AutoLLFramerate`의 자동 기준, `SaltAuto`의 정확한 시점 | 이름·창 위치로 추정 |
| `AllowTwilightComment`, `UbuntuNetItemTarget`, `AutoGetQwilight`의 정확한 의미 | 사용처만 확인, 의미는 추정 |
| `BW`·`LS` 조명이 어느 제조사인지 | 이름으로 추정 |
| BPM 고정이 판정 시각을 바꾸지 않는다는 점 | BPM 표를 비우는 것만 확인 |
| 화면 이름 짝짓기 | BAML 문자열 순서로 짝지음. 사람이 확인했지만 일부 보조 문구는 틀릴 수 있음 |
| 공개 소스와의 대조 | 하지 않음 |
