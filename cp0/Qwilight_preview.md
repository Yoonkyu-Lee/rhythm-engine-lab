# Qwilight 선곡 화면의 채보 미리보기

> 작성일 2026-10-16. 대상: 이 PC의 Qwilight 1.17.13 (`Qwilight.dll`, SHA-256 `b936e76a…0a06ac6e`). 짝 문서: `Qwilight_timing_policy.md`, `Qwilight_settings.md`, `Qwilight_scroll_policy.md`.
> 표기: **(확인)** = 설치본 CIL에서 직접 확인, **(추정)** = 정황으로 붙인 해석, **(계산)** = 확인한 식에 숫자를 넣은 값.
> 목적: 우리 엔진은 곡을 시작할 때 소리를 모두 디코딩해 메모리에 올리려 한다. 이런 미리보기를 만든다면 소리를 어떻게 올리고 버려야 하는지 판단하는 근거를 얻는다.

---

## 0. 결론

1. **정체**: 미리보기는 별도의 가벼운 경로가 아니라 **실제 플레이 엔진을 자동 모드로 돌리는 것**입니다. (확인)
   - 클래스는 `AutoCompute`이고, `DefaultCompute`를 상속합니다. 차이는 세 가지입니다: `IsAutoMode = true`, `IsInfiniteMode = true`, 시작 위치 `LevyingWait`.
   - 선곡 커서가 멈추면 `MainViewModel.SetAutoComputer` → (디바운스) → `SetAutoComputerImmediately` → `NewAutoComputer`가 `AutoCompute`를 새로 만들어 채보 컴파일 → 소리 로드 → 채보 루프까지 플레이와 똑같이 돌립니다.
   - 오토 플레이는 노트 시각에 가상 입력을 넣으므로, 키음도 실제 플레이와 같은 경로(`HandleInput` → 판정 → `HandleInputAudio`)로 납니다.
   - 선곡 화면은 진행 바(`LoopingCounter / Length`)와 테마 스킨 요소를 그립니다. 노트까지 보이는 인게임 화면은 "오토 플레이 화면 진입"(`EnterAutoComputingMode`)으로 같은 인스턴스를 띄울 때 그립니다. (확인 + 추정: 화면 세부)
2. **소리 로딩**: 미리보기를 시작하면 그 차분의 **`#WAV` 정의 전부**를 읽어 **전체 디코딩해 메모리에 올립니다**. 스트리밍도, 필요한 것만 올리기도 없습니다. (확인)
   - FMOD `createSound` 모드는 `2D | OPENMEMORY | ACCURATETIME | MPEGSEARCH | IGNORETAGS | LOOP_OFF`입니다. `CREATESTREAM`·`CREATECOMPRESSEDSAMPLE`·`NONBLOCKING`이 없으므로 FMOD 기본값인 "샘플로 완전 디코딩"입니다.
   - 파일 읽기와 디코딩은 `Parallel.ForEachAsync`로 소리 하나당 작업 하나씩, 기본 병렬도(코어 수)로 돌립니다.
   - 모든 소리가 끝나야 채보 루프가 시작되므로, 첫 소리까지의 시간 ≈ 디바운스(OS 더블클릭 시간, 기본 500 ms) + 채보 해석 + 전체 소리 디코딩입니다.
3. **탐색(seek)**: 진행 바를 누르면 `LevyingWait = 누른 시각`, `SetUndo = Just`로 채보 루프만 다시 시작합니다. 컴파일과 소리 로드는 다시 하지 않습니다. (확인)
   - 루프는 목표 시각 이전의 모든 이벤트 시각을 큐에 넣고, 대기 없이 한 반복에 하나씩 점프해 빨리 감습니다.
   - 이때 **목표 시각에 아직 울리고 있어야 할 소리만** 골라 `setPosition(목표 − 시작)`으로 **중간부터** 재생합니다. 긴 반주뿐 아니라 키음도 마찬가지입니다. 이미 끝났을 소리는 건너뜁니다.
4. **메모리 수명**: 디코딩한 소리는 그 `AutoCompute` 인스턴스(소리 그릇 `IAudioContainer`)에 묶여 있습니다. (확인)
   - 다른 곡으로 옮기면 새 미리보기를 시작하기 전에 이전 인스턴스를 `Close(dispose: true)`로 닫고 **소리를 모두 즉시 해제**합니다.
   - 여러 곡을 담는 캐시는 없습니다.
   - 대신 같은 곡의 설정이 바뀌어 미리보기를 다시 만들 때, 그리고 **미리보기 중인 곡을 실제로 플레이할 때**는 `Migrate`로 디코딩된 소리를 새 인스턴스에 넘겨 **다시 디코딩하지 않습니다**.
   - 메모리 상한은 없습니다.
5. **지연·자원 관리**: 선곡 커서를 빠르게 움직이면 다음과 같이 처리합니다. (확인)
   - `CancellableHandler`가 새 요청마다 이전 작업의 `CancellationToken`을 취소하고, 세마포어로 한 번에 하나만 실행합니다.
   - 시작 전에 `QwilightComponent.StandardWaitMillis`(= Win32 `GetDoubleClickTime()`, 기본 500 ms)만큼 기다립니다(디바운스).
   - 소리 로드(`Parallel.ForEachAsync`)와 파일 읽기에도 취소 토큰이 전달됩니다.
   - 관련 설정:
     - `AutoCompute`("미리듣기", 끄면 미리보기 자체를 만들지 않음)
     - `AutoHighlight`("미리듣기 시작 지점": 켜면 노트 밀도로 계산한 하이라이트부터, 끄면 0부터)
     - `LoadedMedia`·`BanalMedia` 등(바뀌면 미리보기를 다시 만듦)
6. **따로 녹음된 미리듣기 파일**: BMS `#PREVIEW`(`Computing.TrailerAudioName`/`TrailerAudioPath`)가 있으면 사용합니다. (확인 + 추정)
   - `NewAutoComputer`가 채보를 컴파일하기 **전에** 그 파일을 통째로 읽어 `AudioSystem.HandleImmediately`로 바로 반복 재생합니다. 이 소리도 디코딩 샘플입니다.
   - 채보 루프가 처음 시작될 때 `TrailerAudioHandler.Stop()`이 500 ms 페이드아웃으로 멈춥니다.
   - 즉 **로딩 동안 미리듣기 파일로 공백을 메우고, 준비되면 채보 미리보기로 넘어가는** 우선순위로 보입니다.
   - 다만 미리듣기 파일이 있을 때 미리보기 인스턴스를 `SetPause = true`로 만드는 코드가 함께 있어, 두 동작이 실제로 어떻게 맞물리는지는 미확인입니다.

**미리보기용 소리를 어떻게 올리고 버리나 (한 단락).**
Qwilight는 커서가 한 곡에 500 ms 머물면 그 차분을 실제 플레이용 엔진 인스턴스로 컴파일합니다. 그 차분이 정의한 모든 키음·배경음 파일을 병렬로 읽어 FMOD 샘플로 **전부 디코딩해 그 인스턴스 소유로 올립니다**. 미리듣기 파일(`#PREVIEW`)이 있으면 로딩 동안 그것을 먼저 틉니다. 진행 바 탐색은 다시 로드하지 않고, 이미 올린 소리를 목표 시각의 위치부터 재생하는 것으로 처리합니다. 곡을 바꾸면 이전 인스턴스의 소리를 **곧바로 전부 해제**하고 새로 올립니다. 곡 간 캐시는 없습니다. 유일한 재사용은 "지금 미리보기 중인 곡을 실제로 시작할 때 디코딩 결과를 그대로 넘기는 것"(`Migrate`)이라, 플레이 시작 로딩이 거의 없어집니다. 메모리 상한이나 형식 선택은 없고, 정확히 "한 곡 분량의 전체 디코딩"이 상주합니다.

---

## 1. 범위와 방법

| 대상 | 메서드 |
|---|---|
| 시작·디바운스·취소 | `MainViewModel.SetAutoComputer`(`<<SetAutoComputer>b__414_0>d`), `CancellableHandler.Handle`/`Cancel`, `QwilightComponent..cctor`(`StandardWaitMillis`) |
| 미리보기 생성 | `MainViewModel/<SetAutoComputerImmediately>d__415`, `<SetAutoComputerImmediately>g__NewAutoComputer|0`, `AutoCompute..ctor`, `AutoCompute/<OnCompiled>d__34` |
| 소리 로드 | `BMSCompiler/<CompileImpl>d__22`(`Parallel.ForEachAsync`), `<<CompileImpl>b__3>d`(키음), `AudioSystem.Load`, `AudioSystem.HandleImmediately` |
| 탐색 | `DrawingSystem` 포인터 처리 → `MainViewModel.HandleSetStatus`, `DefaultCompute/<Compute>d__839`(SetUndo 분기), `<Compute>b__1`(이벤트 큐), `<Compute>b__2`, `HandleInputAudio` |
| 수명 | `CloseAutoComputer`, `CancelAutoComputer`, `DefaultCompute.Close(bool)`, `DefaultCompute.Migrate`, `AudioSystem.Close`/`Migrate`, `SetComputingModeImpl` |

- 도구는 앞 문서들과 같습니다(rea 관리 코드 디코더로 푼 CIL).
- 정적 분석만 했고, 게임 실행·파일 수정은 하지 않았습니다.

---

## 2. 질문 1 — 기능의 정체

### 2-1. 흐름 (확인)

```text
선곡 커서 이동
 └ MainViewModel.SetAutoComputer()
     └ _setAutoComputerHandler.Handle(token => {          // CancellableHandler: 이전 작업 취소 + 직렬화
          await Task.Delay(StandardWaitMillis, token);     // 디바운스 = GetDoubleClickTime() (기본 500 ms)
          await SetAutoComputerImmediately(doTrailerAudio: true, token);
       })

SetAutoComputerImmediately
 ├ 차분이 "논리 파일"(소리만 있는 항목)이면 → CloseAutoComputer
 ├ Configure.AutoCompute == false → CloseAutoComputer("Default") 후 종료      // 미리듣기 끔
 ├ 기존 AutoComputer가 같은 차분이고, 모드 호환(IsCompatible)이며
 │   LoopUnit·LoadedMedia·BanalMedia…가 같으면 → 그대로 둠(계속 재생)
 ├ 같은 차분인데 위 설정이 바뀌었으면 → NewAutoComputer(현재 위치, doMigrate: true)
 └ 그 밖 → NewAutoComputer(NaN, doMigrate: false)

NewAutoComputer(levyingWait, doMigrate)
 ├ autoComputer = new AutoCompute([차분], ModeComponent, 아바타…, levyingWait)
 ├ doMigrate && 기존 있음 → 기존.Migrate(autoComputer)            // 소리·BGA·그림 넘김
 ├ 미리듣기 파일(#PREVIEW) 있고 미이전 → ReadAllBytesAsync → AudioSystem.HandleImmediately(…, loop: true)
 │                                    autoComputer.SetPause = true
 ├ 미이전이면 CloseAutoComputer() (이전 인스턴스 Close(true) = 해제)
 ├ MainViewModel.AutoComputer = autoComputer
 └ autoComputer.HandleCompiler()      // 컴파일 → 소리 로드 → OnCompiled → 채보 루프
```

### 2-2. `AutoCompute`는 플레이 엔진 그 자체 (확인)
- 생성자는 `DefaultCompute..ctor`를 부른 뒤 `IsInfiniteMode = true`, `LevyingMeter`, `LevyingWait`, `IsAutoMode = true`만 바꿉니다.
- 재정의는 `CanPause`, `CanSetPosition`, `IsEscapable` 같은 상수와 `HandledLength = AudioLength`, `OnCompiled` 정도입니다.
- `OnCompiled`에서 `LevyingWait`가 NaN이면 `AutoHighlight ? Computing.AudioLevyingPosition : 0`으로 정한 뒤 `HandleComputer`(채보 루프)를 시작합니다.
- 오토 플레이에서는 노트 시각이 되면 `HandleInput(±레인, …, 255)`로 가상 입력을 큐에 넣습니다(`<Compute>b__1` `IL_3147` ~ `IL_31f5`). 그래서 판정과 **키음이 실제 플레이와 같은 경로**로 처리됩니다.
- 따라서 미리보기는 판정 루프 주기(`LoopUnit`), 오디오 정책(`StopLastEqualAudio`, `InputAudioVariety` 등), 채보 변형(랜덤 등)을 모두 실제 플레이와 똑같이 따릅니다.

### 2-3. 그리는 것 (확인 + 추정)
- 선곡 화면(`DrawingSystem.HandleSystem`)은 `AutoComputer`를 참조해 두 가지를 그립니다.
  - 테마 스킨의 그림 요소(`BaseUI.PaintProperties`)
  - **진행 바**: 위치 = `Clamp(LoopingCounter / Computing.Length)`, 상태 = `DefaultCompute.Status`
- 노트까지 보이는 인게임 화면은 `EnterAutoComputingMode`가 `MainViewModel.Computer = AutoComputer`로 바꾸고 모드를 전환할 때 그립니다. 같은 인스턴스를 그대로 화면에 올리는 것입니다.
- 선곡 화면 자체에 노트를 그리는지는 테마 스킨에 달렸고, 이번에 확인하지 않았습니다.

### 2-4. 하이라이트 시작 위치 (확인)
- `Computing.AudioLevyingPosition`은 `PostCompile`에서 계산합니다. 재료는 노트 밀도 표(`InputNoteCounts`, NPS 그래프의 칸)와 상수 2500·50·제곱근이고, 곡 길이의 백분율로 위치를 정합니다(가중식 세부는 미추적).
- BMS `#PREVIEW`와는 무관합니다.

---

## 3. 질문 2 — 소리 로딩

### 3-1. 무엇을 올리나 (확인)
- `BMSCompiler/<CompileImpl>d__22`는 헤더 줄을 훑으며 `WAV`로 시작하는 정의마다 키음 로드 작업(`b__3`)을, `BMP` 정의마다 BGA 로드 작업(`b__2`)을 목록에 넣습니다.
- **채보에서 쓰이는지와 상관없이 정의된 소리를 모두 올립니다.**
- 작업 하나(`b__3`)의 순서:
  1. `Utility.GetFile`로 파일을 찾습니다(확장자 대체 포함).
  2. `File.ReadAllBytesAsync(경로, token)`로 읽습니다.
  3. `RecyclableMemoryStream`에 담습니다.
  4. `Utility.AwaitableGetID128`로 **내용 해시**를 냅니다.
  5. `AudioSystem.Load(스트림, 해시, container: 이 Compute, volume 1, loop false)`를 부릅니다.
  6. 스트림을 반납합니다(`DisposeAsync`).

### 3-2. FMOD 모드 (확인: `AudioSystem.Load`, `HandleImmediately`)

```text
mode = 0x0200C808 | (loop ? 0x2 LOOP_NORMAL : 0x1 LOOP_OFF)
     = IGNORETAGS(0x02000000) | MPEGSEARCH(0x8000) | ACCURATETIME(0x4000) | OPENMEMORY(0x800) | 2D(0x8)
createSound(바이트 배열, mode, exinfo{length}, out sound)
```

- `CREATESTREAM`(0x80), `CREATECOMPRESSEDSAMPLE`(0x200), `NONBLOCKING`(0x10000)가 없습니다. FMOD에서 `createSound`의 기본 동작은 `CREATESAMPLE`, 즉 로드 시 전체를 PCM으로 디코딩해 메모리에 둡니다(FMOD 문서 기준).
- `OPENMEMORY`라 FMOD가 바이트를 복사해 디코딩하고, 관리 쪽 버퍼는 곧바로 풀에 돌려줍니다.
- `ACCURATETIME`과 `MPEGSEARCH`는 정확한 길이와 MP3 탐색 정확도를 위해 로드 시간을 더 쓰는 옵션입니다.
- 디코딩된 PCM의 샘플 형식은 코드에서 정하지 않고 FMOD 코덱 기본값을 따릅니다.
  - 2026-10-16 추가 실험으로 그 기본값을 확인했습니다(확인). 방법: 설치본 `fmod.dll`(FMOD 2.03.13) 복사본을 우리 프로세스에서 불러 같은 mode로 표본을 올림. 게임은 실행하지 않음.
  - Ogg Vorbis·MP3·WMA·ADPCM WAV → **PCM16**. WAV 8/16/24비트·float → 원래 형식 그대로. FLAC 16/24비트 → PCM16/PCM24. 샘플레이트는 로드 때 바꾸지 않습니다.
  - Ogg Vorbis는 소리 하나마다 PCM 밖에 약 150 KB가 더 붙습니다(해제하면 돌아옴). 짧은 Ogg 키음이 많은 BMS에서는 이 부가분이 PCM보다 클 수 있습니다.

### 3-3. 병렬도와 시작까지의 시간 (확인 + 계산)
- 실행 방식: `Parallel.ForEachAsync(작업 목록, new ParallelOptions { CancellationToken = SetCancelCompile }, …)`입니다. `MaxDegreeOfParallelism`을 정하지 않으므로 .NET 기본값(`Environment.ProcessorCount`)만큼 동시에 돕니다(추정: .NET 동작).
- 디코딩은 각 작업 안의 `createSound` 호출에서 동기로 일어납니다. 그 호출들은 `AudioSystem._audioSharing` **읽기 잠금** 안이라 서로 막지 않습니다.
- 채보 루프는 `HandleCompiler`가 끝난 뒤(`OnCompiled`) 시작합니다. 따라서 첫 소리까지의 시간은 다음과 같습니다.

  `≈ GetDoubleClickTime()(기본 500 ms) + 채보 해석 + max(파일 읽기 + 디코딩)의 병렬 합 + 루프 시작`

  키음이 수백 ~ 수천 개인 BMS에서는 디코딩이 지배적입니다(계산이 아닌 구조적 판단, 실측 없음).
- 미리듣기 파일이 있으면 컴파일 전에 그것부터 재생하므로 귀로 듣는 공백은 줄어듭니다.

---

## 4. 질문 3 — 탐색(seek)

### 4-1. 진입 (확인)
- `DrawingSystem`의 포인터 처리(`<HandleSystem>b__117_6`) → `MainViewModel.HandleSetStatus(levyingWait, isEnter)`
- 조건: 입력이 한가함, 미리보기 생성 작업이 진행 중이 아님, 인스턴스가 계산 중
- 이 조건이면 `computer.LevyingWait = 누른 시각`, `computer.SetUndoValue = 2 (Just)`를 넣습니다. `isEnter`면 오토 플레이 화면으로도 들어갑니다.
- 루프는 `SetUndoValue`가 0이나 4가 아니면 끝납니다. `<Compute>d__839`의 분기는 다음과 같습니다(`SetUndo` 값: Not 0, Pass 1, Just 2, ModifySalt 3, Escape 4).

| SetUndo | 동작 |
|---|---|
| Not / Escape | 계산 종료(`IsComputing = false`) |
| Pass | `LevyingWait = PassableWait`, "Pass" UI 소리, `HandleComputer` |
| **Just** | **`HandleComputer` — 컴파일·로드 없이 채보 루프만 다시 시작** |
| ModifySalt | 새 시드로 `HandleCompiler`(다시 컴파일) |

### 4-2. 빨리 감기 (확인: `<Compute>b__1` `IL_09f3` ~ `IL_0c88`, `IL_43ef`)

```csharp
levyingWait = Clamp(Round(LevyingWait), -3000, Length + 2000);
events = (노트 시각들 ∪ 롱노트 끝 시각들 ∪ 리플레이 입력·배속 이벤트 시각들)
         .Where(t => t < levyingWait).Append(levyingWait).Distinct().Order();
_eventPositions = new Queue(events);  lastEventPosition = levyingWait;
...
// 매 반복
if (_eventPositions.TryDequeue(out p)) LoopingCounter = p;   // 이벤트 시각으로 점프
...
if (IsInEvents) goto 반복끝;                                  // 대기(Utility.Wait)·벽시계 전진 생략 → CPU 속도로 빨리 감기
```

- 빨리 감는 동안에도 노트의 오토 판정, 배경음 큐(`LoopBefore`), 노트 이동이 그대로 처리됩니다. 판정 표시·점수 통지 등은 `IsInEvents`면 건너뜁니다.
- 큐가 비면 벽시계(`Stopwatch`)가 그때 시작되고, 목표 시각부터 실시간 재생이 이어집니다.

### 4-3. 이미 울리고 있어야 할 소리 (확인: `<Compute>b__2`, `HandleInputAudio`)

```csharp
double now = IsInEvents ? lastEventPosition /* = 목표 시각 */ : LoopingCounter;
if (now > t + 길이) continue;                                 // 목표 시각에 이미 끝났을 소리는 건너뜀
AudioLevyingPosition = (uint)(now − t + 원래 시작 위치);       // setPosition(…, MS): 중간부터
```

- **배경음**(긴 반주 포함): 목표 시각에 아직 울리고 있어야 하면 `목표 − 시작` ms 지점부터 재생합니다.
- **키음**: 오토 플레이가 가상 입력으로 낸 키음도 같은 식(`HandleInputAudio`)으로 목표 시각에 걸치는 것만 중간부터 냅니다.
- 이 소리들은 빨리 감기가 진행되는 몇 ms 동안 거의 동시에 `playSound`됩니다. 그 뒤 실시간 재생이 이어지므로, 탐색 직후 반주와 채보가 맞습니다(추정: 빨리 감기 소요 시간만큼의 작은 차이).
- 빨리 감기 동안은 루프가 대기 없이 돌고 벽시계도 시작 전이라, 이 차이는 이벤트 수 × 반복 처리 시간 정도입니다(계산, 실측 없음).

---

## 5. 질문 4 — 메모리 수명

### 5-1. 소유 구조 (확인)
- `AudioSystem._audioMap : ConcurrentDictionary<IAudioContainer, ConcurrentDictionary<string 내용해시, AudioItem>>`
- 그릇(container)은 Compute 인스턴스입니다. 키음 로드 때 `container: defaultComputer`로 넘깁니다.
- `AudioSystem.Load`는 먼저 `_audioMap[그릇][해시]`가 있는지 보고, **있으면 디코딩하지 않고 그대로 돌려줍니다**. 같은 그릇 안에서 내용이 같은 파일은 한 번만 디코딩됩니다.

### 5-2. 해제와 재사용 (확인)

| 상황 | 경로 | 소리 |
|---|---|---|
| 다른 곡으로 이동 | `NewAutoComputer` → `CloseAutoComputer` → `AutoComputer.Close(true)` | `AudioSystem.Close(this)`가 그릇의 모든 `AudioItem`을 해제(FMOD `Sound.release`). BGA·그림도 해제 |
| 미리듣기 끔 / 소리 없는 항목 / 창 열림 등 | `CloseAutoComputer` / `CancelAutoComputer`(취소 후 닫기) | 해제 |
| 같은 곡, 설정만 바뀜(LoopUnit, BGA 등) | `NewAutoComputer(…, doMigrate: true)` → `기존.Migrate(새)` | `Close(false)`로 멈추기만 하고, `AudioSystem.Migrate`가 그릇을 통째로 새 인스턴스로 옮김. 새 컴파일의 `Load`는 캐시 적중 |
| **미리보기 중인 곡을 실제 플레이** | `SetComputingModeImpl` → `(AutoComputer ?? Computer).Migrate(새 DefaultCompute)` | 같은 방식으로 옮김 → **플레이 시작 시 재디코딩 없음** |
| 플레이 끝 | `DefaultCompute.Close(true)` | 해제 |

- **곡 간 캐시는 없습니다.** 동시에 올라가 있는 것은 현재 미리보기(또는 플레이) 한 곡분입니다. 옮기는 순간에는 이전 인스턴스를 먼저 닫으므로, 두 곡이 겹쳐 올라가는 시간은 짧습니다.
- `Migrate`는 차분이 같은지 검사하지 않고 그릇을 통째로 옮깁니다(`SetComputingModeImpl`).
  - 보통은 미리보기 곡 = 플레이 곡이라 문제가 없습니다.
  - 다른 곡을 바로 시작하면 이전 곡 소리가 새 인스턴스에 남아, 그 플레이가 끝날 때까지 함께 상주할 수 있습니다(추정).
- 메모리 상한, LRU, 압축 보관(`CREATECOMPRESSEDSAMPLE`) 같은 정책은 없습니다.
- 플레이 중에는 GC를 막는 `TryStartNoGCRegion`(`Qwilight_timing_policy.md` 3-4)이 따로 있을 뿐입니다.

---

## 6. 질문 5 — 지연·자원 관리 (확인)

| 장치 | 내용 |
|---|---|
| 디바운스 | `Task.Delay(QwilightComponent.StandardWaitMillis, token)`. `StandardWaitMillis = GetDoubleClickTime()`(Windows 기본 500 ms) |
| 취소 | `CancellableHandler.Handle`이 새 `CancellationTokenSource`로 교체하고(`Interlocked.Exchange`) 이전 것을 `CancelAsync`. 지연 대기·파일 읽기·`Parallel.ForEachAsync`·컴파일이 이 토큰을 받음 |
| 직렬화 | `SemaphoreSlim`으로 한 번에 미리보기 생성 하나 |
| 닫기 | `DefaultCompute.Close`가 컴파일·계산 토큰을 취소하고 세마포어를 기다린 뒤 자원 해제 |
| 탐색 차단 | 생성 작업이 진행 중이면(`_setAutoComputerHandler.IsHandling`) 진행 바 클릭을 무시 |

관련 설정(`Qwilight_settings.md`):

| 설정 | 영향 |
|---|---|
| `AutoCompute` "미리듣기" | 끄면 미리보기 인스턴스를 만들지 않음(소리 로드도 없음) |
| `AutoHighlight` "미리듣기 시작 지점" | 켜면 하이라이트(노트 밀도 기반)부터, 끄면 0 ms부터 |
| `LoopUnit`, `LoadedMedia`, `BanalMedia`, `AlwaysBanalMedia`, `BanalMediaFilePath`, `BanalFailedMedia`, `AlwaysBanalFailedMedia`, `BanalFailedMediaFilePath` | 값이 바뀌면 같은 곡도 미리보기를 다시 만듦(소리는 `Migrate`로 재사용) |
| `ModeComponent`(호환되지 않는 변경, 예: 랜덤·키 변환) | 다시 만듦 |
| `DefaultAudioVarietyValue`, `DefaultAudioFileItems` | 선곡 BGM. 미리보기를 닫을 때 `ClosePausableAudioHandler`와 함께 다시 틀어짐(추정) |

---

## 7. 질문 6 — 따로 녹음된 미리듣기 파일 (확인 + 추정)

- BMS `#PREVIEW`(BMS 해석기가 `"PREVIEW"` 헤더를 읽어 `Computing.TrailerAudioName`에 넣음, `TrailerAudioPath` = 곡 폴더 + 이름)를 씁니다.
- 재생 조건: `NewAutoComputer`에서 `doTrailerAudio`(디바운스 경로에서 true)이고, 경로가 있고, 이전 인스턴스에서 옮겨 오는 경우가 아닐 때입니다.
- 재생 방식:
  1. `File.ReadAllBytesAsync`로 통째로 읽습니다.
  2. `AudioSystem.HandleImmediately(바이트, 그릇: autoComputer, handler: TrailerAudioHandler, loop: true)`로 재생합니다. 같은 FMOD 모드(전체 디코딩)이고 반복(`LOOP_NORMAL`)입니다.
  3. 이 소리도 그 인스턴스 그릇에 들어가 함께 해제됩니다.
- 순서: 미리듣기 파일 재생 → 이전 인스턴스 닫기 → 채보 컴파일과 소리 로드 → 채보 루프 시작. 루프의 첫 반복에서 벽시계를 시작할 때 `TrailerAudioHandler.Stop()`이 `StandardWaitMillis`(500 ms) 페이드아웃으로 미리듣기 파일을 끕니다.
  - 즉 **미리듣기 파일 = 로딩 중 대체음, 채보 미리보기 = 본 재생**의 우선순위입니다(추정: 동작 순서는 코드로 확인, 체감은 미확인).
- 미리듣기 파일이 있을 때 `autoComputer.SetPause = true`도 설정합니다. 자동 플레이 인스턴스는 `CanPause`가 false라, 이 요청이 루프 시작 직후 일시정지로 이어지는지, 미리듣기 파일 재생과 어떻게 맞물리는지는 미확인입니다.
- BMSON은 미리듣기 파일을 쓰지 않습니다. `BMSONInfo` 클래스에 `preview_music` 필드가 없습니다(확인).
- `Computing.AudioLevyingPosition`(하이라이트)은 `#PREVIEW`와 별개입니다.

---

## 8. 우리 엔진에 주는 시사점 (메모리·로딩)

| 판단거리 | Qwilight의 선택 | 우리에게 |
|---|---|---|
| 미리보기 = 본 엔진인가 | 같은 엔진을 자동 모드로 돌림. 판정·키음·변형이 플레이와 일치 | 같은 엔진·같은 소리 표를 쓰면 "미리보기에서 들은 것 = 플레이"가 보장되고, 플레이 시작 시 디코딩을 생략할 수 있음(Migrate식 인계) |
| 무엇을 올리나 | 정의된 `#WAV` 전부를 전체 디코딩 | 우리 곡 형식은 쓰는 소리만 목록에 두므로 "정의 = 사용"이 되게 하면 낭비가 없음. 미리보기는 하이라이트 구간에 걸치는 소리만 먼저 올리고, 나머지는 백그라운드로 채우는 2단계도 가능 |
| 형식 | FMOD 기본 PCM: Ogg·MP3 등은 PCM16, WAV·FLAC은 원래 비트 깊이(3-2, 실험으로 확인) | 전체 디코딩 상주라면 16비트가 f32의 절반 메모리. 미리보기는 곡 하나라 어느 쪽이든 상주량은 "한 곡분"이지만, 로딩 시간도 메모리 쓰기량에 비례하므로 16비트가 유리할 수 있음 → CP의 측정 항목 |
| 수명 | 인스턴스(그릇) 소유, 곡 바꾸면 즉시 전부 해제, 캐시 없음 | 단순하고 상한이 자명함(한 곡분). 커서를 오가는 패턴이 잦다면 최근 1 ~ 2곡 보관을 측정해 볼 만함 |
| 취소·디바운스 | 500 ms 디바운스 + 토큰 취소 + 직렬화 | 디코딩은 취소 가능 단위(소리 하나)로 쪼개고, 커서가 머문 뒤에만 시작 |
| 탐색 | 다시 로드하지 않고 빨리 감기 + 중간부터 재생 | 소리를 전부 올려 두면 탐색이 싸짐. 우리 엔진은 "목표 시각에 걸친 소리를 샘플 위치로 예약"하면 같은 효과를 오프셋 오차 없이 얻음 |
| 로딩 중 공백 | `#PREVIEW`를 먼저 틀어 메움 | 곡 형식에 짧은 미리듣기 조각을 둘지, 하이라이트 구간 소리만 먼저 올릴지 선택 |

---

## 9. 확인하지 못한 것

| 항목 | 상태 |
|---|---|
| 실제 곡에서의 메모리 사용량 | 형식은 FMOD 실험으로 확인함(3-2). 실제 곡 전체의 상주량은 미측정 |
| 첫 소리까지의 실제 시간 | 정적으로는 구조만. 키음 수·코어 수에 따른 실측 필요 |
| 미리듣기 파일이 있을 때 `SetPause = true`와 페이드아웃의 실제 맞물림 | 코드 조각은 확인, 동작 순서의 결과는 미확인 |
| 선곡 화면(테마 스킨)에 노트를 그리는지 | 스킨 의존, 미확인 |
| 하이라이트 계산식의 세부 | 재료(NPS 표·상수)만 확인 |
| `Migrate` 뒤 다른 곡 소리가 남는 경우의 실제 발생 여부 | 코드상 가능성만(추정) |
| BMSON의 미리듣기 | `BMSONInfo`에 `preview_music` 필드가 없음(확인). 다른 경로는 없다고 봄(추정) |
