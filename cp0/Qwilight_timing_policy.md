# Qwilight의 플레이 구동 방식 (시계·오디오·입력·판정·키음)

> 작성일 2026-10-16. 대상: 이 PC에 설치된 Qwilight 1.17.13 (Steam 앱 1910130, `Qwilight.dll` 정적 분석).
> 표기: 근거 없는 단정은 하지 않음. **(확인)** = 설치된 바이너리를 역분석해 직접 확인(클래스·메서드 이름 명시), **(추정)** = 정황으로 붙인 해석, **(계산)** = 확인한 식에 숫자를 넣은 값.
> 공개 소스(GitHub `thenist/Qwilight`)는 이 문서에서 근거로 쓰지 않았습니다. 아래 내용은 모두 설치본 CIL에서 읽은 것입니다.
> 비교 기준 문서: `IIDX_timing_policy.md`, `DJMAX_runtime_policy.md`, `BMS_player_timing_policy.md`(beatoraja, Lunatic Vibes F).

---

## 0. 결론

1. **시계**: 채보는 **`Stopwatch`(벽시계)로 박자를 맞추는 고정 간격 루프**로 진행합니다. 곡 시각 `LoopingCounter`는 반복마다 `1000 / LoopUnit` ms(× 배속)씩 더해지는 **반복 카운터**입니다. 오디오 재생 위치나 DSP 클록은 읽지 않고, 화면 프레임과도 무관합니다. (확인)
   - 렉으로 루프가 멈추면 다시 돌 때 **대기 없이 연달아 반복해서 벽시계를 따라잡습니다.** 속도 제한도, 허용 폭도 없습니다. **5초 넘게 뒤처지면 `TimeoutException`으로 플레이를 끊습니다.** (확인)
   - 기본 `LoopUnit`은 1000 Hz(1 ms)입니다. 이 PC의 설정값은 8000 Hz(0.125 ms)입니다. (확인)
2. **오디오**: FMOD Core 2.03.13을 씁니다. 출력은 사용자가 고르는 FMOD 출력 형식이고, 이 PC는 **ASIO**입니다(기본값은 WASAPI). DSP 버퍼는 `AudioDataLength` × 2개입니다(이 PC 64, 기본 256). (확인)
   - 소리는 모두 **"지금 울려"**입니다. `playSound(paused=false)` 뒤에 `setPosition`으로 시작 위치를 줍니다. `setDelay`는 시작 예약에 쓰지 않고 **끝 자르기**에만 씁니다. (확인)
   - 이미 울리는 채널의 위치를 채보 시계에 다시 맞추는 코드가 없습니다. 그래서 렉 뒤 긴 반주와 채보의 관계는 **아무도 보정하지 않는 개방 루프**입니다. (확인)
3. **입력과 판정**:
   - 키보드는 Raw Input(`RIDEV_INPUTSINK`)으로 받습니다. 전용 숨은 창의 `GetMessage` 루프에서 처리합니다. (확인)
   - 패드(DirectInput·XInput·Windows.Gaming.Input)는 `LoopUnit` Hz 폴링 루프로 받고, MIDI는 WinRT `MidiMessageReceived` 이벤트로 받습니다. (확인)
   - **시각 도장은 없습니다.** `(입력 번호, 세기)`만 큐에 넣습니다. **판정은 채보 루프 스레드가 큐를 꺼낸 반복의 `LoopingCounter`로 합니다.** (확인)
4. **키음**: **Qwilight는 키음을 채보 루프 스레드(`DefaultCompute.Compute`의 전용 작업)의 입력 처리 단계에서 FMOD `playSound`로 즉시 재생합니다.** 판정과의 순서는 다음과 같고, 모두 같은 반복 안에서 일어납니다. (확인)
   1. `BaseNote.Judge`로 판정을 계산합니다.
   2. `HandleInputAudio`로 키음을 냅니다.
   3. `HandleJudged`로 점수·게이지·표시를 반영합니다.
   - 소리 고르기 (확인):
     - 판정된 누름: **그 노트의 소리**
     - 빈 누름, `InputAudioVariety = DJMAX`(이 PC 값): **그 레인에서 아직 처리 목록에 있는 첫 노트의 소리**
     - 빈 누름, `InputAudioVariety = IIDX`(새 설정의 기본값): **그 레인에서 마지막으로 판정된 노트의 소리**
   - "중복 키음 방지"(`StopLastEqualAudio`)를 켜면 같은 `#WAV` ID의 이전 채널을 끊습니다. 끄면 겹쳐 울립니다. 반주와 키음이 같은 표를 씁니다. (확인)
5. **"렉 뒤 스템이 끝날 때까지 어긋남"**: 경로는 두 가지입니다(7장).
   - **(A) 채보 루프가 멈춘 경우**: 루프가 멈춘 사이에 시작했어야 할 스템은 따라잡기 도중에 시작합니다. 시작 위치는 "논리 시계 기준으로 늦은 만큼"만 건너뛰는데, 논리 시계 자체가 뒤처져 있으므로 **사실상 처음부터 벽시계로는 늦게** 시작합니다. 채보는 곧 벽시계로 돌아오지만 그 스템은 끝까지 늦습니다. (확인 + 계산)
   - **(B) 오디오 출력이 멈춘 경우**: FMOD 믹서나 출력이 멈춘 동안 울리던 스템은 그만큼 뒤로 밀립니다. 채보는 벽시계로 계속 가고, 스템 위치를 다시 맞추는 장치가 없으므로 **그 스템이 끝날 때까지** 어긋납니다. (확인: 재조정 없음 / 추정: 출력 정지 시 FMOD가 시간을 건너뛰지 않음)
   - 두 경로 모두, 새로 시작하는 소리부터는 다시 맞습니다.
6. **오프셋·싱크 설정** (판정 계산식에는 오프셋 항이 아예 없습니다):

   | 설정 | 무엇을 옮기나 | 무엇을 안 옮기나 |
   |---|---|---|
   | `AudioWait`(곡별) + 장치별 `AudioConfigure.AudioWait` | 배경음 레인의 발동 시각. `HandleInputAudio`를 끈 경우에는 자동 재생되는 노트 소리도 | 판정, 노트 표시, 누를 때 나는 키음 |
   | `MediaWait`(곡별) + `BanalMediaWait`(전역) | BGA | |
   | `UIConfigure.NoteWait`(스킨별) | 노트를 그리는 Y 위치만 | 판정 |

   (확인)

---

## 1. 분석 범위와 방법

### 1-1. 대상 (확인)

| 항목 | 값 |
|---|---|
| 설치 경로 | `D:\Games\Steam\steamapps\common\Qwilight` (Steam 앱 1910130, `buildid` 23843633) |
| 본체 | `Qwilight.dll` 1.17.13.0, ProductVersion `1.17.13+ee1a345c13b44bcbec06b97bc249c24842f8fa12`, 7,524,352 바이트, SHA-256 `b936e76a…0a06ac6e`, MVID `38daffb3-d75a-494b-a9ef-49d1b22f4240`, 파일 시각 2026-09-18 |
| 런타임 | .NET 10 (`.NETCoreApp,Version=v10.0`), **ReadyToRun**(IL과 미리 컴파일된 네이티브 코드가 함께 들어 있음). 메서드 12,772개, 타입 1,816개 |
| 오디오 | `fmod.dll` 2.3.13 (build 162576), C# 래퍼 `FMODCoreAPI.dll`. `NAudio.Wasapi/Asio` 2.3.0은 마이크 입력(`AudioInputSystem`)용 |
| 입력 | Win32 Raw Input(P/Invoke), `Vortice.DirectInput`/`XInput`, `Windows.Gaming.Input`, MIDI |
| 사용자 설정 (읽기만) | `SavesDir\Configure.json`: `LoopUnit` 8000, `AudioVariety` 7(ASIO), `AudioDataLength` 64, `InputAudioVarietyValue` 0(DJMAX), `StopLastEqualAudio` true, `BanalAudio` false, `AudioWait` 0, `MediaWait` 0, 장치별 `AudioWait` 0·`HandleInputAudio` true, 스킨별 `NoteWait` 0, `ExeClass` 128(High) |

### 1-2. 도구와 절차

| 단계 | 도구 | 결과 |
|---|---|---|
| 아티팩트 판별 | rea `inspect_managed_artifact` | 어셈블리 정체, 참조 92개, R2R 서명, .NET 10 확인 |
| 네이티브 경계 | rea `inspect_managed_native_boundaries` | P/Invoke 51개. 시간 관련은 `CreateWaitableTimerExW`, `SetWaitableTimerEx`, `WaitForSingleObject`뿐이고 **`timeBeginPeriod`·MMCSS(`AvSetMmThreadCharacteristics`)는 없음**. 입력 관련은 `RegisterRawInputDevices`, `GetRawInputData`, `GetMessageW` 등 |
| 멤버·호출 관계 | rea `inspect_managed_members` | **실패**: 메타데이터 표현 예산 64 MB(`ManagedDecodeBudget.js`의 고정 상수)를 넘음 |
| 전 메서드 CIL 역어셈블 | rea의 관리 코드 디코더(`rea-agents/dist/dotnet/*.js`)를 scratchpad에 복사하고 **예산 상수만 올려** 직접 호출 | 12,772개 메서드의 CIL을 토큰 이름까지 풀어 텍스트로 출력. 예외 절 타입은 같은 디코더의 `exception_regions`로 확인 |
| FMOD 래퍼 | 같은 방법으로 `FMODCoreAPI.dll` | `OUTPUTTYPE` 순서(WASAPI = 6, ASIO = 7), `TIMEUNIT.MS` 확인 |

- 정적 분석만 했습니다. 게임을 실행하지 않았고, 프로세스에 붙거나 파일을 바꾸지 않았습니다.
- R2R 네이티브 코드는 IL을 컴파일한 결과이므로, 동작은 IL로 판단했습니다(추정: R2R 코드가 IL과 다르게 동작할 이유가 없음).
- 이 문서의 IL 오프셋(`IL_xxxx`)과 메타데이터 토큰은 위 SHA-256/MVID 빌드에서만 유효합니다.

---

## 2. 구동 구조 한눈에 (확인)

```text
채보 루프 스레드  ← DefaultCompute.Compute: Task.Factory.StartNew(<Compute>b__1, LongRunning)
 └─ 반복 1회 (목표 간격 MillisLoopUnit = 1000/LoopUnit ms)
     ├─ 일시정지 처리 (멈춰 있으면 아래 시계 전진·노트 처리를 건너뜀)
     ├─ LoopingCounter += MillisLoopUnit × AudioMultiplier          ← 곡 시각
     ├─ 5초 앞까지의 노트를 처리 목록(_handlingNotes)에 추가
     ├─ LoopBefore: 배경음(WaitAudioNoteMap) → <Compute>b__2 → AudioSystem.Handle   (발음)
     │              BGA(WaitMediaNoteMap) → <Compute>b__3
     ├─ 처리 목록 노트: 이동, 오토 판정, 지나간 노트 미스 처리
     ├─ 입력 큐(_rawInputQueue) 모두 꺼내기
     │    → BaseNote.Judge(입력, LoopingCounter)  → HandleInputAudio(키음) → HandleJudged
     ├─ 게이지·네트워크 등
     └─ 목표시각 += MillisLoopUnit; 남은 시간 = 목표시각 − Stopwatch
          남은 시간 < −5000 ms → TimeoutException,  > 0 → Utility.Wait(대기)

Raw Input 스레드  ← DefaultControllerSystem.HandleSystem: 숨은 창 + GetMessage 루프
 └─ OnWMInput(WM_INPUT) → DefaultCompute.Input(±번호, 1, 255) → _rawInputQueue.Enqueue

컨트롤러 폴링 스레드 ← ControllerSystem / HandlingController.HandleLooping (LoopUnit Hz, DInput·XInput·WGI)
MIDI               ← MIDISystem.Handle (WinRT MidiMessageReceived 이벤트) → DefaultCompute.Input

FMOD 갱신 작업   ← AudioSystem.HandleSystem: PeriodicTimer 16.67 ms마다 System.update + 시각화
FMOD 믹서·출력   (네이티브, ASIO/WASAPI) — 받은 playSound를 다음 믹스 블록부터 출력 (추정: FMOD 일반 동작)

그리기 스레드    ← DrawingSystem.HandleSystem — 채보 루프가 갱신한 노트 좌표를 그림
```

---

## 3. 시계 (질문 1)

### 3-1. 곡 시각은 반복 카운터다 (확인)

`DefaultCompute/<>c__DisplayClass839_0::<Compute>b__1` (토큰 `0x06003050`, IL 23,301바이트)이 플레이 루프 본체입니다. 이를 C#처럼 풀어 쓰면 다음과 같습니다.

```csharp
var component = new Component(LevyingBPM, Comment.LoopUnit);   // MillisLoopUnit = 1000 / LoopUnit
double step = component.MillisLoopUnit;                          // IL_0d6b
double target = 0;                                               // loc19
var sw = new Stopwatch();                                        // IL_0ce8
var timer = CreateWaitableTimerEx(null, null, 2 /*HIGH_RESOLUTION*/, 0x1F0003);   // IL_0cfb
do {
    ... // 일시정지면 아래 시계 전진을 건너뜀 (IL_1eb9 → IL_4133)
    if (_eventPositions.TryDequeue(out var p)) LoopingCounter = p;          // 리플레이 탐색용
    else LoopingCounter += step * AudioMultiplier;                          // IL_231e ~ IL_233e
    ... // 노트·소리·입력·판정
    if (!sw.IsRunning) { sw.Start(); TrailerAudioHandler.Stop(); }          // IL_5a3e
    target += step;                                                         // IL_5a5e
    double wait = target - GetMillis(sw);                                   // GetMillis = ElapsedTicks*1000/Frequency
    if (wait < -5000) throw new TimeoutException();                         // IL_5a73
    if (wait > 0) Utility.Wait(wait, sw, timer);                            // IL_5a97
} while (!SetStop && ...);
```

- **`LoopingCounter`는 시간을 읽어서 정하는 값이 아닙니다.** 반복할 때마다 `step × 배속`을 더하는 카운터입니다. 벽시계(`Stopwatch`, 내부적으로 QPC — .NET 동작 기준 추정)는 **반복 속도를 조절하는 데만** 씁니다.
- 오디오 재생 위치(`Channel.getPosition`)나 DSP 클록(`getDSPClock`)은 이 루프에서 읽지 않습니다. `getPosition`은 `AudioSystem.Fade`·`Pause`에서만, `getDSPClock`은 `AudioSystem.Handle`·`Fade`·`HandleImmediately`에서 끝 자르기·페이드 시각 계산에만 씁니다. (확인: FMOD 호출 전수 집계)
- 화면 프레임과도 무관합니다. 그리기는 `DrawingSystem.HandleSystem`이 따로 돕니다. 그래서 **그리기만 끊기는 렉은 곡 시각에 영향을 주지 않습니다**(확인: 스레드 분리 / 추정: 공유 잠금 경합은 미분석).
- 스레드 생성: `Task.Factory.StartNew(func, token, (TaskCreationOptions)2, TaskScheduler.Default)`로, 2는 `LongRunning`입니다. 이 옵션이면 .NET은 전용 스레드를 만듭니다(추정: .NET 동작). 스레드 우선순위나 MMCSS는 설정하지 않습니다. 프로세스 우선순위만 설정값 `ExeClass`(이 PC 128 = High)로 바꿉니다. (확인)

### 3-2. 루프 간격과 대기 (확인 + 계산)

| 상수 | 값 | 위치 |
|---|---|---|
| `MillisLoopUnit` | `1000 / LoopUnit` | `Component..ctor(double, int)` |
| `LoopUnit` 기본값 | 1000 (→ 1 ms) | `Configure.Validate` `IL_050b` |
| 이 PC의 `LoopUnit` | 8000 (→ **0.125 ms**, 계산) | `Configure.json` |
| 뒤처짐 한계 | −5000 ms → `TimeoutException` | `<Compute>b__1` `IL_5a73` |

`Utility.Wait(ms, sw, timer)`의 동작 (확인):

```csharp
double until = GetMillis(sw) + ms;
if (ms >= 0.5) { SetWaitableTimerEx(timer, -(long)(ms*10000), ...); WaitForSingleObject(timer, INFINITE); }
while (GetMillis(sw) < until) Thread.SpinWait(1);
```

- 남은 시간이 0.5 ms 이상이면 고해상도 대기 타이머로 자고, 그다음 남은 부분은 회전 대기(spin)로 채웁니다.
- `LoopUnit` 8000이면 남은 시간이 거의 항상 0.5 ms 미만입니다. 그래서 **채보 루프 스레드는 사실상 회전 대기만 하며 CPU 코어 하나를 계속 씁니다**(계산 + 추정).

### 3-3. 렉 뒤: 즉시 따라잡기, 상한 5초 (확인)

- 루프가 D ms 멈췄다가 다시 돌면 `wait = target − 경과`가 음수입니다. 그래서 **대기 없이 반복을 연달아 실행**합니다. 반복마다 논리 시각이 `step`만큼 오르므로 D/step번을 몰아서 돌면 벽시계에 다시 붙습니다.
  - 예: D = 300 ms, `LoopUnit` 8000이면 2,400회입니다(계산).
  - 따라잡는 데 걸리는 실제 시간은 반복 한 번의 처리 시간에 달려 있고, 정적으로는 알 수 없습니다(미확인).
- IIDX처럼 "1.5배 이하로 천천히" 같은 속도 제한도, 허용 폭(dead band)도 없습니다. 목표는 오디오가 아니라 **벽시계**입니다.
- 5초 넘게 뒤처지면 `TimeoutException`이 납니다. `DefaultCompute/<Compute>d__839.MoveNext`가 이를 잡아(`catch System.TimeoutException`, 예외 절 확인) `NotifySystem.Notify(LanguageSystem.SlowLoopingContents)`를 띄우고 루프를 끝냅니다. (확인)
- 루프 반복 하나가 평균적으로 `step`보다 오래 걸리면 뒤처짐이 계속 쌓여서, 결국 5초에서 끊깁니다(계산). `LoopUnit`을 높일수록 반복당 허용 시간이 짧아집니다(8000이면 125 µs).
- **일시정지는 어긋나지 않습니다.** 멈춰 있는 동안 `LoopingCounter` 전진을 건너뛰고, `AudioSystem.Pause(handler, true)`가 채널마다 위치를 기록한 뒤 `setPaused(true)`를 합니다. 재개 때는 `setPaused(false)`와 끝 자르기 `setDelay`를 다시 겁니다. (확인)

### 3-4. 플레이 중 GC (확인)

`MainViewModel.OnIsPoweredModeChanged`가 처리합니다. `IsPoweredMode` = 플레이 중이고 WPF 창이 떠 있지 않을 때입니다.

1. `Utility.GreatGC`를 실행합니다.
2. `GC.TryStartNoGCRegion(LazyGCV2 × 1000 × 1000)`을 시도합니다. 이 PC는 `LazyGCV2` 8192이므로 약 8.2 GB입니다(계산).
3. 실패하면 `FailedAutoLazyGC` 알림을 띄웁니다.

플레이 중 GC 정지를 피하려는 장치입니다. 실제로 성공하는지는 실행해 봐야 압니다(미확인).

---

## 4. 오디오 (질문 2)

### 4-1. 초기화 `AudioSystem/<Init>d__62` (확인)

```csharp
FMOD.Factory.System_Create(out _targetSystem);
_targetSystem.setSoftwareChannels(4093);
_targetSystem.getSoftwareFormat(out int rate, ...);  _rate = rate / 1000;     // ms당 샘플 수
_targetSystem.setDSPBufferSize(Configure.AudioDataLength, 2);
_targetSystem.init(4093, INITFLAGS.NORMAL, IntPtr.Zero);
createChannelGroup × 3 (_audioGroups[0..2]);  setCallback(_onModified, ...);
... SetAudioVariety(Configure.AudioVariety)  → _targetSystem.setOutput(...)
```

| 항목 | 값 |
|---|---|
| 출력 형식 | `Configure.AudioVariety`(`FMOD.OUTPUTTYPE`). 기본 6 = WASAPI(`Configure.Validate` `IL_00e4`), 이 PC 7 = **ASIO** |
| DSP 버퍼 | `AudioDataLength` 샘플 × 2개. 기본 256, 이 PC 64 → 48 kHz면 블록 1.33 ms, 2개 2.67 ms(계산. 장치 샘플레이트는 미확인. ASIO에서는 드라이버 버퍼 크기가 함께 작용할 수 있음 — 추정) |
| 채널 그룹 | 0 = 배경음(BGM 레인), 1 = 키음·기본 타격음, 2 = UI 효과음 (확인: `SEAudioVolume`, Qwilight_settings.md) |
| 갱신 | `AudioSystem/<HandleSystem>d__61`: `PeriodicTimer(16.67 ms)`마다 `System.update` + 시각화 값 |

### 4-2. 플레이 중 소리를 내는 함수 `AudioSystem.Handle` (확인)

`playSound`를 부르는 곳은 둘입니다. 하나는 `AudioSystem.Handle`이고, 다른 하나는 `AudioSystem.HandleImmediately`입니다. 후자는 선곡 화면의 자동 미리보기(`MainViewModel.SetAutoComputerImmediately`)에서만 쓰고 플레이 루프에서는 쓰지 않습니다. 플레이 중의 배경음·키음·기본 타격음은 모두 `Handle`을 거칩니다.

시그니처는 `FMOD.Channel Handle(AudioNote note, int group, IAudioHandler handler, bool reverse, double fadeIn, byte volume)`입니다.

```csharp
if (note.AudioItem.Length <= note.AudioLevyingPosition) return default;    // 시작 위치가 소리 끝을 넘으면 안 냄
system.playSound(sound, _audioGroups[group], paused: false, out ch);       // ★ 즉시 재생
ch.getDSPClock(out _, out ulong parentClock);
if (fadeIn > 0) { ch.addFadePoint(parentClock, 0); ch.addFadePoint(parentClock + _rate*fadeIn, vol); }
else ch.setVolume(item.Volume * volume / 255f);
if (handler?.AudioMultiplier != 1) ch.setPitch(multiplier);
if (reverse) { ch.setPosition(len - pos - 1, MS); ch.setFrequency(-freq); }
else          ch.setPosition(note.AudioLevyingPosition, TIMEUNIT.MS);      // 시작 위치(ms)
if (note.Length.HasValue)
    ch.setDelay(0, parentClock + _rate * note.Length.Value, stopchannels: true);   // ★ 끝 자르기만
_audioHandlerMap[handler].Add(new AudioHandlerItem { Channel = ch, LevyingPosition = pos, Length = .., AudioStandardUnit = parentClock });
```

- **미래 샘플 예약은 없습니다.** `setDelay`의 시작 인자는 항상 0이고, 끝 시각만 줍니다. 그래서 모든 소리는 "부른 순간 + 다음 믹스 블록"에 나옵니다. (확인 / 블록 단위는 FMOD 일반 동작으로 추정)
- `paused: false`로 바로 시작한 뒤 `setPosition`을 거는 순서입니다. 그래서 짧은 순간 0 위치부터 섞일 여지가 있습니다(추정: FMOD는 같은 믹스 블록 전에 반영할 가능성이 큼. 미측정).

### 4-3. 배경음 발동과 "늦은 만큼 건너뛰기" (확인)

`Utility.LoopBefore(queue, now, offset, action)`는 큐 앞에서부터 `노트시각 + offset < now`인 것을 모두 꺼내 `action(노트시각 + offset, 소리들)`을 부릅니다.

배경음은 반복마다 다음처럼 발동합니다.

```csharp
LoopBefore(WaitAudioNoteMap 큐, LoopingCounter, Configure.AudioWait + Configure.BanalAudioWait, <Compute>b__2);
```

`<Compute>b__2(double t, List<AudioNote> notes)`:

```csharp
double now = IsInEvents ? lastEventPosition : LoopingCounter;
if (now > t + (note.Length ?? item.Length)) continue;          // 이미 끝났어야 할 소리는 건너뜀
StopLastEqualAudioItem(note.BMSID);                             // 중복 키음 방지 (6-4)
AudioSystem.Handle(new AudioNote {
    AudioItem = item,
    AudioLevyingPosition = (uint)(now - t + note.AudioLevyingPosition),   // ★ 논리 시계 기준으로 늦은 만큼 건너뜀
    Length = note.Length.HasValue ? (uint)(t + note.Length - now) : null,
    BMSID = note.BMSID }, group: 0, handler: this, reverse: IsCounterWave, fadeIn: 0, volume: 255);
audioChannelMap[BMSID] = channel;
```

- 늦게 발동된 소리를 늦은 만큼 안쪽에서 시작하는 보상 장치가 **있습니다.** 다만 기준이 `LoopingCounter`(논리 시계)입니다.
- 평상시에는 `now − t`가 반복 한 칸(0.125 ~ 1 ms)보다 작습니다. 보상이 실제로 쓰이는 곳은 리플레이 탐색(`IsInEvents`) 같은 위치 점프입니다.
- **벽시계 기준으로 늦은 정도는 보상하지 않습니다.** 이것이 7장 경로 (A)의 핵심입니다.

### 4-4. 렉 뒤 긴 반주(스템)와 새 소리의 관계 (확인)

- 울리고 있는 채널은 FMOD 믹서가 자기 속도로 계속 재생합니다.
- 게임 쪽에는 채널 위치를 읽어 채보와 비교하거나 다시 맞추는(`setPosition`) 코드가 없습니다. `setPosition`을 부르는 곳은 `Handle` 한 곳뿐이고, 시작할 때만 부릅니다.
- 따라서 렉 뒤에는 다음처럼 됩니다.
  - 이미 울리던 스템: 오디오 출력이 멈추지 않았다면 맞고, 멈췄다면 끝까지 늦습니다.
  - 렉 중에 시작했어야 할 스템: 늦게 시작해 끝까지 늦습니다.
  - 렉 뒤에 새로 시작하는 소리: 채보 시계에 맞습니다.
- 소리 하나하나가 각자 "시작 순간"에만 채보와 맞춰지고, 그 뒤로는 서로 독립입니다.

---

## 5. 입력과 판정 (질문 3)

### 5-1. 키보드: Raw Input, 시각 도장 없음 (확인)

`DefaultControllerSystem.HandleSystem(nint)` (전용 스레드로 추정)의 처리 순서입니다.

1. `RegisterClassEx` + `CreateWindowEx`로 숨은 창 "Qwilight"를 만듭니다.
2. `RegisterRawInputDevices`로 두 장치를 등록합니다. 키보드(UsagePage 1, Usage 6)와 마우스(1, 2)이고, 플래그는 `RIDEV_INPUTSINK`(0x100)입니다.
3. `GetMessage` / `TranslateMessage` / `DispatchMessage` 루프를 돕니다.

`OnWMInput`(WM_INPUT = 0xFF)은 `GetRawInputData`로 키 코드와 눌림/뗌을 읽습니다. 그다음 `<OnWMInput>g__HandleInput|0`을 거쳐 `DefaultCompute.Input(±입력번호, InputFlag 1, 255)`를 부릅니다. 누름은 양수, 뗌은 음수입니다.

```csharp
bool DefaultCompute.HandleInput(int input, InputFlag flag, byte power) {
    if (IsComputing == true && !IsPausing) {
        _rawInputQueue.Enqueue((input, power));      // ★ 시각 없음
        _inputFlags |= flag;  return true;
    }
    return false;
}
```

- **누른 시각을 기록하지 않습니다.** 큐에 들어가는 것은 `(int, byte)` 튜플뿐입니다.
- DirectInput·XInput·Windows.Gaming.Input 패드는 `ControllerSystem.HandleSystem`의 작업들(`<HandleSystem>b__1/3/5`)이 `HandlingController<T>.HandleLooping`으로 `LoopUnit` Hz 폴링하고, `HandlingController<T>.Input`을 거쳐 같은 `DefaultCompute.Input`을 부릅니다. 이 루프도 채보 루프와 같은 "목표시각 += 1000/LoopUnit; `Utility.Wait`" 구조입니다. (확인)
- MIDI는 `MIDISystem.Handle`(WinRT `MidiMessageReceived` 이벤트 처리기)이, 터치·포인터는 `MainWindow.OnD2DPoint*`가 바로 `DefaultCompute.Input`을 부릅니다. 어느 경로도 시각을 넘기지 않습니다. (확인)

### 5-2. 판정: 채보 루프 스레드, 큐를 꺼낸 반복의 `LoopingCounter` (확인)

`<Compute>b__1` `IL_3416` ~ `IL_3bd0`은 반복마다 `_rawInputQueue.TryDequeue`를 큐가 빌 때까지 돌립니다.

```csharp
while (_rawInputQueue.TryDequeue(out var (input, power))) {
    bool isPress = input > 0;  int lane = Math.Abs(input);
    foreach (var note in _handlingNotes)                       // 레인이 같고, 실패 안 했고, HasStand인 노트
        if (note.TargetInput == lane && !note.IsFailed && note.HasStand && (!isPress || note.Judged == Not)) {
            var j = note.Judge(input, LoopingCounter, ModeComponent, JudgmentStage, ...);   // ★ 판정 시각 = LoopingCounter
            ...
        }
}
```

`InputNote.Judge`의 계산은 다음과 같습니다.

```csharp
double meter = (loopingCounter - Wait) / ModeComponent.AudioMultiplier;      // 오프셋 항 없음
var judged = Component.GetJudged(meter, ModeComponent, judgmentStage, ...);  // 창 밖이면 Not → null
```

- 판정 시각은 "입력이 큐에서 꺼내진 반복의 논리 시각"입니다. 이 시각에는 세 가지가 섞입니다.
  - 루프 간격(0.125 ~ 1 ms) 양자화
  - Raw Input 스레드가 깨어나 큐에 넣기까지의 지연
  - 루프가 뒤처져 있다면 그 뒤처짐
- 렉 따라잡기 중에는 멈춰 있던 동안 쌓인 입력이 **첫 반복에서 한꺼번에**, 멈춘 시점 근처의 `LoopingCounter`로 판정됩니다. 그래서 실제보다 이르게 판정될 수 있습니다(계산: 최대 D ms 이르게).
- 판정 결과 기록(`InputEvent.Wait`, `JudgmentMeterEvent.Wait`)도 `LoopingCounter`입니다.

---

## 6. 키음 (질문 4)

### 6-1. 위치와 순서 (확인)

> **Qwilight는 키음을 채보 루프 스레드(`DefaultCompute.Compute` → `<Compute>b__1`)의 입력 처리 단계에서 `AudioSystem.Handle` → FMOD `playSound`로 즉시 재생한다.**
> **판정과의 순서는 `BaseNote.Judge`(판정 계산) → `HandleInputAudio`(키음) → `HandleJudged`(판정 반영)이고, 모두 같은 반복 안에서 일어난다.**

`IL_3703` ~ `IL_3765` (판정이 나온 누름):

```csharp
var data = judgedNoteData.Value;                       // Judge 결과 (이미 계산됨)
if (!IsInEvents) {
    if (isPress) HandleInputAudio(note, ref inputState);   // ★ 키음 (power = 입력 세기)
    if (!(data.IDValue == AutoLongNoteJudgment && IsAutoLongNote)) InputCountQueue.Enqueue(LoopingCounter);
}
judgmentMeter = HandleJudged(note, data, ref ...);     // 점수·게이지·판정 표시·OnNoteJudged
```

- 같은 스레드, 같은 반복 안에서 판정 계산 직후에 소리를 내므로, 판정과 소리의 기준 시각이 같습니다. 소리 요청은 판정을 반영하기 전에 나갑니다.
- 뗄 때(`input < 0`)는 판정만 하고 키음은 내지 않습니다(`HandleInputAudio`는 `isPress`일 때만 호출).
- 지나간 노트의 미스 처리 경로(`IL_2d10` ~)에는 `HandleInputAudio`가 없습니다. **놓친 노트의 소리는 나지 않습니다.**

### 6-2. `HandleInputAudio` 본체 (확인)

```csharp
void HandleInputAudio(BaseNote note, ref State s) {
    if (Configure.HandleInputAudio) {                                // 장치별 설정
        foreach (var a in note.AudioNotes) {
            double now = IsInEvents ? lastEventPosition : LoopingCounter;
            if (now >= LoopingCounter + (a.Length ?? a.AudioItem.Length)) continue;
            StopLastEqualAudioItem(a.BMSID);
            var ch = AudioSystem.Handle(new AudioNote {
                AudioItem = a.AudioItem, Length = a.Length,
                AudioLevyingPosition = (uint)(a.AudioLevyingPosition + now - LoopingCounter),  // 평상시 = 노트 자체 시작 위치
                Salt = a.Salt, BMSID = a.BMSID }, group: 1, handler: this, IsCounterWave, 0, s.inputPower);
            audioChannelMap[a.BMSID] = ch;  note.AudioChannels.Add(ch);
        }
    }
    HandleBanalAudio(ref s);                                         // BanalAudio 켜면 기본 타격음 추가
}
```

- 키음은 **노트 시각과 누른 시각의 차이와 상관없이 소리의 처음(노트에 적힌 시작 위치)부터** 납니다.
- 음량은 입력 세기 `inputPower`/255입니다. 키보드는 255이고, MIDI 등은 세기가 반영됩니다(추정).

### 6-3. 무슨 소리를 내나 (확인)

| 경우 | 소리 | 위치 |
|---|---|---|
| 창 안에서 판정된 누름 | **판정된 그 노트**의 `AudioNotes` | `IL_371d`, `IL_37af` |
| `JudgeModeDate` ≥ 1.16.57이고 결과가 가장 낮은 판정(이른 POOR)인 경우 | 같은 레인에 더 나은 후보가 없으면 그 노트(후보로 보류했다가 사용) | `IL_36e7` ~ `IL_37f2` |
| 빈 누름, `InputAudioVariety = DJMAX`(0, **이 PC**) | 처리 목록(`_handlingNotes`)에서 **그 레인의 실패하지 않은 첫 노트**. 처리 목록에는 `LoopingCounter`보다 5000 ms 앞까지의 노트가 들어감(`BaseNote.IsClose`) → 보통 "다음에 칠 노트" | `IL_3982` ~ `IL_39bd` |
| 빈 누름, `InputAudioVariety = IIDX`(1, 새 설정 기본값) | `lastIIDXInputAudioNoteMap[레인]` = **그 레인에서 마지막으로 판정된 노트**(맞힘·놓침 모두, `OnNoteJudged → SetIIDXInputAudioVariety`)나 `HandleVoid`로 처리된 노트(투명 노트로 추정)의 소리. 그 뒤 `HandleBanalAudio` | `IL_39de` ~ `IL_3b6d` |
| 빈 누름, `LowestJudgmentConditionMode = Wrong` | 빈 POOR 판정을 주고 **소리는 내지 않음**(`SetJudgedValue(Lowest)` 후 소리 경로를 건너뜀) | `IL_36a4`, `IL_38f3` |
| `HandleInputAudio = false`(장치별) | 누를 때는 소리가 없음. 대신 노트 소리를 `WaitInputAudioMap`에서 **채보 시각에 자동 재생**(배경음과 같은 `LoopBefore` 경로, `AudioWait` 적용) | `IL_2cb2` ~ `IL_2ce2` |

### 6-4. 같은 소리 다시 울리기: "중복 키음 방지" (확인)

```csharp
void StopLastEqualAudioItem(string bmsId) {
    if (Configure.StopLastEqualAudio && !string.IsNullOrEmpty(bmsId) && audioChannelMap.TryGetValue(bmsId, out var ch))
        AudioSystem.Instance.Stop(ch);
}
```

- 키는 `#WAV` ID(`BMSID`)이고, 표 `audioChannelMap`은 **배경음·키음·기본 타격음이 함께** 씁니다. 그래서 켜 두면 배경음 레인의 소리와 키 레인의 소리가 같은 ID일 때도 서로 끊습니다.
- 끄면 같은 소리가 겹쳐 울립니다. 보이스 상한은 FMOD 소프트웨어 채널 4093개입니다.
- beatoraja와 LVF는 이 동작(끊기)이 항상 켜져 있어 끌 수 없고, DJMAX는 항상 겹칩니다. Qwilight만 고를 수 있습니다.

---

## 7. 렉 뒤 "스템이 끝날 때까지 어긋남"의 경로 (질문 5)

사용자 경험: Qwilight에서 렉이 걸리면 채보와 반주의 위치가 그 반주 스템이 끝날 때까지 어긋나 있다.

1~6장에 따르면 Qwilight에는 **스템의 재생 위치를 채보 시계에 다시 맞추는 장치가 없습니다**(4-4). 그래서 한 번 생긴 어긋남은 그 스템이 끝나야 사라집니다. 어긋남이 생기는 길은 다음 둘입니다.

### 7-1. 경로 (A): 채보 루프 스레드가 멈춤 (확인 + 계산)

원인 예: GC, 디스크 I/O, 잠금 경합, OS 스케줄링. 예시로 멈춤 D = 300 ms, 스템 S의 시작 시각 t가 멈춘 구간 안에 있다고 하겠습니다.

| 순간 | 채보(`LoopingCounter`) | 벽시계 | 스템 S |
|---|---|---|---|
| 멈춤 시작 t₀ | t₀ | t₀ | 아직 시작 전 (t₀ < t < t₀+D) |
| 멈춤 끝 | t₀ (정지) | t₀ + 300 | 시작 안 됨 |
| 따라잡기 (대기 없이 2,400회 연속, 계산) | t₀ → t₀+300으로 빠르게 증가 | t₀ + 300 + ε | `LoopingCounter`가 t를 넘는 반복에서 `<Compute>b__2` 호출. 시작 위치 = `LoopingCounter − t` ≤ 0.125 ms → **사실상 처음부터** |
| 따라잡기 끝 | 벽시계와 일치 | | **벽시계 기준 약 (t₀+300 − t) + ε만큼 늦게 시작** |
| 이후 | 정상 | | 그 스템이 끝날 때까지 늦은 채로 재생 |

- `<Compute>b__2`의 "늦은 만큼 건너뛰기"는 논리 시계로 잽니다. 그런데 따라잡기 중에는 논리 시계 자체가 늦어 있으므로 보상이 0에 가깝습니다. 벽시계(`Stopwatch`)로 쟀다면 S를 정확한 위치에서 시작할 수 있었습니다.
- 멈추기 전부터 울리던 스템은 FMOD가 계속 재생했으므로 맞습니다. 이 경로에서는 **멈춘 동안 시작했어야 할 스템만** 늦습니다.
- 같은 시기의 키 입력은 따라잡기 첫 반복에서 이르게 판정되고(5-2), 지나간 노트는 따라잡는 동안 한꺼번에 미스가 됩니다.

### 7-2. 경로 (B): 오디오 출력이 멈춤 (확인: 재조정 없음 / 추정: 정지 동작)

원인 예: FMOD 믹서 스레드 지연, ASIO 버퍼 언더런. 이 PC는 64샘플 버퍼라 여유가 작습니다.

- 채보 루프는 벽시계로 계속 갑니다.
- 그동안 울리던 채널은 믹서가 멈춘 만큼 재생 위치가 뒤로 밀립니다(추정: FMOD는 출력이 끊겨도 건너뛰지 않고 이어서 재생).
- 게임은 이 차이를 모르므로 **멈춘 순간 울리던 모든 스템이 끝날 때까지** 늦습니다. 새로 시작하는 소리는 채보와 맞습니다.

### 7-3. 둘 다 멈춤 (계산)

시스템 전체가 멈추면(예: 드라이버 문제) 다음처럼 됩니다.

- 울리던 스템은 (B)처럼 늦습니다.
- 멈춘 사이 시작했어야 할 스템은 (A)처럼 늦습니다.
- 채보는 벽시계로 돌아옵니다.

결과적으로 **그 구간에 걸친 모든 스템이 각자 끝날 때까지 채보보다 D 가까이 늦습니다.**

### 7-4. 어느 경로인지 가려내는 방법 (추정, 미실험)

| 관찰 | (A) 루프 정지 | (B) 출력 정지 |
|---|---|---|
| 렉 직전부터 울리던 긴 반주 | 맞음 | 늦음 |
| 렉 도중에 시작한 스템 | 늦음 | 맞음(새로 시작) |
| 렉 직후 키음 | 반주와 맞음 | 렉 전 스템에 대해 이름 |
| 판정 | 렉 중 누른 키가 이르게 판정 | 영향 없음 |

루프백 녹음으로 긴 반주 곡을 녹음하면서 의도적으로 멈춤을 넣으면 가려낼 수 있습니다. 채보 루프만 멈추게 하려면 CPU 점유, 출력만 멈추게 하려면 ASIO 버퍼를 줄이는 식입니다.

---

## 8. 오프셋·싱크 설정 (질문 6)

| 설정 | 저장 위치 | 쓰이는 곳 | 옮기는 것 | 안 옮기는 것 |
|---|---|---|---|---|
| `Configure.AudioWait` | 곡별 (`BaseNoteFile.SetConfigure`로 불러오고 `DefaultCompute.SetWait` → `DB.SetWait`로 저장, ±1000 ms로 자름) | `<Compute>b__1` `IL_2c9b`, `IL_2cd0` | `LoopBefore`의 기준 시각 → **배경음 레인 발동 시각**, `HandleInputAudio`를 끈 경우 자동 재생되는 노트 소리 | 판정, 노트 표시, **누를 때 나는 키음** |
| `AudioConfigure.AudioWait` (게터 이름 `Configure.BanalAudioWait`) | **출력 장치별** (`AudioConfigureValues[장치 이름]`) | 위와 같은 곳에서 곡별 값과 **더해짐** | 위와 같음 | 위와 같음 |
| `Configure.MediaWait` + `Configure.BanalMediaWait` | 곡별 + 전역 | `IL_2cf9` | BGA(`WaitMediaNoteMap`) 발동 시각 | 소리, 판정 |
| `UIConfigure.NoteWait` | **스킨별** (`UIConfigureValuesV2`) | `BaseNote.GetY`, `LongNote.Paint` | **노트를 그리는 Y 위치만**: `(_y + NoteWait × LogicalYMillis − 720) × 배율 + 판정선 위치` | 판정 |
| `AutoNoteWait` | 전역 | `DefaultCompute.SetAutoNoteWait` (곡 끝) | 플레이 중 판정 오차 평균(`_noteWaits`)을 `NoteWait`에 더함 → 화면 보정 자동화 | |
| 판정 오프셋 | **없음** | `InputNote.Judge`: `(LoopingCounter − Wait) / AudioMultiplier` | | |

(모두 확인)

- **`AudioWait`는 "배경음만 옮기는" 설정입니다.** 값을 양수로 주면 배경음은 늦게 나오지만, 정확히 친 키음은 판정 시각에 바로 납니다. 그 결과 **키음과 반주가 `AudioWait`만큼 어긋납니다**(계산).
  - 출력 지연이 있는 장치에서 화면과 소리를 맞추는 용도로 보입니다(추정).
  - 키음 위주 BMS에서는 같은 곡 안의 키음과 배경음 사이를 벌리는 효과가 납니다.
- 판정을 옮기는 설정이 없으므로, 입력 지연이나 오디오 지연에 대한 판정 보정은 화면 쪽 `NoteWait`로 "보이는 위치"를 옮겨 사람이 맞추는 수밖에 없습니다. beatoraja와 LVF의 판정 오프셋이 화면 전용인 것과 같은 결론입니다.

---

## 9. 비교와 시사점

### 9-1. 다섯 구동기 비교

| | 시계 | 키음을 내는 곳 | 소리를 고르는 방법 | 렉 뒤 반주와의 관계 | 오프셋 |
|---|---|---|---|---|---|
| **IIDX** (`bm2dx`) | 화면 프레임 고정 틱(측정 Hz). 2프레임마다 반주 Voice 재생 위치와 비교해 17 ms 넘게 늦으면 틱 추가 | 틱 처리 때 (판정 입력 시각 경로는 미추적) | 레인에 걸어 둔 소리(채보의 레인 소리 교체 이벤트) | **채보가 반주 쪽으로** 약 1.5배속으로 따라잡음(300 ms → 약 0.6초) → 반주 기준으로 다시 맞음 | 이 저장소에서는 미분석 |
| **DJMAX RESPECT V** | 프레임 시계 `Time.timeAsDouble`(채보·자동 발음) + 200 Hz 고정 스텝 `fixedTimeAsDouble`(누름 판정) | FixedUpdate의 누름 판정 경로(`bhi.olk → osa/osb`) 안 | 트랙 노트의 소리, 항상 0부터 재생, 겹쳐 울림 | 0.333초 넘는 멈춤은 시간을 영구히 잃음 → 울리던 긴 반주와 곡 끝까지 어긋남. 짧은 멈춤은 밀린 소리를 처음부터 늦게 | 판정 싱크 = 판정 기준점, 노트 싱크 = 플레이 노트(표시·판정·노트 키음) 이동, 자동 반주는 고정. 1레벨 = 1 ms |
| **beatoraja** | `System.nanoTime` 벽시계, 렌더 프레임 단위 | `JudgeThread`, **판정 전에** 무조건 `keysound.play` | 판정 대상 노트의 소리. 없으면 누른 시각 이전의 가장 최근 노트(숨은 노트 포함) | 개방 루프: 밀린 소리를 한꺼번에 처음부터 늦게. 울리던 반주는 그대로 | `judgetiming`은 화면만 |
| **LR2 계열 (LVF)** | `system_clock` 벽시계 + 1000 Hz 가상 고정 틱(렌더 프레임마다 몰아서 처리) | 메인 스레드 `processInput()`(프레임 주기), 판정 결과와 무관하게 | 레인별 `keySampleIndex`: 다음 노트가 1초 이내면 그 소리, 아니면 직전 소리 | 개방 루프, late fire(처음부터 늦게) | `JudgeOffset`은 화면만 |
| **Qwilight** | `Stopwatch` 고정 간격 루프(`LoopUnit` Hz, 기본 1 kHz, 이 PC 8 kHz). 뒤처지면 대기 없이 연속 반복해 **벽시계로** 즉시 따라잡음, 5초 넘으면 중단 | 채보 루프 스레드의 입력 처리 단계. **`Judge` 계산 → 키음 → `HandleJudged`** (같은 반복) | 판정된 노트의 소리. 빈 누름은 DJMAX식(레인의 다음 노트) 또는 IIDX식(레인에서 마지막으로 판정된 노트) 중 선택 | 울리던 스템 재조정 없음. 렉 중 시작했어야 할 스템은 논리 시계 기준 보상만 받아 사실상 처음부터 늦게 → **스템 끝까지 어긋남** | `AudioWait`(곡별 + 장치별) = 배경음 발동만, `MediaWait` = BGA, `NoteWait`(스킨별) = 화면만, 판정 오프셋 없음 |

### 9-2. Qwilight에서 볼 점

| 항목 | 내용 |
|---|---|
| 판정과 소리가 한 자리에 있음 | 판정 계산 직후, 같은 스레드에서 키음을 냅니다. 그래서 "판정은 PERFECT인데 소리가 늦다"는 어긋남은 키음 쪽에서는 생기지 않습니다(둘 다 같은 `LoopingCounter`를 기준으로 함). 남는 차이는 FMOD 블록과 출력 지연뿐입니다. CP2 시도 1의 교훈(키음과 반주의 상대 오차 0 ms)과 비교하면, Qwilight는 키음은 맞추지만 **반주를 기준에 묶지 않아서** 렉 뒤 둘이 갈라집니다. |
| 고해상도 고정 간격 루프 | 화면과 분리된 1 ~ 8 kHz 루프는 판정 양자화를 줄입니다. 하지만 입력 시각 도장이 없어서 판정 정밀도는 "루프 간격 + 입력 스레드 지연"에 묶입니다. 8 kHz에서는 회전 대기로 코어 하나를 씁니다. |
| 늦은 만큼 건너뛰기가 있지만 기준이 틀림 | 시작 위치 보상 장치(`AudioLevyingPosition = now − t`)를 이미 갖고 있습니다. 이것을 벽시계(또는 오디오 장치 시계) 기준으로 쟀다면 경로 (A)의 어긋남은 사라집니다. 우리 엔진에서 가장 싸게 가져갈 수 있는 개선점입니다. |
| 오디오 시계를 보지 않음 | 경로 (B)는 장치가 실제로 내보낸 샘플 수를 기준으로 쓰지 않는 한 고칠 수 없습니다. BMS 구동기 분석(0-4)의 결론과 같습니다. |
| 즉시 따라잡기 대 천천히 당기기 | IIDX는 반주를 기준으로 채보를 천천히 당기고(화면 연속), Qwilight는 벽시계로 즉시 점프합니다(화면이 튐, 반주와는 무관). 두 방식 모두 정책 P1(기준 시계)과 렉 회복 정책의 후보로 측정할 가치가 있습니다. |

---

## 10. 미확인 사항과 다음 단계

| 항목 | 상태 | 확인 방법 |
|---|---|---|
| 반복 하나의 실제 처리 시간, 따라잡기에 걸리는 시간 | 정적으로 알 수 없음 | 실행 중 측정(외부 프로파일러, ETW CPU 샘플). 단, 프로세스에 붙지 않는 방법으로 |
| 출력이 끊길 때 FMOD(ASIO)가 시간을 건너뛰는지 이어서 재생하는지 | 추정 | 루프백 녹음 + 인위적 언더런 |
| `playSound(paused=false)` 뒤 `setPosition`이 같은 믹스 블록 전에 반영되는지 | 추정 | 시작 위치가 큰 소리로 루프백 녹음 |
| `TryStartNoGCRegion(약 8.2 GB)`이 실제로 성공하는지 | 미확인 | 실행 중 알림(`FailedAutoLazyGC`) 확인 |
| ASIO 출력에서 `setDSPBufferSize`와 드라이버 버퍼 크기의 관계 | 추정 | FMOD 문서 또는 측정 |
| 그리기 스레드와 채보 루프의 공유 잠금 경합(`_targetContentsSharing` 등) | 미분석 | 잠금을 잡는 곳 전수 추적 |
| 공개 소스와의 차이 | 교차 확인 안 함 | 같은 커밋(`ee1a345c`)의 `Compute/DefaultCompute.cs`와 대조 |
| 체감 검증 (7-4 표) | 미실험 | 루프백 녹음으로 경로 (A)/(B) 구분 |

### 부록: 주요 메서드 (`Qwilight.dll` 1.17.13, MVID `38daffb3…`)

| 메서드 | 토큰 | 내용 |
|---|---|---|
| `DefaultCompute/<Compute>d__839::MoveNext` | `0x0600306b` | 루프 작업 시작(`LongRunning`), `TimeoutException` 처리 |
| `DefaultCompute/<>c__DisplayClass839_0::<Compute>b__1` | `0x06003050` | 플레이 루프 본체 (IL 23,301바이트) |
| `DefaultCompute/<>c__DisplayClass839_1::<Compute>b__2` | `0x06003059` | 배경음 발동, 늦은 만큼 건너뛰기 |
| `…::<Compute>g__HandleInputAudio\|22` | `0x0600305d` | 키음 |
| `…::<Compute>g__StopLastEqualAudioItem\|4` | `0x06003064` | 중복 키음 방지 |
| `…::<Compute>g__SetIIDXInputAudioVariety\|15` | `0x06003063` | IIDX식 레인 소리 갱신 |
| `DefaultCompute::HandleInput` | `0x06002fc5` | 입력 큐에 넣기 (시각 없음) |
| `InputNote::Judge` | `0x06002cd0` | `(LoopingCounter − Wait) / AudioMultiplier` |
| `BaseNote::GetY` | `0x06002cad` | 화면 오프셋 `NoteWait` |
| `Utility::Wait` / `Utility::GetMillis` / `Utility::LoopBefore` | `0x060028bb` / `0x0600289d` / `0x060028aa` | 대기, Stopwatch ms, 발동 큐 |
| `Component::.ctor(double,int)` | `0x060003ac` | `MillisLoopUnit = 1000 / LoopUnit` |
| `AudioSystem::Handle` | `0x060005b1` | FMOD 재생 (즉시, 시작 위치, 끝 자르기) |
| `AudioSystem::Pause` | `0x060005af` | 일시정지·재개 |
| `AudioSystem/<Init>d__62::MoveNext` | `0x060005c9` | FMOD 초기화 |
| `DefaultControllerSystem::HandleSystem` / `OnWMInput` | `0x06000c64` / `0x06000c65` | Raw Input |
| `HandlingController\`1::HandleLooping` | `0x06000d75` | 컨트롤러 폴링 루프 |

재현:

1. rea `inspect_managed_artifact`와 `inspect_managed_native_boundaries`로 위 정체 정보를 얻습니다.
2. 멤버와 CIL은 `rea-agents/dist/dotnet`의 디코더(`ManagedPeReader`, `ManagedMetadataLayout`, `ManagedMemberRows`, `ManagedMemberInstructionDecoder`)를 복사해, `ManagedDecodeBudget.js`의 `MAX_METADATA_REPRESENTATION_BYTES`만 올린 뒤 호출합니다.
3. 메서드별 CIL 텍스트를 만들고 위 이름으로 검색합니다.
