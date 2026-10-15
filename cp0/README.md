# CP0 (번외편): 기존 게임 분석

엔진(CP1~)을 만들기 전에 기존 리듬게임이 채보와 시간을 어떻게 다루는지 분석한 문서와 도구입니다.
진행 기록과 결론은 [REPORT.md](REPORT.md)에 있습니다.

## 배치 규칙

분석 문서는 `cp0/` 루트에 모으고, 도구·원본·수집 결과·소스 복제본은 게임별 폴더에 둡니다.

```text
cp0/
├─ *.md, *.pdf             분석 문서와 참고 자료
├─ iidx/tools/            IIDX 채보·데이터·실행 파일 분석 도구
├─ bms/
│  ├─ tools/              BMS 열람·통계 도구
│  ├─ converter/          IIDX → BMS 변환기·검증 도구·시험 출력
│  └─ third_party/        beatoraja·Lunatic Vibes F 소스 복제본
└─ djmax/
   ├─ DJMAX RESPECT V_Data/  설치 데이터 복사본
   ├─ collected/            복호화 결과·캐시·로그
   └─ work/                 분석·수집 도구와 작업 자료
```

외부 프로젝트와 Python 의존성의 원래 디렉터리 구조는 게임 폴더 안에서 유지합니다. 로컬 전용 문서·데이터의 Git 제외 규칙도 유지합니다.

## 문서

| 문서 | 내용 | 공개 |
|---|---|---|
| `IIDX_vs_BMS.md` | IIDX `.1` 채보 형식과 BMS 구조 비교, 변환 시 손실 | 로컬 전용 |
| `IIDX_timing_policy.md` | IIDX 실행 파일 정적 분석: 고정 틱, 반주 동기화, 주사율 재추정 | 로컬 전용 |
| [BMS_player_timing_policy.md](BMS_player_timing_policy.md) | beatoraja · Lunatic Vibes F 소스 분석, 정책 공간(P1~P12)과 측정 지표(M1~M8) | 공개 |
| [BMS_CONVERTER.md](BMS_CONVERTER.md) | 컨버터 실행·기본 경로·경로 의존성 검사 | 공개 |
| [beatoraja A](beatoraja_A_clock_audio_input.md), [beatoraja B](beatoraja_B_judge_mix_render.md) | beatoraja 상세 분석: 시계·오디오·입력 / 판정·믹싱·렌더 | 공개 |
| [LVF A](lvf_A_clock_audio_input.md), [LVF B](lvf_B_judge_mix_render.md) | Lunatic Vibes F 상세 분석: 시계·오디오·입력 / 판정·믹싱·렌더 | 공개 |
| `DJMAX_analysis.md` | DJMAX 파일시스템·곡 데이터·구동 정책과 IIDX/BMS 비교 | 로컬 전용 |
| `DJMAX_song_analysis.md` | 곡 구성 조사 기준과 첫 표본 D2의 통계 | 로컬 전용 |
| `DJMAX_BATCH_COLLECTOR.md` | 온라인 수집 / 오프라인 복호화·재개 방법과 완료 기록 | 로컬 전용 |
| `DJMAX_class_map.md` | DJMAX 선언부 기반 클래스·함수 지도 | 로컬 전용 |

분석한 소스 복제본(`bms/third_party/beatoraja/` `8320241`, `bms/third_party/lunaticvibesf/` `3e8a655`)은 저장소에 올리지 않습니다.

## 도구

다음 예시는 저장소 루트에서 실행합니다. IIDX 원본 데이터 위치는 기존 스크립트의 `D:\Arcade\LDJ-010-2026081900`입니다.

```powershell
python -I -X utf8 cp0/iidx/tools/dot1_peek.py "<채보 경로>.1" 2
python -I -X utf8 cp0/bms/tools/bms_peek.py "<채보 경로>.bms"
python -I -X utf8 cp0/djmax/work/audit_collection.py
python -I -X utf8 cp0/bms/tools/check_paths.py
```

| 스크립트 | 하는 일 | 공개 |
|---|---|---|
| [iidx/tools/dot1_peek.py](iidx/tools/dot1_peek.py) | `.1` 하나: 난이도 칸 표, 지정 칸 앞쪽 이벤트, type 집계 | 공개 |
| [iidx/tools/iidx_survey.py](iidx/tools/iidx_survey.py) | 전곡 조사: 소리 묶음 접미사, 칸 ↔ 난이도 대응, 소리 번호 범위, 롱노트 | 공개 |
| [iidx/tools/type8_survey.py](iidx/tools/type8_survey.py) | 모든 `.1`의 type 8(판정 창) 값 분포 | 공개 |
| [iidx/tools/scene_export.py](iidx/tools/scene_export.py) | `.1` 앞 N소절을 JSON으로 + 같은 장면의 BMS 표현 | 공개 |
| [bms/tools/bms_peek.py](bms/tools/bms_peek.py) | BMS 하나: 앞 바이트, BOM·인코딩·줄바꿈, 헤더, 채널별 줄 수 | 공개 |
| [bms/tools/bms_stats.py](bms/tools/bms_stats.py) | 폴더 아래 BMS 전체: 인코딩 분포, 채널·헤더 명령 사용 빈도, `#WAV` 수 | 공개 |
| [bms/tools/check_paths.py](bms/tools/check_paths.py) | IIDX·BMS·DJMAX 도구의 경로·로컬 import·의존성 검사 | 공개 |
| `iidx/tools/re_disasm_index.py` | 실행 파일 `.text`를 선형 역어셈블해 검색용 텍스트로 저장 | 로컬 전용 |
| `djmax/work/collect_all.py` | DJMAX 온라인 수집과 오프라인 추출 | 로컬 전용 |
| `djmax/work/audit_collection.py` | 수집 인덱스·출력 크기·목차 배치 점검 | 로컬 전용 |

변환기와 변환 결과 검증 도구(`iidx2bms.py`, `bms_render.py`, `compare_bms.py`, `compare_preview.py` 등)는 [bms/converter/](bms/converter/)에 있습니다. 명령은 저장소 루트 기준으로 [BMS_CONVERTER.md](BMS_CONVERTER.md)에 정리했습니다.
