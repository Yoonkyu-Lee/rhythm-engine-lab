# 리듬게임 연구 작업 공간

beatmania IIDX 데이터 분석에서 시작해, IIDX → BMS 컨버터를 완성하고, 리듬게임 엔진을 바닥부터 만드는 작업 공간입니다.

| 폴더 | 내용 | 상태 |
|---|---|---|
| [converter/](converter/) | IIDX → BMS 컨버터와 보조 도구 | 완료 (2026-10-11) |
| [research/](research/) | 분석 문서: IIDX 형식과 타이밍, BMS 구동기(beatoraja, Lunatic Vibes F) 소스 분석 | 사실 기록 |
| [design/](design/) | 게임 설계 구상: 타이밍 설계, 곡 데이터 형식, 세미 게임 레이어 | 구상 |
| [cp1/](cp1/) | 엔진 Checkpoint 1: 싱크로나이저와 테스트벤치 (Rust) | 진행 예정 |
| `HANDOFF_PROMPT.md` | 원본 데이터 환경과 폴더 규칙 (인수인계) | 로컬 전용 |

게임 데이터(변환 결과, 추출 음원, 커뮤니티 변환본)와 다른 프로젝트의 소스 복제본은 저장소에 올리지 않습니다 (`.gitignore`).

## converter/

| 경로 | 내용 |
|---|---|
| `tools/iidx2bms.py` | 변환기 본체. 출력 기본값은 `converter/out` |
| `tools/` 나머지 | 분석·검증 도구 (BMS 렌더러, 미리듣기 비교, `.1` 열람, 역어셈블 색인 등), 시스템 BGM 추출, 난이도 테이블 생성, FLAC→WAV 이전 |
| `system_bgm/` | 추출한 시스템 BGM |
| `out/`, `out_charts/`, `out_soflan/` | 시험 변환 출력 |
| `logs/` | 전곡 변환·이전 로그 |
| `converted example/` | 비교용 커뮤니티 변환본 (25편까지) |

변환 결과 라이브러리는 이 폴더가 아니라 `D:\Games\BMS\IIDX converted SP`, `... DP`, 난이도 테이블은 `D:\Games\BMS\IIDX tables`에 있습니다.

## research/

| 문서 | 내용 |
|---|---|
| `IIDX_vs_BMS.md` | `.1` 채보 형식과 BMS 비교 |
| `IIDX_timing_policy.md` | `bm2dx.dll` 정적 분석: 고정 틱, 반주 동기화, 주사율 재추정 |
| `BMS_player_timing_policy.md` | beatoraja · LVF 소스 분석과 정책 공간(측정 축) |
| `third_party/` | 두 구동기 소스(얕은 복제)와 축별 분석 보고서 `_analysis/` |

## 엔진 계획 (요약)

1. **CP1 싱크로나이저**: 장치 시계를 정규 시간(ns)으로 묶는 맨 밑층과, 고장을 일부러 넣어 동작을 재는 테스트벤치
2. **CP2 정규 코어 + 최소 게임**: 레인, 판정, 키음, 스크롤, 리플레이
3. **CP3 곡 데이터 형식**: 작성 형식, 컴파일러, IIDX·BMS 가져오기
