# beatoraja 소스 분석 (Part A): 시간축 · 메인 루프 · 오디오 · 입력

- 대상: `third_party/beatoraja` (origin `exch-bms2/beatoraja`, HEAD `8320241`, 2026-10-07)
- 소스 루트: `src/bms/player/beatoraja/` (아래 경로는 이 루트 기준 상대경로)
- 백엔드: libGDX **LWJGL2** 백엔드 (`lib/gdx-backend-lwjgl.jar`, 내장 LWJGL 2.9.x / OpenAL Soft DLL, 2014년 빌드). 컨트롤러는 `gdx-controllers-desktop.jar` = **OIS** 기반.
- 차트 파서: `lib/jbms-parser.jar` (`bms.model.BMSModel`, `TimeLine`, `Note`, `BMSModelUtils` …). 디컴파일은 하지 않았고, jar 내부 동작은 클래스명과 바이트코드 문자열 상수로만 확인했음.
- 표기: **(확인)** = 코드에서 직접 읽음, **(추정)** = 코드를 근거로 한 추론 또는 외부 라이브러리 동작에 대한 지식.

---

## 0. 한눈에 보는 구조

```
[Render thread (LWJGL main loop)]  frame마다:
   TimerManager.update()  ← System.nanoTime()  (유일한 게임 시계 갱신 지점)
   BMSPlayer.render()      : 상태머신, TIMER_PLAY 조정(재생속도)
   Skin.drawAllObjects()   : 노트 위치 = (frameTime - TIMER_PLAY) + judgetiming
   current.input()         : 키빔/스크래치 각도 (ms 단위로 게이트)
   Display.update / Display.sync(maxFPS)       (libGDX 내부)

[Input poll thread]   1ms마다(최대 1 kHz): keyboard(Gdx.input) + OIS 컨트롤러 + 마우스 스크래치
                      → keystate[i], time[i] = nanoTime 기반 µs 타임스탬프
[MIDI callback thread] javax.sound.midi Receiver → 즉시 nanoTime 타임스탬프
[JudgeThread]          TIMER_PLAY 값이 바뀔 때마다(= 프레임마다) JudgeManager.update(mtime)
                       → 판정 + 플레이어 키음 play() (0.5ms→실제 1ms sleep 폴링)
[AutoplayThread(BG)]   TIMER_PLAY를 읽어 BG 레인 노트 play(), 다음 이벤트까지 sleep
[Audio mixer thread]   PortAudio / AudioDevice: 자체 소프트웨어 믹서 (MAX_PRIORITY)
                       OpenAL: OpenAL Soft 내부 스레드 (beatoraja 자체 믹서 없음)
```

결론부터 정리하면 다음과 같다. beatoraja는 **가변 스텝 + 벽시계(System.nanoTime) 마스터 + 오픈루프 오디오** 구조이다. 오디오 장치 재생 위치를 읽어 시계를 보정하는 코드가 전혀 없다. 판정 시각은 입력 폴링 스레드가 찍은 µs 타임스탬프를 쓰지만, 판정 처리와 키음 발음은 **렌더 프레임 주기에 묶여** 있다. IIDX의 "고정 틱 + BGM 보이스 커서 추종(>17 ms 지연 시 +1틱)"과 대비된다.

---

## 1. 시간축 / 마스터 클록

### 1.1 게임 시계는 `System.nanoTime()`을 µs로 바꾼 값이며 렌더 프레임마다 1회 갱신된다 (확인)

`TimerManager.java:101-111`
```java
public void setMainState(MainState current) {
    ...
    starttime = System.nanoTime();
    nowmicrotime = ((System.nanoTime() - starttime) / 1000);
}
public void update() {
    nowmicrotime = ((System.nanoTime() - starttime) / 1000);
}
```
- 단위: 내부 저장은 **µs** (`nowmicrotime`), ms getter(`getNowTime() = nowmicrotime/1000`)도 있음 (`TimerManager.java:33-53`) (확인)
- 기준점: 상태(MainState) 전환 시점 = 0 (`MainController.java:280` `timer.setMainState(newState)`) (확인)
- 단조성: `System.nanoTime()`은 JVM이 단조 증가를 보장한다(Windows에서는 QPC 기반). 따라서 시계도 단조이다 (추정: JVM 사양). 코드상 역행 보정이나 클램프는 없다 (확인)
- 갱신 지점은 **오직** `MainController.render()` 맨 앞 한 곳이다 (확인)
  `MainController.java:401-407`
  ```java
  public void render() {
  //    input.poll();
      timer.update();
      Gdx.gl.glClear(GL20.GL_COLOR_BUFFER_BIT);
      current.render();
  ```
  따라서 다른 스레드(판정, BG)가 읽는 "현재 시각"도 **마지막 프레임 시작 시각으로 양자화**된다 (확인: 다른 스레드에서 `timer.update()`를 호출하는 곳이 없음. `BMSPlayer.java:590`의 practice 경로만 예외).
- `libGDX Gdx.graphics.getDeltaTime()`은 게임플레이에 쓰지 않는다. Scene2D `stage.act()`에만 쓴다 (`MainController.java:417`) (확인)

### 1.2 플레이 시각 = `nowmicrotime - timer[TIMER_PLAY]` (확인)

- 스킨 타이머 배열 `timer[id]`에 "그 타이머가 켜진 시각(µs)"을 저장하고, 경과시간은 `getNowMicroTime(id) = nowmicrotime - timer[id]`로 구한다 (`TimerManager.java:48-53`)
- READY에서 PLAY로 넘어갈 때 TIMER_PLAY를 설정한다. `BMSPlayer.java:605-616`
  ```java
  if (timer.getNowTime(TIMER_READY) > skin.getPlaystart()) {
      state = STATE_PLAY;
      timer.setMicroTimer(TIMER_PLAY, micronow - starttimeoffset * 1000);
      timer.setMicroTimer(TIMER_RHYTHM, micronow - starttimeoffset * 1000);
      input.setStartTime(micronow + timer.getStartMicroTime() - starttimeoffset * 1000);
      ...
      keyinput.startJudge(model, ...);          // JudgeThread 시작
      keysound.startBGPlay(model, starttimeoffset * 1000); // BG 스레드 시작
  ```
  - `micronow`는 이번 프레임의 `nowmicrotime`이다 (`BMSPlayer.java:468`). 즉 곡의 0 ms는 "READY 지연이 끝난 직후의 첫 렌더 프레임 시작 시각"이다 (확인)
  - 입력 기준 시각 `input.setStartTime(...)`은 같은 순간을 **절대 nanoTime µs**로 환산한 값이다 (`getStartMicroTime() = starttime/1000`, `TimerManager.java:29-31`). 따라서 입력 타임스탬프와 TIMER_PLAY 상대 시각은 같은 축에 있다 (확인)
- 차트 시각 단위: `TimeLine.getMicroTime()`, `Note.getMicroTime()` (µs) (확인: 호출부 `KeySoundProcessor.java:79`, `JudgeManager.java:229,386`). 파서 내부 계산 정밀도는 jar 디컴파일 불가로 미확인.

### 1.3 시작 지연 (ready / countdown) (확인)

- STATE_PRELOAD는 리소스 로딩 완료, 스킨의 `loadstart+loadend` 경과, START 키를 뗀 뒤 1초 경과를 기다린다 (`BMSPlayer.java:499-500`)
- 그다음 `TIMER_READY`를 켜고, STATE_READY에서 스킨의 `playstart`(ms) 경과를 기다려 PLAY로 넘어간다 (`BMSPlayer.java:513-515, 606`)
- 리드인: `PlayerResource.java:214` `marginTime = BMSModelUtils.setStartNoteTime(model, 1000);`. 첫 노트가 1000 ms 미만이면 차트 전체를 뒤로 민다 (시프트 양 = marginTime). 리플레이 키로그는 이 margin을 빼서 저장하고(`BMSPlayerInputProcessor.java:335`) 재생 시 다시 더한다(`KeyInputProccessor.java:174-182`) (확인. `setStartNoteTime`의 정확한 동작은 jar 내부라서 추정)
- 즉 고정 카운트다운 길이가 없고, **스킨 파라미터(playstart)와 1 s 리드인**으로 결정된다 (확인)

### 1.4 재생 속도(play speed)는 TIMER_PLAY 원점을 매 프레임 밀어서 구현한다 (확인)

`BMSPlayer.java:620-625`
```java
case STATE_PLAY -> {
    final long deltatime = micronow - prevtime;
    final long deltaplay = deltatime * (100 - playspeed) / 100;
    timer.setMicroTimer(TIMER_PLAY, timer.getMicroTimer(TIMER_PLAY) + deltaplay);
    final long ptime = timer.getNowTime(TIMER_PLAY);
```
- `playspeed`(25/50/100/200/300)는 **오토플레이·리플레이에서만** NUM1~4로 바뀐다 (`ControlInputProcessor.java:208-220`) (확인)
- 오디오는 `fastForward == FREQUENCY`일 때 `setGlobalPitch(playspeed/100)`로 따라간다 (`BMSPlayer.java:791-795`) (확인). UNPROCESSED이면 피치는 그대로이고 트리거 시각만 빨라진다.
- 입력 기준 시각(`input.starttime`)은 고정이라서 playspeed≠100이면 입력 축과 TIMER_PLAY 축이 어긋난다. 그래서 PLAY 모드에서는 배속을 허용하지 않는 것으로 보인다 (추정)

### 1.5 Practice의 FREQ(재생 배율) (확인)

`BMSPlayer.java:554-565, 585`
```java
if (property.freq != 100) BMSModelUtils.changeFrequency(model, freqRate);
if (freqOption == FrequencyType.SPEED) { audio.setGlobalPitch(1f); audio.setModel(model, freqRate); }
else if (freqOption == FrequencyType.FREQUENCY) { audio.setGlobalPitch(freqRate); }
...
starttimeoffset = (property.starttime > 1000 ? property.starttime - 1000 : 0) * 100 / property.freq;
```
- 시계는 1배속 그대로 두고, **차트 시각 자체를 재스케일**한다 (`changeFrequency`는 jar 내부라서 추정)
- 오디오 처리 방식은 두 가지이다.
  - FREQUENCY: 믹서의 전역 피치를 바꾼다. 속도와 음정이 함께 변한다.
  - SPEED: TarsosDSP WSOLA로 키음 전체를 **오프라인 타임스트레치해서 다시 로드**한다 (`audio/TimeStretchProcessor.java:3-22`, `AbstractAudioDriver.java:726-734`)
- 시작 지점 오프셋(`starttimeoffset`)은 TIMER_PLAY 원점을 과거로 당기는 식으로 반영한다 (`BMSPlayer.java:609`)

### 1.6 일시정지 (확인)

- 게임플레이 일시정지 기능이 **없다**. `MainController.pause()`는 libGDX 라이프사이클(창 최소화 등) 콜백을 `current.pause()`로 넘기기만 하고, `MainState.pause()`는 빈 메서드이며 BMSPlayer는 오버라이드하지 않는다 (`MainController.java:611-613`, `MainState.java:63-65`)
- 창이 비활성이어도 nanoTime 시계는 계속 흐른다 (확인)
- 비활성일 때 렌더 루프는 `backgroundFPS`(= maxFPS로 설정됨, `MainLoader.java:184`)로 계속 돈다 (추정: libGDX 동작)

---

## 2. 메인 루프 구조와 스레드

### 2.1 게임플레이 갱신은 "가변 스텝, 렌더 프레임 1회 = 시계 1회 갱신"이다 (확인)

- 고정 틱이나 누산기(accumulator)가 없다. 렌더 콜백 1회마다 시계를 갱신하고, 상태머신을 돌리고, 그린다 (`MainController.java:401-418`)
- 렌더 스레드의 입력 후처리(`current.input()`: 키빔 타이머, 스크래치 각도, 컨트롤 키)는 `System.currentTimeMillis()`가 바뀐 프레임에서만 실행된다 (`MainController.java:495-498`). ms 미만 간격의 고FPS 프레임에서는 건너뛴다 (확인)

### 2.2 게임플레이 관련 스레드 (모두 확인)

| 스레드 | 생성 위치 | 주기/대기 | 하는 일 |
|---|---|---|---|
| Render (LWJGL main) | `MainLoader.java:201` `new LwjglApplication` | vsync 또는 `Display.sync(maxFPS)` | 시계 갱신, 상태머신, 그리기 |
| Input poll | `MainController.java:355-370` | ms 경계마다 1회, 그 외 `Thread.sleep(0, 500000)` | `input.poll()`: 키보드, OIS 컨트롤러, 마우스 |
| MIDI receiver | `MidiInputProcessor.java:56` `setReceiver` | 이벤트 콜백 | `keyChanged(…, currentTime())` |
| JudgeThread | `KeyInputProccessor.java:45-48, 140-212` | TIMER_PLAY 값이 바뀌면 처리, 아니면 `sleep(0,500000)` | 리플레이 키 주입, `JudgeManager.update(mtime)` (판정과 키음) |
| AutoplayThread (BG) | `KeySoundProcessor.java:33-36, 49-98` | 다음 BG 이벤트까지 `sleep(ms)` | BG 레인 노트 `audio.play()` |
| PortAudio Mixer | `PortAudioDriver.java:81-88` | 블로킹 `stream.write` | 소프트웨어 믹싱 |
| GdxAudioDevice Mixer | `GdxAudioDeviceDriver.java:54-56` | 블로킹 `writeSamples` | 소프트웨어 믹싱 |

입력 폴링 스레드 본체 (`MainController.java:355-369`):
```java
Thread polling = new Thread(() -> {
    long time = 0;
    for (;;) {
        final long now = System.nanoTime() / 1000000;
        if (time != now) { time = now; input.poll(); }
        else { try { Thread.sleep(0, 500000); } catch (InterruptedException e) {} }
    }
});
```
- ms가 바뀔 때만 poll하므로 **최대 1 kHz**이다. `MainLoader.java:231` 주석에는 "1000Hz or 8000Hz"라고 되어 있지만 코드상 상한은 1 kHz이다 (확인)
- Java 17(README의 요구 버전)의 `Thread.sleep(0, 500000)`은 nanos≥500000이면 millis를 1로 올려 `sleep(1)`이 된다. Windows에서는 실제로 약 1~2 ms 대기한다 (추정: JDK 17 구현, JDK 21+에서는 서브ms sleep 개선)
- 스레드 우선순위나 affinity 설정은 없다 (확인). 오디오 믹서만 `MAX_PRIORITY`이다.

JudgeThread 본체 (`KeyInputProccessor.java:168-200`):
```java
while (!stop) {
    final long mtime = player.timer.getNowMicroTime(TIMER_PLAY);
    if (mtime != prevtime) {
        // replay key injection ...
        judge.update(mtime);
        ...
        prevtime = mtime;
    } else {
        try { sleep(0, 500000); } catch (InterruptedException e) {}
    }
```
- `mtime`은 렌더 스레드만 갱신하므로, 별도 스레드인데도 **판정 처리는 사실상 프레임당 1회**이다 (확인)
- 스레드 간 공유 변수(`nowmicrotime`, `timer[]`, `keystate[]`, `time[]`)는 volatile도 락도 없다. x86-64 HotSpot에서는 실무상 동작하지만 JMM상 가시성 보장은 없다 (확인: 선언부 / 위험 평가는 추정)

### 2.3 프레임 페이싱 / vsync / FPS 상한 (확인 + 추정)

`MainLoader.java:183-185`
```java
cfg.vSyncEnabled = config.isVsync();
cfg.backgroundFPS = config.getMaxFramePerSecond();
cfg.foregroundFPS = config.getMaxFramePerSecond();
```
- `Config.vsync` (기본 false), `Config.maxFramePerSecond` (기본 240, 0~50000으로 클램프. 주석은 "vsync OFF일 때만 유효") (`Config.java:50, 73, 578`) (확인)
- libGDX LWJGL2 `LwjglApplication.mainLoop`은 `Display.sync(frameRate)`(LWJGL2 `org.lwjgl.opengl.Sync`, sleep/yield 기반)로 상한을 건다. `foregroundFPS=0`이면 무제한이다 (추정: 바이트코드 문자열에서 `foregroundFPS`, `sync`, `processMessages`, `update` 참조를 확인. 제어 흐름은 libGDX 1.9.x 소스 지식)
- 모니터 Hz 측정이나 추정 로직이 **없다**. IIDX처럼 곡 종료 시 Hz를 재측정하지 않는다 (확인: 관련 코드 없음)
- `Config.prepareFramePerSecond` (기본 0): 스킨 오브젝트 `prepare()` 호출 빈도 제한이다. 0이면 매 프레임 호출한다 (`skin/Skin.java:265, 316-322`) (확인)
  ```java
  prepareduration = cfg.getPrepareFramePerSecond() > 0 ? 1000000 / cfg.getPrepareFramePerSecond() : 1;
  ...
  if (nextpreparetime <= microtime) { for (obj) obj.prepare(time, state);
      nextpreparetime += ((microtime - nextpreparetime) / prepareduration + 1) * prepareduration; }
  ```
- 노트 그리기 시각은 **프레임 시작 시각**(`timer.getNowTime()`, ms)이다. present 시점 예측이나 보간은 없다 (`LaneRenderer.java:300-301`, `Skin.java:317`) (확인)
- `Config.frameskip`(기본 1)은 BGA 동영상(FFmpeg) 전용이고 게임 루프와 무관하다 (`play/bga/BGAProcessor.java:95`) (확인)

---

## 3. 오디오 시스템

### 3.1 드라이버 3종 (`AudioConfig.DriverType`, `AudioConfig.java:186-201`) (확인)

| DriverType | 클래스 | 출력 경로 | 믹싱 | 샘플 데이터 |
|---|---|---|---|---|
| `OpenAL` (기본) | `audio/GdxSoundDriver.java` | libGDX `Sound` → LWJGL2 OpenAL(OpenAL Soft) | OpenAL 소스 (하드웨어/OpenAL Soft) | OpenAL 버퍼 (메모리) |
| `PortAudio` | `audio/PortAudioDriver.java` | jportaudio BlockingStream, float32 스테레오 | **자체 소프트웨어 믹서** | `PCM` (메모리) |
| `AudioDevice` | `audio/GdxAudioDeviceDriver.java` | libGDX `AudioDevice`(OpenAL 스트리밍 큐) | **자체 소프트웨어 믹서**, int16 | `PCM` (메모리) |

- ASIO 전용 코드는 없다. PortAudio 장치 목록은 `hostApiName : deviceName` 형식으로 표시되므로 PortAudio 빌드에 ASIO/WASAPI host API가 포함돼 있으면 선택할 수 있다 (`PortAudioDriver.java:445-447`) (확인. 번들 jportaudio의 host API 포함 여부는 미확인)
- 드라이버 선택은 `MainController.java:318-328`에서 하고, PortAudio 초기화가 실패하면 OpenAL로 폴백한다 (`MainController.java:188-197`). PortAudio 사용 시 libGDX 오디오를 끈다: `LwjglApplicationConfiguration.disableAudio = true` (`MainLoader.java:195-197`) (확인)

### 3.2 로드 방식: 모두 디코드 후 메모리에 상주하며 스트리밍은 없다 (확인)

- `AbstractAudioDriver.setModel()`은 모든 WAV 정의를 `parallelStream()`으로 병렬 디코드한다 (`AbstractAudioDriver.java:396-441`). 곡 시작 전에 전부 로드한다 (PRELOAD 상태에서 `resource.mediaLoadFinished()`를 기다림, `BMSPlayer.java:499`)
- 세대 기반 캐시 `AudioCache extends ResourcePool`(`Config.songResourceGen`, 기본 1)로 다음 곡에 재사용한다 (`AbstractAudioDriver.java:680-764`)
- bmson의 슬라이스(start/duration)는 **로드 시점에 PCM을 잘라 별도 샘플로** 만든다 (`AbstractAudioDriver.java:415-434, 688-720`). 재생 중 오프셋 시크는 없다.

### 3.3 포맷과 디코더 (`audio/PCM.java:179-357`) (확인)

| 확장자 | 디코더 |
|---|---|
| `.wav` (PCM type 1, float type 3) | 자체 `WavInputStream` 파싱, 8/16/24/32bit |
| `.wav` 안의 MP3 (type 85) | JLayer `javazoom.jl.decoder.MP3Decoder` |
| `.ogg` | libGDX `com.badlogic.gdx.backends.lwjgl.audio.OggInputStream` (jorbis) → 16bit |
| `.mp3` | JLayer |
| `.flac` | `org.jflac.FLACDecoder` (jflac-codec 1.5.3) + `FlacProcessor` |

- 확장자 대체 탐색: `AudioDriver.getPaths()`가 같은 이름의 다른 확장자를 찾는다 (`AudioDriver.java:178`) (확인. 순서 세부는 읽지 않음)
- 디코드 후 **꼬리 무음(0x00) 제거**를 한다 (`PCM.java:330-343`) (확인)
- OpenAL 드라이버의 `.wav/.flac`는 `PCM.load()`로 디코드한 다음 WAV 헤더를 붙인 가상 스트림으로 `Gdx.audio.newSound`에 넘긴다 (`GdxSoundDriver.java:74-117`). `.ogg/.mp3`는 libGDX 디코더를 직접 쓴다 (확인)

### 3.4 리샘플링 / 채널 변환 (확인)

- 로드 시점(PCM 드라이버 계열)에 `convertToDriverFormat()`이 드라이버의 채널 수와 샘플레이트로 변환한다 (`PCM.java:359-375`)
  - 샘플레이트 변환은 **선형 보간**이다 (`PCM.java:391-410`)
    ```java
    long position = frame * sourceSampleRate / targetSampleRate;
    long mod = (frame * sourceSampleRate) % targetSampleRate;
    ... sample = (sample * (targetSampleRate - mod) + nextSample * mod) / targetSampleRate;
    ```
  - 채널 변환은 **첫 채널만 복제**한다 (스테레오→모노 시 L만 사용) (`PCM.java:377-389`)
- 재생 중 피치(전역 피치 × 노트 피치)는 믹서에서 **보간 없이 샘플을 건너뛰거나 반복**한다 (`PortAudioDriver.java:628-633`, `GdxAudioDeviceDriver.java:197-202`)
  ```java
  input.posf += gpitch * input.pitch;
  int inc = (int)input.posf;
  if (inc > 0) { input.pos += 2 * inc; input.posf -= (float)inc; }
  ```
- OpenAL 드라이버는 원본 샘플레이트 버퍼를 그대로 넘기고 리샘플링은 OpenAL Soft에 맡긴다 (추정)

### 3.5 버퍼 / 지연 관련 설정과 실제 적용 (확인 + 추정)

`AudioConfig` 기본값 (`AudioConfig.java:15-41`, validate `:172-185`):
- `deviceBufferSize = 384` (4~4096으로 클램프)
- `deviceSimultaneousSources = 256` (16~1024)
- `sampleRate = 0` (0이면 자동, 아니면 8000~384000)

드라이버별 적용:
- **PortAudio**: `framesPerBuffer = deviceBufferSize` (`PortAudioDriver.java:69`)
  - `suggestedLatency = framesPerBuffer / sampleRate`이다. CoreAudio/ALSA에서는 `min(그 값, defaultLowOutputLatency)` (`:270-276`)
  - 열기에 실패하면 3단계로 폴백한다: high-latency → `framesPerBuffer=0` (`:242-268`)
  - 믹서는 `framesPerBuffer` 프레임을 미리 믹스한 뒤 블로킹 `write()`한다 (`:602-650`)
  - `put()`으로 추가된 새 소리는 다음 믹스 블록부터 들어간다. 따라서 발음 지연 ≈ (0~1 블록 대기) + 블록 1개 + host API 버퍼이다 (추정)
- **AudioDevice**: 믹서 블록은 `max(deviceBufferSize, 1024)` 프레임이다 (`GdxAudioDeviceDriver.java:59-61`)
  - libGDX 장치 버퍼 크기는 `max(deviceBufferSize*4, 4096)` 바이트이다 (`MainLoader.java:188-189`)
  - `audioDeviceBufferCount`는 설정하지 않으므로 libGDX 기본값(9로 기억함)을 쓴다 (추정)
  - 구조상 최소 1024프레임(44.1 kHz 기준 ≈23 ms) 이상의 큐 지연이 생긴다 (추정)
- **OpenAL**: `cfg.audioDeviceBufferSize = deviceBufferSize`, `audioDeviceSimultaneousSources = 256` (`MainLoader.java:191-193`)
  - libGDX에서 bufferSize는 Music/AudioDevice 스트리밍에만 쓰이고, `Sound.play()` 지연은 OpenAL Soft의 내부 period(alsoft.ini)가 정한다 (추정)
  - beatoraja의 `soundthread` 옵션은 `final boolean soundthread = false;`로 고정되어 있다. 그래서 `play()`는 호출한 스레드(BG/Judge 스레드)에서 `Sound.play()`를 바로 호출한다 (`GdxSoundDriver.java:27, 121-135`) (확인)

### 3.6 보이스 관리 (확인)

- 채널 키 = `wavId*256 + pitch + 128`이다. **같은 WAV·같은 피치를 다시 트리거하면 이전 발음을 끊는다**(choke) (`AbstractAudioDriver.java:507-526`)
  ```java
  final int channel = channel(id, pitchShift);
  ...
  stop(wav, channel);
  play(wav, channel, volume, pitch);
  ```
- 레이어 노트(`getLayeredNotes`)는 함께 발음한다 (`:486-491`)
- PortAudio/AudioDevice 믹서는 빈 슬롯이 없으면 새 소리를 **버린다** (`put()`이 -1 반환, `PortAudioDriver.java:537-554`). 보이스 스틸링은 없다.
- 클리핑:
  - PortAudio는 `FLAG_CLIP_OFF`, float 합산 그대로이고 리미터가 없다 (`:72, 639-640`)
  - AudioDevice는 하드 클립이다 (`toShort`, `:245-253`)
- 볼륨은 `#VOLWAV × keyvolume/bgvolume`이다 (`AbstractAudioDriver.java:355-359, 487`)

---

## 4. 오디오 ↔ 차트 동기

### 4.1 완전 오픈루프 방식이다 (확인)

- 오디오 장치 재생 위치(PortAudio stream time, OpenAL `AL_SAMPLE_OFFSET`, 출력 지연)를 읽어 게임 시계를 보정하는 코드가 **없다**. `getLatency`는 로그 출력에만 쓰인다 (`GdxAudioDeviceDriver.java:52`) (확인: `getPosition|getLatency|getStreamTime|drift` 전수 검색)
- 시계(System.nanoTime)와 오디오 장치 클럭 사이의 드리프트 보정이 없다 (확인)
  - BMS는 키음과 BGM 조각을 이벤트마다 새로 트리거하므로 드리프트가 쌓이는 단위는 "한 샘플의 길이"에 그친다 (추정)
  - 곡 전체를 한 WAV로 넣은 차트에서는 장치 클럭 오차(예: 50 ppm → 2분에 6 ms)가 그대로 누적된다 (추정)
- **오디오 출력 지연을 보상하는 오프셋 설정도 없다.** 판정 오프셋 성격의 `judgetiming`은 시각(노트 표시) 쪽에만 적용된다 (5.4 참조) (확인)

### 4.2 BG(BGM 레인) 트리거: `KeySoundProcessor.AutoplayThread` (확인)

`KeySoundProcessor.java:67-97`
```java
for (long time = starttime; p < timelines.length && timelines[p].getMicroTime() < time; p++);
while (!stop) {
    final long time = player.timer.getNowMicroTime(TIMER_PLAY);
    while (p < timelines.length && timelines[p].getMicroTime() <= time) {
        for (Note n : timelines[p].getBackGroundNotes())
            audio.play(n, config.getAudioConfig().getBgvolume(), 0);
        p++;
    }
    if (p < timelines.length) {
        final long sleeptime = timelines[p].getMicroTime() - time;
        if (sleeptime > 0) sleep(sleeptime / 1000);
    }
```
- 비교식은 `noteTime <= TIMER_PLAY 경과(µs)`이다. 여기서 `time`은 **마지막 렌더 프레임 시각**이다 (1.1 참고) (확인)
- 실효 발음 시각 = "noteTime 이후 첫 렌더 프레임" + "BG 스레드가 깨어날 때까지(오래된 time으로 계산한 sleep 때문에 최대 수 ms)"이다 (추정. 근거: time이 프레임 사이에 갱신되지 않으므로 같은 sleeptime을 반복하게 됨)
  - 예: 240 fps에서 이벤트가 프레임 사이에 걸리면 지연은 대략 0~4.2 ms + 1~2 ms이다 (추정)
- **늦은 경우(렉 스파이크)**: 기한이 지난 이벤트를 while 루프가 **모두 한 번에 늦게 발음**한다. 스킵하지 않고, 지연만큼 샘플 시작 위치를 당기는 오프셋 보상도 없다. `audio.play(n, vol, 0)`의 세 번째 인자는 피치이고, 시작 오프셋 API 자체가 없다 (확인)
- 시작 지점 이전 이벤트(Practice 중간 시작)는 **발음하지 않고 건너뛴다**. 시작 시점에 이미 울리고 있어야 할 긴 BGM 샘플은 들리지 않는다 (확인: `:73-74`)

### 4.3 플레이어 레인 키음 / 오토플레이: `JudgeManager.update()` (JudgeThread) (확인)

- 오토플레이에서는 노트 시각이 `mtime`을 지나면 그 틱에서 발음하고 PG로 판정한다 (`JudgeManager.java:229, 251-257`)
  ```java
  for (Note note = state.lanemodel.getNote(); note != null && note.getMicroTime() <= mtime; ...) {
      if (note.getMicroTime() <= prevmtime) continue;
      ...
      if (autoplay) { if (note instanceof NormalNote && note.getState() == 0) {
          auto_presstime[...] = now;
          keysound.play(note, config.getAudioConfig().getKeyvolume(), 0);
          this.updateMicro(state, note, mtime, 0, 0, true);
  ```
  - `(prevmtime, mtime]` 구간에 들어온 노트를 처리하는 방식이다. 지연되면 늦게 발음하고 오프셋 보상은 없다 (확인)
- 수동 플레이에서는 키 입력을 처리할 때 발음한다 (`JudgeManager.java:424, 455, 485` 등). 이 처리도 **프레임 주기로 도는 JudgeThread** 안에서 일어나므로, 키 입력에서 발음까지 걸리는 지연은 (다음 프레임까지 0~1 프레임) + (JudgeThread 깨어남 ~1 ms) + 오디오 지연이다 (추정. 프레임 의존성 자체는 확인)
- 판정 결과 가이드 SE: `keysound.play(judge, fast)` (`JudgeManager.java:694`) (확인)

---

## 5. 입력

### 5.1 캡처 방식 (확인 + 추정)

| 장치 | 방식 | 타임스탬프를 찍는 위치 | 해상도 |
|---|---|---|---|
| 키보드 | 폴링 스레드가 `Gdx.input.isKeyPressed()` 호출 | 폴링 순간 `System.nanoTime()/1000 - starttime` | µs 값, 폴링 간격 ~1 ms |
| BM 컨트롤러 (OIS/DirectInput) | 폴링 스레드가 OIS 업데이트 runnable을 직접 실행한 뒤 `controller.getButton()` | 폴링 순간 | ~1 ms |
| 마우스 스크래치 | 폴링 스레드 | 폴링 순간 (ms로 변환) | ~1 ms |
| MIDI | `javax.sound.midi.Receiver.send()` 콜백 | **콜백 순간** nanoTime. MIDI `timeStamp` 인자는 무시 | µs |

- 폴링 진입점과 타임스탬프 (`BMSPlayerInputProcessor.java:500-511`)
  ```java
  public void poll() {
      final long now = System.nanoTime() / 1000 - starttime;
      kbinput.poll(now);
      if (controllerPollRunner != null) controllerPollRunner.run();
      for (BMControllerInputProcessor controller : bminput) controller.poll(now);
  }
  ```
  - 타임스탬프 `now`는 OIS 갱신(`controllerPollRunner.run()`) **전에** 찍는다. 그래서 컨트롤러 이벤트는 최대 OIS 업데이트 소요 시간만큼 이른 값을 받는다 (확인: 순서. 크기는 추정)
- **OIS 하이재킹** (`MainLoader.java:227-250`): OisControllers가 매 프레임 `Gdx.app.postRunnable`로 등록하는 폴링 runnable을 가로챈다. 이 runnable을 폴링 스레드에서 직접 실행해 컨트롤러를 1 kHz로 폴링한다 (확인)
- **키보드 주의점 (중요)**: LWJGL2에서 `Gdx.input.isKeyPressed` → `Keyboard.isKeyDown()`은 `keyDownBuffer`를 읽는다. 이 버퍼는 **렌더 스레드의 `Display.update()/processMessages()`가 프레임마다 채운다**. 결과적으로 키보드 입력은 1 kHz 폴링을 해도 **프레임 단위로 양자화**되고, 타임스탬프는 "OS 이벤트 시각"이 아니라 "다음 프레임의 메시지 처리 직후 첫 폴링 시각"이 된다 (추정. 근거: 바이트코드 문자열로 `LwjglInput`→`Keyboard.isKeyDown`, `Keyboard`→`keyDownBuffer/poll/pollKeyboard`, `Display`→`processMessages/pollDevices` 참조 확인, 제어 흐름은 LWJGL 2.9 소스 지식)
  - LwjglInput은 이벤트 타임스탬프(`getEventNanoseconds`)를 갖고 있지만 beatoraja는 `getCurrentEventTime()`을 쓰지 않는다 (확인: 검색 결과 0건)
- 키보드 폴링과 디바운스 (`KeyBoardInputProcesseor.java:101-113`)
  ```java
  final boolean pressed = Gdx.input.isKeyPressed(keys[i]);
  if (pressed != keystate[keys[i]] && microtime >= keytime[keys[i]] + duration * 1000) {
      keystate[keys[i]] = pressed; keytime[keys[i]] = microtime;
      this.bmsPlayerInputProcessor.keyChanged(this, microtime, i, pressed);
  ```
- 컨트롤러도 같은 구조이다 (`BMControllerInputProcessor.java:127-186`): 버튼마다 `duration` 잠금, 아날로그 스크래치 알고리즘 v1/v2, JKOC 해킹 (확인)
- MIDI (`MidiInputProcessor.java:66-77, 159-161, 175-180`)
  ```java
  setHandler(keys[i], pressed -> { bmsPlayerInputProcessor.keyChanged(this, currentTime(), key, pressed); ...});
  long currentTime() { return System.nanoTime() / 1000 - starttime; }
  public void send(MidiMessage message, long timeStamp) { ... noteOn/noteOff ... }
  ```
  MIDI에는 디바운스(duration)가 없다 (확인)

### 5.2 상태 저장과 판정 쪽 소비 (확인)

- `keyChanged()`는 키마다 **최신 변화 1건**(`keystate[i]`, `time[i]`)만 저장하고 큐가 없다 (`BMSPlayerInputProcessor.java:326-337`)
  ```java
  if (keystate[i] != pressed) { keystate[i] = pressed; time[i] = presstime; ...
      if (starttime != 0) keylog.add(presstime - microMarginTime, i, pressed); }
  ```
- JudgeThread가 프레임마다 `getKeyChangedTime(key)`를 읽고, 판정한 뒤 `resetKeyChangedTime(key)` 한다 (`JudgeManager.java:346, 553`)
- 판정 오차는 **차트 시각과 입력 타임스탬프의 차이**로 계산한다. 판정 정밀도 자체는 프레임레이트와 무관하다 (`JudgeManager.java:386`)
  ```java
  final long dmtime = judgenote.getMicroTime() - pmtime;   // +면 FAST
  ```
- 위험 1 (추정): 한 판정 틱(=1 프레임) 안에 같은 키의 press와 release가 모두 일어나면 마지막 상태(release)만 남아 **press를 잃는다**. 디바운스 16 ms가 이를 줄이지만 60 fps(16.7 ms) 이하에서는 가능성이 남는다.
- 위험 2 (추정): 폴링 스레드가 쓰는 시점과 JudgeThread의 read→reset 사이에 경합이 있다 (락 없음)
- 리플레이: 키로그를 시각순으로 `setKeyState(..., time + margin)`에 주입한다 (`KeyInputProccessor.java:173-184`) (확인)

### 5.3 디바운스 ("inputduration") (확인)

- `PlayModeConfig.KeyboardConfig.duration = 16` (ms), `ControllerConfig.duration = 16`, 0~100으로 클램프 (`PlayModeConfig.java:118, 148, 270, 498`)
- 설정 setter 인자 이름은 `inputduration`이다 (`:341, 607`)
- 의미: 같은 키의 상태 변화 후 `duration` ms 동안 **다음 변화(누름/뗌 모두)를 무시**한다 (채터링 방지)

### 5.4 입력·지연 보정 설정 (확인)

- `PlayerConfig.judgetiming` (기본 0, ±500 ms, `PlayerConfig.java:82-85, 937`). 이름은 "判定タイミング"이지만 실제로는 **노트 표시 시각에만 더해진다**.
  `LaneRenderer.java:300-301`
  ```java
  time = (main.timer.isTimerOn(TIMER_PLAY) ? time - main.timer.getTimer(TIMER_PLAY) : ...) + config.getJudgetiming();
  ```
  - 판정식(`dmtime = noteTime - pressTime`)에는 들어가지 않는다 → **시각 오프셋 방식**이다. +값이면 노트가 판정선에 더 일찍 도달하도록 그려진다 (확인)
  - 런처 UI 라벨도 `notesdisplaytiming`이다 (`launcher/PlayConfigurationView.java:568`)
- `PlayerConfig.notesDisplayTimingAutoAdjust` (기본 false): PG/GR/GD이고 |오차|≤150 ms인 판정마다 judgetiming을 `round(mfast/30ms)` ms만큼 조정한다. 사실상 ±15 ms 이상 어긋난 판정에서 1 ms씩 움직인다 (`JudgeManager.java:697-702`)
  ```java
  if (judge <= 2 && mfast >= -150000 && mfast <= 150000)
      player.setJudgetiming(player.getJudgetiming() - (int)((mfast >= 0 ? mfast + 15000 : mfast - 15000) / 30000));
  ```
- 오디오 지연 보정, 입력 지연 보정, 판정 오프셋(판정식 자체를 미는 것)은 **존재하지 않는다** (확인)

---

## 6. 동작에 영향을 주는 설정 항목 전체 목록

| 클래스.필드 | 기본값 | 범위/값 | 영향 | 근거 |
|---|---|---|---|---|
| `AudioConfig.driver` | `OpenAL` | OpenAL / PortAudio / AudioDevice | 오디오 출력 경로, 믹서 유무 | `AudioConfig.java:15` |
| `AudioConfig.driverName` | null | PortAudio 장치 표시명 | 장치/host API 선택 | `:20`, `PortAudioDriver.java:94-113` |
| `AudioConfig.deviceBufferSize` | 384 | 4~4096 | PortAudio framesPerBuffer, AudioDevice 블록(≥1024), OpenAL cfg | `:24`, `MainLoader.java:188-191` |
| `AudioConfig.deviceSimultaneousSources` | 256 | 16~1024 | 동시 발음 수(초과하면 버림) | `:28` |
| `AudioConfig.sampleRate` | 0(자동) | 0 또는 8000~384000 | 장치 레이트, 로드 시 리샘플 목표 | `:32`, `PortAudioDriver.java:227-240`, `GdxAudioDeviceDriver.java:31` |
| `AudioConfig.freqOption` | FREQUENCY | UNPROCESSED/FREQUENCY/SPEED | Practice 배율의 오디오 처리 | `:37`, `BMSPlayer.java:554-565` |
| `AudioConfig.fastForward` | FREQUENCY | 위와 같음 | 오토/리플레이 배속의 오디오 처리 | `:41`, `BMSPlayer.java:793` |
| `AudioConfig.systemvolume/keyvolume/bgvolume` | 0.5 | 0~1 | 믹스 볼륨 | `:46-54` |
| `Config.vsync` | false | bool | LWJGL vsync | `Config.java:50`, `MainLoader.java:183` |
| `Config.maxFramePerSecond` | 240 | 0~50000 (0=무제한) | `Display.sync` 상한(전경/배경) | `:73, 578`, `MainLoader.java:184-185` |
| `Config.prepareFramePerSecond` | 0(매 프레임) | 0~100000 | 스킨 prepare 빈도 | `:75, 579`, `Skin.java:265` |
| `Config.frameskip` | 1 | 0~10 | BGA 동영상 디코드만 | `:158, 589` |
| `Config.displaymode` | WINDOW | FULLSCREEN/BORDERLESS/WINDOW | 표시 경로(전체화면 여부 → present 지연) | `:46`, `MainLoader.java:170-181` |
| `Config.songResourceGen` | 1 | | 오디오 캐시 세대 | `AbstractAudioDriver` 생성자 |
| `PlayerConfig.judgetiming` | 0 | ±500 ms | **노트 표시** 오프셋 | `PlayerConfig.java:82`, `LaneRenderer.java:301` |
| `PlayerConfig.notesDisplayTimingAutoAdjust` | false | bool | judgetiming 자동 조정 | `:90`, `JudgeManager.java:697` |
| `PlayerConfig.customJudge` + `key/scratchJudgeWindowRate*` | false, 400/400/100 | % | 판정 창 배율(어시스트) | `:128-134`, `JudgeManager.java:169-174` |
| `PlayerConfig.longnoteMarginRate` | 100 | % | LN 떼기 마진 | `:135`, `JudgeManager.java:186` |
| `PlayConfig.judgetype` | `Combo` | JudgeAlgorithm | 겹친 노트 중 판정 대상 선택 | `PlayConfig.java:103`, `JudgeManager.java:146` |
| `KeyboardConfig.duration` | 16 ms | 0~100 | 키보드 디바운스 | `PlayModeConfig.java:270` |
| `ControllerConfig.duration` | 16 ms | 0~100 | 컨트롤러 디바운스 | `:498` |
| `ControllerConfig.jkoc_hack / analogScratch / analogScratchMode / analogScratchThreshold` | false/false/0/100 | | 스크래치 검출 | `:502-514` |
| `MouseScratchConfig.mouseScratchEnabled / TimeThreshold / Distance / Mode` | false/150/12/0 | | 마우스 스크래치 | `:368-380` |
| `Config.analogScroll` | true | bool | 선곡/레인커버 아날로그 스크롤(플레이 판정과 무관) | `Config.java:95` |

(하드코딩된 상수, 설정 불가: 입력 폴링 1 ms 게이트, JudgeThread와 폴링의 `sleep(0,500000)`, 리드인 1000 ms, `TIME_MARGIN=5000 ms`, AudioDevice 최소 블록 1024, OpenAL `soundthread=false`.)

---

## 정책으로 본 요약

| 메커니즘(정책 축) | beatoraja의 선택 | 대안 | 비교 지표 |
|---|---|---|---|
| 마스터 클록 | `System.nanoTime()` 벽시계, 상태 전환 시 0 (확인) | 오디오 장치 시계(재생 샘플 수), 오디오 커서 추종(IIDX식), 하이브리드(벽시계 + PLL 보정) | 장시간 곡에서 오디오 대비 차트 드리프트(ms/분), 시계 지터(σ) |
| 시뮬레이션 스텝 | 가변 스텝, 렌더 프레임당 1회 갱신 (확인) | 고정 틱(1/60, 1/120, 1/1000) + 누산기, 독립 로직 스레드 | 판정/미스 처리 시각의 양자화 오차, FPS 변화에 대한 결과 불변성(리플레이 재현성) |
| 시계 읽기 지점 | 프레임 시작에서 1회 → 다른 스레드는 프레임 양자화된 값을 공유 (확인) | 각 스레드가 직접 nanoTime 읽기, present 시각 예측값 사용 | BG 발음 지연 분포, 화면-소리 정렬 오차 |
| 오디오↔차트 동기 | 오픈루프, 보정 없음 (확인) | 장치 위치 피드백 + 드리프트 보정, 샘플 정확 스케줄링(미래 시각 예약 믹싱) | 트리거 지터(측정: 루프백 녹음), 누적 드리프트 |
| 늦은 이벤트 처리 | 늦게라도 전부 발음, 시작 오프셋 보정 없음 (확인) | 늦은 만큼 샘플 오프셋 재생, 임계 초과 시 스킵, 미래 예약 | 렉 스파이크 후 위상 오차(ms), 청감상 플램 발생 횟수 |
| BG 스케줄러 | 별도 스레드, `sleep(Δms)` + 프레임 양자화된 시각 (확인) | 오디오 콜백 안에서 샘플 단위 스케줄, 렌더 스레드에서 처리 | 이벤트 발음 시각 오차 히스토그램(p50/p99) |
| 믹싱 | 드라이버 선택: OpenAL(라이브러리 믹싱) / 자체 소프트 믹서(PortAudio, AudioDevice) (확인) | WASAPI 배타, ASIO 직접, 콜백 방식 PortAudio | 키 입력→소리 왕복 지연, 언더런 횟수, CPU |
| 버퍼 크기 | 384 프레임(설정 가능), AudioDevice는 ≥1024 강제 (확인) | 64~2048 스윕 | 출력 지연 vs 언더런율 곡선 |
| 리샘플링 | 로드 시 선형 보간, 재생 피치는 비보간 샘플 스킵 (확인) | sinc/polyphase, 재생 시 보간 | THD+N, 앨리어싱, 로드 시간 |
| 보이스 정책 | 같은 WAV+피치 재트리거 시 choke, 슬롯이 꽉 차면 신규 버림 (확인) | 폴리포니 허용, 가장 오래된/작은 보이스 스틸링 | 누락 발음 수, 클리핑 샘플 비율 |
| 입력 획득 | 1 kHz 폴링 스레드(키보드/OIS), MIDI는 콜백 (확인). 키보드는 LWJGL2 특성상 프레임 양자화 (추정) | Raw Input/HID 이벤트 + OS 타임스탬프, 전용 고우선 스레드, 8 kHz 폴링 | 입력 타임스탬프 오차(외부 기준 대비, 예: 포토/LED 루프), FPS 의존성 |
| 입력 타임스탬프 원천 | 폴링 순간 nanoTime (OS 이벤트 시각 아님) (확인) | OS 이벤트 시각(`getEventNanoseconds`, WM 메시지 시각, MIDI timeStamp) | 동일 |
| 입력 버퍼링 | 키당 최신 1건, 프레임당 소비 (확인) | 이벤트 큐(모든 변화 보존) | 짧은 탭 누락률, 경합 손실 |
| 디바운스 | 16 ms 고정 잠금(누름/뗌 모두), MIDI는 없음 (확인) | 0 ms, 비대칭(뗌만), 상태 기반 | 연타 상한, 채터링 오입력률 |
| 판정 시점 | 판정 계산은 입력 타임스탬프 기반, 처리 실행은 프레임 주기 JudgeThread (확인) | 입력 이벤트 즉시 판정(입력 스레드 내), 고정 틱 판정 | 키 입력→키음 지연, 판정 결과의 FPS 불변성 |
| 지연 보정 | 시각 오프셋 `judgetiming`만 있음(±500 ms) + 자동조정(±1 ms/노트) (확인) | 오디오 오프셋, 입력 오프셋(판정식 시프트), 3축 분리 캘리브레이션 | 캘리브레이션 후 평균 오차와 분산, 수렴 속도 |
| 렌더 페이싱 | vsync on/off + `Display.sync(maxFPS=240)`, Hz 측정 없음 (확인/추정) | 측정 Hz 기반 페이싱, VRR, present 시각 예측 렌더 | motion-to-photon 지연, 프레임 타임 지터 |
| 그리기 시각 | 프레임 시작 시각(ms 정밀도), 예측 없음 (확인) | 예상 present 시각으로 외삽 | 노트 위치 오차(px) vs 실제 표시 시각 |
| 일시정지 | 없음 (확인) | 일시정지 시 시계 정지 + 오디오 정지/재개 | 해당 없음 / 재개 후 동기 오차 |
| 재생 속도 | TIMER_PLAY 원점을 매 프레임 밀기 + 전역 피치(FREQUENCY) 또는 오프라인 WSOLA(SPEED) (확인) | 시계 배율 스케일, 실시간 타임스트레치 | 배속 중 동기 오차, 음질 |

### IIDX(사용자 분석)와의 핵심 차이
1. **시간축**: IIDX는 고정 틱 시뮬레이션이 BGM 보이스 커서를 추종한다(>17 ms 지연 시 +1틱). beatoraja는 nanoTime 벽시계를 가변 스텝으로 쓰고 오디오에서 피드백을 받지 않는다.
2. **드리프트 대책**: IIDX는 2프레임마다 검사하며 catch-up한다. beatoraja는 아무것도 하지 않는다. 키음 단위 재트리거라 실질적 드리프트는 작지만 단일 BGM 차트에서는 누적된다(추정).
3. **Hz 처리**: IIDX는 곡 종료 시 모니터 Hz를 재추정한다. beatoraja는 Hz를 모른다(vsync와 FPS cap만 있음).
4. **입력**: beatoraja는 판정 시각을 1 ms 폴링 타임스탬프로 계산하므로 프레임과 분리돼 있다. 하지만 판정 처리와 키음은 프레임에 묶여 있고, 키보드 상태도 LWJGL2 구조상 프레임에 묶여 있다(추정).
5. **보정**: beatoraja는 시각 오프셋 하나만 있고 오디오나 입력 오프셋은 없다.
