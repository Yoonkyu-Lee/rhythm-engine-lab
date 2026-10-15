# Lunatic Vibes F 분석 (Part B): 차트 타이밍 / 판정 / 키음 믹싱 / 노트 렌더링

- 대상: `bms/third_party/lunaticvibesf` (HEAD `3e8a655f`, 2025-03-14), 소스 루트 `src/`
- 표기: **(확인)** = 코드에서 직접 읽음, **(추정)** = 코드 경로로부터의 추론 또는 외부 지식
- 경로는 모두 `src/` 기준

---

## 0. 한눈에 보는 시간 축 (판정/렌더/오디오가 어떤 시계를 쓰는지)

| 소비자 | 사용하는 시각 | 해상도 | 근거 |
|---|---|---|---|
| 키 누름 판정 | 입력 스레드 폴링 시각 `pTimes[i] = now.norm()` → `tt[k] - _startTime` | **1 ms 정수** | `game/input/input_wrapper.cpp:52-55`, `game/ruleset/ruleset_bms.cpp:1499` (확인) |
| 홀드/릴리즈 판정 | 입력 이벤트 시각 `event.t` (폴링 시각, ns) − `_startTime.norm()` | ns 보관하지만 판정 시 ms 절삭 | `ruleset_bms.cpp:1528,1556`, `:950` (확인) |
| 아날로그 스크래치 판정 | **고정 틱 시각** (`ruleset->update(t)` 내부) | 1 ms 틱 | `ruleset_bms.cpp:1769-1830` (확인) |
| MISS(놓침) 검출 | 고정 틱 시각 (1000 Hz) | 1 ms | `ruleset_bms.cpp:1607-1680`, `scene_play.cpp:211` rate=1000 (확인) |
| 노트 위치(렌더) | 틱에서 계산된 `vt = rt + TIMING_ADJUST_VISUAL`의 metre | ns→double | `game/chart/chart.cpp:338,426-429` (확인) |
| BGM 키음 트리거 | 틱 시각 `at = rt` (오프셋 미적용) | 1 ms 틱 | `chart.cpp:339,405-413` (확인) |
| 키음 재생 호출 | 메인 스레드 `processInput()` (프레임 단위) | 프레임 | `game/game.cpp:439`, `scene_play.cpp:2915` (확인) |

시간 타입 `lunaticvibes::Time`은 `_regular`(ms, long long)와 `_highres`(ns, long long)를 동시에 보관한다. `Time::now()`는 `std::chrono::system_clock` 기반이다 (확인, `common/beat.h:55-59`). 단조 시계(steady_clock)가 아니라는 점을 기억해 둘 필요가 있다 (추정: NTP 보정 시 점프 가능성이 있음).

---

## 1. 차트 타이밍 모델

### 1.1 표현 타입 (확인)
`common/beat.h`
```cpp
13 class Metre final : public fraction            // 시각적 위치 (마디 단위, 4/4 = 1)
21     Metre(double value) : fraction(static_cast<long long>(value * 1e12), 1'000'000'000'000, true) {}
35 using Segment = fraction;                       // 마디 내부 위치 [0,1)
50     decltype(std::declval<timeNormRes>().count()) _regular;   // ms
51     decltype(std::declval<timeHighRes>().count()) _highres;   // ns
75 [[nodiscard]] static constexpr Time singleBeatLengthFromBPM(BPM bpm)
78     return {static_cast<long long>(6e4 / bpm * nsInMs), true};   // 1박 길이를 ns 정수로 절삭
```
- `Note`는 `Metre pos`(시각 위치, 유리수), `Time time`(ns), `measure`, `dvalue`(WAV id 등), `fvalue`(BPM 등)를 가진다 (확인, `beat.h:114-139`).
- `fraction`은 `long long` 분자/분모를 쓰고 gcd로 약분한다 (확인, `common/fraction.cpp:6-27`). 마디 길이(`#xxx02`)는 `Metre(double)` 경로를 타서 분모가 1e12가 된다 (확인, `chartformat_bms.cpp:629`). 분모가 큰 상태에서 lcm/곱셈을 거듭하면 오버플로 위험이 있다. 다만 일반적인 차트에서는 약분으로 해소된다 (추정).

### 1.2 변환 알고리즘 (`ChartObjectBMS::loadBMS`, `game/chart/chart_bms.cpp`) (확인)
- 마디마다 모든 채널의 노트를 `(Segment, {lane-priority, value})`로 모은 뒤 Segment 기준 **stable_sort**한다. 같은 위치에서는 **Notes > BGM/BGA > BPM > STOP** 순서로 처리된다 (`chart_bms.cpp:233-255, 375`).
- 노트 시각 계산 (BPM 변경 지점 기준의 구간 선형식):
```cpp
388 double metreFromBPMChange = (noteSegment - lastBPMChangedSegment) * barMetre;
389 auto notemetre = static_cast<Metre>(basemetre + noteSegment * barMetre);
390 lunaticvibes::Time notetime = bpmfucked ? LLONG_MAX : basetime + beatLength * (metreFromBPMChange * 4);
```
- BPM 변경 시: `basetime = notetime; lastBPMChangedSegment = noteSegment; beatLength = singleBeatLengthFromBPM(bpm)` (`:727-738`). BPM ≤ 0이면 `bpmfucked = true`가 되고 이후 노트는 전부 `LLONG_MAX`라서 도달할 수 없다 (`:736-737`).
- STOP: `#STOPxx` 값/192를 마디(=4박) 단위로 해석해 `noteStopTime = floor(beatLength.hres() * stopMetre * 4)` ns를 `basetime`에 더한다. STOP은 `lastBPMChangedSegment`를 바꾸지 않는다 (`:753-763`). 마디 길이와 무관하게 1/192 온음표 단위이므로 BMS 표준과 일치한다.
- 마디 끝: `basetime += beatLength * (1.0 - lastBPMChangedSegment) * barMetre * 4; basemetre += barMetre;` (`:775-776`). 각 마디 시작 시각은 `_barTimestamp`, 시작 metre는 `_barMetrePos`에 기록된다 (`:229-231`).
- `#BPM xx` 중복 검사 `if (bpm == static_cast<BPM>(val))`는 pitch 배율이 곱해진 `bpm`과 원시 값을 비교한다. 그래서 pitchSpeed≠1이면 중복 BPM 노트도 스킵되지 않는다. 동작에는 무해하다 (확인, `:728`).
- pitch(FREQ) 모드는 BPM 자체에 `gSelectContext.pitchSpeed`를 곱하는 방식으로 반영된다 (확인, `:73,733`).

### 1.3 정밀도와 누적 오차
- 1박 길이는 ns 정수로 **절삭**되므로 박당 오차는 1 ns 미만이다. 노트 시각은 `beatLength × double`이라 BPM 구간 내부에서는 누적되지 않는다. 다만 `basetime`은 마디마다 `beatLength * double`(절삭)을 더하므로 마디당 수 ns 이하의 오차가 쌓인다. 1000마디를 진행해도 µs 수준에 그친다 (확인: 산식 / 추정: 크기).
- 실질적인 양자화는 **판정 시점의 ms 절삭**에서 생긴다 (2.4절 참조). 차트 쪽 정밀도(ns)는 판정 해상도(1 ms)보다 훨씬 높다.
- 재생 중 현재 metre는 매 틱 **무상태(stateless) 재계산**된다. 마디 시작 시각과 직전 BPM 변경 시각에서 경과 시간을 나눠 구하므로 프레임 간 누적 오차가 없다 (확인, `chart.cpp:426-429`).
```cpp
426 lunaticvibes::Time currentMeasureTimePassed = vt - _barTimestamp[_currentBarTemp];
427 lunaticvibes::Time timeFromBPMChange = currentMeasureTimePassed - _lastChangedBPMTime;
428 _currentMetreTemp =
429     _lastChangedBPMMetre + static_cast<double>(timeFromBPMChange.hres()) / _currentBeatLength.hres() / 4;
```
STOP 구간에서는 `postUpdate`가 metre를 STOP 위치에 고정하고, STOP이 끝난 뒤에는 누적된 `_stopMetre`를 빼서 보정한다 (확인, `chart_bms.cpp:870-918`).

### 1.4 LN 관련
- LN은 `#xxx5x/6x` 채널 쌍(LNTYPE 1)과 `#LNOBJ`만 지원한다. `#LNMODE`(CN/HCN)와 `#LNTYPE 2`는 경고만 내고 무시한다 (확인, `common/chartformat/chartformat_bms.cpp:514-545`). LR2와 같은 범위다.

---

## 2. 판정 (RulesetBMS)

### 2.1 판정 창 테이블 (확인, `game/ruleset/ruleset_bms.h:263-285`)
```cpp
274 // https://github.com/wcko87/lr2oraja/blob/readme/README.md
275 // https://iidx.org/misc/iidx_lr2_beatoraja_diff#timing-window
276 // This has been verified with LR2 disassembly.
277 inline static const JudgeTime judgeTime[] = {
278     {8, 24, 40, 200, 1000},   // VERY HARD
279     {15, 30, 60, 200, 1000},  // HARD
280     {18, 40, 100, 200, 1000}, // NORMAL
281     {21, 60, 120, 200, 1000}, // EASY
282     // TODO: VERY EASY? beatoraja uses EASY*1.25 windows, lr2oraja uses NORMAL.
```

| #RANK | PGREAT | GREAT | GOOD | BAD | 공POOR(빈 POOR) 조기 한계 | MISS(늦음) |
|---|---|---|---|---|---|---|
| 0 VERY HARD | ±8 | ±24 | ±40 | ±200 | −1000 | ≥ +200 |
| 1 HARD | ±15 | ±30 | ±60 | ±200 | −1000 | ≥ +200 |
| 2 NORMAL | ±18 | ±40 | ±100 | ±200 | −1000 | ≥ +200 |
| 3 EASY | ±21 | ±60 | ±120 | ±200 | −1000 | ≥ +200 |
| 4 (VERY EASY) | NORMAL로 대체 | | | | | |
| 없음/무효 | NORMAL (`LR2_DEFAULT_RANK`) | | | | | |

- `#RANK 4`는 "LR2는 무효값을 NORMAL로 대체, lr2oraja도 NORMAL"이라는 주석과 함께 NORMAL로 처리된다 (확인, `chartformat_bms.cpp:142-145`). 값이 없으면 `RulesetBMS::LR2_DEFAULT_RANK`(NORMAL)이다 (확인, `scene_play.cpp:797-803`). `#DEFEXRANK`는 무시된다 (확인, `chartformat_bms.cpp:452-458`).
- LR2와의 일치 여부: 코드 주석은 "LR2 디스어셈블리로 검증"이라고 적고 있다. 널리 알려진 LR2 값(VH 8/24/40, H 15/30/60, N 18/40/100, E 21/60/120, BAD 200)과도 같다 (확인: 주석 / 추정: 외부 값 대조).
- **beatoraja와의 차이** (추정, 외부 지식): beatoraja 기본 판정은 NORMAL 기준 PG ±20, GR ±60, GD ±150, BD -280/+220 등이다. LVF는 LR2 값을 고정 테이블로 쓰며, 판정 폭을 사용자가 고르는 설정은 없다.
- **IIDX와의 차이**: IIDX는 차트별로 1/60 s 단위 창을 저장하고 고정 틱으로 판정한다. LVF는 **전 차트 공통 4단계 ms 정수 테이블**을 쓰고, 입력 이벤트 시각(1 ms)으로 판정한다.

### 2.2 경계 처리: 정확한 오차 계산과 비대칭 (확인, `ruleset_bms.cpp:946-997`)
```cpp
950 int error = time.norm() - note.time.norm();          // ms 정수 (각각 ns→ms 절삭)
951 if (error > -judgeTime[...].KPOOR)
953     if (error < -judgeTime[...].BAD)        a = EARLY_KPOOR;
955     else if (error < -judgeTime[...].GOOD)  a = EARLY_BAD;
 ...
961     else if (error < 0)  a = EARLY_PERFECT;   else if (error == 0) a = EXACT_PERFECT;
965     else if (error < judgeTime[...].PERFECT) a = LATE_PERFECT;
 ...
971     else if (error < judgeTime[...].BAD)     a = LATE_BAD;
```
- 판정 결과는 **조기 쪽 경계 포함, 지각 쪽 경계 미포함**의 비대칭이 된다. NORMAL 예시: PG = [−18, +17], GR = [−40, −19] ∪ [+18, +39], GD = [−100, −41] ∪ [+40, +99], BD = [−200, −101] ∪ [+100, +199], 공POOR = [−999, −201] (확인: 산식).
- `note.time.norm()`과 `time.norm()`을 각각 절삭한 뒤 빼므로 실제 오차 대비 최대 ±1 ms의 양자화 오차가 생긴다 (확인: 산식).
- **판정 오프셋(사용자 조정)은 판정에 적용되지 않는다.** `JudgeOffset` 설정은 `TIMING_ADJUST_VISUAL`로 들어가 **시각(노트 표시) 시간에만 더해진다** (확인).
```cpp
// config/config_mgr.cpp:46
State::set(IndexNumber::TIMING_ADJUST_VISUAL, ConfigMgr::Profile()->get(P_JUDGE_OFFSET, 0));
// game/chart/chart.cpp:338-339
lunaticvibes::Time vt = rt + lunaticvibes::Time(State::get(IndexNumber::TIMING_ADJUST_VISUAL), false);
const lunaticvibes::Time& at = rt;
```
  판정 함수 `_calcJudgeByTimes`에는 어떤 오프셋 항도 없다. 따라서 LVF의 오프셋 조정은 "노트를 더 일찍/늦게 보이게 해서 사용자의 입력 타이밍을 옮기는" 방식이다. 오디오(BGM, `at`)와 판정은 원시 시간 그대로 쓴다.
- 자동 보정(`uAutoAdjust`, 기본 0): BAD 이상 판정이 10회 쌓일 때마다 마지막 오차의 부호에 따라 `TIMING_ADJUST_VISUAL`을 ±1 ms 옮긴다. 클램프는 없다 (확인, `ruleset_bms.cpp:1389-1410`). LVF가 추가한 기능이다.

### 2.3 누름 → 노트 매칭 (확인, `ruleset_bms.cpp:1412-1448`)
```cpp
1418 auto itNote = chart.incomingNote(cat, idx);
1419 while (!chart.isLastNote(cat, idx, itNote) && itNote->expired) ++itNote;   // 첫 미만료 노트
 ...
1434 if (note2 && (note == nullptr || note2->time < note->time))  // 일반 노트 vs LN 헤드 중 더 이른 것
```
- 매칭 기준은 **"가장 가까운 노트"가 아니라 "레인의 첫 미만료 노트(가장 이른 노트)"**다. 일반 노트 리스트와 LN 리스트 중 시각이 더 이른 쪽을 고른다.
- 해당 노트가 [−1000, −201] ms 안에 있으면 공POOR(EARLY_KPOOR)이 되고, 노트는 소모되지 않는다 (`:1041`). −1000 ms보다 이르면 NOTHING이 되어 아무 일도 일어나지 않는다. **노트가 지나간 뒤 누른 경우의 LATE_KPOOR는 enum에만 있고 생성 경로가 없다** (확인: grep 결과 case 문에서만 등장). 즉 빈 POOR는 "다음 노트 1초 이내의 조기 누름"으로만 발생한다.
- 결과적으로 같은 레인에 노트가 촘촘하면, 앞 노트를 BAD 창 끝(+199 ms)까지 계속 붙잡는다. 다음 노트가 더 가까워도 앞 노트로 판정된다 (확인: 산식). IIDX/beatoraja의 "가장 가까운 노트" 방식과 비교할 지점이다.

### 2.4 어떤 타임스탬프를 쓰는가 (확인)
- 누름: `judgeNotePress(k, t, tt[k] - _startTime, slot)` (`ruleset_bms.cpp:1499`). `tt[k]`는 입력 스레드가 키 상태 변화를 감지한 순간의 `now.norm()`이며 **ms 정수**다 (`input_wrapper.cpp:52-55`). 주석 `// TODO: take time from the input manager lol.`이 있다. 업데이트 루프 시각이 아니라 **입력 이벤트 자체의 시각**을 쓴다.
- 이벤트는 큐에 쌓였다가 다음 프레임의 `processInput()`에서 처리된다. 프레임 안의 순서는 processInput(판정) → 고정 틱 catch-up(MISS 검출) → 렌더다 (확인, `game.cpp:438-462`). 판정이 MISS 처리보다 먼저 일어나므로, 프레임 지연 때문에 억울한 MISS가 생기지 않는다.
- 홀드/릴리즈: `rt = t - _startTime.norm()`이고 `t`는 이벤트의 `now`다 (`:1528,1556`).
- 릴리즈 시각은 실제보다 늦게 찍힌다: 입력 관리자의 디바운스(`MinInputInterval`, 기본 16 ms) 동안 키를 눌림으로 유지하고 (`game/input/input_mgr.cpp:135-138`), 래퍼가 다시 `release_delay_ms = 5`를 기다린 **뒤의 `now`**를 릴리즈 시각으로 쓴다 (`input_wrapper.cpp:57-69`, `input_wrapper.h:21`). 따라서 LN 릴리즈 판정은 물리적 릴리즈보다 약 16~21 ms 늦은 시각으로 계산된다 (확인: 코드 / 추정: 정확한 지연은 폴링 주기에 따라 다름).
- **기준점 불일치 (추정)**: 차트 시간은 `rt = t - PLAY_START`(`scene_play.cpp:1886`)이다. 반면 룰셋의 `_startTime`은 첫 `ruleset->update(t)` 호출 시각으로 정해진다 (`ruleset_bms.cpp:1609-1610`). PLAY_START는 LOAD_END 틱에서 설정되고 (`scene_play.cpp:1862`) 룰셋의 첫 update는 그다음 PLAYING 틱이므로, 룰셋의 0점이 차트보다 1틱(1 ms) 늦을 가능성이 높다. 그러면 판정 오차가 체계적으로 −1 ms 편향된다. 리플레이 재판정은 `PLAY_START` 기준 ms로 기록된 값을 쓰므로 (`scene_play.cpp:2923`) 라이브 판정과 1 ms 어긋날 수 있다.

### 2.5 MISS 검출 (확인, `ruleset_bms.cpp:1655-1680`)
```cpp
1663 const auto latePoorWindow = (!scratch || _judgeScratch) ? judgeTime[...].BAD : 0;
1665 if (rt.norm() - itNote->time.norm() >= latePoorWindow) { itNote->expired = true; updateJudge(t, idx, JudgeArea::MISS, slot); ...
```
- 1000 Hz 고정 틱(`ScenePlay` rate=1000, `FixedUpdater` dt=1 ms, `game.cpp:327-345`)마다 각 레인을 스캔해, 노트 시각 + 200 ms에 도달하면 MISS로 처리한다. 틱 시각은 `t - _acc`로 과거 시점에 맞춰 계산되므로 프레임 지터와 무관하게 1 ms 해상도를 유지한다 (확인).
- 오토스크래치 레인은 창 0으로 즉시 만료되고, 판정 없이 최대 콤보에서도 빠진다 (`:1937-1953`).

### 2.6 LN (LR2식 "LN": 하나의 노트, 끝에서 확정) (확인)
- 헤드 PG~GD: `_lnJudge[idx] = area`에 저장만 하고 점수와 콤보에는 아직 반영하지 않는다. LN 봄 타이머만 시작한다 (`:1057-1072`).
- 헤드 BAD: 즉시 BAD를 확정하고 노트를 만료시킨다 (`:1074-1081`).
- 홀드 유지: `_judgeHold`에서 테일 오차가 `EARLY_PERFECT && time < -2`, `EXACT`, 또는 `LATE_PERFECT && time < 2`이면 헤드 판정으로 확정한다 (`:1141-1162`). 홀드 이벤트는 매 입력 폴링마다 생성되므로, 실제로는 **테일 −PG창(NORMAL에서 −18 ms) 시점에 자동 완료**된다. `time < -2` 조건은 의도가 불분명하며 `> -2`의 오타일 가능성이 있다 (추정).
- 릴리즈 (`_judgeRelease`, `:1168-1231`): 테일 기준 오차가 BAD보다 이르면(NOTHING/EARLY_KPOOR/EARLY_BAD) LN 전체가 BAD가 된다. 그 밖에는 헤드 판정과 릴리즈 판정 중 **더 나쁜 쪽**(EARLY_PG/GR/GD로 강등)으로 정한다.
- 헤드를 놓치면 헤드 시각 + 200 ms(테일이 더 이르면 테일 시각)에 MISS가 되고, 테일 시각에 만료 카운트가 올라간다 (`:1689-1731`).
- **이중 카운트 가능성 (추정)**: 헤드 BAD일 때 `notesExpired++`가 한 번 일어나고(`:1077`), 테일 시각에 `!itHead->hit` 분기에서 또 `notesExpired++`가 일어난다(`:1724-1728`). 그러면 `isFinished()`와 진행 중 정확도(`acc`) 계산이 왜곡될 수 있다.
- CN/HCN(테일 판정, 홀드 중 게이지 감소)은 미지원이다 (확인, 1.4절).

### 2.7 스크래치 (확인)
- 키보드/버튼 스크래치는 `S1L`/`S1R` 두 입력이 모두 `Sc1` 레인에 매핑된다 (`chart.cpp:137-141`). 마지막 방향을 `playerScratchDirection`에 기록하고, 릴리즈는 그 방향의 키에만 반응한다 (`ruleset_bms.cpp:1506-1524, 1574-1590`).
- 아날로그 TT: 입력 이벤트에서는 누적기에 더하기만 하고 (`updateAxis`, `:1592-1605`), 실제 판정은 **틱의 `update()` 안에서** 임계값 0.001을 넘을 때 수행한다. 방향이 바뀌면 이전 방향 릴리즈와 새 방향 누름이 일어나고, 133 ms 동안 움직임이 없으면 릴리즈로 본다 (`:1769-1828`).
  - 판정 시각이 이벤트 시각이 아니라 **그 프레임 첫 틱의 시각**이다. 틱은 과거 시점에 맞춰 실행되므로, 실제 회전보다 최대 1프레임 이른 시각으로 판정될 수 있다 (추정: 코드 경로에서 도출).
- 스크래치 LN 도중 방향이 바뀌면 릴리즈로 처리된다 (`:1002-1010`).

### 2.8 지뢰 (확인)
- 홀드 이벤트에서만 검사한다. 조건은 LN 테일과 같은 기묘한 식이다 (`:1110-1139`). 피해량은 `-0.01 * value / 2`이다 (`:438-443`).

---

## 3. 키음 믹싱

### 3.1 누를 때 어떤 소리를 내는가 (확인, `scene_play.cpp:2649-2743, 3174-3205`)
- 키별 `keySampleIndex[k]`를 **매 틱 갱신**한다. 대상은 레인의 "incoming" 노트(일반 노트, LN 헤드, 투명 노트 중 가장 이른 것)이고, 그 노트가 **현재 시각 + 1000 ms 이내**일 때만 교체한다 (`MIN_REMAP_INTERVAL{1000}`).
```cpp
2683 const HitableNote* soonest_note = r::min_element(notes, {}, &NotePair::second)->first;
2685 if (soonest_note && (rt == USE_FIRST_KEYSOUNDS || soonest_note->time - rt <= MIN_REMAP_INTERVAL))
2687     keySampleIndex[static_cast<size_t>(k)] = static_cast<size_t>(soonest_note->dvalue);
```
  - 따라서 다음 노트가 1초 넘게 남아 있으면 **직전 노트의 소리(마지막 레인 사운드)**가 유지된다. 플레이 시작 시에는 `USE_FIRST_KEYSOUNDS`로 각 레인의 첫 노트 소리로 초기화한다 (`:1860`).
  - 차트의 incoming 이터레이터는 `vt >= time && expired`일 때만 전진한다 (`chart.cpp:377`). 그래서 노트를 일찍 친 뒤 그 노트 시각 전에 다시 누르면 같은 소리가 난다.
- 누름 시 판정 결과와 무관하게(NOTHING이어도) `keySampleIndex[i]`를 재생한다. 단, 배틀에서 FAILED된 쪽은 재생하지 않는다 (`:3179-3204`). 릴리즈 시에는 소리를 내지 않는다.
- 키음 재생 호출은 **메인 스레드의 `processInput()`(프레임 주기)**에서 일어난다. 입력 스레드에서 직접 호출하지 않는다 (확인). 따라서 키음 지연 = 폴링 지연 + 최대 1프레임 + FMOD DSP 버퍼(기본 128×2 샘플) (추정: 합산).

### 3.2 같은 소리 재트리거 (확인 + 추정)
```cpp
// game/sound/sound_fmod.cpp:678
int flags = FMOD_LOOP_OFF | FMOD_UNIQUE | FMOD_CREATESAMPLE | FMOD_IGNORETAGS | FMOD_LOWMEM;
// :718-724
r = fmodSystem->playSound(noteSamples[index[i]].objptr, &*channelGroup[ch], false, nullptr);
```
- 코드 차원에서 이전 채널을 직접 멈추는 로직은 없다. 다만 모든 노트 샘플이 **`FMOD_UNIQUE`** 플래그로 로드된다 (확인).
- FMOD 문서상 `FMOD_UNIQUE`는 "한 번에 하나만 재생 가능한 사운드"다. 같은 샘플을 다시 `playSound`하면 기존 인스턴스가 끊기고 처음부터 재생된다 (추정: FMOD 문서 기준, 런타임 검증 안 함).
  - 따라서 LVF의 정책은 **같은 WAV id는 항상 이전 재생을 끊고 재시작(cut)**이며, 서로 다른 WAV는 겹친다. Qwilight의 "중복 키음 방지" ON과 비슷한 효과가 강제로 적용된 셈이고, 끄는 옵션은 없다.
  - BGM 레인과 키 레인이 같은 WAV id를 공유하면 BGM 재생도 서로를 끊는다 (추정).
- 시스템 사운드도 `FMOD_DEFAULT | FMOD_UNIQUE`로 로드된다 (`:773`).

### 3.3 보이스 한도 / 채널 그룹 / 볼륨 (확인)
- `setSoftwareChannels(512)`, `init(512, ...)`이므로 **동시 보이스는 512개**다 (`sound_fmod.cpp:105,144`). 우선순위나 voice stealing 정책은 따로 설정하지 않는다 (FMOD 기본값).
- 채널 그룹은 `BGM_SYS, BGM_NOTE, KEY_SYS, KEY_LEFT, KEY_RIGHT`다. 볼륨 식은 다음과 같다.
  - `KEY_*` = noteVolume × MASTER × KEY
  - `BGM_*` = noteVolume × MASTER × BGM (`:915-948`)
- **주의 (확인)**: BGM 레인 노트(`procCommonNotes`)도 `SoundChannelType::KEY_LEFT`로 재생된다 (`scene_play.cpp:2591`). 따라서 **BGM 볼륨 슬라이더는 차트 BGM 키음에 영향을 주지 않고 KEY 볼륨이 적용된다**. `BGM_NOTE` 그룹은 실제 플레이 중 거의 사용되지 않는다. 의도된 동작인지는 불분명하다 (추정).
- 기본값: `MasterVolume/KeyVolume/BgmVolume = 1.0`, `BufferLength=128`, `BufferCount=2` (`config/cfg_profile.cpp:54-56`, `sound_fmod.cpp:132-133`).
- 스케줄링: `setDelay`나 DSP clock 기반 예약 재생이 없다. BGM은 고정 틱에서 `noteBgmExpired`를 모아 즉시 `playSound`한다. 틱은 프레임 시점에 몰아서 실행되므로 **BGM 발음 시각은 프레임 주기로 양자화**된다 (확인: 코드에 예약 API 없음 / 추정: 양자화 효과).

---

## 4. 노트 렌더링과 스크롤

### 4.1 위치 계산 (확인, `game/graphics/sprite_lane.cpp:119-181`)
```cpp
139 auto currTotalMetre = pChart->getBarMetrePosition(bar).toDouble() + metre;
148 auto noteMetreOffset = currTotalMetre - it->pos.toDouble();
150     y = (c.y + c.h); // expired notes stay on judge line, LR2 / pre RA behavior
152     y = (c.y + c.h) -
153         static_cast<int>(std::floor(-noteMetreOffset * _noteAreaHeight * _basespd * _hispeed));
```
- 기본(비 CONSTANT) 모드는 **metre(박자) 기반**이다. y = 판정선 − floor(Δmetre × 레인높이 × basespeed × hispeed)이고, 1 metre(4/4 한 마디)가 HS 1.0에서 레인 높이 하나에 해당한다. 주석에 "120BPM with 1.0x HS is 2000ms"라고 되어 있다.
  - BPM이 바뀌면 metre 진행 속도만 달라지므로 **스크롤 속도 자체가 변한다** (LR2 방식). STOP 중에는 metre가 고정되어 화면이 멈춘다.
  - 먼 노트의 화면 간격은 미래 BPM과 무관하게 metre 차이에 비례한다.
- CONSTANT(`SpeedType=Constant`) 모드는 **시간 기반**이다: `y = 판정선 − floor(-(now−note.time)/1600 × 높이 × bs × hs)` (`:160-179`). 150 BPM 고정으로 계산한다.
  - 이 경로의 현재 시각은 `(t − PLAY_START).norm()`이라서 **`TIMING_ADJUST_VISUAL`이 적용되지 않는다** (확인, `:162`). 비 CONSTANT 경로는 vt를 쓰므로 둘 사이에 일관성이 없다.
- 지나간(만료 전) 노트는 판정선에 머문다. 주석에 "LR2 / pre RA behavior"라고 명시되어 있다.
- 이터레이터는 차트의 incoming(만료되고 시각이 지난 노트만 제거됨)부터 시작하고, y < 0이 될 때까지 그린다.

### 4.2 프레임당 무상태 계산 여부와 보간 (확인)
- 현재 metre는 고정 틱(1000 Hz)에서 `chart->update(rt)`가 계산한다 (`scene_play.cpp:1895`). 렌더 프레임의 `SkinBase::update`는 그 값을 복사해서 쓰기만 한다 (`skin/skin.cpp:49-56`). 노트 y는 매 프레임 metre 차이로 **무상태 재계산**한다.
- **틱과 프레임 사이의 보간은 없다.** 다만 틱이 1 ms이고 렌더 직전에 catch-up되므로, 렌더 시점 대비 최대 1 ms(잔여 `_acc`) 오래된 값을 쓴다 (확인: `FixedUpdater`, `game.cpp:331-341`).
- y는 `static_cast<int>(floor(...))`로 **정수 픽셀 양자화**된다.

### 4.3 하이스피드 / 녹색 숫자 (확인, `scene_play.cpp:122-152, 1435-1503, 1603-1627`)
- `green = round(visible × 120 × 1200 / hs / bpm)`이고, `visible = (1000 − top − bottom)/1000`이다. 녹색 숫자 1200은 2000 ms이므로 **녹색 1 = 1/600 s**다.
- HS 변경은 200 ms 사인 이징으로 보간된다 (`hispeedGradientNow`).
- `LOCK_SPEED`(녹색 고정)가 켜져 있으면 레인커버를 움직일 때 HS를 재계산한다.
- 녹색 숫자 표시는 **현재 BPM 기준**이다. CONSTANT 모드에서는 BPM을 150으로 둔다.

### 4.4 SUDDEN/HIDDEN/LIFT (확인 + 추정)
- 레인커버 값은 0~1000 정수(‰)다. SUD+는 `LANECOVER_TOP`, HID+는 `LANECOVER_BOTTOM`, LIFT는 `LANECOVER_BOTTOM`을 리프트 높이로 쓴다 (`scene_play.cpp:69-125`).
- LIFT는 판정선, 노트 스프라이트, 판정/콤보 표시를 `lift = bottom/1000 × noteLaneHeight`만큼 위로 옮긴다. 주석에 "LR2 black box testing" 기반이라고 적혀 있다 (`skin/skin_lr2.cpp:4189-4517`).
- SUDDEN/HIDDEN은 노트를 잘라내지 않는다. 스킨의 커버 스프라이트가 위를 덮는 방식이다. HIDDEN은 호환용 회색 사각형도 그릴 수 있다 (`sprite_lane.cpp:215-262`). 노트는 커버 아래에도 계산되고 그려진다 (추정).

---

## 5. 게이지와 리플레이

### 5.1 게이지 (확인, `ruleset_bms.cpp:165-334, 430-518`)
- 갱신 시점은 **판정 확정 순간**이다 (`updateJudge → processHp`). LN은 테일에서 확정되고, 지뢰는 피격 즉시 반영된다.
- GROOVE: PG/GR `+0.01×TOTAL/노트수`, GD는 절반, BD −4%, 공POOR −2%, MISS −6%다. 시작 20%, 클리어 80%, 최소 2%.
- HARD: +0.1%, BD −6%, 공POOR −2%, MISS −10%에 TOTAL/노트수 기반 배율(`calculateHardNegativeHpDiffMultiplier`)을 곱한다. HP가 32% 미만이면 피해 ×0.6.
- 출처 주석: "LR2GAS_pub / LR2 disassembly", EXHARD와 ASSISTEASY는 "lr2oraja와 일치".
- `#TOTAL`이 0 이하거나 없으면 LR2 기본값(GROOVE 계열 160, HARD 계열 300)을 쓴다 (`:387-413`).
- GAS(`bEnableGas`)는 여러 게이지를 동시에 굴린다.
- HP 애니메이션 시간 공식과 점수 애니메이션 500 ms는 "Matches LR2" 주석이 붙어 있다 (`:501-505, 1254-1255`).
- 점수(money score)는 LR2의 콤보 보너스 공식을 **주석 처리**하고 단순 비례식(PG 1.0, GR 0.5, GD 0.25 × max/노트수)을 쓴다. 의도적 단순화다 (`:1262-1291`).

### 5.2 리플레이 (확인, `game/replay/replay_chart.h`, `scene_play.cpp:2918-2950`, `ruleset_bms_replay.cpp`)
- 판정 결과가 아니라 **입력 명령 스트림**을 저장한다: `{int64 ms, Type(키 DOWN/UP, 스크래치 ±/STOP, HISPEED, LANECOVER, ESC), double value}`. 여기에 randomSeed와 모디파이어를 함께 저장하고, cereal로 직렬화한다.
- 타임스탬프는 `t.norm() - PLAY_START`이며 **1 ms 정수**다. 입력 폴링 시각(이벤트 시각) 기준이다.
- 재생 시에는 기록된 ms를 `tt`에 넣어 `RulesetBMS::updatePress/Hold/Release/update`를 다시 호출하는 **재판정 방식**이다. pitch가 다르면 `replayTimestampMultiplier`로 스케일한다.
- 결론적으로 **LVF의 정식(canonical) 시간 기준은 "PLAY_START 기준 정수 ms"**다. 판정(ms 정수 비교)과 리플레이 포맷 모두 같은 해상도를 쓰므로, 재판정은 결정적이다(같은 입력이면 같은 결과). 단 라이브 판정의 `_startTime` 1틱 편차(2.4절)는 예외일 수 있다 (추정).

---

## 6. 판정/믹싱/렌더링을 바꾸는 설정 키 (기본값)

| 키 (파일) | 기본값 | 영향 |
|---|---|---|
| `JudgeOffset` (profile `settings.yml`) | 0 | `TIMING_ADJUST_VISUAL`이 되어 **시각 시간에만** 더해진다. 스킨 버튼으로 −99..99 범위 조정 (`skin_lr2_button_callbacks.cpp:1498`) |
| `uAutoAdjust` | 0 | 10노트마다 시각 오프셋 ±1 ms 자동 이동 |
| `InputPollingRate` | 1000 Hz | 입력 스레드 폴링 주기, 즉 판정 타임스탬프 해상도 |
| `MinInputInterval` | 16 ms | 키 디바운스(누름 유지). 릴리즈 판정을 지연시킨다 |
| (상수) `release_delay_ms` | 5 ms | 릴리즈 추가 지연, 설정 불가 |
| 차트 `#RANK` | NORMAL | 판정 창. 사용자 오버라이드 설정은 없음 (추정: 관련 키를 찾지 못함) |
| `GaugeOption`, `EnableNewGauges`(false), `bEnableGas` | Groove / false | 게이지 종류, GAS |
| `ChartAssistOption` (AutoScratch) | None | 스크래치 판정 제외 |
| `SpeedType` | Normal | 하이스피드 고정 방식(MAX/MIN/AVG/CONSTANT/INITIAL/MAIN). CONSTANT면 시간 기반 스크롤 |
| `Basespeed` / `Hispeed` / `GreenNumber` | 1.0 / – / 300 | 스크롤 속도 |
| `LaneEffect`, `LanecoverTop/Bottom` | Off | SUD+/HID+/LIFT |
| `MasterVolume/KeyVolume/BgmVolume` | 1.0 | 채널 그룹 볼륨 (BGM 키음은 KEY 그룹, 3.3절) |
| `Freq`/`FreqType`/`FreqVal` | – | 피치/속도. BPM에 곱해 차트 시간 자체를 바꾼다 |
| `AudioOutputMode`, `BufferLength`(128), `BufferCount`(2) | Auto | FMOD 출력과 DSP 버퍼(지연) |
| `MaxFPS`(480), `VSync`(true) (general) | | 렌더 주기. 키음과 BGM 트리거 양자화에 영향 |
| (코드 상수) ScenePlay rate | 1000 Hz | 고정 틱(MISS 검출, BGM, 스크래치 판정) |

---

## 7. LR2 정확성 주장 vs 의도적 이탈

**LR2 재현이라고 코드가 명시하는 부분 (확인, 주석 근거)**
- 판정 창 테이블: "verified with LR2 disassembly" (`ruleset_bms.h:276`)
- `#RANK 4`와 무효값은 NORMAL (`chartformat_bms.cpp:142`)
- 게이지 증감: "LR2 disassembly project", `#TOTAL 0`은 미설정으로 취급 (`ruleset_bms.cpp:169,409`)
- HP와 점수 애니메이션 시간: "Formula matches LR2" (`:501, 1254`)
- 지나간 노트가 판정선에 머무는 동작: "LR2 / pre RA behavior" (`sprite_lane.cpp:150`)
- LIFT 배치: "black box testing on LR2" (`skin_lr2.cpp:4190`)
- 턴테이블 정적 회전: "matches LR2" (`scene_play.cpp:2748`)

**의도적 이탈이나 확장 (확인)**
- 점수 공식 단순화 (LR2 콤보 보너스 공식 주석 처리)
- 자동 판정 보정(AutoAdjust), CONSTANT 스크롤, 신규 게이지(EXHARD/ASSIST, lr2oraja 기준), 신규 랜덤(RRAN, DB 계열), GAS
- FMOD 2 사용: LR2의 `FMOD_DSP_TYPE_REVERB` 대신 SFXREVERB (`sound_fmod.cpp:955`)
- VERY EASY(`#RANK 4`)는 TODO 상태로 NORMAL 대체

**LR2와 다를 수 있는 구현 디테일 (추정)**
- ms 정수 판정과 조기 포함/지각 미포함 경계, 룰셋 0점의 1틱 편차, 릴리즈 디바운스 지연, 아날로그 스크래치의 틱 시각 판정, BGM 키음의 KEY 그룹 라우팅

---

## 정책으로 본 요약

| 메커니즘(정책) | LVF의 선택 | 대안 | 비교 지표 |
|---|---|---|---|
| 차트 시간 표현 | ns 정수 + 시각 위치는 유리수 metre. BPM 구간 선형, 마디마다 basetime 누적 | 단일 double 초, 정수 tick(IIDX식 1/60 s, 1/1000 s), 정확 유리수 시간 | 장곡/다BPM 차트의 노트 시각 drift(기준 구현 대비 최대 오차 ns), 파싱 비용 |
| 판정 시간 양자화 | 노트와 입력을 각각 ms로 절삭한 뒤 정수 비교 | µs/ns 직접 비교, IIDX식 프레임 단위 | 합성 입력(알려진 오차)에 대한 판정 오분류율, 경계 대칭성 |
| 판정 경계 | 조기 포함 / 지각 미포함 (PG = [−18, +17]) | 대칭 `≤`, 대칭 `<` | 오차 0 중심 분포에서 early/late 비율의 편향 |
| 판정 창 | 고정 4단계 LR2 테이블 (#RANK) | beatoraja 창, 차트별 창(IIDX), 사용자 선택 | 동일 플레이어 입력 로그에 대한 등급 분포와 레이팅 호환성 |
| 입력 타임스탬프 | 입력 스레드 폴링 시각 (기본 1 kHz, ms), 이벤트 큐 후 프레임에서 처리 | OS 이벤트 타임스탬프(raw input/QPC), 업데이트 루프 시각 | 하드웨어 기준(광센서/로직 분석기) 대비 타임스탬프 오차의 평균과 분산 |
| 릴리즈 타임스탬프 | 디바운스 16 ms + 5 ms 지연 후의 시각 | 첫 릴리즈 감지 시각, 디바운스 없음 | LN 릴리즈 오차 분포, 채터링 오검출률 |
| 노트 매칭 | 레인의 첫 미만료 노트(가장 이른 것) | 최근접 노트, 창 안의 최선 판정 | 밀집 연타에서의 "억울한 판정" 비율 (리플레이 재판정 비교) |
| 빈 POOR | 다음 노트 [−1000, −201] ms 조기 누름만 해당. 늦은 빈 POOR 없음 | 늦은 빈 POOR 포함, 빈 POOR 없음 | BP 수, 게이지 영향 |
| MISS 검출 | 1 kHz 고정 틱 스캔 (+200 ms) | 입력 이벤트 시점 지연 평가, 프레임 단위 | MISS 확정 지연, 판정과 MISS 경쟁 조건 발생 수 |
| LN | LR2식: 헤드 판정 저장, 테일 −PG에 자동 완료, 조기 릴리즈 시 강등 | CN/HCN(테일 판정), IIDX CN | LN 판정 분포, 플레이어 체감 |
| 아날로그 스크래치 | 누적기 + 틱에서 임계 판정, 133 ms 정지면 릴리즈 | 이벤트 시각 판정, 속도 기반 | 스크래치 오차 분포 (이벤트 대비 판정 시각 차이) |
| 판정 오프셋 | 시각에만 적용 (판정과 오디오는 원시 시간) | 판정 시간 이동, 오디오 이동, 분리된 3개 오프셋 | 오프셋 조정 후 평균 오차 수렴 속도, 시청각 동기 |
| 키음 선택 | 다음 노트가 1 s 이내면 그 소리, 아니면 직전 소리 유지 | 최근접 노트, 판정된 노트의 소리, 레인 고정 | "틀린 소리" 발생률 (차트 의도 대비) |
| 동일 키음 재트리거 | `FMOD_UNIQUE`로 이전 인스턴스를 끊고 재시작(강제) | 중첩 허용, 페이드 후 컷, N개까지 중첩 (Qwilight 옵션) | 클리핑/피크 레벨, 연타 시 음색 왜곡, 보이스 사용 수 |
| 보이스 한도 | 512 소프트웨어 채널, FMOD 기본 스틸링 | 우선순위 기반 스틸링, 동적 한도 | 드롭된 보이스 수, CPU 시간 |
| 오디오 트리거 시점 | 키음은 프레임에서 즉시 `playSound`, BGM은 틱 catch-up(프레임 시점) | DSP clock 예약 재생(sample-accurate), 오디오 스레드 트리거 | 기대 발음 시각 대비 실제 출력 오차 (루프백 녹음) |
| 볼륨 라우팅 | BGM 레인 키음이 KEY 그룹으로 감 | BGM 그룹 분리 | 사용자 볼륨 조정 의도와의 일치 |
| 스크롤 모델 | metre 기반 (BPM 변화 = 속도 변화), CONSTANT는 시간 기반 | 시간 기반, soflan 정규화(IIDX식) | 가독성 설문, 녹색 숫자 대비 실측 체류 시간 |
| 렌더 보간 | 보간 없음 (1 kHz 틱 값 그대로, 최대 1 ms 지연), 정수 픽셀 | 틱 간 선형 보간, 렌더 시각 직접 계산, 서브픽셀 | 프레임 간 노트 변위 지터 (px 표준편차) |
| 게이지 | 판정 확정 즉시, LR2 공식 | IIDX 공식, beatoraja 공식 | 클리어율 분포 |
| 리플레이 | 입력 명령 + ms 정수 + 시드, 재판정 | 판정 결과 저장, µs 입력 저장 | 재판정 결정성 (라이브 결과와의 일치율) |
