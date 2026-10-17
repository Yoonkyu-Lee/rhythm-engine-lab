# 리듬게임 연구 작업 공간

beatmania IIDX 데이터 분석에서 시작해, IIDX → BMS 컨버터를 완성하고, 리듬게임 엔진을 바닥부터 만드는 작업 공간입니다.

| 폴더 | 내용 | 상태 |
|---|---|---|
| [cp0/](cp0/) | 번외편: 기존 게임 분석 (IIDX 형식과 타이밍, BMS 구동기 beatoraja · Lunatic Vibes F 소스 분석, 분석 도구). [리포트](cp0/REPORT.md) | 완료 |
| [cp0/bms/converter/](cp0/bms/converter/) | IIDX → BMS 컨버터와 검증 도구. [사용법](cp0/BMS_CONVERTER.md) | 완료 (2026-10-11) |
| [design/](design/) | 게임 설계 구상: 타이밍 설계, 기록 시계 설계, 곡 데이터 형식, 세미 게임 레이어 | 구상 |
| [cp1/](cp1/) | 엔진 Checkpoint 1: 싱크로나이저와 테스트벤치 (Rust). [리포트](cp1/REPORT.md) | 완료 (CP1-3 렉 조사는 2026-10-15 닫음) |
| [cp2/](cp2/) | 엔진 Checkpoint 2: 플레이 코어와 정책 시험장 (건반 하나짜리 게임, 구동 프로필 비교). [리포트](cp2/REPORT.md) | 진행 중 |
| `HANDOFF_PROMPT.md` | 원본 데이터 환경과 폴더 규칙 (인수인계) | 로컬 전용 |

게임 데이터(변환 결과, 추출 음원, 커뮤니티 변환본), 다른 프로젝트의 소스 복제본, IIDX 역공학 문서와 도구는 저장소에 올리지 않습니다 (`.gitignore`). 그래서 일부 문서의 IIDX 분석 링크는 저장소에서 열리지 않습니다.

## cp0/bms/converter/

| 경로 | 내용 |
|---|---|
| `tools/iidx2bms.py` | 변환기 본체. 출력 기본값은 `cp0/bms/converter/out` |
| `tools/` 나머지 | 검증 도구 (독립 BMS 렌더러, 변환본 비교, 미리듣기 비교), 시스템 BGM 추출, 난이도 테이블 생성, FLAC→WAV 이전. `.1`·BMS 열람과 전수 조사 도구는 [cp0/iidx/tools/](cp0/iidx/tools/) / [cp0/bms/tools/](cp0/bms/tools/) |
| `system_bgm/` | 추출한 시스템 BGM |
| `out/`, `out_charts/`, `out_soflan/` | 시험 변환 출력 |
| `logs/` | 전곡 변환·이전 로그 |
| `converted example/` | 비교용 커뮤니티 변환본 (25편까지) |

변환 결과 라이브러리는 이 폴더가 아니라 `D:\Games\BMS\IIDX converted SP`, `... DP`, 난이도 테이블은 `D:\Games\BMS\IIDX tables`에 있습니다.

## cp0/

분석 문서와 도구 목록은 [cp0/README.md](cp0/README.md), 결론은 [cp0/REPORT.md](cp0/REPORT.md).

## 엔진 계획 (요약)

1. **CP1 싱크로나이저**: 장치 시계를 정규 시간(ns)으로 묶는 맨 밑층과, 고장을 일부러 넣어 동작을 재는 테스트벤치
2. **CP2 정규 코어 + 최소 게임**: 레인, 판정, 키음, 스크롤, 리플레이
3. **CP3 곡 데이터 형식 + 칠 수 있는 게임**: 작성 형식, 컴파일러, DJMAX·IIDX·BMS 가져오기, 여러 레인 게임·선곡·미리보기·설정 (완료 2026-10-17 — [cp3/README.md](cp3/README.md), [cp3/DEMO.md](cp3/DEMO.md))
