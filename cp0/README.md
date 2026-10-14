# CP0 (번외편): 기존 게임 분석

엔진(CP1~)을 만들기 전에 기존 리듬게임이 채보와 시간을 어떻게 다루는지 분석한 문서와 도구입니다.
진행 기록과 결론은 [REPORT.md](REPORT.md)에 있습니다.

## 문서

| 문서 | 내용 | 공개 |
|---|---|---|
| `IIDX_vs_BMS.md` | IIDX `.1` 채보 형식과 BMS 구조 비교, 변환 시 손실 | 로컬 전용 |
| `IIDX_timing_policy.md` | IIDX 실행 파일 정적 분석: 고정 틱, 반주 동기화, 주사율 재추정 | 로컬 전용 |
| [BMS_player_timing_policy.md](BMS_player_timing_policy.md) | beatoraja · Lunatic Vibes F 소스 분석, 정책 공간(P1~P12)과 측정 지표(M1~M8) | 공개 |
| [third_party/_analysis/](third_party/_analysis/) | beatoraja · LVF 축별 상세 보고서 (시계·오디오·입력 / 판정·믹싱·렌더) | 공개 |

분석한 소스 복제본(`third_party/beatoraja/` `8320241`, `third_party/lunaticvibesf/` `3e8a655`)은 저장소에 올리지 않습니다.

## 도구 (`tools/`)

모두 `python -I tools\<스크립트>`로 실행합니다. IIDX 원본 데이터 위치는 스크립트 안에 고정되어 있습니다 (`D:\Arcade\LDJ-010-2026081900`).

| 스크립트 | 하는 일 | 공개 |
|---|---|---|
| `dot1_peek.py` | `.1` 하나: 난이도 칸 표, 지정 칸 앞쪽 이벤트, type 집계 | 공개 |
| `iidx_survey.py` | 전곡 조사: 소리 묶음 접미사, 칸 ↔ 난이도 대응, 소리 번호 범위, 롱노트 | 공개 |
| `type8_survey.py` | 모든 `.1`의 type 8(판정 창) 값 분포 | 공개 |
| `scene_export.py` | `.1` 앞 N소절을 JSON으로 + 같은 장면의 BMS 표현 | 공개 |
| `bms_peek.py` | BMS 하나: 앞 바이트, BOM·인코딩·줄바꿈, 헤더, 채널별 줄 수 | 공개 |
| `bms_stats.py` | 폴더 아래 BMS 전체: 인코딩 분포, 채널·헤더 명령 사용 빈도, `#WAV` 수 | 공개 |
| `re_disasm_index.py` | 실행 파일 `.text`를 선형 역어셈블해 검색용 텍스트로 저장 | 로컬 전용 |

변환기와 변환 결과 검증 도구(`iidx2bms.py`, `bms_render.py`, `compare_bms.py`, `compare_preview.py` 등)는 [../converter/](../converter/)에 있습니다.
