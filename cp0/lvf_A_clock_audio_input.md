# Lunatic Vibes F 소스 분석 (A): 시간 기준, 스레드/루프, 오디오, 입력

- 대상: `bms/third_party/lunaticvibesf` (v0.27.2, 커밋 `3e8a655`, shallow clone), FMOD Core 2.02.07 (`ext/fmod/inc/fmod_common.h:60`)
- 범위: (1) 시간 기준 (2) 스레드/루프 (3) 오디오 (4) 오디오와 차트의 동기화 (5) 입력 (6) 설정 키
- 표기: **(확인)** 코드에서 직접 읽은 사실 / **(추정)** 코드에서 추론했거나 외부 문서·상식에 의존한 내용
- 경로는 모두 `src/` 기준이고 줄 번호는 해당 파일 기준이다.

---

## 0. 한눈에 보는 구조

```
[Main thread "LunaticVibesF"] ── 렌더 프레임마다 1회 (MaxFPS 기본 480, VSync 기본 ON)
   t = Time::now()  (system_clock)
   ├─ SDL_PollEvent             (창 이벤트. Linux에서는 키 상태도 여기서 갱신)
   ├─ scene->processInput()     ← InputLoop 큐를 비우고 판정과 키음 재생 콜백 실행
   ├─ FixedUpdater(t, rate)     ← 플레이 씬은 1000Hz 가상 tick을 '몰아서' 실행
   │     └─ ScenePlay::update_fixed(tick_t) → chart->update(rt), ruleset->update(t), BGM 노트 발음
   ├─ scene->update()/draw()    (스킨은 Time::now()로 다시 갱신)
   └─ graphics_flush()          (SDL_RenderPresent 후 preciseSleep로 프레임 캡)

[InputLoop]   AsyncLooper, 기본 1000Hz. DirectInput8 상태를 폴링하고 Time::now()를 찍어 mutex 큐에 push
[SoundDriver] AsyncLooper 1000Hz. FMOD System::update()와 볼륨 페이드
[FMOD 내부 믹서 스레드]  (추정) DSP buffer 256×4가 기본
[thread_pool / std::async] 키음 디코드 로딩, BGA, 동영상 디코드(자체 system_clock 페이싱), 아레나 네트워크(60Hz looper)
```

핵심 결론:
- 게임 시계는 `std::chrono::system_clock` 하나다. 오디오 장치 위치는 한 번도 읽지 않는 순수 open-loop 구조다.
- 게임 로직은 1ms 고정 tick으로 돈다. 다만 tick은 별도 스레드가 아니라 렌더 프레임마다 메인 스레드에서 몰아서(burst) 실행된다. 그래서 BGM 발음 시각은 렌더 프레임 단위로 양자화된다.
- 입력은 별도 스레드가 1kHz로 폴링하고, 폴링 시각(ms 해상도)을 판정에 쓴다. 판정 시각은 렌더와 분리되어 있지만, 키음 발음은 메인 스레드 프레임 시점에 일어난다.

---

## 1. 시간 기준 (master clock)

### 1.1 Time 타입: system_clock 기반, ns와 ms를 함께 저장 (확인)

`common/beat.h:37-59`
```cpp
// Regular time, expect millisecond ...
using timeNormRes = std::chrono::milliseconds;
// High resolution time, expect nanosecond ...
using timeHighRes = std::chrono::nanoseconds;
...
static Time now()
{
    return {std::chrono::duration_cast<timeHighRes>(std::chrono::system_clock::now().time_since_epoch()).count(), true};
};
```
- `_regular`(ms)와 `_highres`(ns)를 둘 다 들고 있다. `norm()`은 ms, `hres()`는 ns를 돌려준다(`beat.h:108-109`). (확인)
- `Time(long long n)` 생성자의 기본값은 **ms로 해석**한다(`beat.h:60-73`). 그래서 `t - State::get(IndexTimer::PLAY_START)`처럼 ms 정수를 빼는 코드가 암묵적으로 ms 단위 Time이 된다. (확인)
- **monotonic이 아니다.** 게임 시계 전체가 `system_clock`이다. NTP 보정이나 사용자의 시각 변경이 있으면 시계가 점프할 수 있다. (확인: system_clock 사용 / 추정: 점프가 미치는 영향)
  - MSVC의 `system_clock::now()`는 `GetSystemTimePreciseAsFileTime` 기반으로 해상도가 100ns다. (추정, MSVC STL 구현 지식)
  - 루프 페이싱에는 이와 별개로 `high_resolution_clock`을 쓴다(`common/asynclooper.cpp`, `common/utils.cpp`, `window_SDL2.cpp`). MSVC에서는 이것이 steady_clock(QPC)이고, libstdc++에서는 system_clock의 별칭이다. (추정)
- QPC 직접 호출, SDL_GetTicks, 오디오 위치를 시계로 쓰는 코드는 없다. grep 결과 `QueryPerformance`와 `SDL_GetTicks`는 0건이다. (확인)

### 1.2 Timer 저장소는 ms 정수 (확인)
- `State::set(IndexTimer::X, t.norm())` 형태로 모든 스킨 타이머와 씬 타이머를 **ms 정수**로 저장한다. 예: `game/game.cpp:427-429`, `scene_play.cpp:1862`.

### 1.3 플레이 시간(rt) 계산 (확인)

`game/scene/scene_play.cpp:1854-1863` (READY에서 PLAYING으로 넘어가는 tick)
```cpp
const auto rt = t - State::get(IndexTimer::PLAY_READY);
if (rt > pSkin->info.timeGetReady)
{
    changeKeySampleMapping(USE_FIRST_KEYSOUNDS);
    State::set(IndexTimer::PLAY_START, t.norm());   // ms로 잘림
    _handleJudgeInput = true;
    gChartContext.started = true;
```
`scene_play.cpp:1886-1896` (PLAYING 상태의 각 tick)
```cpp
auto rt = t - State::get(IndexTimer::PLAY_START);
...
chart->update(rt);      // 차트는 PLAY_START 기준
ruleset->update(t);     // 룰셋은 자기 _startTime 기준
```
- `t`는 FixedUpdater가 넘겨주는 **가상 tick 시각**(1ms 격자)이다. 실제 현재 시각이 아니다(2절 참고). (확인)
- 차트 시각 `rt = tick_t − PLAY_START(ms)`. 노트 시각 `Note::time`은 차트 컴파일 시점에 정해진 ns 값이다. (확인)
- **룰셋 원점은 차트 원점과 다르다.** `RulesetBMS::update`는 처음 호출될 때 `setStartTime(t)`를 한다(`ruleset/ruleset_bms.cpp:1607-1612`). 처음 호출되는 시점은 PLAY_START를 기록한 tick의 다음 tick이다. 그래서 `_startTime ≈ PLAY_START + 1 tick(1ms)`가 된다. 판정 rt가 차트 rt보다 약 1ms 작다는 뜻이다. (확인: 호출 순서 / 추정: 정량 효과)

### 1.4 시작 카운트다운 (확인)
- PREPARE → LOADING → LOAD_END(READY) → PLAYING 상태 머신이다(`scene_play.cpp:1292-1301`).
  - PREPARE: `SCENE_START`에서 스킨 `#STARTINPUT`(timeIntro)이 지나면 로딩을 시작한다(`1786-1798`).
  - LOADING: 로딩 완료, `t - delayedReadyTime > 1000`ms, 스킨 `#LOADEND`(timeMinimumLoad)가 모두 충족되면 READY로 간다(`1822`).
    - START나 SELECT를 누르면 `delayedReadyTime = t`가 되어 시작이 미뤄진다(`3227-3231`). LR2의 "시작 지연" 동작을 재현한 것으로 보인다. (추정)
  - READY: 스킨 `#PLAYSTART`(timeGetReady)가 지나면 PLAY_START를 기록한다(`1858-1862`, 파서는 `skin/skin_lr2.cpp:1000-1003`).
- 카운트다운 동안 차트 시계는 돌지 않는다. PLAY_START가 원점이고, 그 이전에는 `chart->update`를 호출하지 않는다. (확인)

### 1.5 일시정지 (확인)
- **일시정지 기능이 없다.** `grep -i pause game/` 결과 키 코드(K_PAUSE 등) 외에는 0건이다. LR2에도 플레이 중 일시정지가 없다. (추정)

### 1.6 재생 속도 (pitch/speed) (확인)
- 시계 자체의 배속은 바꾸지 않는다. 대신 **차트 컴파일 단계에서 BPM에 `pitchSpeed`를 곱해** 노트 시각을 다시 계산한다.
  - `game/chart/chart_bms.cpp:73`: `BPM bpm = objBms.startBPM * gSelectContext.pitchSpeed;` (733, 746, 801도 같은 방식)
- 오디오 쪽은 FMOD ChannelGroup의 pitch를 바꾼다(`skin/skin_lr2_button_callbacks.cpp:102-125`, `sound/sound_fmod.cpp:1094-1123`).
  - FREQUENCY: `setFreqFactor(f)`(ChannelGroup::setPitch(f)), `pitchSpeed=f`. 속도와 음정이 함께 변한다.
  - PITCH: ChannelGroup pitch는 1.0이고 PITCHSHIFT DSP만 쓴다. `pitchSpeed=1`이므로 속도는 그대로다.
  - SPEED: ChannelGroup pitch를 f로, PITCHSHIFT를 1/f로 둔다. `pitchSpeed=f`. 음정은 유지하고 속도만 바뀐다.
- `pitchSpeed < 1.0`이면 스코어를 저장하지 않는다(`scene/scene_context.cpp:34-35`). (확인)
- BGA 동영상에도 `pitchSpeed`를 넘긴다(`graphics/texture_bms_bga.cpp:44, 310`). (확인)

---

## 2. 스레드와 루프

### 2.1 AsyncLooper: 공용 주기 스레드 (확인)

Windows 구현(`common/asynclooper.cpp:65-133`)의 요지:
```cpp
long long us = _rate > 0 ? 1000000 / _rate : 0;          // :75
long long reset_threshold = us * 4;                        // :76
...
SetWaitableTimerEx(handler, &dueTime, 0, ...);             // :92
WaitForSingleObjectEx(handler, 1000, TRUE);                // :94
run();                                                     // :97
if (us > (t2 - tStart)) dueTime = -(us - (t2 - tStart))*10; else dueTime = 0;
if ((t2 - tStart) < reset_threshold) tStart += us; else tStart = t2;   // :110-116
```
- **데드라인(절대 일정) 방식**이다. 다음 깨어남 시각은 `tStart + us`다. 늦어진 정도가 4주기 미만이면 대기 없이 연속 실행해서 따라잡는다. 4주기 이상이면 일정을 리셋한다(밀린 tick은 버린다). (확인)
- 타이머는 `CreateWaitableTimerExA(nullptr, nullptr, 0, TIMER_ALL_ACCESS)`로 만든다(`:25`). `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` 플래그를 쓰지 않는다. (확인)
  - 따라서 해상도는 `timeBeginPeriod(1)`(`game/game.cpp:285-287`)에 기대며 약 1ms다. (추정)
  - UI 도움말은 "Windows Timer 최소 간격 0.5ms (Win7은 1ms)"라고 설명한다(`res/translations/en.txt:116`). (확인)
- Linux 대체 구현(`asynclooper.cpp:160-194`)은 `run()` 뒤에 `preciseSleep(prev + period - now)`를 하고 `prev = now()`로 갱신한다. 매 주기 이전 깨어남 시각을 기준으로 삼으므로 수면 오차가 누적된다. 절대 일정 방식이 아니다. (확인 / 추정: 드리프트 누적)
- 생성자의 `single_inst` 인자는 쓰이지 않는다. 스레드 우선순위(SetThreadPriority, MMCSS)를 설정하는 코드도 없다(grep 0건). (확인)

### 2.2 preciseSleep (프레임 캡과 Linux looper에서 사용) (확인)

`common/utils.cpp:458-500`
```cpp
while (sleep_ns > nsInMs) {            // 1ms 단위로 waitable timer
    due.QuadPart = -int64_t((sleep_ns - sleep_ns % nsInMs) / 100);
    SetWaitableTimerEx(timer, &due, ...); WaitForSingleObjectEx(timer, INFINITE, TRUE);
    sleep_ns -= observed;
}
while (high_resolution_clock::now() < end) std::this_thread::yield();   // 나머지는 spin
```
- Windows는 ms 단위 waitable timer와 yield-spin을 섞는다. Linux는 `sleep_for`만 쓴다. 주석에 "spin을 넣으면 키 입력 시 FPS 저하"가 기록되어 있다. (확인)

### 2.3 스레드 목록

| 스레드 | 생성 위치 | 주기 | 대기 방식 | 하는 일 |
|---|---|---|---|---|
| Main "LunaticVibesF" | `game/game.cpp:89, 356-472` | 렌더 프레임 (MaxFPS 기본 480, VSync) | Present(VSync) 후 `preciseSleep` 프레임 캡 (`window_SDL2.cpp:235-243`) | SDL 이벤트, 입력 큐 처리(판정·키음), **고정 tick 로직(차트, BGM, 룰셋)**, 스킨 갱신, 렌더 |
| InputLoop | `input/input_wrapper.cpp:14-17` (AsyncLooper "InputLoop") | `InputPollingRate`, 기본 1000Hz. UI 선택지는 1000/2000(Win10+), Linux는 8000까지 | 2.1의 waitable timer | 장치 폴링, 엣지 검출, 타임스탬프, mutex 큐 push |
| SoundDriver | `sound/sound_driver.h:61` | 1000Hz 고정 | 2.1 | `fmodSystem->update()`, sys/note 볼륨 그라디언트 (`sound_fmod.cpp:844-890`) |
| FMOD 내부 믹서/스트림 스레드 | FMOD 내부 | DSP 버퍼 주기 | - | 믹싱 (추정, FMOD 구조) |
| boost::asio thread_pool | `common/thread_pool.cpp:10-21` | 이벤트 | - | 하드웨어 스레드 수 − 2개. 키음 병렬 로드(`chart/chart_load_task.cpp:59-81`) |
| std::async 작업 | `scene_play.cpp:1194, 1204`; `graphics/video.cpp:181` 등 | 이벤트 | - | 샘플/BGA 로딩 관리, **동영상 디코드(자체 system_clock 페이싱)** |
| ArenaClient / ArenaHost | `arena/arena_client.h:36`, `arena_host.h:25` | 60Hz | 2.1 | LAN 대전 |

### 2.4 메인 루프와 고정 tick: 실제 구조 (확인)

`game/game.cpp:326-354` FixedUpdater ("Fix your time step")
```cpp
_acc += t - _last_update_time;
const auto dt = lunaticvibes::Time{1000 / rate};   // ms 정수 나눗셈
while (_acc >= dt) {
    const auto this_tick_time = t - _acc;
    cb(this_tick_time);
    _acc -= dt;
}
_last_update_time = t;
```
`game/game.cpp:438-469` (한 프레임 안의 순서)
```cpp
if (scene) scene->processInput();                 // 입력 콜백 (판정, 키음)
lunaticvibes::window::graphics_clear();
doMainThreadTask();
update_scene(t, scene->getRate(), ...update_fixed...);   // tick 몰아서 실행
scene->update(); scene->draw();
lunaticvibes::window::graphics_flush();          // Present + 프레임 캡
```
- tick 비율: ScenePlay는 1000Hz(`scene_play.cpp:211`), Result/Decide/CourseResult도 1000Hz, Select는 250Hz, 그 외 씬은 240Hz(기본값 `scene.h:89`). (확인)
  - `dt = Time{1000/rate}`가 ms 정수 나눗셈이라 rate 240은 실제로 4ms, 즉 250Hz가 된다. (확인: 산술)
- **tick 시각은 `t0 + k·dt` 격자에 정렬된다.** 각 tick은 실제 시각이 `tick 시각 + dt` 이후가 되어야 실행된다. 결과적으로 tick의 처리 지연은 `dt + 프레임 내 위상`이고 범위는 [1ms, 1ms + 렌더 프레임 주기]다. (확인: 산술 / 추정: 지연 해석)
- **따라잡기 상한이 없다.** `while (_acc >= dt)`라서 렌더가 100ms 멈추면 다음 프레임에 100개의 tick을 연속 실행한다. IIDX처럼 "+1 tick씩 서서히" 보정하는 방식이 아니다. (확인)
- **tick은 렌더 스레드에 묶여 있다.** README의 "Asynchronous input handling"은 입력 폴링만 비동기라는 뜻이다. 게임 로직 tick은 렌더 프레임에 종속된다. (확인)
- `scene->update()`는 **tick 시각이 아니라 `Time::now()`** 로 스킨을 다시 갱신한다(`scene/scene.cpp:125-132`). (확인)
  - 노트의 y 위치는 NORMAL 속도 모드에서 차트의 `_currentMetre`(마지막 tick 기준)를 쓴다(`graphics/sprite_lane.cpp:127-153`).
  - CONSTANT 모드에서는 렌더 시각 `t - PLAY_START`를 쓴다(`sprite_lane.cpp:162`).

### 2.5 공유 상태 전달 방식 (확인)
- **입력**: InputLoop가 `std::mutex _inputQueueMutex`로 보호되는 `std::queue<...Event>`에 push한다. 메인 스레드는 `processInput()`에서 lock을 잡고 vector로 drain한 뒤 lock 없이 콜백을 실행한다(`input/input_wrapper.cpp:233-248, 256-300`). 커서와 `_prev/_curr`는 별도의 `_inputMutex`로 보호한다.
- **Linux SDL 입력 상태**: `sdl::state::g_input_mutex`(shared_mutex). 메인 스레드의 `SDL_PollEvent`가 쓰고 InputLoop가 읽는다(`graphics/SDL2/window_SDL2.cpp:439-451`, `input/platform/sdl2/input_sdl2.cpp:108-112`).
- **사운드 볼륨**: `std::atomic<float>`(`sound/sound_fmod.h`). FMOD Core API는 기본적으로 thread-safe다(FMOD_INIT_THREAD_UNSAFE 미사용). (확인: 플래그 / 추정: FMOD 동작)
- **로딩 진행 상황과 리플레이**: `gPlayContext._mutex`(shared/unique lock), `gChartContext.concurrent.mutex`.
- **메인 스레드 작업 큐**: `s_main_thread_task_mutex`가 shared_mutex인데, `doMainThreadTask()`는 **shared_lock을 잡은 상태로 pop**한다(`common/sysutil.cpp:24-25, 48-56`). 잠재적인 데이터 레이스다. (확인: 코드 / 추정: 실제 영향)
- lock-free 큐는 없다. atomic은 `_running`, 볼륨, 캔버스 크기 정도에만 쓴다. (확인)

### 2.6 프레임 캡과 VSync (확인)
- `window_SDL2.cpp:111-118`
  ```cpp
  int maxFPS = ConfigMgr::General()->get(cfg::V_MAXFPS, 480);
  if (maxFPS < 30 && maxFPS != 0) maxFPS = 30;
  ...
  if (ConfigMgr::General()->get(cfg::V_VSYNC, false)) renderer_flags |= SDL_RENDERER_PRESENTVSYNC;
  ```
- VSync는 렌더러를 만들 때 플래그로만 적용된다. 그래서 변경하려면 재시작이 필요해 보인다(UI는 config 저장만 한다, `scene_select_imgui.cpp:1620`). (확인 / 추정: 재시작 필요)
- 프레임 캡은 Present 뒤에 `frameTimestampPrev + period - now`만큼 수면한다. `frameTimestampPrev`를 수면 **후**에 갱신하므로 이전 프레임 종료 시점 기준 방식이다(`window_SDL2.cpp:235-243`). (확인)
- Windows 렌더러는 `direct3d11` 고정 힌트다(`window_SDL2.cpp:73`). 주석에 각 렌더러의 문제점이 정리되어 있다. (확인)
- "Adaptive" VSync 번역 문자열은 있지만(`en.txt:35`) 코드는 bool만 지원한다. (확인)

---

## 3. 오디오 시스템

### 3.1 백엔드: FMOD Core (Low Level API) 2.02.07 (확인)
- `sound/sound_fmod.h:14`: "This game uses FMOD Low Level API"
- 추상 계층은 `SoundDriver`(AsyncLooper 상속)이고 구현체는 `SoundDriverFMOD` 하나뿐이다(`sound/sound_mgr.cpp:18`).
- LR2도 FMOD를 썼다는 단서가 있다: `sound_fmod.cpp:955` "The DSP type LR2 used (FMOD_DSP_TYPE_REVERB) is long gone". (확인: 주석)

### 3.2 출력 장치와 드라이버 (확인)
- 기본값은 `FMOD_OUTPUTTYPE_AUTODETECT`다(`sound_fmod.cpp:62`). Windows에서는 FMOD 기본값인 **WASAPI shared**가 된다. (추정, FMOD 동작)
- **ASIO 지원**: 레지스트리 `HKLM\SOFTWARE\ASIO`를 열거해서(`sound_fmod.cpp:361-408`) 장치 목록에 `DriverIDUnknownASIO(-10)`으로 추가한다. 선택하면 `setOutput(FMOD_OUTPUTTYPE_ASIO)`를 호출한다(`:76-78`, `:546-554`).
- **WASAPI exclusive 옵션은 없다.** `AudioOutputMode` 키(`config/cfg_general.h:74-84`, WinMM/DirectSound/WASAPI/ASIO 등)는 정의되어 있지만, 코드에서 읽는 곳이 없다. UI에서 Auto 또는 ASIO로 저장만 한다(`scene_select_imgui.cpp:1691`). (확인)
- 사소한 버그: `if (driver > 0)`(`:107`)라서 드라이버 인덱스 0을 명시적으로 선택할 수 없다. 기본값이 0이라 실질적인 해는 거의 없다. `FIXME: switch on a wrong value?`(`:76`)도 있다. 채널 그룹 생성 루프가 두 번 반복되어 첫 그룹이 누수된다(`:227-255`). (확인)

### 3.3 DSP 버퍼(지연)와 voice 수 (확인)
```cpp
int len = ConfigMgr::General()->get(cfg::A_BUFLEN, 128);    // :132
int count = ConfigMgr::General()->get(cfg::A_BUFCOUNT, 2);  // :133
fmodSystem->setDSPBufferSize(len, count);
fmodSystem->setSoftwareChannels(512);                       // :105 "64 is obviously not enough"
fmodSystem->init(512, FMOD_INIT_NORMAL, nullptr);           // :144
```
- 설정 파일의 실효 기본값은 **BufferLength 256, BufferCount 4**다(`config/cfg_general.cpp:14-15`). `vConfig::load()`가 `setDefaults()`를 먼저 호출하기 때문이다(`config/config.cpp:47`). 코드 안의 폴백 값 128/2는 쓰이지 않는다. (확인)
  - 48kHz 기준으로 256×4는 약 21.3ms의 출력 버퍼 지연이다. (추정: FMOD 지연 ≈ len×count)
- init 후 실제로 적용된 값(드라이버가 바꿨을 수 있음)을 config에 다시 기록한다(`:212-215`). (확인)
- 소프트웨어 voice 512개, 가상 voice 512개. (확인)

### 3.4 샘플 로딩 (확인)
- 키음과 BGM(`loadNoteSample`, `sound_fmod.cpp:667-716`):
  ```cpp
  int flags = FMOD_LOOP_OFF | FMOD_UNIQUE | FMOD_CREATESAMPLE | FMOD_IGNORETAGS | FMOD_LOWMEM;
  ```
  - `FMOD_CREATESAMPLE`이므로 **로드할 때 전체를 메모리에 디코드한다(스트림 아님)**. (확인: 플래그 / 추정: FMOD 의미)
  - `FMOD_UNIQUE`: FMOD 문서상 "한 번에 하나의 인스턴스만 재생"이다. **같은 WAV를 다시 트리거하면 이전 voice를 끊는다(choke)**. (추정: FMOD 문서 기준. 믹싱 정책으로 중요하다)
  - 슬롯 수는 62×62+1 = 3845개다(`sound_fmod.h:46-47`, base-62 BMS).
  - 확장자를 찾지 못하면 `.wav/.ogg/.flac`로 대체 탐색한다(Linux는 대문자 확장자도)(`:659-665, 688-703`). 실제 디코드 가능한 포맷은 FMOD가 지원하는 전부다. (추정)
  - 로딩은 asio thread_pool에서 병렬로 `createSound`를 호출한다(`chart/chart_load_task.cpp:59-81`). (확인)
- 시스템 사운드는 `FMOD_DEFAULT|FMOD_UNIQUE`에 `isStream`이면 `CREATESTREAM`을 쓴다(`:773-775`). 선곡 BGM과 코스 BGM만 stream+loop다(`sound/sound_sample.cpp:23, 43-44`). (확인)

### 3.5 채널 그룹과 믹싱 경로 (확인)
- 그룹은 `BGM_SYS, BGM_NOTE, KEY_SYS, KEY_LEFT, KEY_RIGHT` 다섯 개다. 각 그룹에 DSP 슬롯 3개(Master/Key/Bgm용 FX), PITCHSHIFT(FFT 512), 3밴드 EQ 2개가 붙는다(`sound_fmod.cpp:225-322`).
- **차트 BGM 노트는 `KEY_LEFT` 그룹으로 재생된다**(`scene_play.cpp:2591`). 그래서 BGM 볼륨 슬라이더가 아니라 KEY 볼륨을 따른다. `BGM_NOTE` 그룹은 재생 경로에서 쓰이지 않는다. (확인)

---

## 4. 오디오와 차트의 동기화

### 4.1 결론: 완전한 open-loop이고 지연 보정이 없다 (확인)
- `getDSPClock`, `setDelay`, `Channel::getPosition`, `getLatency`를 grep하면 **0건**이다. 오디오 장치 위치나 DSP clock을 읽지도 않고, 샘플 단위로 예약하지도 않는다. (확인)
- 모든 발음은 `fmodSystem->playSound(sample, group, false, nullptr)` 즉시 재생이다(`sound_fmod.cpp:718-730`). (확인)

### 4.2 BGM과 오토 노트를 트리거하는 코드 (확인)

`game/chart/chart.cpp:336-339, 404-413`
```cpp
lunaticvibes::Time vt = rt + lunaticvibes::Time(State::get(IndexNumber::TIMING_ADJUST_VISUAL), false);
const lunaticvibes::Time& at = rt;
...
// Skip expired plain note
for (size_t idx = 0; idx < _bgmNoteLists.size(); ++idx) {
    auto it = incomingNoteBgm(idx);
    while (!isLastNoteBgm(idx) && at >= it->time) {
        noteBgmExpired.push_back(*it);
        it = nextNoteBgm(idx);
    }
}
```
`scene_play.cpp:2581-2591` (`procCommonNotes`는 매 PLAYING tick의 `updatePlaying`에서 호출된다, `:2141`)
```cpp
for (; i < max && it != ...noteBgmExpired.end(); ++i, ++it)
    _bgmSampleIdxBuf[i] = static_cast<unsigned>(it->dvalue);
SoundMgr::playNoteSample(SoundChannelType::KEY_LEFT, i, _bgmSampleIdxBuf.data());
```
- 비교식은 `tick의 at ≥ note.time`이다. 늦은 tick에서도 **건너뛰지 않고 늦게 발음**한다(late fire). 시각 오프셋도 보정하지 않으므로 샘플 앞부분이 잘리지 않고 통째로 늦게 나온다. (확인)
- 한 tick에 발음할 수 있는 수는 `_bgmSampleIdxBuf`(128개, `scene_play.h:131`)로 제한된다. 넘치는 BGM 노트는 `min(...)` 때문에 **조용히 버려진다**. 대량 따라잡기 burst가 일어날 때 의미가 생긴다. (확인: 코드 / 추정: 발생 조건)
- 오토플레이 키음: `noteExpired`(vt 기준이며 ruleset이 expired 표시한 노트)를 같은 함수에서 발음한다(`:2593-2610`). 차트 update가 룰셋 update보다 먼저 돌기 때문에(`:1895-1896`), 오토 노트는 다음 tick에 발음된다. BGM보다 약 1 tick(1ms) 늦다. (확인: 순서 / 추정: 효과)
  - 시각 오프셋(`TIMING_ADJUST_VISUAL`)이 0이 아니면 오토 키음은 vt의 영향을 받고 BGM은 받지 않는다. 오토 모드에서 BGM과 키음이 어긋날 수 있다. (추정)

### 4.3 실제 발음 시각의 오차 모델 (추정, 2.4절에서 도출)
```
실제 발음 시각 ≈ PLAY_START_real + note.time
                + [tick 처리 지연: dt(1ms) + 렌더 프레임 위상(0 ~ 1프레임)]
                + FMOD 믹스 버퍼 지연(256×4 @48k ≈ 21ms, 추정)
```
- 렌더 MaxFPS 480이면 지터는 약 2ms 이내다. VSync 60Hz이면 **BGM 발음 지터가 최대 약 16.7ms**에 이른다. 키음이 BGM과 같은 지터를 공유하므로 상대적인 리듬도 흔들린다.
- **드리프트 보정은 없다.** 다만 FMOD가 각 샘플을 독립적으로 시작하므로 오디오 장치 클럭과 system_clock 사이의 장기 드리프트가 누적되지 않는다. 긴 BGM WAV 하나로 된 곡에서만 장치 클럭 드리프트가 그 WAV 길이만큼 발생한다. (추정)
- IIDX와의 대비: IIDX는 BGM voice 커서와 비교해 +1 tick씩 보정하는 closed-loop 방식이다. LVF는 시스템 시계만 따른다.

### 4.4 판정 오프셋은 시각 전용이다 (확인)
- 프로필 키 `JudgeOffset`은 `IndexNumber::TIMING_ADJUST_VISUAL`로 로드된다(`config/config_mgr.cpp:46`). 이 값은 **`chart.cpp:338`의 vt(노트 표시와 노트 소진 판정)에만** 쓰인다.
- 판정식 `_calcJudgeByTimes`는 `press_rt − note.time`을 오프셋 없이 계산한다(`ruleset/ruleset_bms.cpp:946-950`).
- AUTO ADJUST는 판정이 BAD 이상인 노트 10개마다 `TIMING_ADJUST_VISUAL`을 ±1ms 움직인다(`ruleset_bms.cpp:1389-1410`). 시각 쪽을 판정에 맞추는 방향이다.
- 오디오 출력 지연이나 입력 지연을 보정하는 별도 설정은 없다. (확인: 설정 키가 없음)

---

## 5. 입력

### 5.1 캡처 방식: 전용 폴링 스레드 + 상태 스냅샷 (확인)
- **Windows: DirectInput8 즉시 상태(immediate) 폴링**(`input/platform/windows/input_dinput8.cpp:186-233`)
  ```cpp
  deviceKeyboard.lpdid->GetDeviceState(sizeof(deviceKeyboard.state), deviceKeyboard.state)
  ...
  HRESULT hres = j.lpdid->Poll();
  j.lpdid->GetDeviceState(sizeof(j.state), &j.state)
  ```
  - 버퍼 모드(`GetDeviceData` 또는 `DIPROP_BUFFERSIZE`)는 **쓰지 않는다**. 그래서 OS 이벤트 시각이 없다. 폴링 간격보다 짧게 눌렀다 뗀 입력은 놓칠 수 있다(1kHz면 1ms 미만의 탭). (확인: 코드 / 추정: 영향)
  - `SetCooperativeLevel` 호출이 없다. 키보드, 마우스, 조이스틱(`DIPROPAXISMODE_ABS`) 모두 해당한다. Raw Input이나 WM_INPUT은 쓰지 않는다. (확인)
- **Linux(SDL2)**: `pollInput()`이 비어 있다(`platform/sdl2/input_sdl2.cpp:103-106`). 키 상태는 **메인 스레드의 `SDL_PollEvent`가 갱신**한다(`window_SDL2.cpp:439-451`). 시작 시 다음 경고를 남긴다.
  ```cpp
  LOG_WARNING << "[SDL2] Direct input isn't available, input precision is limited by draw FPS";  // input_sdl2.cpp:59
  ```
  SDL 이벤트의 `timestamp`도 쓰지 않는다. (확인)

### 5.2 타임스탬프: 폴링 시각, ms 해상도 (확인)

`input/input_wrapper.cpp:24-56`
```cpp
void InputWrapper::loopAsync()
{
    const auto now = lunaticvibes::Time::now();       // 폴링 '시작' 시각 (system_clock)
    auto [curr, scratch_data] = InputMgr::detect();   // 그 다음에 장치 폴링
    ...
    if (curr[i] && !stat) {
        ms = now.norm();                              // ms로 잘림
        ...
        pTimes[i] = ms; // TODO: take time from the input manager lol.
```
- 타임스탬프는 **OS 이벤트 시각이 아니라 폴링 루프가 시작된 시각**이다. 장치를 읽기 직전에 찍으므로 체계적으로 0 ~ 폴링 주기만큼 이르게 기록된다. (확인: 순서 / 추정: 편향)
- 해상도: 판정에 들어가는 `pTimes`는 ms 정수다(`Time(ms)`). 이벤트의 `t`는 ns를 보존한다. 룰셋은 `tt[k] - _startTime`을 쓴다(`ruleset_bms.cpp:1499`). (확인)
- `InputMgr::_detect()`도 자체적으로 `Time::now()`를 찍지만(`input/input_mgr.cpp:105`) 디바운스 계산에만 쓴다. (확인)
- 판정은 메인 스레드에서 일어난다. `processInput()`이 큐의 이벤트를 순서대로 `inputGamePress(mask, t, times)`에 넘기고(`input_wrapper.cpp:256-280`, `scene_play.cpp:2892-2905`), 판정 계산 자체는 폴링 시각을 쓴다. **판정 정확도는 렌더 FPS와 무관하다.** (확인)
- 반면 **키음 발음은 `processInput()` 시점**, 즉 다음 렌더 프레임 시작에 일어난다(`scene_play.cpp:3174-3204`). 입력에서 소리까지의 지연은 `폴링 지연 + 0~1 렌더 프레임 + FMOD 버퍼`다. (확인: 위치 / 추정: 지연 분해)

### 5.3 엣지 처리와 디바운스 (확인)
- 떼기 지연: `release_delay_ms = 5`(`input_wrapper.h:21`). 손을 뗀 상태가 5ms 이상 이어져야 release를 확정한다(`input_wrapper.cpp:57-70`).
- 떼기 디바운스: `MinInputInterval`(ms). 마지막으로 눌린 시각부터 이 시간 안이면 계속 눌린 것으로 간주한다(`input_mgr.cpp:135-139`). UI 설명은 "Also known as 'up debounce'"(`en.txt:43`).
  - 기본값은 profile 파일 기준 **5ms**(`config/cfg_profile.cpp:21`)이고, 코드 폴백은 16ms(`input_mgr.cpp:21`)다. 실효값은 5ms다. (확인)
- 창이 비활성일 때의 입력: ScenePlay는 `backgroundInput=true`로 생성되므로(`scene_play.cpp:211`) 포커스가 없어도 입력을 받는다. 다른 씬은 포커스가 없으면 입력을 버린다(`input_wrapper.cpp:38-41`). (확인)
- 1P 전용 모드에서는 2P 입력을 1P로 합친다(`setMergeInput`, `scene_play.cpp:219-222`). (확인)

### 5.4 폴링 주기 설정 (확인)
- `InputPollingRate`가 SceneBase 생성 시 InputWrapper의 looper rate가 된다(`scene/scene.cpp:31`).
- UI 선택지는 1000/2000/4000/8000Hz다. Windows는 Win10 이상에서 2000까지만 허용한다(`scene_select_imgui.cpp:830-856`). Windows 타이머 해상도 때문에 2000Hz가 실제로 나오는지는 보장되지 않는다. 실측값은 `INPUT_DETECT_FPS`로 확인할 수 있다(`generic_info.cpp:23`). (확인 / 추정)

---

## 6. 정책을 바꾸는 설정 키 (config.yml / settings.yml)

`config.yml` (General, `config/cfg_general.h`, 기본값은 `cfg_general.cpp:10-27`)

| 키 | 기본값 (setDefaults) | 코드 폴백 | 사용처 | 의미 |
|---|---|---|---|---|
| `AudioDeviceName` | (없음) | "" | `sound_fmod.cpp:64` | FMOD 드라이버 또는 ASIO 장치 이름 |
| `AudioOutputMode` | `Auto` | - | **읽히지 않음**(UI가 Auto/ASIO로 저장만 함) | WinMM/DS/WASAPI/ASIO 등이 정의만 됨 |
| `BufferLength` | **256** | 128 | `sound_fmod.cpp:132, 566` | FMOD DSP 버퍼 길이(샘플) |
| `BufferCount` | **4** | 2 | `sound_fmod.cpp:133, 567` | FMOD DSP 버퍼 개수 |
| `MaxFPS` | **480** | 480 (UI 폴백 240) | `window_SDL2.cpp:111` | 렌더 프레임 캡. 0이면 무제한, 30 미만은 30으로 올림. **로직 tick burst 주기도 결정** |
| `VSync` | **true** | false | `window_SDL2.cpp:117` | SDL PRESENTVSYNC |
| `WindowMode` | Windowed | - | `window_SDL2.cpp:83` | FullScreen / Borderless / Windowed |

`settings.yml` (Profile, `config/cfg_profile.h`, 기본값은 `cfg_profile.cpp:16-60`)

| 키 | 기본값 | 코드 폴백 | 사용처 | 의미 |
|---|---|---|---|---|
| `InputPollingRate` | 1000 | 1000 | `scene.cpp:31` | 입력 폴링 스레드 Hz |
| `MinInputInterval` | **5** | 16 | `input_mgr.cpp:21` | 떼기 디바운스(ms) |
| `JudgeOffset` | 0 | 0 | `config_mgr.cpp:46` → `chart.cpp:338` | **시각 오프셋(ms)**. 판정에는 영향 없음 |
| (스킨/옵션) `PLAY_AUTOADJUST` | - | - | `ruleset_bms.cpp:1393` | 시각 오프셋 자동 조정 |
| (선곡 옵션) PITCH 종류와 값 | - | - | `skin_lr2_button_callbacks.cpp:102-125` | FREQUENCY/PITCH/SPEED 배속 |

하드코딩된 값 (설정 불가) (확인)
- 게임 로직 tick: Play 1000Hz, Select 250Hz, 그 외 240Hz(실제 250Hz). `scene_*.cpp` 생성자에 있다.
- SoundDriver update 주기: 1000Hz (`sound_driver.h:61`).
- 떼기 확정 지연: 5ms (`input_wrapper.h:21`).
- `timeBeginPeriod(1)` (`game.cpp:286`).
- FMOD voice 512, PITCHSHIFT FFT 512.
- 렌더 드라이버 direct3d11 (Windows).

---

## 7. LR2 재현 주장과 의도적인 차이

| 항목 | 근거 | 성격 |
|---|---|---|
| "It is a clone of Lunatic Rave 2 (beta3 100201)" | `README.md` | 전체 목표 (확인) |
| "Asynchronous input handling" | `README.md` Features | **LR2와 다른 점**으로 내세운 기능. 입력을 렌더와 분리된 1kHz 스레드에서 폴링한다(LR2는 프레임 단위 입력으로 알려져 있음, 추정) (확인: 문구) |
| FMOD 사용, LR2의 FMOD_DSP_TYPE_REVERB를 SFXREVERB로 대체 | `sound_fmod.cpp:955` | LR2 DSP 이펙트 재현, API 차이로 대체 (확인) |
| 소진된 노트는 판정선에 머문다 ("LR2 / pre RA behavior") | `sprite_lane.cpp:150, 173` | LR2 재현 (확인) |
| 턴테이블 회전 각도 "This matches LR2 on static spin" | `scene_play.cpp:2748` | LR2 재현 (확인) |
| 기본 내부 해상도 스케일은 Nearest, "that's what LR2 does" | `window_SDL2.cpp:151-153` | LR2 재현 (확인) |
| START/SELECT로 시작 지연, LR2 스킨 `#STARTINPUT/#LOADEND/#PLAYSTART` 시퀀스 | `scene_play.cpp:1786-1879, 3227-3231`, `skin_lr2.cpp:972-1003` | LR2 스킨 타이밍 재현 (확인: 파싱 / 추정: LR2 동작과 일치 여부) |
| 판정 창 출처 | `ruleset_bms.h:275` (iidx.org의 LR2/beatoraja 비교 링크) | LR2 판정 창 재현 (확인: 주석) → 상세는 파트 B |
| 일시정지 없음 | grep 결과 | LR2와 동일하다고 추정 (추정) |
| 같은 WAV를 다시 트리거하면 이전 voice를 끊음 (`FMOD_UNIQUE`) | `sound_fmod.cpp:678` | LR2 키음 choke 재현으로 추정 (추정) |
| 1000Hz 고정 tick + system_clock, 렌더 프레임에서 burst 실행 | `game.cpp:326-354` | LVF 고유 구조. LR2 내부와의 관계는 코드 근거 없음 (추정) |

---

## 정책으로 본 요약

| 메커니즘 (정책 축) | LVF의 선택 (근거) | 대안 | 비교 지표 |
|---|---|---|---|
| 마스터 시계 소스 | `system_clock` (non-monotonic), ns 저장, 타이머는 ms 정수 (`beat.h:55-59`) | steady_clock/QPC; 오디오 DSP clock(sample count); IIDX식 BGM voice 커서 | 장기 드리프트(장치 클럭 대비 ppm), 시계 점프 내성, 차트와 오디오 정렬 오차의 분포 |
| 게임 로직 갱신 | 1000Hz 고정 tick, **렌더 스레드에서 프레임마다 burst**, 따라잡기 무제한 (`game.cpp:326-354, 449`) | 독립 로직 스레드 고정 tick; 가변 dt; IIDX식 +1 tick 점진 보정; 따라잡기 상한 | tick 실행 시각과 tick 가상 시각의 차이(지연·지터), 프레임 드랍 시 이벤트 burst 크기 |
| 오디오 ↔ 차트 동기화 | **open-loop**: tick에서 `at ≥ note.time`이면 `playSound` 즉시 호출. 장치 위치 미사용, 드리프트 보정 없음 (`chart.cpp:404-413`, `scene_play.cpp:2581-2591`) | DSP clock 예약(`setDelay`); 장치 위치로 시계 보정(closed-loop, IIDX); BGM 커서 기준 따라잡기 | 노트 시각과 실제 출력 onset 오차(루프백 녹음), 지터(표준편차), 곡 길이에 따른 드리프트 |
| 늦은 이벤트 처리 | 늦게 발음(late fire), 오프셋 보정 없음. tick당 128개 초과분은 버림 | 건너뛰기(skip); 샘플 오프셋 시작(seek into sample); DSP 예약 | 누락 수, 지연된 onset 오차, burst 시 동시 발음 수 |
| 오디오 출력 경로 | FMOD Core AUTODETECT(WASAPI shared 추정) 또는 ASIO; DSP 256×4; voice 512 (`sound_fmod.cpp:62-144`) | WASAPI exclusive/event; 더 작은 버퍼; 자체 믹서 | 출력 지연(ms), 언더런 발생률, CPU 사용률 |
| 샘플 로딩과 재트리거 | 전부 메모리 디코드(CREATESAMPLE); 같은 샘플은 FMOD_UNIQUE로 choke (`sound_fmod.cpp:678`) | 스트리밍; 다성(polyphonic) 재트리거; 샘플당 voice 상한 | 로드 시간과 메모리, 재트리거 시 클릭과 잘림 빈도, 원곡과의 스펙트럼 차이 |
| 배속 | 차트 BPM×pitchSpeed로 재컴파일 + FMOD ChannelGroup pitch/PITCHSHIFT (`chart_bms.cpp:73`, `sound_fmod.cpp:1094-1123`) | 시계 배속(time scale); 오디오 time-stretch만 적용 | 배속 시 차트와 오디오 정렬 오차, 음질(아티팩트) |
| 입력 캡처 | 전용 스레드, DirectInput8 **즉시 상태 폴링** 1000Hz(설정으로 2000) (`input_dinput8.cpp:186-233`) / Linux는 메인 스레드 SDL 이벤트 | 버퍼 모드 DI 또는 Raw Input(OS 이벤트 시각); 이벤트 기반 + QPC 타임스탬프; 더 높은 폴링 | 타임스탬프 오차(하드웨어 기준 대비), 짧은 탭 누락률, CPU 사용률 |
| 입력 타임스탬프 | **폴링 시작 시각**, system_clock, 판정 경로는 **ms 정수** (`input_wrapper.cpp:26, 52-55`) | 폴링 직후 시각; OS 이벤트 시각; ns 유지 | 판정 오차의 체계적 편향(평균)과 분산 |
| 판정 실행 위치 | 메인 스레드 `processInput`(프레임 시작), 시각은 폴링 타임스탬프 사용 | 입력 스레드에서 즉시 판정; tick 안에서 판정 | 판정 결과의 FPS 의존성, 판정 표시 지연 |
| 키음 트리거 | 메인 스레드 프레임 시점에 즉시 재생 (`scene_play.cpp:3204`) | 입력 스레드에서 즉시 재생; 입력 시각 기준 DSP 예약 | 입력에서 소리까지의 지연(ms)과 지터 |
| 떼기와 디바운스 | release 확정 5ms + MinInputInterval 5ms (`input_wrapper.h:21`, `cfg_profile.cpp:21`) | 0ms; 누름 디바운스; 장치별 설정 | LN 끝 판정 오차, 채터링 오입력률 |
| 오프셋 | `JudgeOffset`은 **시각 전용**(vt), AUTO ADJUST는 시각을 ±1ms씩 조정 (`chart.cpp:338`, `ruleset_bms.cpp:1389-1410`) | 판정 오프셋과 시각 오프셋 분리; 오디오 지연 보정; 입력 지연 보정 | 보정 후 판정 오차 평균(0에 수렴 여부), 사용자 보정 편의 |
| 루프 페이싱 | Windows waitable timer(비 high-res) + timeBeginPeriod(1), 데드라인 방식이며 4주기 이상 밀리면 리셋 / Linux는 sleep_for 상대 방식 (`asynclooper.cpp:65-133, 160-178`) | high-res waitable timer; busy-wait; MMCSS 우선순위 | 실제 Hz와 주기 지터(p50/p99), CPU 사용률 |
| 렌더 페이싱 | VSync(기본 ON) + MaxFPS 480 캡(Present 후 sleep + spin) (`window_SDL2.cpp:111-118, 235-243`) | VSync OFF 무제한; 프레임 타이밍 예측; 로직과 렌더 분리 | 프레임 시간 분포, 표시 지연(입력에서 픽셀까지), **BGM 발음 지터(로직이 렌더에 묶여 있어서)** |
| 스레드 간 공유 | std::mutex 큐(입력), shared_mutex(SDL 상태, 컨텍스트), atomic(볼륨). lock-free 없음 | SPSC lock-free 링버퍼; 메시지 패싱 | 큐 대기 지연, 경합 시 최악 지연 |

### IIDX와의 핵심 대비 (요약)
- IIDX는 고정 tick이 BGM voice 커서를 기준으로 +1 tick씩 보정하는 closed-loop다. **LVF는 system_clock 기준 open-loop이고 오디오 피드백이 전혀 없다.** 
- IIDX의 tick은 프레임과 1:1(60/120Hz)이다. **LVF는 1kHz 가상 tick을 렌더 프레임마다 몰아서 실행한다.** 판정 시각은 입력 스레드의 폴링 시각을 쓰기 때문에 FPS와 무관하다. 그러나 BGM과 키음 발음은 렌더 프레임에 양자화된다.
- LVF에는 IIDX처럼 종료 시 모니터 Hz를 다시 추정하는 로직이 없다. 렌더는 시간 기반이고 FPS는 표시용 EMA만 계산한다(`generic_info.cpp:13-23`). (확인)
