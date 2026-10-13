# beatoraja 소스 분석 (Part B): 차트 타이밍 모델 · 판정 · 키음 믹싱 · 노트 렌더링

- 대상: `third_party/beatoraja` (HEAD `8320241d`, 2026-10-07). 경로는 별도 표기가 없으면 `src/bms/player/beatoraja/` 기준입니다.
- 차트 모델(`bms.model.*`)은 `lib/jbms-parser.jar`에 들어 있습니다. 디컴파일러가 없어서 직접 만든 class 파일 파서(constant pool, 멤버 시그니처, opcode 나열)로 시그니처·상수·바이트코드 흐름만 확인했습니다. 이 방법으로 확인한 내용은 **(확인-bytecode)**로 따로 표시했습니다.
- 표기: **(확인)** = 코드에서 직접 읽음, **(추정)** = 정황과 구조로 추론함.
- 시간 단위: beatoraja 내부 시간은 거의 모두 **µs(long)**입니다. 판정창 표는 ms로 환산해 적었습니다.

---

## 0. 한눈에 보는 실행 구조 (판정·렌더·오디오가 어느 스레드에서 무엇을 기준으로 도는가)

| 스레드 | 주기 | 시간 소스 | 하는 일 | 근거 |
|---|---|---|---|---|
| 렌더 스레드 (libGDX `render()`) | 프레임마다 (기본 maxFPS 240, vsync off) | `TimerManager.update()`에서 `System.nanoTime()` 읽기 | `nowmicrotime` 갱신, 노트 그리기 | `MainController.java:401-403`, `TimerManager.java:109-111`, `Config.java:50,73` (확인) |
| 입력 폴링 스레드 | 1 ms (ms 값이 바뀔 때마다 `input.poll()`) | `System.nanoTime()/1000 - starttime` | 키 상태 변화와 **변화 시각(µs)** 기록 | `MainController.java:355-369`, `BMSPlayerInputProcessor.java:500-501, 326-337` (확인) |
| 판정 스레드 `JudgeThread` | `TIMER_PLAY` µs 값이 바뀔 때만 `judge.update(mtime)`, 아니면 0.5 ms sleep | `timer.getNowMicroTime(TIMER_PLAY)` = **렌더 스레드가 갱신한 값** | 판정, 놓침(POOR) 처리, 키음 재생 트리거 | `play/KeyInputProccessor.java:158-205` (확인) |
| BGM 스레드 `AutoplayThread` | 다음 BG 타임라인까지 sleep(ms) | `timer.getNowMicroTime(TIMER_PLAY)` (렌더 프레임 단위) | BG 레인 키음 재생 | `play/KeySoundProcessor.java:76-96` (확인) |
| 믹서 (PortAudio/AudioDevice 선택 시) | 버퍼 단위 blocking write | 오디오 장치 | 소프트웨어 믹싱 | `audio/PortAudioDriver.java:603-650` (확인) |

`TimerManager.nowmicrotime`을 갱신하는 곳은 `MainController.render()`와 `BMSPlayer.java:590`(로딩 중) 두 곳뿐입니다 (확인). 그래서 **판정 스레드와 BGM 스레드가 보는 "현재 시각"은 렌더 프레임 단위로 양자화**됩니다. 판정 스레드는 별도 스레드이지만 실제로는 프레임당 한 번 돕니다 (`if (mtime != prevtime)` 조건, `KeyInputProccessor.java:171`) (확인).

---

## 1. 차트 타이밍 모델

### 1.1 자료구조 (jbms-parser)
- `TimeLine` 필드 (확인-bytecode, 시그니처): `long time`, `double section`, `double bpm`, `long stop`, `double scroll`, `double speed`, `boolean speedObj`, `boolean sectionLine`, `Note[] notes / hiddennotes / bgnotes`.
  - 접근자는 `getMicroTime()`, `getMilliTime()`, `getTime()(int)`, `getMicroStop()`, `getMilliStop()`입니다. constant pool에 있는 숫자 상수가 `1000L` 하나뿐이므로 `time`과 `stop`은 **µs로 저장하고 ms는 /1000으로 파생**한다고 봅니다 (확인-bytecode: 상수, 추정: 정확한 식).
- `Note` 필드: `long time`, `double section`, `int wav`, `long start, duration`(음 자르기용 µs), `int state`, `long playtime`, `Note[] layerednotes` (확인-bytecode).
- `LongNote`: `boolean end`, `LongNote pair`, `int type` (`TYPE_UNDEFINED/LONGNOTE/CHARGENOTE/HELLCHARGENOTE` = 0..3) (확인-bytecode).
- `Lane`: `notes[]`, `hiddens[]`와 탐색 포인터(`notebasepos`, `noteseekpos`). `mark(int ms)`, `reset()`, `getNote()`로 레인별 순차 탐색을 합니다 (확인-bytecode). JudgeManager는 이것으로 판정 후보를 훑습니다.
- 한 차트는 `BMSModel.timelines[]`(정렬된 TimeLine 배열) 하나로 표현됩니다. 같은 section 위치의 노트는 같은 TimeLine을 공유합니다.

### 1.2 시간 계산식: section(마디 단위 double) → µs
`bms/model/Section.getTimeLine(double section)`의 바이트코드 흐름은 다음과 같습니다 (확인-bytecode, 위 의사코드는 opcode를 재구성한 것):

```
entry  = tlcache.lowerEntry(section)       // 바로 앞 TimeLine (TimeLineTreeMap, key=section double)
prevT  = entry.time  (double, µs)           // TimeLineEntry.time 은 double
t      = prevT + entry.timeline.getMicroStop()
         + 240000000.0 * (section - entry.section) / entry.timeline.getBPM()
tl     = new TimeLine(section, (long) t, mode.key)   // d2l = 0 방향 절삭
tlcache.put(section, t /*double 유지*/, tl)
```

- **240,000,000 = 4박 × 60 s × 10⁶ µs**입니다. BPM 1에서 한 마디(4/4)가 걸리는 µs입니다.
- **누적 오차 없음**: 누적값은 `TimeLineEntry.time`(double)으로 이어지고, 각 TimeLine에 저장할 때만 `long µs`로 **절삭(truncation)**합니다. 그래서 노트 시각의 오차는 노트마다 독립적으로 최대 1 µs 미만이고, 체인을 따라 쌓이지 않습니다 (확인-bytecode).
- **마디 길이(#xxx02)**: `Section.rate`(double)입니다. 채널 위치를 `sectionnum + pos * rate` 식으로 section 좌표에 반영합니다 (확인-bytecode: `makeTimeLines`에 `getfield sectionnum; ... getfield rate; dmul; dadd` 패턴이 있음). 즉 section 좌표 자체가 "마디 길이를 반영한 4/4 마디 단위"입니다.
- **STOP**: `#STOPxx` 값을 `/192.0`으로 나눠 마디 비율로 바꾸고, 음수면 `abs`를 취한 뒤 경고합니다 (`BMSDecoder`, 확인-bytecode). 그다음 `setStop((long)(240000000.0 * stopValue / bpm))`로 해당 TimeLine에 µs 단위 정지 시간을 저장합니다 (`Section.makeTimeLines`, 확인-bytecode). 정지는 **다음 TimeLine의 시각에 더해집니다**. 정지가 있는 TimeLine의 노트 자체는 정지 직전 시각입니다.
- **BPM 변경**: 해당 TimeLine의 `bpm`을 바꾸고, 그 뒤 구간의 시간 계산에 씁니다 (확인-bytecode).
- **#SCROLL / #SPEED**: `TimeLine.scroll`, `speed`에 저장됩니다. **시간 계산식에는 들어가지 않고 렌더링에만 쓰입니다** (getTimeLine 식에 BPM과 stop만 등장함, 확인-bytecode). 렌더러 사용처는 §4에 있습니다.
- **시작 여유**: `BMSModelUtils.setStartNoteTime(model, 1000)`. 첫 노트가 1000 ms보다 앞이면 전체를 뒤로 미루고 그 양을 `marginTime`으로 돌려줍니다 (`PlayerResource.java:214`, 확인 / 내부 구현은 추정).

### 1.3 정밀도 요약
| 항목 | 표현 | 비고 |
|---|---|---|
| 노트/타임라인 시각 | `long` µs | 계산은 double, 저장할 때 절삭 (확인-bytecode) |
| section 위치 | `double` (마디) | 렌더 y 계산의 기준 (확인) |
| BPM, scroll, speed | `double` | (확인-bytecode) |
| 판정 비교 | `long` µs 정수 뺄셈 | §2.4 (확인) |
| 렌더 시각 | **ms(long)** × 1000 | `LaneRenderer.java:300-306`. 렌더는 1 ms 단위로 양자화됨 (확인) |

**IIDX 비교 포인트**: IIDX는 고정 tick(1/60 s 프레임) 시뮬레이션이고 판정창도 1/60 s 단위입니다. beatoraja는 **연속 시간(µs) 모델**을 씁니다. 차트 시각, 판정창, 입력 타임스탬프가 모두 µs입니다. 다만 "현재 시각"을 샘플링하는 시점이 렌더 프레임에 묶여 있습니다.

---

## 2. 판정 (JudgeManager / JudgeProperty / JudgeAlgorithm)

### 2.1 판정창 원본 표 (`play/JudgeProperty.java:12-55`, 확인)
배열은 `{LATE 하한, EARLY 상한}` 쌍이 PG, GR, GD, BD, MS(=빈 POOR) 순서로 들어 있습니다. 부호 규약은 `dmtime = noteTime − pressTime`이므로 **양수 = 빨리 누름(FAST)**, 음수 = 늦게 누름(SLOW)입니다 (`JudgeManager.java:386`, 주석 `JudgeManager.java:67` "+は早押し", 확인).

```java
SEVENKEYS(new long[]{ -20000, 20000, -60000, 60000, -150000, 150000, -280000, 220000, -150000, 500000 }, // note
          new long[]{ -30000, 30000, -70000, 70000, -160000, 160000, -290000, 230000, -160000, 500000}, // scratch
          new long[]{ -120000, 120000, -160000, 160000, -200000, 200000, -280000, 220000},             // LN end
          0,                                                                                          // LN release margin
          new long[]{ -130000, 130000, -170000, 170000, -210000, 210000, -290000, 230000},             // BSS end
          0, new boolean[]{true, true, true, false, false, true },   // combo 유지: PG GR GD BD PR MS
          MissCondition.ALWAYS, new boolean[]{true, true, true, true, true, false }, // judgeVanish
          JudgeWindowRule.NORMAL),
```

7KEYS, **judgerank = 100 (= #RANK 3 EASY)** 기준 (ms):

| 종류 | PGREAT | GREAT | GOOD | BAD | 빈 POOR(MS) |
|---|---|---|---|---|---|
| 일반 노트 | ±20 | ±60 | ±150 | −280(늦음) / +220(빠름) | −150 / +500 (**랭크와 무관하게 고정**) |
| 스크래치 | ±30 | ±70 | ±160 | −290 / +230 | −160 / +500 (고정) |
| LN/CN 종단 (떼기) | ±120 | ±160 | ±200 | −280 / +220 | 없음 |
| BSS 종단 | ±130 | ±170 | ±210 | −290 / +230 | 없음 |

- 그 밖의 모드: 5KEYS(PG ±20, GR ±50, GD ±100, BD ±150), PMS(GD ±117, BD ±183, LN 떼기 여유 200 ms, `MissCondition.ONE`), KEYBOARD(비대칭)가 있습니다 (확인 `JudgeProperty.java:12-55`).
- `BMSPlayerRule`: BEAT_7K와 14K는 `JudgeProperty.SEVENKEYS`를 씁니다 (`BMSPlayerRule.java:14`, 확인).

### 2.2 #RANK / #DEFEXRANK / bmson judge_rank 스케일링
1단계: 로드할 때 퍼센트로 정규화합니다 (`BMSPlayerRule.validate`, `BMSPlayerRule.java:61-66`, 확인).
```java
case BMS_RANK -> judgerank >= 0 && judgerank < 5 ? rule.judge.windowrule.judgerank[judgerank][1] : ...[2][1];
case BMS_DEFEXRANK -> judgerank > 0 ? judgerank * rule.judge.windowrule.judgerank[2][1] / 100 : ...[2][1];
case BMSON_JUDGERANK -> judgerank > 0 ? judgerank : 100;
```
- `JudgeWindowRule.NORMAL.judgerank = {{25..},{50..},{75..},{100..},{125..}}` (`JudgeProperty.java:210`)이므로 **#RANK 0/1/2/3/4 → 25/50/75/100/125 %**입니다.
- `#DEFEXRANK n` → `n × 75 / 100` %입니다. 즉 DEFEXRANK 100이 NORMAL(75 %)과 같습니다.
- bmson은 `judge_rank < 5`면 BMS_RANK로 취급하고, 그 외에는 값을 그대로 %로 씁니다 (`BMSONDecoder`, 확인-bytecode). 따라서 **bmson judge_rank 100은 BMS EASY(#RANK 3)와 같은 창**입니다 (확인: 위 식에서 도출).
- 정의되지 않았거나 범위를 벗어나면 NORMAL(75 %)입니다.

2단계: 창을 생성합니다 (`JudgeWindowRule.create`, `JudgeProperty.java:230-276`, 확인).
- NORMAL 규칙의 `fixjudge = {F,F,F,F,T}`이므로 PG, GR, GD, BD는 `org × rank / 100`(µs 정수 연산)으로 스케일하고, **MS(빈 POOR)는 고정**입니다.
- 그다음 커스텀 판정 배율(`judgeWindowRate`, PG/GR/GD만)을 곱하고, BAD보다 넓으면 BAD로 자르고 아래 등급보다 좁으면 아래 등급으로 맞춥니다 (`JudgeProperty.java:263-273`).

7KEYS 일반 노트의 랭크별 실제 창 (ms, 확인: 위 식으로 계산):

| #RANK | 배율 | PG | GR | GD | BD (늦음/빠름) | MS |
|---|---|---|---|---|---|---|
| 0 VERYHARD | 25 % | ±5 | ±15 | ±37.5 | −70 / +55 | −150/+500 |
| 1 HARD | 50 % | ±10 | ±30 | ±75 | −140 / +110 | 〃 |
| 2 NORMAL | 75 % | ±15 | ±45 | ±112.5 | −210 / +165 | 〃 |
| 3 EASY | 100 % | ±20 | ±60 | ±150 | −280 / +220 | 〃 |
| 4 VERYEASY | 125 % | ±25 | ±75 | ±187.5 | −350 / +275 | 〃 |

> IIDX와의 차이: IIDX는 차트마다 1/60 s 단위 정수로 창을 저장합니다. beatoraja는 **모드별 µs 기준표 하나에 차트의 % 배율을 곱하는 방식**입니다. 결과가 frame 경계에 맞춰지지 않습니다 (예: 112.5 ms).

### 2.3 판정 루프 구조 (`JudgeManager.update(long mtime)`, `JudgeManager.java:212-634`, 확인)
`JudgeThread`가 `mtime`(= 현재 `TIMER_PLAY` 경과 µs, 프레임 단위로 갱신)을 넘길 때마다 다음 순서로 처리합니다.
1. **통과 처리** (219-304): `(prevmtime, mtime]` 구간을 지난 노트를 처리합니다. HCN 통과 상태를 기록하고, 지뢰는 "그 순간 레인 키를 누르고 있으면" 데미지와 소리를 줍니다 (244-249). 오토플레이 처리도 여기서 합니다.
2. **HCN 게이지 틱** (306-338): 200 ms(`hcnmduration = 200000`, 81행)마다 누르고 있으면 `gauge.update(1, 0.5f)`(GREAT의 절반), 떼고 있으면 `update(3, 0.5f)`(BAD의 절반)입니다.
3. **키 이벤트 처리** (341-554): 키마다 `pmtime = input.getKeyChangedTime(key)`를 읽고, 바뀐 것이 없으면 건너뜁니다. 처리 후 `resetKeyChangedTime`을 호출합니다.
4. **LN 종단 / 놓침 POOR 스캔** (557-633).

### 2.4 판정 시각: 입력 이벤트 타임스탬프를 쓰는가, 프레임 시각을 쓰는가
- **누름과 뗌의 판정은 입력 이벤트 타임스탬프를 씁니다** (확인).
```java
final long pmtime = input.getKeyChangedTime(key);           // JudgeManager.java:346
...
final long dmtime = judgenote.getMicroTime() - pmtime;      // :386  (노트 − 입력)
judge = window.getJudge(noteType, dmtime);                  // :403
```
  `getJudge`는 PG부터 차례로 `difftime >= late && difftime <= early`인 첫 등급을 돌려줍니다. 어디에도 들지 않으면 `length/2`(=5)입니다 (`JudgeProperty.java:184-189`).
- 타임스탬프의 출처는 입력 폴링 스레드가 기록한 `System.nanoTime()/1000 − starttime`입니다 (`BMSPlayerInputProcessor.java:501`, `keyChanged` 326-337). `starttime`은 플레이를 시작할 때 `TIMER_PLAY`와 같은 원점으로 맞춥니다 (`BMSPlayer.java:609-612`: `input.setStartTime(micronow + timer.getStartMicroTime() - starttimeoffset*1000)`) (확인).
- 해상도 (확인/추정 구분):
  - 폴링 주기 1 ms (확인, `MainController.java:358-361`).
  - 키보드는 `Gdx.input.isKeyPressed()` 상태를 폴링합니다 (`KeyBoardInputProcesseor.java:107`). LWJGL2 백엔드에서 키보드 상태는 렌더 스레드의 `Display.processMessages()`에서 갱신되므로, **키보드 타임스탬프는 사실상 렌더 프레임에 양자화될 수 있습니다** (추정. Part A 입력 분석과 교차 확인이 필요합니다).
  - 디바운스: 키보드와 컨트롤러 모두 기본 `duration = 16` ms입니다. 마지막 변화 후 16 ms 안의 변화는 무시합니다 (`PlayModeConfig.java:270,498`, `KeyBoardInputProcesseor.java:108`, `BMControllerInputProcessor.java:61,136`, 확인).
  - **키마다 "마지막 변화" 1개만 저장합니다** (`time[id]` 덮어쓰기, `BMSPlayerInputProcessor.java:330-331`). 판정 갱신 사이(대략 1 프레임) 안에 누름과 뗌이 모두 일어나면 누름 이벤트를 잃습니다. 키 상태는 현재값(`getKeyState`)으로 다시 읽습니다 (확인: 구조 / 실제 영향은 추정. 16 ms 디바운스가 있어 고 FPS에서는 드뭅니다).
- **놓침(POOR), LN 종단 자동 완료, HCN 틱, 지뢰는 프레임 시각 `mtime` 기준**입니다 (확인 557-633). 따라서 놓침 POOR가 확정되는 시점은 최대 1 프레임 늦을 수 있습니다. 다만 판정 결과 자체(POOR)는 같습니다.
- **판정 오프셋**: JudgeManager에는 **입력 쪽 판정 오프셋이 없습니다** (확인: `judgetiming`이 사용되는 곳은 `LaneRenderer.java:301`과 자동조정 `JudgeManager.java:697-703`뿐이고, `audiooffset`이나 `inputoffset` 류의 필드도 검색되지 않음). `PlayerConfig.judgetiming`(−500..+500 ms, 기본 0)은 **노트 표시 시각만** 옮깁니다 (스킨 속성명도 `notesdisplaytiming`, `IntegerPropertyFactory.java:67`).
  ```java
  time = (... time - main.timer.getTimer(TIMER_PLAY) ...) + config.getJudgetiming();   // LaneRenderer.java:300-301
  ```
  자동 조정 옵션 `notesDisplayTimingAutoAdjust`가 켜져 있으면, GOOD 이상이면서 |Δ| ≤ 150 ms인 판정마다 `judgetiming -= (Δ ± 15 ms) / 30 ms`(정수 나눗셈)만큼 보정합니다. Δ가 15–45 ms면 1 ms씩입니다 (`JudgeManager.java:697-703`, 확인). 이 보정은 **판정이 아니라 표시를 사람 쪽에 맞추는 방식**입니다.

### 2.5 창 안에 노트가 여러 개일 때 대상 선택 (`JudgeManager.java:380-417`, `JudgeAlgorithm.java`, 확인)
레인의 노트를 시간 순서대로 훑습니다. `dmtime < mjudgestart`(이미 너무 늦은 노트)면 건너뛰고, `dmtime >= mjudgeend`(너무 이른 노트)면 멈춥니다. 지뢰와 LN 종단은 후보에서 빠집니다. `mjudgestart/end`는 노트와 스크래치의 모든 등급(MS 포함) 창의 합집합입니다 (190-197).
```java
if (tnote == null || tnote.getState() != 0 || algorithm.function.compare(tnote, judgenote, pmtime, window, noteType)) {
    ...
    if (judgenote.getState() != 0) judge = (MS창 안) ? 5 : 6;           // 이미 처리된 노트 → 빈 POOR 후보
    else { judge = window.getJudge(noteType, dmtime); judge = (judge >= 4 ? judge + 1 : judge); }
    if (judge < 6) { if (judge < 4 || tnote == null || |tnote−p| > |judgenote−p|) tnote = judgenote; }
    else tnote = null;
}
```
판정 코드는 0 PG, 1 GR, 2 GD, 3 BD, 4 POOR(놓침), 5 빈 POOR(MS), 6 창 밖입니다.

`PlayConfig.judgetype`으로 고르는 알고리즘 (`JudgeAlgorithm.java:17-37`)은 다음과 같습니다. compare가 true이면 뒤 노트(t2)를 고릅니다.
| 이름 | 규칙 | 의미 |
|---|---|---|
| **Combo (기본)** | `t2 미처리 && t1.time < p + GD_late(음수) && t2.time <= p + GD_early` | 아래 노트를 GOOD 이상으로 칠 수 없을 때(GOOD 늦음 한계를 넘었을 때)만 다음 노트로 넘어감. 콤보 보존 우선 |
| Duration | `|t1−p| > |t2−p| && t2 미처리` | 시간상 가장 가까운 노트 |
| Lowest | 항상 false | 가장 아래(가장 이른) 미처리 노트 |
| Score | Combo와 같은 형태에 GREAT 창 사용 | 정의는 있지만 `defaultAlgorithm = {Combo, Duration, Lowest}`라서 `PlayConfig.validate()`가 Score를 Combo로 되돌림. **설정으로 선택 불가** (`PlayConfig.java:262-264`, 확인) |

### 2.6 빈 POOR (空POOR)
- 대상 노트 판정이 MS창에만 들면(BD 밖, 7K 기준 최대 500 ms 빠름 / 150 ms 늦음) judge 5가 됩니다. `judgeVanish[5] = false`이므로 **노트는 소모되지 않고 남습니다**. 카운트, 게이지 −2 %(NORMAL), 판정 표시는 일어납니다 (`JudgeManager.java:401-404, 458`, `updateMicro` 639-652, 확인).
- 이미 처리된 노트의 MS창 안에서 눌러도 judge 5가 됩니다 (401행).
- 콤보: SEVENKEYS의 `combo[5] = true`이므로 **빈 POOR는 콤보를 끊지 않습니다**. `judge < 5` 조건 때문에 콤보가 늘지도 않습니다 (660-669, 확인). BAD(3)와 POOR(4)는 콤보를 끊습니다.
- 소리: 빈 POOR 대상 노트의 키음을 그대로 재생합니다 (455행 `keysound.play(tnote...)`가 판정 전에 무조건 호출됨, 확인). 그래서 500 ms 이르게 쳐도 다가오는 노트의 키음이 납니다.
- 창 안에 후보가 하나도 없으면(`tnote == null`) 판정은 없고 키음만 냅니다 (§3.1).

### 2.7 놓침 POOR 감지 (`JudgeManager.java:592-628`, 확인)
```java
for (Note note = state.lanemodel.getNote(); note != null
        && note.getMicroTime() < mtime + window.getTime(noteType, 3, false); ...) {   // 노트시각 < now − BD_late
    if (note instanceof NormalNote && note.getState() == 0) this.updateMicro(state, note, mtime, 4, mjud, true);
```
- 프레임마다 레인별로 "BAD 늦음 한계를 지난 미처리 노트"를 스캔해서 POOR(4)로 처리합니다. 스캔 시작점은 `lanemodel.mark((prevmtime + mjudgestart − 100000)/1000)`(220행)로 앞쪽을 잘라 둡니다.
- CN/HCN 시작을 놓치면 시작과 종단에 POOR를 **두 번** 줍니다 (605-606). LN(LNTYPE_LONGNOTE) 시작을 놓치면 시작만 POOR입니다 (611).

### 2.8 LN / CN / HCN / BSS
LN 종류는 차트의 `LongNote.type` 또는 `PlayerConfig.lnmode`(0=LN, 1=CN, 2=HCN)로 정해집니다 (`model.getLntype()`, `PlayerResource.java:161`) (확인).

- **LN (시작만 판정하는 LR2식)** (425-441, 532-549, 563-578):
  - 누름이 판정되면 결과를 바로 반영하지 않고 `lnstartJudge`와 `lnstartDuration`에 보관합니다. `processing = 종단`.
  - 뗄 때 `judge = max(종단창 판정, 시작판정)`이고, Δ는 둘 중 |Δ|가 큰 쪽입니다. 결과는 **시작 노트 한 개의 판정**으로 기록합니다 (`updateMicro(..., processing.getPair(), ...)`).
  - BAD 이하이면서 일찍 뗐다면(`judge>=3 && dmtime>0`) `releasetime`을 기록해 두고 `releasemargin`(7K = 0 ms, PMS = 200 ms) 뒤에 BAD로 확정합니다. 그 사이에 다시 누르면 취소됩니다 ("押し直し", 373, 377).
  - 끝까지 누르고 있으면 종단 시각을 지나는 프레임에 `lnstartJudge`로 자동 확정합니다 (572-578).
- **CN / HCN** (443-453, 496-521, 579-590, 615-626):
  - 시작은 바로 판정합니다. 뗄 때는 LONGNOTE_END 창(PG ±120 ms …)으로 **종단을 따로 판정**합니다.
  - BAD 이하로 일찍 떼면 margin 뒤에 BAD 또는 POOR입니다.
  - **누른 채 종단을 지나쳐 노트 BD 늦음 한계(7K/EASY 기준 280 ms)를 넘기면 종단이 POOR**입니다. 반드시 떼야 합니다 (615-626, 확인).
  - HCN은 위의 200 ms 틱 게이지 증감이 추가되고, 떼고 있는 동안 해당 키음 볼륨을 0으로 만듭니다 (322-336).
- **스크래치와 BSS**: 스크래치 레인은 키 2개(위/아래 방향)가 한 레인에 할당되고, 각 키 입력이 독립된 누름 이벤트입니다. 창은 스크래치 전용 표(약 10 ms 넓음)를 씁니다 (`noteType = sc>=0 ? SCRATCH : NOTE`, 380행).
  - BSS(스크래치 CN)는 시작한 방향 키를 `sckey[sc]`에 기억합니다. **반대 방향 키를 누르는 순간** LONGSCRATCH_END 창으로 종단을 판정합니다 (359-370).
  - 시작 키를 떼는 것은 종단창 밖(judge == 4)일 때만 "중간에 뗌"으로 인정하고, 창 안에서 떼는 것은 무시합니다 (502-509) (확인).

### 2.9 판정 결과가 퍼지는 경로 (`updateMicro`, 639-705, 확인)
`note.setState(judge+1)` → ScoreData 카운트(FAST/SLOW 구분) → 최근 100개 Δ 링버퍼 → 콤보 → 스킨 타이머(봄, 판정 표시) → `main.update(judge, ...)`(게이지) → `keysound.play(judge, fast)`(판정 가이드 SE).

---

## 3. 키음 믹싱 / 보이스

### 3.1 키를 누르면 어떤 소리가 나는가 (확인)
1. **판정 대상 노트(tnote)가 있으면 그 노트의 키음**입니다 (424, 455행). BAD나 빈 POOR여도 재생합니다.
2. **창 안에 후보가 없으면 (`tnote == null`)** (461-486):
   ```java
   Note n = notes.length > 0 ? notes[0] : null;                 // 기본값 = 레인 첫 노트
   for (Note note : state.lanemodel.getHiddens()) { if (note.getMicroTime() >= pmtime) break; n = note; }
   for (Note note : notes) { if (note.getMicroTime() >= pmtime) break;
       if ((n == null || n.getMicroTime() <= note.getMicroTime()) && !(note instanceof LongNote && note.getState() != 0)) n = note; }
   if (n != null && state.passing == null && !(n instanceof MineNote)) keysound.play(n, keyvolume, 0);
   ```
   → **누른 시각 이전에서 가장 최근의 노트 또는 숨은 노트(#3x 채널)의 키음**입니다. 해당하는 것이 없으면 레인의 첫 노트 키음입니다. 미래의 가장 가까운 노트를 쓰지 않습니다. HCN 통과 중에는 소리를 내지 않습니다.
3. LN과 CN 종단에서는 종단 노트의 키음을 재생합니다 (516, 543, 568, 585). 대개 wav = −1이라 소리는 나지 않습니다. 실패한 LN 떼기에서는 `setVolume(pair, 0)`으로 LN 키음을 끊습니다 (566, 582).
4. BGM(BG 레인)은 `AutoplayThread`가 `TIMER_PLAY ≥ timeline 시각`이 되면 `bgvolume`으로 재생합니다 (`KeySoundProcessor.java:77-84`). **트리거 시각이 렌더 프레임 단위**이고, 샘플 정확도 스케줄링은 없습니다 (확인: 시간 소스 / 추정: 지터 크기는 1 프레임 이내).

### 3.2 같은 소리를 다시 트리거하면: **같은 WAV ID는 이전 보이스를 끊고 다시 재생 (choke)**
```java
private int channel(int id, int pitch) { return id * 256 + pitch + 128; }           // AbstractAudioDriver.java:507-509
...
final int channel = channel(id, pitchShift);
if (wav != null) { stop(wav, channel); play(wav, channel, volume, pitch); }        // :517-526
```
- 키음과 BGM이 모두 `play(Note,...)` → `play0`를 거칩니다. 그래서 **같은 `#WAVxx`가 다시 울리면 앞의 보이스를 멈추고 처음부터 재생합니다**. 키 레인이든 BG 레인이든, 레인과 무관하게 WAV ID 단위로 그렇습니다 (확인).
- 음 자르기(bmson slice: `start/duration`) 노트는 slice별 PCM을 쓰지만, 채널 번호는 같은 WAV ID라서 같은 ID의 다른 slice도 서로 끊습니다 (`stop(slice.wav, channel)`은 PCM 객체와 채널이 둘 다 같아야 함. 서로 다른 slice는 PCM 객체가 다르므로 **실제로는 겹칠 수 있음**) (확인: 비교 조건 / 추정: 실제 동작).
- 판정 가이드 SE도 판정 종류별 채널에서 stop 후 play합니다 (493-505).
- **Qwilight의 "중복 키음 방지"와 비교**: beatoraja에는 이 동작을 끄고 켜는 옵션이 없습니다. **항상 "같은 키음은 1 보이스(재트리거 시 처음부터)"**로 동작합니다 (확인: AudioConfig와 PlayerConfig에 관련 필드 없음). 겹쳐 울리기(overlap) 정책은 선택할 수 없습니다. 같은 WAV가 짧은 간격으로 연타되는 차트에서는 소리 꼬리가 잘립니다.

### 3.3 보이스 수 / 드라이버별 동작
| 드라이버 (`AudioConfig.driver`, 기본 **OpenAL**) | 믹싱 | 동시 보이스 | 초과 시 | 근거 |
|---|---|---|---|---|
| OpenAL (libGDX `Sound`) | OpenAL 소스 | `deviceSimultaneousSources` 기본 **256** → `cfg.audioDeviceSimultaneousSources` | libGDX가 빈 소스를 못 찾으면 재생 실패(−1) (추정, libGDX 구현) | `MainLoader.java:193`, `GdxSoundDriver.java:122-136` (확인). stop/setVolume용 추적 배열도 256개 링버퍼라서, 오래된 핸들은 추적에서 빠짐 (`sounds[soundPos]`, 확인) |
| PortAudio | 자체 소프트웨어 믹서 (float 합산) | `MixerInput[deviceSimultaneousSources]` = 256 | **빈 슬롯이 없으면 새 소리를 버림 (voice stealing 없음)** | `PortAudioDriver.java:84, 537-553` (확인) |
| AudioDevice (libGDX) | 같은 방식의 자체 믹서 | 256 | (같은 구조로 추정) | `GdxAudioDeviceDriver.java:41` (확인) |

- 소프트웨어 믹서의 특징 (`PortAudioDriver.java:603-635`, 확인):
  - 단순 합 `wav_l += sample * volume`이고 **리미터나 클리핑 처리가 없습니다**. float을 그대로 장치에 씁니다.
  - 피치나 속도를 바꿀 때는 `posf` 누적 후 정수로 절삭하는 **최근접(보간 없는) 리샘플링**입니다.
  - 버퍼 크기는 `deviceBufferSize` 기본 384 프레임입니다.
- 로드할 때 샘플레이트를 장치 레이트로 변환합니다. 이때는 **선형 보간**입니다 (`PCM.java:391-404`, 확인).

### 3.4 볼륨 / 게인
- `AudioConfig`: `systemvolume`, `keyvolume`, `bgvolume` 모두 기본 0.5입니다 (`AudioConfig.java:46-54`, 확인). 키음 = `this.volume(#VOLWAV) × keyvolume`, BGM = `#VOLWAV × bgvolume` (`AbstractAudioDriver.java:486-490`).
- `#VOLWAV`는 1–199일 때 `/100`을 적용하고, 그 외에는 1.0입니다 (355-359). 키와 BGM의 상대 게인은 이 두 슬라이더뿐입니다. 정규화나 라우드니스 처리는 없습니다 (확인).
- 연습 모드의 FREQ 변경: `FrequencyType.FREQUENCY`는 전역 피치를 바꾸고, `SPEED`는 `TimeStretchProcessor`(TarsosDSP)로 미리 늘린 PCM을 씁니다 (`BMSPlayer.java:554-565`, 확인).

### 3.5 포맷별 처리
- 확장자 대체 탐색 순서는 `.wav → .flac → .ogg → .mp3`입니다. 정의된 확장자 파일이 없으면 이 순서로 찾습니다 (`AudioDriver.java:178-179, 207-208`, 확인).
- OpenAL 경로 (`GdxSoundDriver.getKeySound`, 76-81, 확인):
  - `.wav`와 `.flac`은 자체 `PCM.load`로 디코딩한 뒤 WAV 스트림으로 감싸 `newSound`에 넘깁니다.
  - `.ogg`와 `.mp3`는 **libGDX 내장 디코더**로 바로 넘깁니다 (`Gdx.audio.newSound`).
- 자체 PCM 로더 (`PCM.java:184-320`, 확인):
  - WAV는 직접 파싱합니다 (16-bit는 `ShortDirectPCM`, 8-bit는 `BytePCM`, 16-bit 초과는 `FloatPCM`).
  - OGG는 libGDX `OggInputStream`, MP3는 별도 디코더로 16-bit 고정입니다.
  - **FLAC은 jflac**(`FlacProcessor`)이고 원래 bit depth를 유지합니다.
  - 채널과 샘플레이트는 드라이버 설정에 맞춰 변환합니다. 차이는 디코더 경로와 bit depth뿐이고, 재생 정책에는 포맷별 차이가 없습니다.
- 모든 키음은 **로드할 때 전부 메모리에 디코드**합니다 (`setModel`의 병렬 로드, `AudioCache`, `AbstractAudioDriver.java:340-453`). 스트리밍은 없습니다 (확인).

---

## 4. 노트 렌더링과 스크롤 (`play/LaneRenderer.java`)

### 4.1 y 좌표 계산: section(박자) 거리 기반이고, 매 프레임 무상태로 재계산
```java
final double region = nscroll > 0 ? (240000 / nbpm / hispeed / speed) / nscroll : 0;   // :321  (ms, 레인 전체 통과시간)
final double rxhs = (hu - hl) * hispeed * speed;                                         // :326  (1 마디당 픽셀)
...
if (prevtl.getMicroTime() + prevtl.getMicroStop() > microtime) {                         // STOP 중이거나 아직 다음 구간
    y += (tl.getSection() - prevtl.getSection()) * prevtl.getScroll() * rxhs;
} else {                                                                                  // 현재 진행 중인 첫 구간: 시간 비율로 보간
    y += (tl.getSection() - prevtl.getSection()) * prevtl.getScroll() * (tl.getMicroTime() - microtime)
            / (tl.getMicroTime() - prevtl.getMicroTime() - prevtl.getMicroStop()) * rxhs;
}                                                                                         // :503-513
```
- 판정선(`hl`)에서 위로, **타임라인 사이의 section 차이 × 직전 타임라인 scroll × rxhs**를 누적합니다. 현재 시각이 들어 있는 구간만 "남은 시간 / 구간 길이" 비율로 선형 보간합니다 (확인).
- 결과적으로 **화면 간격은 박자(section)에 비례하고, 낙하 속도는 BPM에 비례**합니다. BPM이 2배가 되면 2배 빨리 내려옵니다. STOP 구간에서는 `prevtl.time + stop > now`라서 해당 구간 전체가 고정 거리가 되므로 노트가 멈춥니다 (확인).
- #SCROLL은 구간 길이에 곱하는 배율입니다 (`prevtl.getScroll()`). 0이면 그 구간 길이가 0입니다 (확인).
- #SPEED(beatoraja 확장)는 speed 오브젝트 사이를 **시간 기준 선형 보간**한 값(`getCurrentSpeed`, 259-286)을 rxhs 전체에 곱합니다 (확인).
- **무상태**: 매 프레임 `microtime`에서 바로 y를 계산합니다. `pos`는 이미 지난 타임라인을 건너뛰기 위한 탐색 시작 인덱스일 뿐이고, 위치를 증분으로 누적하지 않습니다 (확인 448-461).
- **시간 해상도**: `drawLane(time)`의 `time`은 `SkinNote.prepare`에서 받은 `timer.getNowTime()`(**ms**)입니다 (`SkinNote.java:44-61`, `Skin.java:317-319`). 여기에 `judgetiming`(ms)을 더하고 `microtime = time*1000`으로 씁니다. 그래서 위치는 1 ms 단위로 양자화됩니다 (확인).
  - `Config.prepareFramePerSecond`(기본 0 = 매 프레임 prepare)가 0보다 크면 prepare 주기가 그보다 낮아집니다. 그동안 **노트 위치가 갱신되지 않습니다** (`Skin.java:265, 316-322`, 확인).
- **프레임 보간이나 표시 시각 예측은 없습니다**. 프레임 시작 시각(`timer.update()` 직후)을 그대로 쓰고, vsync 후 실제 표시 시각을 예측하는 코드는 없습니다 (확인: 해당 코드 없음 / 추정: 영향).

### 4.2 HI-SPEED, 녹색숫자(duration), 고정 HS
- hispeed 1.0이면 **1 마디(section 1.0)가 레인 높이(hu−hl)와 같습니다**. 노출 시간 `region = 240000 / BPM / hispeed / speed / scroll`(ms)입니다 (321행).
- 현재 녹색숫자: `currentduration = round(region × (1 − lanecover))` (330행, 확인).
- `PlayConfig.fixhispeed`(기본 **FIX_HISPEED_MAINBPM**, `PlayConfig.java:43`): 기준 BPM을 OFF, STARTBPM, MAXBPM, MAINBPM(노트 수가 가장 많은 BPM), MINBPM 중에서 고릅니다 (145-152). 그다음
  ```java
  playconfig.setHispeed((float) ((2400f / (targetbpm / 100) / playconfig.getDuration()) * (1 - lanecover)));   // :208
  ```
  로 **duration(목표 녹색숫자, 기본 500 ms)을 기준 BPM에서 맞추도록 HS를 역산**합니다. 곡 도중 BPM이 바뀌어도 HS를 다시 계산하지 않으므로, 기준 BPM이 아닌 구간에서는 녹색숫자가 달라집니다 (확인).
- `hispeedautoadjust`(기본 false): **레인커버나 리프트를 조작할 때만** 현재 BPM으로 HS를 다시 계산합니다 (`ControlInputProcessor.java:241-243`). BPM이 바뀔 때 자동으로 다시 계산하는 플로팅 HS는 아닙니다 (확인).
- **Constant 모드** (`enableConstant`, 기본 false): `speed = 1.0`으로 #SPEED를 무시합니다. 그리고 `now + duration`(ms)보다 먼 타임라인을 그리지 않고, `constantFadeinTime`(기본 100 ms) 동안 알파로 페이드인합니다 (360-388, 472-500). **노트 간격(BPM 비례)은 바꾸지 않고 출현 시점만 시간 기준으로 바꾸는 방식**입니다 (확인).

### 4.3 LIFT / SUDDEN(레인커버) / HIDDEN
- LIFT: `hl = y + height × lift`입니다 (325행). `rxhs = (hu − hl) × hs`라서 리프트를 올려도 **노출 시간(duration)은 그대로**이고 픽셀 속도만 줄어듭니다 (확인).
- 레인커버(SUDDEN+): `OFFSET_LANECOVER.y`로 스킨 커버를 옮깁니다 (333행). 노트 계산에는 영향이 없고, 녹색숫자 계산(330행)과 HS 역산(208행)에만 들어갑니다.
- HIDDEN+: `OFFSET_HIDDEN_COVER`의 y와 alpha로 판정선 쪽 커버를 표시합니다 (335-346). 렌더 로직 변화 없이 스킨 오브젝트로만 가립니다 (확인).
- 스킨 오프셋 기본값은 lanecover 0.2(활성), lift 0.1(비활성), hidden 0.1(비활성)입니다 (`PlayConfig.java:62-82`).

### 4.4 기타
- 지나간 노트는 `tl.time >= microtime`일 때만 그립니다. `showpastnote`를 켜면 미처리 노트를 계속 표시합니다 (546행).
- PMS 모드에는 BAD 늦음 이후 노트가 떨어지는 연출이 따로 있습니다 (604-682).
- 판정 스레드가 바꾸는 `Note.state`를 렌더 스레드가 동기화 없이 읽습니다 (추정: 표시만 1 프레임 늦어지는 무해한 race).

---

## 5. 게이지와 점수 (요약)
- 게이지는 `BMSPlayer.update(judge)`에서 `gauge.update(judge)`로 갱신하고, **모든 게이지 종류를 병렬로** 갱신합니다 (`BMSPlayer.java:1019-1024`, `GrooveGauge.java:57-61`). GAS(게이지 자동 전환)는 이 병렬 값으로 판단합니다 (`BMSPlayer.java:639-650`) (확인).
- 판정 이벤트 외의 증감 (확인):
  - HCN 200 ms 틱 `gauge.update(1|3, 0.5f)` (`JudgeManager.java:316, 328`)
  - 지뢰 `gauge.addValue(-damage)` (246행)
- 증감량은 `GaugeProperty`에 PG, GR, GD, BD, PR, MS 순서로 있습니다. 7K NORMAL은 `{1.0, 1.0, 0.5, −3.0, −6.0, −2.0}`이고, 회복량에 `TOTAL / totalNotes`를 곱합니다 (`GaugeProperty.java:89`, `GrooveGauge.java:260`). HARD는 `LIMIT_INCREMENT`와 gauge 30 % 미만 감소 완화(guts) 표를 씁니다 (90행). TOTAL이 정의되지 않으면 `max(260, 7.605·N/(0.01·N+6.5))`입니다 (`BMSPlayerRule.java:85`) (확인).
- 점수: EX SCORE = `PG×2 + GR` (`ScoreData.java:405-407`). FAST/SLOW는 `dmtime >= 0`이면 FAST입니다 (`updateMicro` 652행) (확인).

---

## 6. 설정 가능한 정책 (판정, 믹싱, 렌더 관련)

| 위치 | 필드 | 기본값 | 영향 |
|---|---|---|---|
| `PlayConfig` (모드별) | `judgetype` | `"Combo"` (Combo/Duration/Lowest만 유효) | 판정 대상 노트 선택 알고리즘 (`PlayConfig.java:103, 262`) |
| `PlayerConfig` | `customJudge` | false | 아래 배율 활성화 (assist 취급) (`PlayerConfig.java:128-135`) |
| 〃 | `keyJudgeWindowRatePerfectGreat/Great/Good` | 400 / 400 / 100 (%) | 키 판정창 배율 (BAD 이하로 clamp) |
| 〃 | `scratchJudgeWindowRate*` | 400 / 400 / 100 | 스크래치 판정창 배율 |
| 〃 | `longnoteMarginRate` | 100 | LN 떼기 여유 배율 (7K는 원래 0) |
| 〃 | `lnmode` | 0 (LN) | 정의되지 않은 LN을 LN/CN/HCN 중 무엇으로 볼지 |
| 〃 | `judgetiming` | 0 ms (±500) | **노트 표시 오프셋** (판정에는 영향 없음) |
| 〃 | `notesDisplayTimingAutoAdjust` | false | 판정 Δ로 judgetiming 자동 보정 |
| 〃 | `gaugeAutoShift`, `bottomShiftableGauge` | NONE, ASSISTEASY | GAS |
| 〃 | `mineMode`, `showhiddennote`, `showpastnote`, `markprocessednote`, `showjudgearea`, `bpmguide` | 0/false… | 표시 보조 |
| `PlayConfig` | `hispeed`, `duration`, `fixhispeed`, `hispeedmargin`, `hispeedautoadjust` | 1.0, 500 ms, MAINBPM, 0.25, false | 스크롤 속도 정책 |
| 〃 | `enableConstant`, `constantFadeinTime` | false, 100 ms | Constant(시간 기준 출현) |
| 〃 | `lanecover/enablelanecover`, `lift/enablelift`, `hidden/enablehidden` | 0.2/true, 0.1/false, 0.1/false | SUDDEN, LIFT, HIDDEN |
| `PlayModeConfig.*Config` | `duration` (입력 디바운스) | 16 ms | 같은 키에서 16 ms 안의 변화 무시 |
| `Config` | `vsync`, `maxFramePerSecond`, `prepareFramePerSecond` | false, 240, 0 | 렌더 페이싱. **판정 스레드 주기와 BGM 트리거 해상도도 이 값을 따름** |
| `AudioConfig` | `driver` | OpenAL | OpenAL / PortAudio / AudioDevice |
| 〃 | `deviceBufferSize`, `deviceSimultaneousSources`, `sampleRate` | 384, 256, 0(장치 기본) | 지연과 보이스 수 |
| 〃 | `keyvolume`, `bgvolume`, `systemvolume` | 0.5 / 0.5 / 0.5 | 게인 |
| 〃 | `freqOption`, `fastForward` | FREQUENCY | 배속·연습 모드에서 피치를 바꿀지 타임스트레치할지 |

설정에 **없는** 것 (확인: 필드 검색 결과 없음):
- 입력 또는 오디오 판정 오프셋
- 중복 키음 방지 on/off
- 보이스 상한 초과 시 스틸링 정책
- 오디오 클럭 기준 동기화
- 렌더 프레임 보간

---

## 정책으로 본 요약

| 메커니즘 (정책 축) | beatoraja의 선택 | 대안 | 비교 지표 |
|---|---|---|---|
| 차트 시간 표현 | µs `long`, double 누적 후 TimeLine마다 절삭 (확인-bytecode) | 프레임 tick(IIDX 1/60 s), 샘플 인덱스, 유리수(박+BPM 구간) | 노트 시각 오차 최대·평균(µs), 긴 곡 끝의 누적 드리프트, 변환기 왕복 시 1:1 재현율 |
| 판정창 표현 | 모드별 µs 표 × 차트 % 배율, MS 고정, BAD·MS 비대칭 (확인) | 차트별 frame 단위 표(IIDX), 대칭 창, 랭크와 연속 함수 | 동일 입력 로그에 대한 판정 분포 차이, 창 경계 해상도 |
| 판정 시각 기준 | 누름·뗌 = **입력 타임스탬프**(1 ms 폴링, 디바운스 16 ms), 놓침·LN 자동 완료 = 프레임 시각 (확인) | 전부 프레임 시각, 전부 이벤트 시각, OS 이벤트 원본 타임스탬프(QPC) | 합성 입력(정확한 시각 주입)에 대한 Δ 오차 분포, FPS를 바꿨을 때 판정 변화 |
| 판정 실행 주기 | 별도 스레드지만 렌더 프레임 단위 `mtime`에 종속 (확인) | 독립 고정 주기(1 kHz) 시뮬레이션, 이벤트 드리븐 | 키 입력부터 판정·키음 트리거까지의 지연 p50/p99, FPS 의존성 |
| 다중 노트 선택 | Combo(GOOD 불가일 때만 다음 노트), 대안 Duration/Lowest (확인) | 최근접, 최하단, 점수 최대화, 미래 입력을 보는 최적 배정 | 같은 리플레이에서 EX score·콤보 차이, "억울한 판정" 비율(사람이 의도한 노트와의 일치율) |
| 빈 POOR | MS창(−150/+500 ms) 안이면 노트 유지, 콤보 유지, 게이지 감소 (확인) | 빈 POOR 없음, 콤보 끊기(PMS), 노트 1개당 1회 제한 | 연타·트릴 차트에서 게이지 변동, 플레이어 체감 조사 |
| LN 판정 | LN = 시작 1판정(떼기로 악화만 가능), CN/HCN = 시작+종단 2판정, 종단을 지나쳐 누르면 POOR (확인) | IIDX식 CN 종단 자동 PG, 홀드 틱 판정 | 판정 수 일관성, LN 차트 점수 분포 |
| 키 입력 없는 공타 키음 | 누른 시각 이전 최근 노트(숨은 노트 포함)의 키음 (확인) | 다음 노트 키음, 레인 고정 키음, 무음 | 차트 작가 의도 재현율(숨은 노트 활용 차트), 청감 테스트 |
| 재트리거 | **같은 WAV ID = 1 보이스, 다시 울리면 stop 후 처음부터** (옵션 없음) (확인) | 겹침 허용, N-보이스 상한, 페이드아웃 choke | 연타 구간 꼬리 잘림량, 클리핑 발생률, 원곡 렌더(오프라인)와의 스펙트럼 차이 |
| 보이스 상한·스틸링 | 256, 초과 시 새 소리 드롭(PortAudio). OpenAL은 libGDX 기본 동작 (확인/추정) | 가장 오래된 것 또는 가장 작은 것부터 스틸 | 밀도 높은 차트에서 누락된 트리거 수 |
| 믹싱 | float 단순 합, 리미터 없음, 최근접 리샘플(피치 변경 시) (확인) | 리미터·소프트클립, 고품질 리샘플러 | 피크·클리핑 샘플 수, THD, CPU 사용량 |
| BGM 트리거 | 프레임 단위 시각을 보고 fire-and-forget (확인) | 오디오 콜백 안에서 샘플 단위 예약 재생, 오디오 클럭을 마스터로 | 키음과 BGM 사이 지터(ms), 장시간 드리프트 |
| 렌더 위치 | section 거리 × scroll × HS, 매 프레임 무상태 재계산, ms 양자화, 예측과 보간 없음 (확인) | 시간 기반 위치(BPM 무관 등속), 표시 시각 예측(present time), µs 렌더 시각 | 노트 위치 지터(px), 판정선 통과 시각과 실제 노트 시각의 차이 |
| BPM 변화 표시 | 속도가 BPM에 비례, HS는 기준 BPM(기본 MAINBPM)으로 한 번만 역산 (확인) | 플로팅 HS(실시간 녹색숫자 고정), Constant(시간 기준 출현만) | 구간별 녹색숫자 분산, 시창(視窓) 일관성 |
| 표시 오프셋 | `judgetiming`이 표시만 옮김 (+ 자동 보정 옵션) (확인) | 입력 오프셋, 오디오 오프셋, 셋 다 분리 | 오프셋 보정 후 Δ 평균·분산, 보정 수렴 속도 |
| 게이지 | 판정 이벤트로 모든 게이지 병렬 갱신 + HCN 틱 + 지뢰 (확인) | 판정 이벤트만 사용, 시간 기반 | 게이지 궤적 재현성(리플레이 대비) |
