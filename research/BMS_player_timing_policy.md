# BMS 구동기의 타이밍 정책 — beatoraja와 Lunatic Vibes F (LR2 클론)

[IIDX_timing_policy.md](IIDX_timing_policy.md)와 같은 축으로 공개 소스 BMS 구동기 두 개를 분석했습니다. 목적은 "어느 쪽이 옳은가"가 아니라, **각 메커니즘을 엔진 설정에서 바꿔 끼울 수 있는 정책으로 보고 무엇을 재서 비교할지**를 정하는 것입니다.

표기: (확인) 소스에서 읽음 · (확인-bytecode) jar의 바이트코드에서 읽음 · (추정) 코드 경로에서 추론, 실행해 보지 않음.
모든 근거는 소스를 읽어서 얻었고, 두 구동기 모두 빌드하거나 실행하지 않았습니다.

---

## 0. 결론

- **두 구동기 모두 "오디오 개방 루프(open-loop)"입니다.** 오디오 장치의 재생 위치를 한 번도 읽지 않습니다. 예정 시각이 지나면 소리를 "울리기만" 하고, 렉이 걸리면 밀린 소리가 한꺼번에 늦게, 처음부터 울립니다. IIDX처럼 반주 재생 위치를 보고 채보를 당겨 맞추는 장치가 없습니다. (확인)
- **시계는 둘 다 OS 벽시계입니다.** beatoraja는 `System.nanoTime()`(단조), LVF는 `std::chrono::system_clock`(비단조)입니다. 오디오 시계와의 차이는 상수(출력 지연)라고 가정하고, 그 상수는 사용자가 "화면 오프셋"으로 흡수합니다. (확인)
- **판정 오프셋은 둘 다 화면에만 적용됩니다.** beatoraja의 `judgetiming`도, LVF의 `JudgeOffset`도 노트를 그리는 위치만 옮기고 판정 계산식에는 들어가지 않습니다. 입력 지연이나 오디오 지연을 보정하는 설정은 없습니다. (확인)
- **판정 정확도는 프레임과 분리했지만, 판정 처리는 분리하지 않았습니다.**
  - 누른 시각: 두 구동기 모두 1 kHz 폴링 스레드가 찍습니다. 그래서 FPS가 낮아도 판정 오차 자체는 정확합니다. (확인)
  - 판정 처리: 미스 판정, LN 완료, 키음·배경음 발음은 렌더 프레임이 올 때 실행됩니다. 그래서 FPS가 지연 시간을 정합니다. (확인)
- **두 구동기의 루프 구조가 다릅니다.**
  - beatoraja: 고정 틱이 없는 가변 프레임 구조입니다. 판정 스레드는 프레임마다 한 번 돕니다. (확인)
  - LVF: 1000 Hz 가상 고정 틱입니다. "Fix your time step" 누산기로, 렌더 프레임마다 밀린 틱을 몰아서 처리합니다. 각 틱에는 1 ms 격자 위의 가상 시각이 붙습니다. (확인)
- **같은 소리를 다시 울리면 둘 다 앞 소리를 끊습니다.** Qwilight의 "중복 키음 방지"를 끌 수 없는 기본 동작으로 둔 셈입니다. (확인 / LVF는 FMOD 문서 기준 추정)
- **판정 창은 철학이 다릅니다.**
  - LVF: LR2 값을 그대로 씁니다. NORMAL은 PG ±18 ms, ms 정수로 자르고 늦은 쪽 경계는 포함하지 않습니다. (확인)
  - beatoraja: 모드별 µs 표에 #RANK 비율을 곱합니다. NORMAL은 75%라서 PG ±15 ms입니다. BAD는 빠른 쪽과 늦은 쪽 폭이 다릅니다. (확인)
- **BMS에는 IIDX식 동기화의 기준점이 없는 경우가 많습니다.** 키음 위주 BMS는 곡 전체를 덮는 반주 트랙이 없거나 짧습니다. 그래서 "반주 재생 위치"를 쓸 수 없고, 정답 오디오 시계는 **장치가 실제로 내보낸 샘플 수(DSP 클록)**여야 합니다. 이것이 BMS 엔진이 IIDX에서 그대로 가져올 수 없는 부분입니다. (추정, 설계 판단)

---

## 1. 분석 범위와 방법

| 대상 | 버전 | 언어 / 라이선스 | 메모 |
|---|---|---|---|
| beatoraja | `8320241` (2026-10-07) | Java + libGDX / GPL-3.0 | 채보 파서(`jbms-parser.jar`)는 소스가 없어서 클래스 파일을 직접 파싱해 상수와 연산을 읽었습니다. |
| Lunatic Vibes F (LVF) | `3e8a655` (2025-03-14, v0.27.2) | C++ / MIT | LR2 beta3 100201을 다시 만든 클론입니다. **LVF의 동작이 LR2의 동작이라는 보장은 없습니다.** 주석에 "LR2 디스어셈블로 검증"이라고 적힌 곳(판정 창, 게이지)만 LR2 근거로 봅니다. |
| LR2 원본 | — | 비공개 | 분석하지 않음. |

축마다 상세 보고서를 따로 썼습니다(파일:줄 근거 포함).

- [beatoraja A — 시계·오디오·입력](third_party/_analysis/beatoraja_A_clock_audio_input.md)
- [beatoraja B — 판정·믹싱·렌더](third_party/_analysis/beatoraja_B_judge_mix_render.md)
- [LVF A — 시계·오디오·입력](third_party/_analysis/lvf_A_clock_audio_input.md)
- [LVF B — 판정·믹싱·렌더](third_party/_analysis/lvf_B_judge_mix_render.md)

이 문서의 코드 경로는 `third_party/beatoraja/src/bms/player/beatoraja/`(이하 `bt/`)와 `third_party/lunaticvibesf/src/`(이하 `lvf/`) 기준입니다.

---

## 2. 한눈에 비교

| 축 | IIDX (bm2dx, RE) | beatoraja | LVF (LR2 클론) |
|---|---|---|---|
| 기준 시계 | 프레임 = 틱 (고정 틱) | `System.nanoTime()` µs, 렌더 프레임 시작에만 갱신 | `system_clock` (비단조), ms 정수로 판정 |
| 게임 로직 | 프레임마다 1틱 | 가변 프레임. 판정 스레드가 프레임마다 1회 | 1000 Hz 가상 틱, 프레임마다 몰아서 처리 |
| 오디오 동기 | **폐루프**: 반주 재생 위치와 2프레임마다 비교, 17 ms 넘게 늦으면 +1틱 | 개방 루프 | 개방 루프 |
| 렉 후 복구 | 약 1.5배속으로 따라잡음 (slew) | 밀린 소리를 한꺼번에 늦게 울림 | 같음. 한 틱에 128개를 넘으면 버림 |
| 장기 드리프트 | 곡 끝에 주사율 재추정 | 없음 | 없음 |
| 입력 시각 | (미분석) | 1 kHz 폴링 시각 (µs) | 1 kHz 폴링 시각 (ms) |
| 판정 창 | 채보마다 1/60 s 단위 (type 8) | 모드별 µs 표 × #RANK 비율 | LR2 표 (#RANK별 ms) |
| 노트 고르기 | (미분석) | Combo (기본) / Duration / Lowest | 레인에서 아직 처리 안 된 가장 이른 노트 |
| 판정 오프셋 | — | 화면 전용 | 화면 전용 |
| 같은 소리 재트리거 | (미분석) | 항상 앞 소리 끊음 | 항상 앞 소리 끊음 (FMOD_UNIQUE) |
| 동시 발음 수 | (미분석) | 256, 넘치면 새 소리 버림 | 512 |
| 노트 위치 | (미분석) | 매 프레임 시계에서 다시 계산 | 매 프레임 마지막 틱 값에서 다시 계산 |

---

## 3. 기준 시계

### beatoraja (확인)
- [TimerManager.java:108-110](third_party/beatoraja/src/bms/player/beatoraja/TimerManager.java#L108-L110): `nowmicrotime = (System.nanoTime() - starttime) / 1000`
- 이 값은 `MainController.render()` 맨 앞에서만 갱신됩니다. 판정 스레드나 배경음 스레드가 읽는 "지금"은 **마지막 프레임이 시작된 시각**입니다.
- 플레이 시각은 `nowmicrotime - timer[TIMER_PLAY]`입니다. 채보 앞에 1000 ms 여유가 붙습니다(`PlayerResource.java:214`).
- 재생 속도(오토플레이·리플레이 전용)는 `TIMER_PLAY` 기준점을 프레임마다 밀어서 구현합니다(`BMSPlayer.java:621-624`).
- 일시정지는 없습니다.

### LVF (확인)
- [beat.h:57](third_party/lunaticvibesf/src/common/beat.h#L57): `std::chrono::system_clock::now()`.
- `system_clock`은 단조 시계가 아닙니다. 플레이 중에 OS 시계가 동기화되면 시간이 튈 수 있습니다. (추정: 실제 영향은 측정하지 않음)
- 플레이 시각은 `rt = tick_t - PLAY_START`입니다(`scene_play.cpp:1886-1896`). 판정은 `Time::norm()`(ms 정수)로 합니다.
- 재생 속도는 시계가 아니라 채보를 다시 컴파일해서 구현합니다(BPM × pitchSpeed). 오디오는 FMOD 피치로 맞춥니다.
- `PLAY_START`가 판정기의 시작점보다 한 틱(1 ms) 앞에 찍힙니다. 그래서 판정 오차가 −1 ms 쪽으로 치우칠 수 있습니다. (추정)

### IIDX와 비교
IIDX는 시간을 "프레임 번호 × 틱 길이"로 셉니다. 렉이 걸리면 프레임 시계와 오디오가 어긋나므로 오디오를 기준으로 되돌립니다. 두 BMS 구동기는 벽시계를 쓰므로 렉이 걸려도 **시계 자체는 틀어지지 않습니다.** 대신 소리와 화면이 그 시계를 늦게 따라갈 뿐입니다. 목표는 같은데(시계는 진실을 유지) 방법이 반대입니다.

---

## 4. 루프와 스레드

### beatoraja (확인)
| 스레드 | 하는 일 | 주기 |
|---|---|---|
| 렌더 (libGDX) | 시계 갱신, 상태 머신, 그리기 | VSync(기본 꺼짐) 또는 `maxFramePerSecond` 240 |
| 입력 폴링 | 키보드·컨트롤러 상태 읽기, 시각 기록 | ms 경계마다 1번 (최대 1 kHz), 아니면 0.5 ms sleep ([MainController.java:355-369](third_party/beatoraja/src/bms/player/beatoraja/MainController.java#L355-L369)) |
| JudgeThread | `JudgeManager.update()` | `TIMER_PLAY`가 바뀔 때만, 즉 **프레임마다 1회** (`KeyInputProccessor.java:168-200`) |
| AutoplayThread | 배경음 발음 | 다음 이벤트까지 sleep ([KeySoundProcessor.java:77-90](third_party/beatoraja/src/bms/player/beatoraja/play/KeySoundProcessor.java#L77-L90)) |
| 믹서 (PortAudio/AudioDevice 드라이버) | 샘플 합산, 장치 쓰기 | 장치 버퍼 단위 |

- 스레드 사이 공유 상태(`nowmicrotime`, 키 배열)에 `volatile`도 잠금도 없습니다. (확인) Java 메모리 모델상 다른 스레드에서 갱신이 늦게 보일 수 있습니다. (추정)

### LVF (확인)
- **게임 로직은 1000 Hz 가상 고정 틱입니다**([game.cpp:326-354](third_party/lunaticvibesf/src/game/game.cpp#L326-L354)).
  ```cpp
  _acc += t - _last_update_time;
  const auto dt = lunaticvibes::Time{1000 / rate};
  while (_acc >= dt) { const auto this_tick_time = t - _acc; cb(this_tick_time); _acc -= dt; }
  ```
  - 틱은 렌더 스레드에서 프레임마다 몰아서 돕니다. 따라잡는 개수에 상한이 없습니다.
  - 각 틱에는 `t - _acc`라는 1 ms 격자 위의 가상 시각이 붙습니다. 그래서 늦게 처리해도 **틱의 시각 자체는 정확**합니다.
  - `dt = 1000 / rate`는 정수 나눗셈입니다. 240 Hz로 설정된 장면은 실제로 250 Hz로 돕니다. (확인)
- 다른 스레드:
  - 입력: `AsyncLooper` 1000 Hz (`InputPollingRate`)
  - 사운드: 1000 Hz, `FMOD update()` 호출
  - 로딩·동영상: 작업자 스레드
- 루퍼는 Windows 대기 타이머에 `timeBeginPeriod(1)`을 씁니다. 4주기 넘게 밀리면 일정을 리셋합니다. 스레드 우선순위나 MMCSS는 설정하지 않습니다. (확인)
- 공유 상태는 `std::mutex` 큐와 atomic으로 넘깁니다. `doMainThreadTask`에는 경쟁 조건이 의심되는 부분이 있습니다. (추정)

### 정리
| | IIDX | beatoraja | LVF |
|---|---|---|---|
| 시뮬레이션 단위 | 프레임 1개 = 틱 1개 | 프레임 (가변) | 1 ms 틱 (프레임마다 몰아서) |
| 렉 걸린 프레임 뒤 | 오디오에 맞춰 틱을 더 돌림 | 다음 프레임이 큰 Δt로 처리 | 밀린 틱을 한꺼번에 처리 |
| 판정·발음 지연을 정하는 것 | 프레임 | 프레임 | 프레임 (틱 시각은 정확하지만 실행은 프레임 때) |

---

## 5. 오디오와 채보 동기

**두 구동기 모두 오디오 장치에서 아무것도 읽지 않습니다.**
- beatoraja: 재생 위치를 읽는 코드가 없습니다. (확인)
- LVF: `getDSPClock`, `setDelay`, `getPosition`, `getLatency` 호출이 없습니다. (확인)

### 배경음 발음
- **beatoraja** ([KeySoundProcessor.java:77-84](third_party/beatoraja/src/bms/player/beatoraja/play/KeySoundProcessor.java#L77-L84)):
  ```java
  final long time = player.timer.getNowMicroTime(TIMER_PLAY);
  while (p < timelines.length && timelines[p].getMicroTime() <= time) {
      for (Note n : timelines[p].getBackGroundNotes()) audio.play(n, bgvolume, 0);
      p++;
  }
  ```
  - 이 스레드가 읽는 `time`은 프레임 단위로 갱신되는 값입니다. 그래서 배경음은 최대 1프레임 + 1–2 ms 늦게 울립니다. (추정)
- **LVF**: 틱마다 `at ≥ note.time`인 배경음을 바로 `playSound`합니다(`chart.cpp:404-413`, `scene_play.cpp:2581-2591`). 틱은 프레임 때 몰아서 돌므로, 실제 발음 시각은 프레임 주기만큼 흔들립니다. 480 fps면 약 2 ms, 60 Hz VSync면 최대 약 16.7 ms입니다. (추정)

### 렉이 걸렸을 때
| | IIDX | beatoraja | LVF |
|---|---|---|---|
| 밀린 소리 | (반주는 오디오 스레드에서 계속 흐름) | 한꺼번에, 늦게, 처음부터 | 같음. 한 틱에 128개를 넘으면 조용히 버림 |
| 시작 위치 보정 (늦은 만큼 건너뛰고 재생) | — | 없음 | 없음 |
| 채보 시계 | 오디오에 맞춰 당김 | 그대로 (벽시계) | 그대로 (벽시계) |

### 왜 이 차이가 중요한가
IIDX는 곡 전체 길이의 반주가 있으니 그 재생 위치가 곧 "오디오의 진실"입니다. BMS는 키음 수백 개로 곡을 이루는 경우가 많아서 이런 기준 트랙이 없습니다. 이번 컨버터 작업에서도 28098의 반주(0001)는 곡의 일부일 뿐이었습니다. 그래서 BMS 엔진에서 오디오 시계는 **장치가 실제로 출력한 샘플 수**(FMOD `getDSPClock`, WASAPI `IAudioClock`, ASIO 콜백의 샘플 위치)여야 합니다. 이렇게 하면 소리를 "지금 울려라"가 아니라 "샘플 N에서 시작하라"로 예약할 수 있습니다(FMOD `setDelay` 같은 것). 두 구동기 모두 이 방식을 쓰지 않습니다.

---

## 6. 입력 시각

| | beatoraja | LVF |
|---|---|---|
| 방식 | 폴링 스레드 (키보드, 컨트롤러는 OIS 폴링을 가로채 1 kHz). MIDI는 콜백 | Windows: DirectInput8 즉시 모드 `GetDeviceState` 폴링. Linux: `SDL_PollEvent` (메인 스레드) |
| 시각 | 폴링한 순간의 `System.nanoTime()/1000` (`BMSPlayerInputProcessor.java:500-511`) | 폴링 직전의 `Time::now()`, 판정 때 ms로 자름 (`input_wrapper.cpp:52-55`) |
| OS 이벤트 시각 | 쓰지 않음. MIDI 콜백의 `timeStamp` 인자도 무시 | 쓰지 않음. 코드에 "TODO: take time from the input manager"라고 적혀 있음 |
| 디바운스 | 누름·뗌 모두 16 ms (키보드·컨트롤러), MIDI 없음 | 뗌 확인 5 ms + 디바운스 `MinInputInterval` (프로필 기본 5 ms, 키가 없을 때 대체값 16 ms: `config/cfg_profile.cpp:21`, `game/input/input_mgr.cpp:21`) |
| 저장 | 키마다 **마지막 변화 하나만** 저장 → 한 프레임 안에서 눌렀다 떼면 누름을 잃을 수 있음 (추정) | 큐로 전달 |

- **beatoraja 키보드 (추정, 근거 강함):** LWJGL2에서는 키보드 상태가 렌더 스레드의 `Display.update` 때 갱신됩니다. 그래서 폴링을 1 kHz로 해도 키보드 시각은 사실상 프레임 단위로 잘립니다. 컨트롤러와 MIDI는 이 영향을 받지 않습니다.
- **LVF Linux:** 코드 로그에 "precision limited by draw FPS"라고 직접 적혀 있습니다. (확인)
- **떼는 시각:** LVF는 뗌을 5 ms 동안 확인한 뒤에야 기록합니다(`input_wrapper.cpp:59-67`). 그 시각은 확인이 끝난 순간입니다(`:247`). 그래서 떼는 시각이 실제보다 늦게 찍힙니다. 기본 설정이면 약 5–10 ms, 디바운스가 16 ms면 약 16–21 ms입니다. (추정) LN 끝 판정에 영향을 줍니다.

---

## 7. 판정

### 7.1 판정 창

**LVF = LR2 값** ([ruleset_bms.h:277-281](third_party/lunaticvibesf/src/game/ruleset/ruleset_bms.h#L277-L281), 주석: "verified with LR2 disassembly") (확인)

| #RANK | PG | GR | GD | BD | 빈 POOR |
|---|---|---|---|---|---|
| 0 VERY HARD | 8 | 24 | 40 | 200 | 1000 |
| 1 HARD | 15 | 30 | 60 | 200 | 1000 |
| 2 NORMAL | 18 | 40 | 100 | 200 | 1000 |
| 3 EASY | 21 | 60 | 120 | 200 | 1000 |

- 오차 = `time.norm() - note.time.norm()`([ruleset_bms.cpp:950](third_party/lunaticvibesf/src/game/ruleset/ruleset_bms.cpp#L950)). 두 시각을 각각 ms 정수로 자른 뒤 뺍니다.
- 빠른 쪽 경계는 포함하고 늦은 쪽 경계는 포함하지 않습니다. 그래서 NORMAL PG는 실제로 −18..+17 ms입니다. (확인)
- #RANK 4, 잘못된 값, #RANK 없음은 모두 NORMAL로 처리합니다. #DEFEXRANK는 무시합니다. (확인)

**beatoraja = 모드별 µs 표 × #RANK 비율** ([JudgeProperty.java:23-33](third_party/beatoraja/src/bms/player/beatoraja/play/JudgeProperty.java#L23-L33), 7KEYS, 100% 기준) (확인)

| 종류 | PG | GR | GD | BAD (빠름/늦음) | 빈 POOR |
|---|---|---|---|---|---|
| 노트 | ±20 | ±60 | ±150 | +220 / −280 | +500 / −150 (고정) |
| 스크래치 | ±30 | ±70 | ±160 | +230 / −290 | +500 / −160 (고정) |
| LN 끝 | ±120 | ±160 | ±200 | +220 / −280 | — |

- 부호: `dmtime = 노트 시각 − 누른 시각`이므로 양수가 FAST입니다.
- #RANK 0..4는 25/50/75/100/125%입니다(`JudgeProperty.java:210`). 빈 POOR 창만 비율을 적용하지 않습니다.
  - 그래서 **#RANK 2(NORMAL)는 PG ±15, GR ±45, GD ±112.5 ms**입니다. LR2(±18)보다 PG가 좁고 GD는 넓습니다.
- #DEFEXRANK n은 n × 75/100 %입니다. bmson의 judge_rank는 % 그대로 씁니다. (확인, `BMSPlayerRule.java:61-66`)

**IIDX**: 채보마다 type 8 이벤트로 1/60 s 단위 정수 창을 가집니다([IIDX_timing_policy.md §8](IIDX_timing_policy.md)).

### 7.2 어느 노트를 판정하나
| | 규칙 |
|---|---|
| LVF | 레인에서 아직 처리 안 된 **가장 이른** 노트. 빈 POOR는 다음 노트 기준 −999..−201 ms 사이에서만 납니다. 늦은 빈 POOR는 없습니다. (확인) |
| beatoraja | 기본은 **Combo**: 아래쪽 노트가 더 이상 GOOD을 받을 수 없을 때만 다음 노트로 넘어갑니다. Duration(가장 가까운 노트)과 Lowest도 고를 수 있습니다. "Score"는 코드에 있지만 설정 검증에서 Combo로 되돌아갑니다. (확인) |

### 7.3 판정에 쓰는 시각
- 두 구동기 모두 **입력 이벤트에 찍힌 시각**으로 오차를 계산합니다. beatoraja는 `JudgeManager.java:386`, LVF는 `ruleset_bms.cpp:1499`입니다. (확인)
- 미스 판정은 다릅니다.
  - beatoraja: 프레임마다 훑습니다(`JudgeManager.java:592-628`).
  - LVF: 1 kHz 틱에서 노트 시각 + 200 ms에 판정합니다.
- 두 구동기 모두 **판정 오프셋을 판정 계산식에 넣지 않습니다.** `judgetiming`과 `JudgeOffset`은 화면 위치만 옮깁니다([LaneRenderer.java:301](third_party/beatoraja/src/bms/player/beatoraja/play/LaneRenderer.java#L301), `lvf/game/chart/chart.cpp:338`). 자동 조정 기능도 이 화면 오프셋만 1 ms씩 움직입니다. (확인)
  - 판정은 "누른 순간의 벽시계 vs 채보 시각"으로만 계산됩니다. 오디오 출력 지연이나 입력 장치 지연은 플레이어가 화면 오프셋으로 간접 보정할 수밖에 없습니다.

### 7.4 LN
| | LN | CN / HCN |
|---|---|---|
| LVF | LR2식: 머리 판정을 저장했다가 끝에서 확정합니다. 일찍 떼면 등급을 낮춥니다. 끝 전 약 PG 창에서 자동 완료합니다. (확인) | 지원 안 함 |
| beatoraja | 머리 판정 하나. 떼는 게 나쁘면 등급이 떨어지기만 합니다. | 머리와 끝을 따로 판정합니다. HCN은 200 ms마다 게이지가 변합니다. BSS는 반대 방향 스크래치로 끝냅니다. (확인) |

LVF에서 BAD 머리 LN이 두 번 끝난 것으로 셈해질 수 있다는 지적이 있습니다(`ruleset_bms.cpp:1077`, `:1724`). (추정)

---

## 8. 키음 믹싱

| | beatoraja | LVF |
|---|---|---|
| 누를 때 울리는 소리 | 판정된 노트의 소리. 창 안에 노트가 없으면 누른 시각 직전의 지난 노트나 투명 노트의 소리 | 틱마다, 1000 ms 안에 다가오는 레인 노트의 소리로 키를 다시 매핑합니다. 아니면 마지막 소리를 유지합니다. 판정과 관계없이 매번 울립니다. |
| 같은 소리 재트리거 | **항상 끊음**: 채널 = `wavId*256 + pitch + 128`, stop 후 play ([AbstractAudioDriver.java:507-526](third_party/beatoraja/src/bms/player/beatoraja/audio/AbstractAudioDriver.java#L507-L526)). 키와 배경음 모두 해당 | **항상 끊음** (추정): `FMOD_UNIQUE`로 로드 (`sound_fmod.cpp:678`) |
| 동시 발음 수 | 256 (`deviceSimultaneousSources`). PortAudio 믹서는 꽉 차면 **새 소리를 버림**, 리미터 없음 | 512 소프트웨어 채널, 우선순위나 뺏기 없음 |
| 로딩 | 전부 메모리에 PCM으로 디코딩. 로드할 때 선형 보간 리샘플 | 전부 메모리 (`CREATESAMPLE`) |
| 볼륨 | #VOLWAV × keyvolume/bgvolume (기본 각각 0.5) | 채보의 배경음이 KEY 그룹으로 나가서 **BGM 볼륨 슬라이더가 안 먹음** (`scene_play.cpp:2591`) (확인) |

**컨버터 작업과의 연결:** 이번에 Qwilight에서 FLAC 반주가 안 울렸던 문제는 디코딩 경로 문제였습니다(WAV로 바꾸자 해결). 같은 소리 재트리거 정책(끊기 vs 겹치기)은 IIDX 원곡을 재현할 때 소리가 실제로 달라지는 정책입니다. IIDX 채보는 같은 키음을 짧은 간격으로 다시 울리는 경우가 많기 때문입니다. 우리가 가진 `../converter/tools/bms_render.py`(겹치기)와 게임 미리듣기를 비교하면 어느 쪽이 원곡에 가까운지 잴 수 있습니다(§11의 실험 M6).

---

## 9. 노트 렌더링

| | beatoraja | LVF |
|---|---|---|
| 위치 | 타임라인을 따라 Δ마디 × scroll × (레인 높이 × HS)를 더함. 현재 구간만 시각으로 보간 ([LaneRenderer.java:321-326](third_party/beatoraja/src/bms/player/beatoraja/play/LaneRenderer.java#L321-L326)) | `y = 판정선 − floor(Δmetre × 레인 높이 × basespeed × HS)` (`sprite_lane.cpp:139-153`) |
| 계산 방식 | 매 프레임 시계에서 다시 계산 (상태 없음) | 매 프레임 마지막 틱 값에서 다시 계산, 정수 픽셀 |
| BPM 변화 | 화면 속도가 BPM에 비례, STOP에서 멈춤 | 같음 |
| 프레임 보간·출력 시각 예측 | 없음 | 없음 (최대 1 ms 묵은 값) |
| 녹숫자 | `240000/BPM/HS/speed/scroll × (1 − 레인 커버)` | `visible × 120 × 1200 / hs / bpm` (1 = 1/600 s) |
| 고정 속도 모드 | MAINBPM 기준으로 HS를 한 번 정함. 변속을 따라가는 floating HS 없음 | CONSTANT: 시간 기준 1600 ms, 화면 오프셋 미적용 |

**공통점:** 두 구동기 모두 **화면에 실제로 보이는 시각**(present 시각 + 디스플레이 지연)이 아니라 **프레임을 시작한 시각**으로 그립니다. 그래서 VSync 큐 깊이나 디스플레이 지연은 전부 플레이어가 화면 오프셋으로 흡수합니다.

---

## 10. 채보 시간 모델과 정밀도

| | beatoraja (jbms-parser) | LVF |
|---|---|---|
| 노트 시각 계산 | `t = prev.time(double) + stop + 240000000.0 × Δsection / BPM`, 저장할 때 `(long)` µs로 자름 (확인-bytecode) | BPM 구간마다 `basetime + beatLength × (Δsegment × barMetre × 4)`. beatLength를 ns 정수로 자름 (`chart_bms.cpp:388-390`) |
| 누적 오차 | 누적 합은 double로 유지 → 누적 없음 | 마디당 ns 수준 |
| STOP | `#STOP/192` 마디, 음수는 abs | value/192 (4박 마디 기준). 같은 위치에서는 Notes > BPM > STOP 순 |
| 위치 표현 | section (double) | 마디 단위 정확한 분수 (분모 1e12) |

둘 다 채보 시간 정밀도는 판정 오차보다 수천 배 작습니다. **채보 모델 정밀도는 병목이 아닙니다.** 오차는 전부 실행 경로(시계를 읽는 시점, 발음 시점, 입력 시각)에서 생깁니다.

---

## 11. 정책 공간 — 엔진 설정으로 뺄 축과 측정 방법

목표: 각 축을 엔진에서 바꿔 끼울 수 있게 만들고, **같은 입력 기록을 정책만 바꿔 다시 돌려서** 지표로 비교합니다. 아래 "후보"는 세 구동기와 [설계 문서](../design/만약%20리듬게임을%20개발한다면.md)에서 나온 선택지입니다.

### 11.1 축과 후보

| # | 정책 축 | 후보 | 관찰된 곳 |
|---|---|---|---|
| P1 | 기준 시계 | (a) 프레임 카운트 × 틱 길이 (b) OS 단조 시계 (c) OS 벽시계 (d) 오디오 DSP 클록 (e) DSP 클록 + 단조 시계 보간 | a: IIDX, b: beatoraja, c: LVF, d·e: 설계 문서 |
| P2 | 오디오 동기 | (a) 개방 루프 (b) slew: 천천히 당겨 맞춤 (c) 하드 재계산: 매 프레임 오디오 시계에서 시각을 다시 계산 | a: 두 BMS 구동기, b: IIDX, c: 설계 문서 |
| P3 | 발음 예약 | (a) 시각이 지나면 즉시 play (b) 늦은 만큼 건너뛰고 시작 (c) DSP 클록 기준 샘플 단위 예약 | a: 두 구동기 |
| P4 | 로직 갱신 | (a) 프레임마다 (b) 고정 틱을 프레임 때 몰아서 (c) 고정 틱 전용 스레드 | a: beatoraja, b: LVF, IIDX는 프레임=틱 |
| P5 | 입력 시각 | (a) 프레임 시각 (b) N Hz 폴링 시각 (c) OS 이벤트 시각 (Raw Input, QPC) (d) 장치 타임스탬프 (MIDI timeStamp 등) | a: beatoraja 키보드(추정), b: 둘 다 |
| P6 | 판정 창 표 | LR2 표, beatoraja 표 × 비율, IIDX 채보별 창 | |
| P7 | 노트 고르기 | 가장 이른 노트, Combo, 가장 가까운 노트, Lowest | LVF / beatoraja |
| P8 | 오프셋 적용 위치 | 화면만 / 판정 / 오디오 출력 지연 / 입력 지연을 각각 분리 | 둘 다 화면만 |
| P9 | 재트리거 | 끊기 / 겹치기 / 겹치기 + 상한 | 둘 다 끊기, Qwilight는 선택 |
| P10 | 보이스 한도 | 새 소리 버림 / 가장 오래된 소리 뺏기 / 가장 작은 소리 뺏기 | beatoraja 버림 |
| P11 | 렌더 시각 | 프레임 시작 시각 / 예측한 화면 표시 시각 | 둘 다 프레임 시작 |
| P12 | 화면 속도 모델 | 박 기준 (BPM 비례) / 시간 기준 (CONSTANT) / floating HS | |

### 11.2 지표

| 지표 | 정의 | 재는 법 |
|---|---|---|
| M1 발음 오차 | 실제 출력된 소리의 시작 − 채보 시각 | 출력을 루프백으로 녹음하고 `../converter/tools/bms_render.py` 렌더(샘플 정확한 정답)와 상호상관. 우리가 이미 쓰는 방법 |
| M2 판정 시각 오차 | 엔진이 기록한 누른 시각 − 실제 누른 시각 | 기준 입력 장치(마이크로컨트롤러가 정해진 시각에 키 신호를 넣고 같은 순간 오디오 클릭도 냄)로 비교 |
| M3 렉 복구 | 인위적 정지(50/100/500 ms)를 넣은 뒤, 오디오–채보 오차가 1 ms 안으로 돌아오기까지 걸린 시간과 그동안의 최대 오차 | 엔진 안에서 정지를 주입하고 로그 기록 |
| M4 화면 불연속 | 복구 중 노트가 한 프레임에 건너뛴 최대 픽셀 | 렌더 로그 |
| M5 장기 드리프트 | 곡 끝에서 오디오 DSP 클록 − 엔진 시계 | 로그 (10분 이상 곡이나 반복 재생) |
| M6 원곡 재현도 | 엔진 출력과 IIDX 미리듣기의 상관계수 | `../converter/tools/compare_preview.py` (P9·P10 비교용) |
| M7 판정 결정성 | 같은 입력 기록을 다시 돌렸을 때 판정이 같은 비율 | 입력 기록 재생 |
| M8 지연 분포 | 입력 → 키음 출력, 입력 → 화면 반응 | M1·M2 장치 조합 |

### 11.3 실험 하네스 (제안)
1. **입력 기록을 진실로 삼습니다.** LVF 리플레이처럼 (키, 시각, 누름/뗌) 스트림과 난수 시드를 저장합니다. 정책만 바꿔서 같은 기록을 다시 돌리면 판정 정책(P5–P8)은 오프라인에서 결정적으로 비교할 수 있습니다.
2. **오디오 출력을 진실로 삼습니다.** 엔진의 믹서 출력(장치로 보내기 직전 샘플)과 실제 루프백 녹음을 둘 다 남깁니다. 정답 렌더와 비교하면 P2–P4·P9·P10을 수치로 비교할 수 있습니다.
3. **렉을 일부러 일으킵니다.** 렌더 스레드에 정지를 주입하는 디버그 스위치를 둡니다. IIDX에서 관찰한 "렉 후 빨리 떨어짐"을 정책별로 재현하고 M3·M4를 잽니다.
4. **정책끼리 묶이는 곳을 표시합니다.**
   - P5가 (a)이면 P4가 무엇이든 판정 정확도가 FPS에 묶입니다.
   - P3 (c)는 P1이 (d)나 (e)일 때만 의미가 있습니다.
   - P2 (b)와 (c)는 반주 트랙이 아니라 DSP 클록이 있어야 BMS에서 동작합니다(§5).

---

## 12. 두 구동기에서 배울 점과 피할 점

**배울 점**
- **LVF의 가상 틱 시각:** 틱을 늦게 처리해도 시각은 격자 위에 정확히 붙습니다. 판정과 미스 판정이 결정적이 됩니다.
- **입력 시각을 폴링 스레드에서 찍는 것:** 두 구동기 모두 판정 오차를 FPS와 분리했습니다.
- **채보 시간을 double로 누적하고 저장할 때만 자르는 것** (beatoraja): 누적 오차가 없습니다.
- **beatoraja의 노트 고르기 알고리즘 선택:** P7을 정책으로 이미 분리해 둔 선례입니다.

**피할 점**
- 오디오 장치 시계를 읽지 않는 개방 루프 (P2 a, P3 a).
- 판정 오프셋이 화면에만 적용되는 것. 지연의 원인(입력·오디오·화면)이 하나의 숫자로 섞입니다.
- 키마다 마지막 변화 하나만 저장하는 입력 (beatoraja). 빠른 연타를 잃을 수 있습니다. (추정)
- 비단조 시계 (LVF `system_clock`).
- ms 정수로 자른 뒤 빼는 판정 (LVF). 경계가 비대칭이 됩니다(−18..+17).
- 보이스가 꽉 찼을 때 새 소리를 버리는 것 (beatoraja PortAudio). 지금 막 누른 키음이 사라집니다.
- 정수 나눗셈 틱 주기 (LVF 240 Hz → 250 Hz) 같은 단위 실수.

---

## 13. 미확인 사항과 다음 단계

1. **실행해서 확인하지 않았습니다.** 특히 다음 세 가지는 실제로 돌려 봐야 합니다.
   - beatoraja 키보드 시각이 프레임 단위로 잘리는지 (LWJGL2 `Display.update`)
   - LVF `FMOD_UNIQUE`의 재트리거 동작
   - LVF의 1 ms 기준점 어긋남
2. **LR2 원본은 분석하지 않았습니다.** LVF가 LR2를 재현했다고 주장하는 부분(판정 창, 게이지)만 LR2 근거로 썼습니다. LR2가 고정 틱을 쓰는지는 근거가 없습니다.
3. **IIDX의 입력 처리, 노트 고르기, 재트리거는 아직 분석하지 않았습니다.** 비교표의 빈칸입니다. `bm2dx.dll`에서 이어서 볼 수 있습니다.
4. **엔진 프로토타입에서 §11의 하네스부터 만드는 것을 제안합니다.** 정책을 바꿔 끼우는 구조보다 "같은 입력과 같은 오디오를 다시 돌려 재는 장치"가 먼저 있어야 비교가 객관적이 됩니다.
