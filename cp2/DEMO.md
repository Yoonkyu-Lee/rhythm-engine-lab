# CP2 데모: 직접 쳐 보기

건반 하나짜리 시험 게임으로 "렉이 왔을 때 엔진마다 어떻게 되나"를 직접 느껴 보는 순서다.
모든 명령은 `cp2/` 폴더의 PowerShell에서 실행한다 (빌드: `cargo build --offline --release`).

## 조작

| 키 | 하는 일 |
|---|---|
| Space · J · F | 치기 (아무거나, 레인은 하나) |
| F1 | 오디오 렉 150 ms (소리가 끊겼다가 이어짐 → 음악이 0.14초 밀림) |
| F2 | 화면 렉 300 ms (화면이 멈춤) |
| F3 | 판정 렉 300 ms (우리 방식: 판정 스레드 / 다른 프로필: 화면 스레드가 멈춤) |
| F4 | 입력 렉 300 ms (입력 스레드가 멈춤) |
| Esc | 끝내기 (곡이 끝나면 3초 뒤 저절로 끝남) |

화면 위쪽 글자(HUD)에 프로필, 게임 상태(Running / Suspect / Resyncing), 최근 렉 사건, 판정 집계가 나온다.

## 곡

| 곡 | 파일 | 특징 |
|---|---|---|
| 합성 곡 | `content\synth\synth.rtc` | 120 BPM 4분음표만 256개. 박자 감 잡기 쉬움 |
| BlythE | `content\djmax_1_4b_nm_t6\chart.rtc` | 실제 곡, 노트 103개 (롱노트 11) |
| Negative Nature | `content\djmax_72_4b_nm_t6\chart.rtc` | 실제 곡, 템포 변화, 노트 42개 (롱노트 10) |

## 해 볼 것 (추천 순서)

1. **기준 — 우리 방식, 렉 없이**
   `.\target\release\cp2game.exe run content\djmax_1_4b_nm_t6\chart.rtc`
   판정이 귀에 맞는지 본다. 끝난 뒤 분석의 판정 오차 평균이 계속 +(늦음)로 나오면 그만큼 빼고(`--out-offset-ms -5`), −(빠름)로 나오면 더해서 다시. 이 값은 같은 누름을 그만큼 늦은 시각으로 치는 보정이다.
2. **우리 방식 + F1 (오디오 렉)**
   같은 명령으로 치다가 F1. 소리가 잠깐 끊기고 노트가 살짝 뒤로 튄다(화면 튐 −0.12초, 2-5의 남은 과제). 그 직후 0.2~0.4초(Resyncing) 동안의 판정은 **무효**로 처리되어 콤보가 끊기지 않는다. 끝에 출력되는 `무효 N`을 확인.
3. **BMS식 + F1**
   `.\target\release\cp2game.exe run content\djmax_1_4b_nm_t6\chart.rtc --profile bms`
   F1 뒤로는 노트가 음악보다 0.14초 먼저 온다 — 음악에 맞춰 치면 끝까지 BAD. 두 번 누르면 더 벌어진다.
4. **IIDX 개량형 + F2 (화면 렉)**
   `... --profile iidx` 로 치다가 F2. 차트가 0.3초 멈췄다가 빠르게(1.5배속) 따라잡는다. 따라잡는 동안 노트가 빨라 보이는 게 정상.
5. **DJMAX식 + 긴 판정 렉**
   `... --profile djmax --faults judge@30:500` (곡 30초에 0.5초 멈춤). 멈춘 0.5초 중 0.333초만 흐른 것으로 쳐서 그 뒤로 차트가 음악보다 0.17초 늦게 온다.
6. **늦은 키음 버리기 비교** (우리 방식, F4 입력 렉)
   기본(100 ms 넘게 늦은 키음은 버림)과 `--late-drop-ms 0`(끝까지 밀어서 냄)을 번갈아 F4를 누르며 쳐 본다. 0일 때 렉 동안 친 노트의 키음이 한참 늦게 "띵" 울린다.

다른 옵션: 재동기화 중 판정을 무효로 하지 않으려면 `--resync-judge count`.

## 친 뒤에 보기

각 실행은 `runs\run-<시각>\`에 기록이 남는다. 분석:

```
python -I -X utf8 tools\analyze_run.py (Get-ChildItem runs -Directory | Sort-Object Name | Select-Object -Last 1).FullName
.\target\release\replay.exe (Get-ChildItem runs -Directory | Sort-Object Name | Select-Object -Last 1).FullName
```

첫 줄은 판정·렉 감지·고장 구간 표, 둘째 줄은 기록된 입력만으로 판정을 다시 해서 실행 중 판정과 맞댄다(우리 방식만, "재현 100%"가 나와야 함).
