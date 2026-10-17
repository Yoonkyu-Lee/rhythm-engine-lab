# 실험 C1·C2·C3 — 선례 스킨을 우리 화면 도메인으로 옮길 수 있나

> 작성 2026-10-17. 대상 문서: `../화면_도메인.md`(초안), `../실험_목록.md` C절.
> 방법: 코드·파일을 **읽기만** 했다(실행 없음). 표시: **(확인)** = 코드·파일에서 직접 봄, **(추정)** = 이름·주석·일반 지식으로 미룸.
> 경로 약어:
> - `BR/` = `cp0/bms/third_party/beatoraja/src/bms/player/beatoraja/skin/` (beatoraja 스킨 로더)
> - `BRS/` = `cp0/bms/third_party/beatoraja/skin/default/` (beatoraja 기본 스킨)
> - `LV/` = `cp0/bms/third_party/lunaticvibesf/src/game/skin/` (LR2 스킨 호환 구현, LunaticVibesF)
> - `QW/` = `D:\Games\Steam\steamapps\common\Qwilight\SavesDir\UI\` (설치본, 읽기만. 형식·키 이름만 적고 원문·그림은 옮기지 않음)
> - IIDX·DJMAX는 `cp0/*.md`(루트 문서만)와 일반 지식. `cp0/djmax/` 등 비공개 자료는 열지 않았다.

---

## 요약 (결론)

1. **그리기 쪽은 거의 다 옮겨진다.** 대응표 75개 개념 중 대응 가능 31 / 부분 35 / 불가 9. 부분의 대부분은 요소 종류가 아니라 **속성·값·사건 목록이 모자란 것**이다(혼합 방식 목록, 보간 곡선, 노트 변형, 값 번호 수백 개 등).
2. **빠진 큰 개념은 넷**이다: ① **상태 시계**(beatoraja·LR2의 "타이머" = 켜진 순간부터 흐르고 꺼지면 숨음. 우리 사건은 순간뿐) ② **박자 시계**(`TIMER_RHYTHM`, Qwilight 메트로놈) ③ **사용자 조정**(판정·기어 위치, 요소 끄기·투명도를 사용자가 바꿈) ④ **레인 틀 펼치기·로드 시 조건**(같은 요소를 레인마다, 옵션마다 다르게 펼침).
3. **불가는 대부분 "입력 위젯"과 "스크립트 부작용"**이다: 버튼·슬라이더 조작·텍스트 입력·마우스, 그리고 Lua의 파일·HTTP·소리·볼륨·상태 있는 사용자 타이머. 셰이더·마스크·파티클·3D는 **선례 스킨 형식에도 없다**(확인) — 지금 막을 이유도, 넣을 이유도 없다.
4. **C2**: 세 화면 시안에서 현재 도메인으로 못 적은 것 20개(중복 빼면 18개), 아래 제안을 더하면 3개(무작위 게이지 깜빡임의 정확한 재현, 선곡 화면 버튼, 검색 입력). beatoraja 기본 Lua 스킨은 Lua를 **로드할 때 표를 만드는 데만** 쓰고 실행 중 식은 JSON의 비교식 4곳뿐이다(확인).
5. **추천: 선언형 + 작은 순수 식 + 로드 시 펼치기, 스크립트는 넣지 않는다.** 기존 스킨 변환기는 CP4 본 범위 밖으로 두고, 하게 되면 beatoraja JSON → LR2 → Qwilight 순서(난이도·결정성 기준).

---

## 1. C1 — 선례 개념 → 우리 도메인 대응표

표기: ○ 대응 가능 / △ 부분 / × 불가. "우리 도메인" 칸의 이름은 `화면_도메인.md`의 것이고, ★는 6장의 제안 이름이다.

### 1-1. 스킨 묶음·화면·옵션

| # | 선례 개념 (근거) | 우리 도메인 | 판정 | 이유 |
|---|---|---|---|---|
| 1 | 화면 종류: BR `SkinType` 19종(플레이 키 모드별·선곡·결정·결과·키 설정·스킨 선택 등, `BR/SkinType.java:12-30`), LR2 `#INFORMATION` 0~17(`LV/skin_lr2.cpp:3244-3394`), QW 인게임 스킨 11개 / 테마(`@`) 4개 (확인) | 화면(Scene) + 플레이 배치의 레이아웃 맞춤 | ○ | 키 모드별 플레이 스킨 = 레인 레이아웃 서명별 배치 |
| 2 | 가상 해상도: BR `w`/`h`(기본 1280×720, `BR/json/JsonSkin.java:12-13`), LR2 `#RESOLUTION` 0~3, 없으면 좌표 분포로 추정(`LV/skin_lr2.cpp:3920-3986`), QW 화면 비율 스킨 옵션 16:9/16:10/21:9/32:9(`cp0/Qwilight_settings.md:466`) (확인) | 공통 속성 단위 = 가상 해상도 ⚑ | △ | 해상도는 대응. **종횡비별 배치**(QW)는 ⚑1(종횡비 정책)에 달림 |
| 3 | 화면 시간 상수: BR `input`·`scene`·`fadeout`·`close`·`loadend`·`playstart`(`JsonSkin.java:14-19`), LR2 `#STARTINPUT`·`#SCENETIME`·`#FADEOUT`·`#LOADSTART`·`#LOADEND`·`#PLAYSTART`·`#CLOSE`·`#SKIP`(`LV/skin_lr2.cpp:968-1036`) (확인) | 전환(Transition), `scene.enter` | △ | "입력 받기 시작까지", "페이드 길이" 같은 화면 시간 상수가 없다. 단 **`playstart`(곡 시작 지연)는 스킨이 정하면 안 된다** — 엔진 고정, 스킨은 연출 길이만 |
| 4 | 스킨 옵션(고르기): BR `property{name, item{name, op}}`(`JsonSkin.java:59-69`), LR2 `#CUSTOMOPTION` 900~999, QW `configure-N`·`configure-comment-N`(`QW/CR_Core/CR_Core.yaml:178-181`) (확인) | 스킨 옵션 `skin.option.*` | ○ | 구조가 같다(QW `configure-N`과 같은 구조라고 이미 적어 둠) |
| 5 | 파일 고르기: BR `filepath`(와일드카드 경로, `JsonSkin.java:71-76`, `BRS/play7.json:30-35`), LR2 `#CUSTOMFILE` + 와일드카드는 **무작위로 하나**(`random_device`, `LV/skin_lr2.cpp:637-671`) (확인) | 스킨 옵션 | △ | 옵션 종류에 "파일 고르기"가 없다. 무작위 선택은 결정성 원칙과 충돌 → 시드 고정 필요 |
| 6 | 사용자 위치 조정: BR `offset`(x·y·w·h·r·a 조정 항목, `JsonSkin.java:78-88`) + 엔진 기본 4종 All/Notes/Judge/JudgeDetail(`BR/json/JSONSkinLoader.java:185-188`), QW `UIConfigure`의 기어 위치·판정선 위치·콤보 위치·노트 너비/두께(`cp0/Qwilight_settings.md:440-455`), LR2 `#CUSTOMOFFSET`(LVF는 미구현) (확인) | 없음 | × | ★사용자 조정(Adjust) 필요 |
| 7 | 옵션 묶음: BR `category`(`JsonSkin.java:90-93`) (확인) | 스킨 옵션 | △ | 설정 화면 묶음(탭) 속성만 더하면 됨 |
| 8 | 로드 시 분기·포함: BR JSON `"if"`(옵션 번호 AND/OR/부정)·`"include"`(`BR/json/JsonSkinSerializer.java:171-238, 285-331`), LR2 `#IF/#ELSEIF/#ELSE/#ENDIF` 중첩 + `#INCLUDE`(`LV/skin_lr2.cpp:3444-3597`) (확인) | 조건(Condition) | △ | 우리 조건은 실행 중 보임만. **배치 자체를 바꾸는 로드 시 조건**(★`when`)과 파일 포함이 따로 필요 |
| 9 | 에셋 묶음: QW `이름.yaml + 이름.lua + 이름.zip`, BR `source`·`font`, LR2 `#IMAGE`·`#LR2FONT`·`#FONT` (확인) | 스킨·에셋 | ○ | |
| 10 | 비트맵 글꼴: BR 글꼴 `type`(비트맵 .fnt·거리장, `JsonSkin.java:100-110`, `BR/SkinTextBitmap.java`), LR2FONT(문자 ID를 Shift-JIS로 매핑, `LV/skin_lr2.cpp:772-920`) (확인) | text 글꼴 에셋 | △ | 글꼴 에셋에 "비트맵 글꼴(글자 그림표)" 종류 필요 |
| 11 | 이름 붙은 색·크기: QW `paint`(글자 색 이름)·`font`(글자 크기) 섹션 (확인) | 없음 | △ | ★스킨 상수(색·크기 토큰) |
| 12 | 키 모드별 레인 배정: QW `drawingN`·`drawingInputModeN`(`CR_Core.yaml:337-385`), LR2 레인 번호 표(모드·`#SCRATCHSIDE`별, `LV/skin_lr2.cpp:1900-1979`), BR 키 모드별 스킨 + Play Side·Scratch Side 옵션(`BRS/play/play7main.lua:31-41`) (확인) | 레이아웃 맞춤, 레인 역할 `[role=scratch]` | ○ | |
| 13 | DP·2P 측: QW `alt-<요소>` 0/1/2(`CR_Core.yaml:400-472`), LR2 `#FLIPRESULT`·`#DISABLEFLIP`·2P 반전 시 타이머 교체(`LV/skin_lr2.cpp:113`), BR 1P/2P별 타이머·옵션 번호 (확인) | 레인 레이아웃 서명 | △ | 자리 이름에 **플레이어 측**(`[p1]`/`[p2]`)이 없다 |

### 1-2. 요소 종류

| # | 선례 개념 (근거) | 우리 도메인 | 판정 | 이유 |
|---|---|---|---|---|
| 14 | 정지 그림·시트 한 칸: BR `image`(x·y·w·h·divx·divy, `JsonSkin.java:112-127`), LR2 `#SRC_IMAGE` 공통 인자(`LV/skin_lr2.cpp:139-175`), QW 그림 이름 규칙 (확인) | `image` | ○ | |
| 15 | 시트 애니메이션: BR `timer`+`cycle`, LR2 `cycle`·`timer`, QW `<요소>-frame`·`-framerate`(`CR_Core.yaml:69-133`, 예 `judgment-paint-frame` 108-109행) (확인) | `sequence` | ○ | |
| 16 | 값으로 칸 고르기: BR `image.ref/len`, `imageset{value, images}`(`JsonSkin.java:129-136`), QW 그림 이름의 상태 칸(키빔 0 안 누름/1 누름) (확인) | `image.frame ← 값` 바인딩 | ○ | |
| 17 | 숫자 그림: BR `value`(digit·align·padding·zeropadding·space·자리별 offset, `JsonSkin.java:138-157`), LR2 `#SRC_NUMBER`(칸 수가 10·11·24 배수인지로 음수·앞 0 형식 결정, `LV/skin_lr2.cpp:1300`), QW `Bin\<접두어>`(콤보 C·점수 S·BPM B 등) (확인) | `number` | △ | 음수 기호·앞 0 전용 칸(11칸·24칸 형식), 자리 간격·자리별 위치 어긋남 속성이 없다 |
| 18 | 실수: BR `floatvalue`(정수부·소수부 자릿수, `gain`, 부호, `JsonSkin.java:159-181`) (확인) | `number`(소수 자릿수·부호) | ○ | |
| 19 | 글자: BR `text`(font·size·align·wrapping·overflow·outline·shadow, `JsonSkin.java:183-201`), LR2 `#SRC_TEXT`(st 번호·align), QW title·artist·genre (확인) | `text` | ○ | |
| 20 | 영상: BR 영상 원본(`BR/SkinSourceMovie.java`), LR2 `#IMAGE`가 영상이면 영상 스프라이트(`LV/skin_lr2.cpp:1274`), QW 테마 `@CR_abstract`의 mp4 24개 (확인) | `video` | ○ | |
| 21 | 맞추기: BR `stretch` 11종(늘이기·안/밖 맞춤·자름·폭/높이 맞춤·원본 크기, `BR/StretchType.java:16-100`) (확인) | `video` 맞추기(채움/맞춤/늘이기) | △ | `image`에도 필요하고 종류가 더 많다(자름 여부·확대 금지) |
| 22 | 도형: LR2 특수 그림 번호 110 검정·111 흰색(`LV/skin_lr2.cpp:1198-1206`), BR·QW는 그림을 늘여 씀 (확인) | `shape` | ○ | 선례는 "1×1 그림 늘이기"뿐이라 우리 쪽이 더 넓다 |
| 23 | 값 막대: BR `graph`(angle 방향·type, `JsonSkin.java:225-242`), LR2 `#SRC_BARGRAPH`(type·muki) (확인) | `bar` | ○ | |
| 24 | 게이지: BR `gauge{nodes, parts=50, type}` + 칸 연출 무작위/증가/감소/깜빡임(`JsonSkin.java:369-378`, `beatoraja/play/SkinGauge.java:38-50`, 무작위는 `Math.random` 113-117행), LR2 `#SRC_GROOVEGAUGE` 50칸 격자(`LV/skin_lr2.cpp:1788`), IIDX 표시 = 2×floor(g/100)%(`cp0/IIDX_scroll_judge_gauge.md:43`) (확인) | `bar`(칸 나누기·구간별 색) | △ | 칸별 그림(켜짐/꺼짐 × 클리어 구간 × 게이지 종류 8장)과 칸 깜빡임 연출 속성이 없다. 무작위 깜빡임은 결정성과 충돌 |
| 25 | 슬라이더(값으로 위치): BR `slider`(angle·range·value, `JsonSkin.java:203-223`), LR2 `#SRC_SLIDER`(muki·range) (확인) | `image` 위치 ← 값 바인딩 | ○ | 표시만이면 새 요소가 필요 없다 |
| 26 | 슬라이더 조작: BR `changeable`·`event`(FloatWriter), LR2 슬라이더 콜백(`LV/skin_lr2_slider_callbacks.cpp:324`) (확인) | 없음 | × | 입력 위젯 |
| 27 | 버튼·클릭: BR `act`·`click` 4종(왼/오 절반 ±1 등, `JsonSkin.java:125-126`, `BR/SkinObject.java:670-700`), `BUTTON_` 74개(`BR/SkinProperty.java`), LR2 `#SRC_BUTTON`(type·click·panel·plusonly, `lunaticvibesf/src/game/graphics/sprite_button.cpp:14-49`) (확인) | 없음 | × | 입력 위젯. 선곡 화면이 옵션 패널을 겸한다 |
| 28 | 마우스오버·마우스 커서: LR2 `#SRC_ONMOUSE`·`#SRC_MOUSECURSOR`, BR `mouseRect`(`JsonSkin.java:466`) (확인) | 없음 | × | 입력 위젯 |
| 29 | 텍스트 입력: BR `text.editable`(`JsonSkin.java:192`, `BR/SkinTextInput.java`), LR2 `edit=1`(`lunaticvibesf/src/game/graphics/sprite_text.cpp:92-118`) (확인) | 없음 | × | 입력 위젯(검색창) |
| 30 | 게이지 기록 그래프: BR `gaugegraph`(게이지 종류별 배경·선 색, `JsonSkin.java:244-261`), LR2 `#SRC_GAUGECHART`(`LV/skin_lr2.cpp:1622`) (확인) | `graph` `result.gauge_graph` | ○ | |
| 31 | 점수 그래프: LR2 `#SRC_SCORECHART`(`LV/skin_lr2.cpp:1647`), BR Score Graph 옵션(`BRS/play7.json:17-20`) (확인) | `graph` | △ | 시계열 값 출처(내 점수·최고·목표의 진행)가 값 목록에 없다 |
| 32 | 노트 밀도·판정 분포: BR `judgegraph` type 0 밀도 / 1 판정 / 2 빠름·늦음(`BR/SkinNoteDistributionGraph.java:69-71`), QW `judgmentInputVisualizer` (확인) | `graph` `select.density_graph` | ○ | 실시간 판정 분포는 값 추가만 |
| 33 | BPM 그래프: BR `bpmgraph`(주·최소·최대 BPM·정지 색, `JsonSkin.java:273-283`) (확인) | `graph` | △ | 값 출처 `chart.bpm_series`가 없다 |
| 34 | 판정 오차 표시기: BR `hiterrorvisualizer`(감쇠·EMA 선, `JsonSkin.java:285-305`)·`timingvisualizer`(307-321), QW `judgmentMeter`·`judgmentVisualizer`·레인별 `mainJudgmentMeter`(`Main\JM a ±b`) (확인) | `graph` `play.timing_meter` | △ | 점 감쇠·이동 평균 선·레인별 표시 속성이 없다 |
| 35 | 오차 분포: BR `timingdistributiongraph`(평균·표준편차 선, `JsonSkin.java:323-337`) (확인) | `graph` `result.timing_histogram` | ○ | |
| 36 | 노트: BR `NoteSet`(note·lnstart/end/body/bodyActive/active·hcn* 9종·mine·hidden·processed·레인별 dst·size·group·bpm·stop·time·expansionrate, `JsonSkin.java:339-367`), LR2 `NOTE`·`LN_*`·`MINE` + 각각 `AUTO_` 판(`LV/skin_lr2.h:262-304`), QW `Note\N a b c d`(종류 0 일반/1 머리/2 몸통/3 꼬리/4 지뢰/9 리플레이/14 히든, 상태 0 보통/1 누르는 중/2 실패, `CR_Core.yaml:495-499`) (확인) | `play.lane[*].note.*` | △ | 빠진 변형: 롱노트 **상태**(누르는 중·끊김), HCN, 자동 플레이 판, 판정이 끝난 노트, 숨은 노트, 리플레이(유령) 노트, BPM·정지 표시선. 그리고 **박자에 맞춘 노트 확대**(`expansionrate`: 4분음표 시각 기준, `beatoraja/play/LaneRenderer.java:525-533`) |
| 37 | 마디선: BR `note.group`(`BRS/play7.json:304-322`), LR2 `#SRC_LINE`(레인 높이 기준이 됨, `LV/skin_lr2.cpp:4052-4101`) (확인) | `play.bar_line` | ○ | |
| 38 | 레인 커버: BR `hiddenCover`·`liftCover`(사라지는 선, `JsonSkin.java:380-408`), LR2 LIFT 때 판정선 근처 그림·폭발·키빔을 **휴리스틱으로 골라** 함께 이동(`LV/skin_lr2.cpp:4189-4294, 4434-4517`), IIDX SUD+·HID+·LIFT + 흰 숫자·녹 숫자(`cp0/IIDX_scroll_judge_gauge.md:17, 99-110, 185-217`) (확인) | `play.lane[*].cover` | △ | 커버 3종(위·판정선 위·LIFT) 값, 흰 숫자·녹 숫자 값, "LIFT만큼 기어 요소 전체가 함께 이동"이 없다(BR는 `offset:3` = `OFFSET_LIFT`로 68곳에 붙임, `BRS/play7.json` 집계, `BR/SkinProperty.java:946`) |
| 39 | BGA: BR `bga`(`JsonSkin.java:410-412`), LR2 `#SRC_BGA` + base/layer/poor 3겹 합성(`lunaticvibesf/src/game/graphics/texture_bms_bga.cpp:72-210`), QW `media` + BGA 불투명도, IIDX 영상 + 레이어 애니메이션(`cp0/IIDX_vs_BMS.md:282`) (확인) | `play.bga`(곡 그래픽 자리) | ○ | 3겹 합성은 Track 그래픽·엔진 일. 불투명도는 ★사용자 조정 |
| 40 | 판정 글자 + 콤보 묶음: BR `judge{images[6], numbers[6], shift}`(`JsonSkin.java:429-435`, `BRS/play7.json:332-395`), LR2 매 프레임 콤보 자릿수×½폭만큼 판정 글자를 밀어 둘을 함께 가운데 맞춤(`LV/skin_lr2.cpp:4385-4432`), QW `judgmentPaint` + `band` (확인) | `play.judge.text` + `play.combo` | △ | 자릿수에 따라 두 요소를 **함께 가운데 맞추는 흐름 배치**가 없다(★`group.flow`) |
| 41 | 판정 상세: BR EARLY/LATE·±ms(`BRS/play7.json:589-601`, 옵션 911/912·1242/1243), QW `Main\JM a ±b` (확인) | `play.judge.timing` | ○ | |
| 42 | 키빔: BR 타이머 101~(누른 순간 켜지고 뗄 때 꺼짐) + `loop:100`(`BRS/play7.json:484-515`), LR2 타이머 100~139, QW `Main\M a b` (확인) | `play.lane[*].beam`, `input.press/release` | △ | "누르는 동안 보이고, 누른 순간부터 시간이 흐름" = ★상태 시계 필요. 사건 둘로 흉내는 가능하나 장황 |
| 43 | 폭발: BR 타이머 50~(판정)·70~(롱노트 유지)·250/270(HCN 활성·피해)(`BR/SkinProperty.java:55-82`, `BRS/play7.json:530-583`, `blend:2`), LR2 50~89, QW `hitNotePaint`·`hitLongNotePaint` + 누르는 동안 반복 칸(`QW/CR_Core/CR_Core.lua:294`) (확인) | `play.lane[*].bomb` | △ | **롱노트 유지 중 반복 폭발** 자리가 없다(★`bomb.hold`) |
| 44 | 건반·턴테이블: LR2 `op4`로 턴테이블 각도만큼 회전(LVF 확장, `LV/skin_lr2.cpp:4370-4374`), BR `OFFSET_SCRATCHANGLE_1P/2P`(`BR/SkinProperty.java:944-945`), QW `Input\I a b` (확인) | `play.key[*]` | △ | 턴테이블 회전 각 값이 없다 |
| 45 | 진행 막대·시간: BR slider·number, QW `status`·`statusSlider`·`hms` (확인) | `play.progress`·`play.time` | ○ | |
| 46 | 점수·판정 개수·최대 콤보: BR `NUMBER_*`, QW `stand`·`highest~lowestJudgmentValue`·`highestBand` (확인) | `play.score`·`play.judge_counts` | ○ | |
| 47 | 목표 점수 차: BR `exscore_best`·`rate_rival`(`BR/lua/MainStatePropertyLuaApiExporter.java:54-59`), QW `hunter` (확인) | `play.pacemaker` | ○ | |
| 48 | 배속·템포·녹 숫자: QW `multiplier`·`audioMultiplier`, IIDX HS·녹 숫자(`cp0/IIDX_scroll_judge_gauge.md:99-110`) (확인) | 값 없음 | △ | `settings.hispeed`·`play.rate`·`view.green_number` 값 추가 |
| 49 | 오디오 시각화: QW `audioVisualizer` + 불투명도(`cp0/Qwilight_settings.md:447`) (확인) | `graph` | × | 오디오 스펙트럼 값이 없고, 오디오 스레드 → 화면 데이터 경계가 필요(D1과 함께 판단) |
| 50 | 캐릭터: BR `pmchara`(팝픈 캐릭터 파일, `BR/PomyuCharaLoader.java`, 타이머 900~909) (확인) | `sequence` + 조건 | △ | 상태별 애니메이션은 표현 가능. 캐릭터 파일 형식 자체는 별개 |
| 51 | 온라인: QW `net`(실시간 랭킹), BR IR 타이머 172~174(`BR/SkinProperty.java:131-133`) (확인) | 없음 | × | 범위 밖 |
| 52 | 자동 플레이 표시: QW `autoMain`·`autoInput`, LR2 `AUTO_` 노트, LR2 옵션 32·33 (확인) | 조건 | ○ | 값 `settings.autoplay`만 더하면 됨 |
| 53 | 일시정지·재개 카운트다운·되감기: QW `pause`·`paused*`·`Drawing\PS`, DJMAX `IngamePauseUI`·`IngameRewindUI`(`cp0/DJMAX_class_map.md:57, 101`) (확인) | 없음 | △ | `play`의 하위 상태(일시정지)와 그 자리가 없다 |
| 54 | 연습 모드 UI: BR `practice`(`JsonSkin.java:418-427`) (확인) | 없음 | △ | 범위 밖이지만 자리 형식으로 넣을 수 있음 |
| 55 | 선곡 목록: BR `songlist{center, clickable, listoff[], liston[], text[], level[], lamp[], …}`(`JsonSkin.java:437-451`) — **행마다 따로 위치**(`BRS/select.json:219-` 꺼진 행 20개·켜진 행 20개), 최대 60행·글자 변형 11종(일반/새 곡/폴더/표/단위/명령/검색 등, `beatoraja/select/SkinBar.java:31-93`); LR2 바 32개·바 본체 11종(`lunaticvibesf/src/game/graphics/sprite_bar_entry.h:12-84`)·`#BAR_CENTER`·스크롤 보간(`LV/skin_lr2.cpp:4545-4577`) (확인) | `list` + `select.song_list.item` | △ | ① 행마다 다른 위치(곡선·기울어진 휠) ② 커서 행 위치 ③ 항목 종류별 틀 ④ 스크롤 보간 ⑤ 행 클릭. 우리 `list`는 "틀 하나 + 간격"뿐 |
| 56 | 바의 램프·트로피·라벨·레벨 그림: BR `lamp`·`playerlamp`·`rivallamp`·`trophy`·`label`·`graph`(폴더 램프 분포) (확인) | `item.lamp` 등 | ○ | 값(`item.*`)만 채우면 됨 |
| 57 | 선곡 패널: LR2 타이머 21~36·옵션 20~29, BR `OPTION_PANEL1~3`(기본 선곡 스킨에서 옵션 21~23이 21곳, `BRS/select.json` 집계) (확인) | 없음 | △ | `select`의 하위 상태(패널 열림)와 사건이 없다 |
| 58 | 도움말: BR `TIMER_README_*`, LR2 `#HELPFILE`·`#SRC_README` (확인) | 없음 | △ | 하위 상태 + text |
| 59 | 배너·스테이지 그림·배경 그림: LR2 특수 그림 100 stagefile·101 backbmp·102 banner(`LV/skin_lr2.cpp:1198-1206`), BR `OPTION_NO_BANNER`(192) (확인) | 곡 그래픽 자리 | ○ | `select.banner`·`decide.stagefile` 자리만 추가 |

### 1-3. 배치·애니메이션 (destination)

| # | 선례 개념 (근거) | 우리 도메인 | 판정 | 이유 |
|---|---|---|---|---|
| 60 | 키프레임: BR `Animation{time, x, y, w, h, a, r, g, b, angle}`(`JsonSkin.java:507-529`), LR2 `#DST_*` 인자(`LV/skin_lr2.cpp:455-489`), 첫 줄이 loop·timer·op를 정함(2591-2699) (확인) | 애니메이션(키프레임) | ○ | |
| 61 | 자르기 키프레임: BR `clip_x/y/w/h`(`JsonSkin.java:515-518`, `BR/SkinObject.java:195-261`) (확인) | `group` 잘라내기 | △ | 잘라내기가 키프레임으로 움직이지 않는다 |
| 62 | 보간: BR 0 선형·1 t²·2 1-(1-t)²·3 끊김(`BR/SkinObject.java:556-563, 388`), LR2 0 선형·1 **t³**·2 **1-(1-t)³**·3 끊김(`LV/skin_lr2_animation.h:10-20`) (확인) | 애니메이션 | △ | 보간 종류 목록이 없다. **같은 번호가 두 구현에서 곡선이 다르다**(t² vs t³) — 변환기는 출처별로 옮겨야 함 |
| 63 | 반복: BR `loop` -1 = 끝나면 숨김, N = N ms부터 끝까지 반복, 끝 = 마지막 유지(`BR/SkinObject.java:361-371`), LR2 같은 뜻 (확인) | 애니메이션 | △ | "끝나면 숨김 / 마지막 유지 / 구간 반복" 정의 필요 |
| 64 | 기준점 9종: BR `CENTERX/Y`(`BR/SkinObject.java:80-81`), LR2 숫자 키패드 배치(`LV/skin_lr2.cpp:611-629`), QW `<요소>System` 0/1/2 (확인) | `anchor` | ○ | |
| 65 | 혼합 방식: BR 알파·2 가산·3 감산·4 곱·9 반전(`BR/Skin.java:655-665`), LR2 0~11(가산 2·5·6, 감산 3, 곱 4, 9 배경 반전 곱, 10 색 반전, 11 알파 곱, `LV/skin_lr2.cpp:491-507`), QW `hitNotePaintComposition` 0~12(`CR_Core.yaml:261`) (확인) | `image` 혼합 방식 | △ | 목록이 정해져 있지 않다 |
| 66 | 확대 필터: BR `filter`(선형, `BR/SkinObject.java:42-44`), LR2 `filter` (확인) | 없음 | △ | 요소 속성 하나(가까운 점/선형) |
| 67 | 색 곱하기·회전·투명도 (확인) | 공통 속성 | ○ | |
| 68 | 그리는 순서: BR `destination` 배열 순서, LR2 DST 순서, QW `pipeline`(요소 번호를 그릴 순서로, `CR_Core.yaml:281`, 범례 283-331) (확인) | 층 + `z` | ○ | |

### 1-4. 시간·조건·값

| # | 선례 개념 (근거) | 우리 도메인 | 판정 | 이유 |
|---|---|---|---|---|
| 69 | 타이머 = "상태가 켜진 시각": BR `TIMER_*` 151개 + 확장(키 10~99) + 스킨 정의 10000~19999(`BR/SkinProperty.java:11-176`), **꺼져 있으면 요소를 그리지 않음**(`BR/SkinObject.java:350-356`); LR2 `timer.h` 약 165개(`lunaticvibesf/src/game/runtime/index/timer.h`) (확인) | 사건 + `clock.event` | △ | 우리 사건은 순간뿐. 선례 타이머의 절반은 **구간**(키 누름, 롱노트 유지, 패널 열림, 실패, 게이지 최대)이다 → ★상태 시계 |
| 70 | 박자 시계: BR `TIMER_RHYTHM`(140), LR2 140, QW 추가 그림 애니 방식 1 = 메트로놈(`CR_Core.yaml:1197` 주석), BR 노트 확대가 4분음표 시각 기준(`LaneRenderer.java:527`) (확인) | 없음 | × | ★`clock.beat` 필요(곡 시각에서 계산하므로 결정적) |
| 71 | 옵션 번호 조건: BR `OPTION_*` 287개, `op` 배열 = AND, 음수 = 부정(`JsonSkin.java:463-480`); LR2 `dst_option` case 453개(`LV/skin_lr2_dst.cpp:132-775`), 음수 부정(778-784) (확인) | 조건 식 | ○ | 번호 대신 값을 노출하면 식으로 대체됨(예: 옵션 2 "곡 바" → `item.kind == "song"`) |
| 72 | 식 조건: BR `"draw":"gauge() >= 75"` 같은 Lua 식(`BRS/play7.json:402-411`, `BR/lua/SkinLuaAccessor.java:601-628`) (확인) | 조건 식 | ○ | 작은 식 범위 안 |
| 73 | 값 번호: BR `NUMBER_` 273·`STRING_` 43·`BARGRAPH_` 22·`SLIDER_` 9·`RATE_` 30(`BR/SkinProperty.java` 상수 집계), LR2 숫자 약 290·글자 약 185·막대 약 80·슬라이더 약 56(`lunaticvibesf/src/game/runtime/index/*.h`) (확인) | 값(Value) | △ | 우리 값 목록은 수십 개 수준. 4-0 단계에서 목록을 채워야 함 |
| 74 | 스킨 정의 타이머·사건: BR `customTimers`·`customEvents{action, condition, minInterval}`(`JsonSkin.java:546-556`), Lua `timer_observe_boolean`·`new_passive_timer`(`BR/lua/TimerUtility.java:18-25`), `event_observe_turn_true` 등(`BR/lua/EventUtility.java:17-23`) (확인) | 없음 | × | 상태 있는 사용자 코드. "조건이 참이 되는 순간"만 ★파생 사건으로 덮음 |
| 75 | 엔진 설정이 스킨 요소를 끄고 흐리게: QW `UIPipeline*` 14종 켜기·끄기(`cp0/Qwilight_settings.md:257-270`), 기어·BGA 불투명도(441-444) (확인) | 없음 | △ | ★사용자 조정에 "요소 켜기·끄기·투명도"를 포함 |

**집계**: ○ 31 / △ 35 / × 9 (75개).

### 1-5. IIDX·DJMAX 고정 화면의 요소 (스킨 형식 없음)

| 화면 | IIDX | DJMAX | 우리 자리 | 판정 |
|---|---|---|---|---|
| 플레이 기어 | 레인·노트·키빔·폭발·턴테이블 그림, 판정선, 레인 커버 SUD+/HID+/LIFT(`cp0/IIDX_scroll_judge_gauge.md:185-217`), 커버 텍스처 교체 폴더(`lane_cover\lane0xx`, 같은 문서 94행) (확인) | 기어 컨트롤러가 키빔·판정 이펙트를 맡음(`cp0/DJMAX_class_map.md:55`), 노트 렌더러 풀(56행) (확인) | `play.lane[*].*`, `play.key[*]` | △ (커버 3종·턴테이블 각, 위 38·44행) |
| 판정 | PGREAT~POOR + FAST/SLOW (일반 지식, 추정) | MAX 100%~MAX 1%·BREAK 11단계 + 합산(`cp0/DJMAX_class_map.md:254`) (확인) | `play.judge.text` frame ← 등급 번호 | ○ (등급 수가 규칙마다 다름 → 등급은 정수 번호 + 규칙이 이름표) |
| 콤보·점수 | 콤보, EX 점수, DJ LEVEL 페이스메이커 그래프 (일반 지식, 추정) | 콤보, 점수, 정확도(%) (일반 지식, 추정) | `play.combo`·`play.score`·`play.pacemaker` | ○ |
| 게이지 | 그루브 게이지 2% 칸, 종류별(EASY·HARD·EX-HARD 등)(`cp0/IIDX_scroll_judge_gauge.md:41-49`) (확인) | HP(`cp0/DJMAX_class_map.md:59` `cpl` HP 시스템) (확인), FEVER 게이지·배수 (일반 지식, 추정) | `play.gauge` | △ (FEVER 같은 **규칙 고유 값**이 없음 → ★`rule.*`) |
| 속도 표시 | HS·흰 숫자·녹 숫자(`cp0/IIDX_scroll_judge_gauge.md:17, 99-110`) (확인) | 배속 (일반 지식) | 값 추가 | △ |
| BGA | 영상 1개 + 레이어 애니메이션(`cp0/IIDX_vs_BMS.md:282`) (확인) | 영상 (일반 지식) | `play.bga` | ○ (레이어 애니메이션은 CP3에서 버리기로 함) |
| 일시정지·되감기 | (아케이드라 없음) | `IngamePauseUI`·`IngameRewindUI`, 되감기 벌점 경고(`cp0/DJMAX_class_map.md:57, 101`) (확인) | 없음 | △ (위 53행) |
| 선곡 | 곡 바 휠·폴더·램프·난이도별 레벨·미리듣기(`cp0/IIDX_preview_memory.md:103-`) (확인/추정) | 곡 목록·자켓·미리듣기 (일반 지식) | `select.*` | △ (위 55행의 목록 표현) |
| 결과 | DJ LEVEL·클리어 램프·게이지 그래프·FAST/SLOW 수·기록 비교 (일반 지식, 추정) | 정확도·최대 콤보·판정 수·등급 (일반 지식) | `result.*` | ○ |
| 모드 고유 | — | 팀전 그래프·래더 연습 UI(`cp0/DJMAX_class_map.md:45`) (확인) | 없음 | 범위 밖 |

- **IIDX의 커스터마이즈(프레임·레인 커버·노트·폭발·턴테이블 그림 교체)는 "정해진 자리에 에셋만 바꿔 끼우기"** 다(추정, 일반 지식 + 커버 텍스처 폴더 확인). 우리 자리(Slot) 모델과 같은 방향이다.

---

## 2. 대응 안 되는 개념 (현재 도메인 기준)

| 종류 | 선례 | 근거 | 의견 |
|---|---|---|---|
| **입력 위젯** (버튼·클릭 영역·슬라이더 조작·텍스트 입력·마우스오버·마우스 커서) | BR, LR2 | 위 26~29행. LVF 키 설정 스킨은 `#SRC_BUTTON` 56·`#SRC_SLIDER` 55·`#SRC_TEXT` 127(`cp0/bms/third_party/lunaticvibesf/res/keyconfig/keyconfig.lr2skin` 집계) — **설정 화면까지 스킨으로 그린다** | 우리 `settings` 화면을 스킨으로 그릴지에 달림 ⚑. 선곡 화면의 옵션 패널(BR 버튼 74종)을 따라 할지도 |
| **상태 있는 사용자 타이머·사건** | BR `customTimers`·`customEvents`, Lua 타이머 함수 | 위 74행, `BR/lua/SkinLuaAccessor.java:702-747`("stateful custom timer") | 받지 않음. 자주 쓰는 형태는 엔진이 ★상태·★파생 사건으로 제공 |
| **스크립트 부작용** (파일 읽기·쓰기, HTTP, 소리 재생, 볼륨 바꾸기, Java 클래스 접근) | BR Lua | `BR/lua/SkinFileLuaApiExporter.java:23-70`, `SkinHttpLuaApiExporter.java:32-33`, `SkinAudioLuaApiExporter.java:20-48`, `MainStatePropertyLuaApiExporter.java:66-85`(set_volume), `LegacySkinLuaApi.java:54-57`(luajava) | 받지 않음 — "화면은 읽기만"·판정·소리 불간섭 원칙과 정면 충돌 |
| **무작위 연출** | BR 게이지 깜빡임 `Math.random`, LR2 와일드카드 무작위 그림 | 위 24·5행 | 시드 있는 의사난수(곡·세션 시드)로만 |
| **박자 시계** | BR·LR2 타이머 140, QW 메트로놈 | 위 70행 | ★`clock.beat`로 대응 가능하게 |
| **사용자 위치 조정** | BR offset, QW UIConfigure | 위 6행 | ★사용자 조정으로 대응 가능하게 |
| **오디오 스펙트럼** | QW `audioVisualizer` | 위 49행 | 보류. 하면 오디오 스레드가 화면용 요약을 무잠금으로 게시(D1과 같은 문제) |
| **온라인** (IR·실시간 랭킹) | BR, QW | 위 51행 | 범위 밖 |
| **외부 애니메이션 형식** (팝픈 캐릭터, IIDX 레이어 애니메이션) | BR `pmchara`, IIDX ifs | 위 50행, `cp0/IIDX_vs_BMS.md:282` | 범위 밖(필요하면 sequence로 변환) |
| **마스크** (모양으로 가리기) | 선례에 없음 — BR은 사각 clip뿐(`BR/SkinObject.java:102-109`) | (확인) | 사각 잘라내기면 충분. 넣지 않음 |
| **셰이더 효과** | 선례 스킨 형식에 없음 — BR 셰이더는 엔진 고정 4종(bilinear·ffmpeg·layer·distance_field, `BR/Skin.java:537-557`) | (확인) | 스킨에 열지 않음. 혼합 방식으로 충분 |
| **파티클** | 선례에 없음(폭발은 모두 시트 애니메이션) | (확인: 세 형식에 파티클 요소 없음) | 넣지 않음. 필요하면 나중에 요소 종류 하나로 |
| **3D** | 선례에 없음(DJMAX는 Unity지만 스킨 형식 없음) | (확인/추정) | 넣지 않음 |
| **레거시 휴리스틱** (LR2 LIFT 연동 대상 추정, 해상도 추정) | LVF | `LV/skin_lr2.cpp:4189-4294, 3920-3986` | 도메인이 아니라 변환기 문제 |

---

## 3. C2 — 선언형으로 충분한가

### 3-0. 시안 표기 (설명용, 확정 아님)

- 자리에 요소를 붙인다: `[[slot."play.combo"]]`, 요소 종류는 `type`.
- 값·식은 문자열: `"gauge.percent / 100"`. 식은 **순수**(읽기 전용 값, 사칙·비교·논리·`?:`·`min/max/clamp/floor`·표 찾기·문자열 잇기). 상태·부작용 없음.
- ★ 표시는 6장에서 더하자는 개념이다. ★를 빼고 읽으면 "현재 도메인"으로 적은 것이다.

### 3-1. 시안 A — beatoraja 기본 플레이 7키(`BRS/play7.json`)의 판정·콤보·게이지·키빔·폭발

```toml
[skin]
virtual = [1280, 720]
layout  = "SC K1 K2 K3 K4 K5 K6 K7"          # 레인 레이아웃 서명

[option.side]          # = BR property "Play Side" (op 920/921)
kind = "choice"; items = ["1P", "2P"]; default = "1P"
[option.judge_detail]  # = "Judge Detail" (op 910/911/912)
kind = "choice"; items = ["off", "early_late", "ms"]; default = "off"

[const]                # ★스킨 상수
gear_x = { "1P" = 20, "2P" = 870 }

[lanes]                # ★레인 틀 펼치기: 레인마다 lane.x / lane.w / lane.index / lane.role
x = "const.gear_x[option.side] + layout.x(lane)"

[[slot."play.lane[*].beam"]]          # = keybeam1~7·keybeams, 타이머 100~107, loop 100
type = "image"; asset = "laser.png"; layer = "effect"
x = "lane.x"; w = "lane.w"; y = 140; h = 580
visible = "input.lane[lane].pressed"
clock   = "since(input.lane[lane].pressed)"    # ★상태 시계 (현재 도메인: press/release 두 사건으로 흉내)
anim = [ { t = 0, sx = 1.0 }, { t = 100, sx = 2.0 } ]
end  = "hold"                                  # 100 ms 뒤 마지막 유지 (BR loop:100 의미)

[[slot."play.lane[*].bomb"]]          # = 110~117, 타이머 50~57, blend 2
type = "sequence"; asset = "bomb.png"; frames = 16; fps = 100
start = "judge.hit(lane, grade <= great)"; blend = "add"; end = "hide"

[[slot."play.lane[*].bomb.hold"]]     # ★자리 추가 = 140~147, 타이머 70~77
type = "sequence"; asset = "hold.png"; frames = 8; fps = 60; blend = "add"
visible = "lane_state[lane].holding"; clock = "since(lane_state[lane].holding)"; end = "loop"

[[slot."play.judge.text"]]            # = judge.images[6], 타이머 46, loop -1
type = "image"; asset = "judge.png"; rows = 6
frame = "judge.last.grade"                     # 0 PG … 5 MISS
x = "option.side == '1P' ? 115 : 965"; y = 240; w = 180; h = 40
start = "judge.any"; end = "hide"; anim = [ { t = 0 }, { t = 500 } ]

[[slot."play.combo"]]                 # = judge.numbers[6]
type = "number"; asset = "judgenum.png"; value = "score.combo"
visible = "score.combo >= 2"; start = "judge.any"; end = "hide"; anim = [ { t = 0 }, { t = 500 } ]

[group."play.judge"]                  # = "shift": true — 판정 글자 + 콤보를 함께 가운데
members = ["play.judge.text", "play.combo"]; flow = "row-center"   # ★흐름 배치

[[slot."play.judge.timing"]]          # = judge-early / judge-late (op 911 + 1242/1243)
type = "image"; asset = "judgedetail.png"; rows = 2
when  = "option.judge_detail == 'early_late'"  # ★로드 시 조건
frame = "judge.last.err_ms < 0 ? 0 : 1"; visible = "judge.last.grade != 0"
start = "judge.any"; end = "hide"; anim = [ { t = 0 }, { t = 500 } ]

[[slot."play.gauge"]]                 # = gauge{nodes[8], parts 50}
type = "bar"; parts = 50; value = "gauge.percent / 100"
cells = "gauge.png"                            # ★칸 그림: [켜짐/꺼짐] × [클리어 구간 여부] × 게이지 종류
cell_effect = "flicker"                        # ★엔진 내장 칸 연출 (BR 무작위 → 시드 의사난수)

[[slot."play.background"]]            # = draw "gauge() >= 75" 등 4곳
type = "image"; asset = "bg_high.png"; visible = "gauge.percent >= 75"

[adjust]                              # ★사용자 조정: BR offset 3(LIFT)·4(레인 커버)
"play.lane[*]" = ["lift"]                      # 기어 요소 전체가 LIFT만큼 함께 이동
```

**현재 도메인으로 못 적은 것 (7)**: ① 키빔의 "누르는 동안" 시계 ② 롱노트 유지 폭발 자리 ③ 판정+콤보 함께 가운데 ④ 게이지 칸 그림·칸 연출 ⑤ 옵션으로 배치를 바꾸는 로드 시 조건(1P/2P 좌표는 식으로 되지만 판정 상세 3택은 요소 유무가 바뀜) ⑥ LIFT를 따라 기어 전체가 움직임 ⑦ 레인마다 같은 요소를 펼치기(BR는 Lua·JSON에서 레인 8개를 손으로 또는 반복문으로 적음).
**제안을 더한 뒤 못 적은 것 (1)**: 게이지 무작위 깜빡임을 BR와 **똑같이** 재현하는 것(의도적으로 결정적 의사난수로 바꿈).

### 3-2. 시안 B — Qwilight `CR_Core` 플레이 화면(`QW/CR_Core/`)

Qwilight는 YAML 값 자리에 Lua 함수 호출을 쓰고(`noteLength: NoteLength(100), ~`, `CR_Core.yaml:512`), 옵션으로 그림 세트를 바꿔 끼운다(`_GetNote` 등, `CR_Core.lua:59-79`). 같은 화면을 우리 식으로:

```toml
[option.note_set]       # = configure-1 (노트 모양 세트)
kind = "choice"; items = ["simple", "DX", "DX II", "natural", "Cylinder", "Brick", "popn"]
[option.bomb_set]       # = configure-2 (폭발 세트)
kind = "choice"; items = ["DX", "DX II", "Quantum", "natural", "Birb", "Diamond", "Fancy"]
[option.bga_place]      # = MediaPosition0가 읽는 BGA 배치 옵션
kind = "choice"; items = ["center", "left", "right"]

[table.note_prefix]     # ★표 찾기: _GetNote()가 하던 "옵션 → 그림 접두어"
simple = "N"; DX = "N1"; "DX II" = "N2"; natural = "N3"; Cylinder = "N4"; Brick = "N5"; popn = "N9"
[table.note_frames]     # = NoteFrame()·NoteFramerate()
simple = [1, 60]; DX = [8, 60]; popn = [1, 60]

[lanes]                 # = drawingInputModeN: 키 모드별 레인 → 버튼 그림 번호
x = "adjust.gear_x + layout.x(lane) * adjust.note_width"   # = MainPosition(): 사용자 조정 + 레인 수
w = "adjust.note_width * (lane.role == 'scratch' ? 1.4 : 1.0)"

[[slot."play.lane[*].note.tap"]]
type = "sequence"
asset  = "'Note/' + table.note_prefix[option.note_set] + ' ' + lane.art + ' {frame} 0.png'"  # ★에셋 경로 식
frames = "table.note_frames[option.note_set][0]"; fps = "table.note_frames[option.note_set][1]"; end = "loop"
h = "option.note_set == 'Brick' ? 30 : 16"          # = NoteHeight(e)

[[slot."play.lane[*].note.long.body"]]
type = "image"; stretch = "tile-y"
asset = "'Note/' + table.note_prefix[option.note_set] + ' ' + lane.art + ' 0 2 ' + (note.holding ? 1 : (note.broken ? 2 : 0)) + '.png'"
                                                     # ★노트 상태(누르는 중·끊김) 값

[[slot."play.lane[*].bomb"]]
type = "sequence"; asset = "'Paint/N ' + lane.art + ' {frame}.png'"; frames = 20; fps = 60
start = "judge.hit(lane)"; blend = "add"; end = "hide"

[[slot."play.lane[*].bomb.hold"]]                   # ★ = hitLongNotePaint, 누르는 동안 앞부분 반복
type = "sequence"; frames = 30; fps = 60; loop_until_frame = 12
visible = "lane_state[lane].holding"; clock = "since(lane_state[lane].holding)"

[[slot."play.combo"]]                                # = band + enlargeBand
type = "number"; asset = "Bin/C"; value = "score.combo"
start = "score.combo_up"                             # ★사건 추가
anim = [ { t = 0, s = 1.15 }, { t = 80, s = 1.0 } ]

[[slot."play.lane[*].timing"]]                       # ★자리 추가 = mainJudgmentMeter (Main\JM a ±b)
type = "image"; rows = 7; frame = "clamp(round(judge.last_on(lane).err_ms / 10), -3, 3) + 3"
start = "judge.hit(lane)"; end = "hide"; anim = [ { t = 0 }, { t = 300 } ]

[[slot."play.bga"]]
x = "option.bga_place == 'left' ? 0 : (option.bga_place == 'right' ? 640 : 320)"   # = MediaPosition0
opacity = "adjust.bga_opacity"                       # ★사용자 조정 = MediaFaintV2

[[slot."play.overlay.metronome"]]                    # = paintProperty 애니 방식 1 (메트로놈)
type = "sequence"; frames = 4; clock = "clock.beat"  # ★박자 시계
```

**현재 도메인으로 못 적은 것 (6)**: ① 사용자 조정값(기어 위치·노트 너비·BGA 불투명도) ② 롱노트 상태별 그림 ③ 롱노트 유지 반복 폭발 ④ 콤보가 늘 때 사건 ⑤ 레인별 오차 표시 자리 ⑥ 메트로놈(박자 시계). 오디오 시각화(`audioVisualizer`)는 시안에서 뺐다(값 없음, 위 49행).
**제안을 더한 뒤 못 적은 것 (0)** — 단 Qwilight가 노트마다 Lua 콜백을 부르는 방식(`_GetNote(args)`, `QW/Default/Default.lua:381`)은 **표 찾기 + 에셋 경로 식**으로 바꿔 적었다. Qwilight Lua 함수 대부분(`NoteLength`, `NoteHeight`, `MainPosition`, `MediaPosition0`, `DrawingInputMode`)은 옵션·키 수·사용자 조정값으로 숫자나 문자열을 고르는 **순수 계산**이다(확인, 함수 목록 `CR_Core.lua:49-381`). `DrawingInputMode`(101-260행, 160줄)는 옵션 조합으로 레인 색 배열을 문자열로 이어 붙이는데, 표 찾기로 옮기면 길어지지만 적을 수는 있다(추정: 옵션 조합 수만큼 표 행).

### 3-3. 시안 C — beatoraja 기본 선곡 화면(`BRS/select.json`, 876줄)

```toml
[[slot."select.song_list"]]           # = songlist{center 10, listoff[20], liston[20]}
type = "list"; rows = 20; cursor_row = 10
row_pos     = "[800, 720 - 36 * row]"           # ★행 위치 식 (BR는 행마다 dst를 따로 적음)
row_pos_sel = "[780, 720 - 36 * row]"           # 커서 행은 20 px 왼쪽 (liston)
scroll = { ease = "out", ms = 100 }             # ★스크롤 보간 (= TIMER_SONGBAR_MOVE, LR2 4545-4577)

[select.song_list.item]               # 항목 틀 (group)
template.when = "item.kind"                     # ★항목 종류별 틀: song·folder·table·grade·command·search·new
[[select.song_list.item.song]]
type = "image"; asset = "bar.png"; rows = 11; frame = "item.bar_style"   # 바 본체 11종
[[select.song_list.item.song]]
type = "text"; font = "main"; size = 24; value = "item.title"; overflow = "shrink"
color = "item.is_new ? '#ffd040' : '#ffffff'"   # BARTEXT_SONG_NEW
[[select.song_list.item.song]]
type = "number"; asset = "level.png"; value = "item.level"
frame_set = "item.difficulty"                   # 레벨 그림 7색
[[select.song_list.item.song]]
type = "image"; asset = "lamp.png"; rows = 11; frame = "item.lamp"; x = 0; y = 19
[[select.song_list.item.folder]]
type = "graph"; source = "item.lamp_distribution"; x = 15; w = 450; h = 8   # = songlist.graph

[[slot."select.density_graph"]]       # = judgegraph type 0
type = "graph"; source = "chart.density_250ms"

[[slot."select.bpm_graph"]]           # = bpmgraph, op 177(변속 있음)/1177(정지 있음)
type = "graph"; source = "chart.bpm_series"     # ★값 추가
visible = "chart.has_bpm_change || chart.has_stop"

[[slot."select.panel[1]"]]            # = OPTION_PANEL1 (21곳에서 쓰임)
type = "group"
visible = "select.panel == 1"; clock = "since(select.panel == 1)"     # ★하위 상태 + 상태 시계
anim = [ { t = 0, y = -200 }, { t = 150, y = 0 } ]

[[slot."select.stats"]]
type = "number"; value = "chart.notes_total"; visible = "item.kind == 'song'"   # = op 2 (곡 바)

[[slot."select.chart.level_lamp"]]    # = op [2, 200~207] (곡 바 + 클리어 램프별)
type = "image"; rows = 8; frame = "item.lamp"; visible = "item.kind == 'song'"

# 적을 수 없음: 정렬·모드·LN 모드·리플레이 버튼(act 10곳), 검색 입력창, 행 클릭
```

**현재 도메인으로 못 적은 것 (4)**: ① 행마다 다른 위치·커서 행 ② 항목 종류별 틀 ③ 스크롤 보간 ④ 패널 열림 하위 상태·BPM 그래프 값. 여기에 아래 셋을 더해 계 **7**.
**제안을 더한 뒤에도 못 적은 것 (2 종류)**: ⑤ 버튼(`act` 10곳: 정렬·모드·LN 모드·리플레이 1~4 등, `BRS/select.json:58-64, 99-102, 152`) ⑥ 검색 텍스트 입력 (⑦ 행 클릭은 ⑤와 같은 종류).

### 3-4. 세 시안의 집계

| 시안 | 현재 도메인으로 못 적은 것 | 6장 제안을 더한 뒤 | 남는 것의 종류 |
|---|---|---|---|
| A beatoraja 플레이 | 7 | 1 | 무작위 연출(의도적 차이) |
| B Qwilight 플레이 | 6 | 0 | — |
| C beatoraja 선곡 | 7 | 2(+행 클릭) | 입력 위젯 |
| **계** | **20 (중복 빼면 18)** | **3** | |

### 3-5. Lua 스킨은 Lua로 무엇을 하나 (beatoraja 근거)

| 하는 일 | 근거 | 우리 대응 |
|---|---|---|
| **① 로드할 때 표 만들기**: Lua 파일을 한 번 실행해 JSON과 **같은 구조의 표**를 돌려받는다 | `BR/lua/LuaSkinLoader.java:69-93`(`exportSkinProperty` → `execFile` → `fromLuaValue(JsonSkin.Skin)`), 진입 파일은 `require` 후 `main()` 또는 `header`만 반환(`BRS/play/play7.luaskin` 6줄) (확인) | ★레인 틀 펼치기 + ★로드 시 조건 + 식 |
| — 그 안의 계산: 레인 반복(`for i = 1, 8`), 옵션 분기(1P/2P·스크래치 위치·점수 그래프·판정 상세), 레인 → 타이머 번호 도우미, 판정 개수 묶음을 만드는 도우미 함수 | `BRS/play/play7main.lua:31-95`(옵션·타이머 도우미), 149·186·223(분기), 259·276·407-438·530·663-703(레인 반복); `BRS/play_parts.lua:19, 38` (확인) | 같음 |
| **② 실행 중 식**: 속성 값 자리에 문자열을 쓰면 `return <식>`으로 컴파일해 **매 프레임 호출** | `BR/lua/SkinLuaAccessor.java:601-700`, 읽기 API `option`·`number`·`text`·`timer`·`time`·`judge`·`gauge`·`rate`·`exscore` 등(`MainStatePropertyLuaApiExporter.java:33-87`) (확인) | 작은 순수 식 |
| — 실제 사용량: 기본 스킨 전체에서 `"draw"` 식 4곳(게이지 구간별 배경), **기본 Lua 스킨 6개에 실행 중 함수 0개** | `BRS/play7.json:402-411`; `main_state`·`function()`·`timer_util` grep 0건(`BRS/**/*.lua`) (확인) | 같음 |
| **③ 상태·부작용**: 상태 있는 타이머 함수, 조건이 바뀌는 순간 사건, 타이머 켜기·사건 실행, 볼륨 바꾸기, 소리 재생, 파일·HTTP | `SkinLuaAccessor.java:702-747`, `TimerUtility.java:18-25`, `EventUtility.java:17-23`, `MainStatePropertyLuaApiExporter.java:46-47, 66-85`, `SkinAudio…:20-48`, `SkinFile…:23-70`, `SkinHttp…:32-33` (확인) | 받지 않음. 상태 시계·파생 사건만 엔진이 제공 |

Qwilight의 Lua(`QW/*/*.lua`)는 ②에 가깝다: YAML 값 자리에서 부르는 함수가 옵션·키 수·사용자 조정값으로 숫자·문자열을 고른다(확인, 위 3-2). 노트마다 인자를 넘겨 부르는 콜백(`_GetNote(args)`)이 실행 중에 몇 번 불리는지는 확인하지 못했다(추정: 노트 그림을 고를 때마다).

**결론**: 선례 Lua가 하는 일의 대부분은 ①(로드 시 생성)과 ②(읽기만 하는 식)이고, 둘 다 "선언형 + 펼치기 + 작은 식"으로 덮인다. ③은 우리 원칙(화면은 읽기만, 판정·소리 불간섭, 결정성)과 어긋나므로 받지 않는다.

---

## 4. C3 — 기존 스킨 불러오기의 가치

### 4-1. 스킨 하나씩 옮겨 볼 때의 손실

| 스킨 | 규모 (집계) | 손실 (현재 도메인) | 손실 (6장 제안 후) |
|---|---|---|---|
| **beatoraja 기본 플레이 7키** `BRS/play7.json`(1062줄) | `timer` 110곳(판정 46이 20곳, 폭발 50~57, 유지 70~77, 키 100~107, 실패 3, READY 40), `op` 99곳(스킨 옵션 900·901·906이 대부분, 레인 커버 변경 중 270, 현재 판정 241, 빠름·늦음 1242/1243), `offset` 72곳(68곳 = LIFT 3, 4곳 = 레인 커버 4), `blend` 36곳(전부 2 = 가산), `if` 24곳(1P/2P), `draw` 식 4곳, `acc` 0곳 (확인, grep 집계) | 상태 시계(키빔·유지 폭발·실패), LIFT 연동 72곳, 판정+콤보 정렬, 게이지 칸 연출, 로드 시 조건 24곳 | 게이지 무작위 깜빡임의 정확한 모양만 다름 |
| **LR2** — 저장소·설치본에 플레이 스킨이 없어 LVF 동봉 키 설정 스킨 `lunaticvibesf/res/keyconfig/keyconfig.lr2skin`(849줄)으로 봄 | `#SRC_TEXT` 127, `#SRC_IMAGE` 62, `#SRC_BUTTON` 56, `#SRC_SLIDER` 55, `#SRC_BARGRAPH` 51, 마우스 커서 1 (확인, grep 집계) | **버튼·슬라이더·커서 112개(약 3분의 1)는 옮길 곳이 없다**. 설정 화면 스킨이기 때문 | 같음 (입력 위젯을 넣지 않는 한) |
| LR2 플레이 스킨 일반 (로더 근거, 실물 없음) | 타이머 약 165·옵션 case 453·숫자 약 290·글자 약 185 번호 체계(`LV/`·`runtime/index/*.h`) (확인) | 번호 → 이름 표, 보간 곡선 차이(t³), 혼합 5·6 모호, LIFT 휴리스틱, 해상도 추정, Shift-JIS 글꼴, 와일드카드 무작위 | 번호 표가 완성되면 그리기 쪽은 대부분 옮겨짐 (추정) |
| **Qwilight `CR_Core`** (`QW/CR_Core/`: YAML 1208줄, Lua 1159줄, zip 안 파일 3411개) | 스킨 옵션 16칸 중 다수 사용, 값 자리 대부분이 Lua 호출, 요소 번호 0~120의 `pipeline` 범례(`CR_Core.yaml:283-331`) (확인) | 사용자 조정 연동, 롱노트 상태 그림, 유지 폭발, 콤보 증가 사건, 레인별 오차, 메트로놈, 오디오 시각화, 실시간 랭킹 | 오디오 시각화·실시간 랭킹 |

### 4-2. 변환기 난이도와 우선순위 의견

| 출처 | 난이도 | 이유 | 가치 |
|---|---|---|---|
| **beatoraja JSON** | 중 | 이름 붙은 필드라 구조가 우리와 가장 가깝다. 할 일: 느슨한 JSON(끝 쉼표) 읽기, `if`/`include` 펼치기, 번호 표(타이머 151·옵션 287·숫자 273·글자 43 등)를 우리 이름으로 옮기기 — 크지만 기계적. **Lua 스킨은 변환할 때 Lua를 한 번 실행해 표를 얻으면 된다**(기본 Lua 스킨은 실행 중 함수가 없으므로, 확인). 실행 중 Lua 식은 번역하거나 버림 | 중: 기본 스킨·소스가 저장소에 있어 **도메인 검증 도구**로 쓰기 좋다 |
| **LR2 CSV** | 상 | 번호 체계가 전부이고, 문서화 안 된 동작이 많다(LVF가 "블랙박스 시험"으로 맞춘 LIFT 휴리스틱, `LV/skin_lr2.cpp:4189-4191`). 보간 곡선이 beatoraja와 다르다. 설정 화면 스킨은 입력 위젯이 많다. beatoraja에도 LR2 → 자기 객체 로더가 있어(`BR/lr2/LR2*Loader.java` 10개) 대응표 참고는 쉽다 | 높음(추정): 스킨 수가 가장 많은 형식(일반 지식). 단 그림 해상도가 낮은 옛 스킨이 많음 |
| **Qwilight** | 상 | YAML이 Lua 없이는 값이 정해지지 않는다. 옵션 조합마다 Lua를 돌려 정적 배치를 뽑아야 하고(옵션 16칸 → 조합 폭발), 엔진이 주입하는 사용자 조정값·내장 함수(`Wall`, `Contents`)를 흉내 내야 한다 | 사용자가 Qwilight 스킨을 쓰고 있다면 높음. 그 외엔 낮음 |

**의견**:
1. 변환기는 **CP4 본 범위에 넣지 않는다**. 사용자의 목표는 "자기 에셋을 자리마다 연결"이고, 변환기는 그 목표의 지름길이 아니라 별도 호환 기능이다.
2. 대신 **4-0(자리·값 목록 확정) 직후 beatoraja 기본 스킨 두 개(play7·select)를 손으로 우리 형식으로 옮겨** 도메인 검증 시험으로 쓴다. 이 문서 3-1·3-3의 연장이다. 비용이 작고 빠진 값·사건이 드러난다.
3. 변환기를 만든다면 순서는 **beatoraja JSON(+ 변환 때 Lua 실행) → LR2 → Qwilight**. LR2는 beatoraja의 LR2 로더를 거치는 대응표를 쓰면 번호 표를 한 벌로 모을 수 있다(추정).
4. 무엇을 하든 변환 결과는 **결정적**이어야 한다: 와일드카드·무작위 연출은 시드 고정 의사난수로 바꾸고, 바꾼 사실을 변환 보고에 적는다.

---

## 5. 스킨 표현 방식 추천 — ⚑2(식 언어 범위)에 대한 근거

| 방식 | 선례에서 실제로 필요한 정도 | 우리 원칙과의 관계 | 판단 |
|---|---|---|---|
| 선언형만 (값 바인딩, 식 없음) | 부족: 1P/2P 좌표, 게이지 구간별 배경(`draw` 식 4곳), Qwilight의 옵션·키 수 계산 대부분이 식 | — | 부족 |
| **선언형 + 작은 순수 식 + 로드 시 펼치기** | 3장 시안에서 위젯·무작위를 뺀 전부를 적음. beatoraja 기본 Lua 스킨의 Lua 사용(로드 시 생성)도 덮음 | 읽기만·결정적·검사 가능(로드할 때 이름·타입 확인), 프레임마다 정해진 비용 | **추천** |
| 스크립트(Lua) | 필요한 것: 상태 있는 사용자 타이머, 부작용(파일·HTTP·소리·볼륨). 기본 스킨들은 이것을 쓰지 않음(확인) | 화면이 판정·소리에 끼어들 통로가 생김, 결정성·안전 검토 부담, 화면 스레드에서 프레임마다 인터프리터 호출(비용은 추정) | 넣지 않음 (나중에 필요가 확인되면 "로드 시 생성 전용" Lua만 검토) |

식 범위 제안: 읽기 전용 값·옵션·상수, 사칙·비교·논리·`?:`, `min/max/clamp/floor/round/abs`, 표 찾기(`table.x[key]`), 문자열 잇기(에셋 경로용). 변수 대입·반복·사용자 함수 없음. 반복은 엔진의 펼치기(레인 틀·목록 행)만.

---

## 6. 우리 도메인에 더할 것 (제안)

### 6-1. 시간·사건·상태

| 제안 | 무엇 | 근거 |
|---|---|---|
| **상태(State)와 `since(상태)` 시계** | "켜져 있는 동안 보이고, 켜진 순간부터 흐르는" 시계. 키 누름, 롱노트 유지, 패널 열림, 실패, 게이지 최대, 일시정지 | 1-4 69행, 3-1 키빔·유지 폭발 |
| **`clock.beat`** (박자·마디 위상) | 곡 시각에서 계산 → 결정적. 메트로놈 연출, 박자 맞춘 노트 확대 | 1-4 70행 |
| **파생 사건 `rise(조건)`·`fall(조건)`** | 조건이 참/거짓이 되는 순간을 엔진이 사건으로 | BR `event_observe_turn_true` 대체(1-4 74행) |
| 사건 추가 | `score.combo_up`, `gauge.max_reached`·`gauge.increase`, `play.ready`·`play.start`, `chart.last_note_passed`, `score.rank_reached(A/AA/AAA/최고/목표)`, `lane.hold_start/end`, `select.list_move/stop`, `select.panel_open/close`, `result.graph_done` | BR 타이머 40~48·143·348~352·10~14·21~36·150~152 (`BR/SkinProperty.java:11-133`) |
| 애니메이션 속성 | 보간 목록(linear·ease_in(지수)·ease_out(지수)·step), 끝 동작(hide·hold·loop_from(ms)), 잘라내기 키프레임 | 1-3 61~63행 |

### 6-2. 요소·자리

| 제안 | 무엇 | 근거 |
|---|---|---|
| 혼합 방식 목록 | alpha·add·subtract·multiply·screen·invert (선례 합집합에서 쓰이는 것) | 1-3 65행 |
| 공통 속성 | 확대 필터(nearest/linear), `stretch`를 image에도(자름 포함) | 1-2 21행, 1-3 66행 |
| `number` 속성 | 음수 기호·앞 0 전용 칸, 자리 간격 | 1-2 17행 |
| `bar` 속성 | 칸별 그림(켜짐/꺼짐 × 구간 × 종류), 칸 연출(엔진 내장, 시드 의사난수) | 1-2 24행 |
| `graph` 속성 | 점 감쇠·이동 평균 선(오차 표시기) | 1-2 34행 |
| **`group.flow`** | 자식들을 크기에 따라 줄 맞춤(판정+콤보 함께 가운데) | 1-2 40행 |
| **`list` 속성** | 행 위치 식(행마다 다른 위치), 커서 행, 스크롤 보간, 항목 종류별 틀(`item.kind`) | 1-2 55행 |
| 자리 추가 (플레이) | `play.lane[*].bomb.hold`, 노트 변형(`long.*.active/broken`, HCN, `auto`, `processed`, `hidden`, `ghost`), `play.bpm_line`·`play.stop_line`, `play.lane[*].timing`, 커버 3종(`play.cover.sudden/hidden/lift`)과 흰·녹 숫자, `play.pause`·`play.countdown`, `play.autoplay` | 1-2 36·38·43·44·53행 |
| 자리 추가 (선곡·결정) | `select.banner`, `decide.stagefile`, `select.panel[*]`, `select.help`, `select.bpm_graph` | 1-2 57~59행, 3-3 |
| 플레이어 측 | 자리 이름에 `[p1]`/`[p2]` (DP·배틀) | 1-1 13행 |

### 6-3. 값

`input.lane[*].angle`(턴테이블), `settings.hispeed`·`play.rate`·`view.green_number`·`view.white_number`·`settings.cover.{sudden,hidden,lift}`·`settings.autoplay`, `chart.bpm_series`·`chart.has_bpm_change`·`chart.has_stop`, `score.series`(그래프용 시계열), `judge.last.{grade, err_ms, lane}`·`judge.last_on(lane)`, `lane_state[*].holding`, `note.holding/broken`(노트 그리기 안), `item.kind`·`item.is_new`·`item.lamp_distribution`, `select.panel`, **`rule.*`**(규칙 프리셋 고유 값: FEVER 단계 등 — DJMAX), 판정 등급은 정수 번호 + 규칙이 이름표.

### 6-4. 스킨 묶음

| 제안 | 무엇 | 근거 |
|---|---|---|
| **사용자 조정(Adjust)** | 엔진이 정한 조정 항목(기어 위치, 판정 위치, 콤보 위치, 노트 너비·두께, LIFT, 요소 켜기·끄기, 불투명도)을 스킨 요소·묶음에 연결. 값은 스킨별 저장 | BR offset(`BR/SkinProperty.java:944-972`), QW UIConfigure·UIPipeline(`cp0/Qwilight_settings.md:257-270, 436-455`) |
| **레인 틀 펼치기** | `play.lane[*]`에 붙인 요소를 레인마다 펼침. `lane.index/role/x/w/art` 제공 | BR Lua 레인 반복, QW `drawingInputModeN` |
| **로드 시 조건 `when`** | 옵션으로 요소 자체를 넣고 뺌(실행 중 `visible`과 구분). 파일 포함(`include`) | 1-1 8행 |
| 스킨 옵션 종류 | 고르기·켜기·숫자 + **파일 고르기**(와일드카드 대신, 무작위 금지 또는 시드) + 묶음(탭) | 1-1 5·7행 |
| 스킨 상수 | 색·크기·좌표 이름 붙이기(테마 토큰) | 1-1 11행 |
| 비트맵 글꼴 | 글꼴 에셋 종류에 "글자 그림표" | 1-1 10행 |
| 화면 시간 상수 | 입력 받기 시작, 페이드 길이 (연출 쪽만; 곡 시작 시각은 엔진) | 1-1 3행 |
| 결정성 규칙 | 스킨 연출의 무작위는 엔진이 주는 시드 의사난수만 | 2장 |

### 6-5. ⚑ 사용자 판단이 필요한 것

1. **입력 위젯**(버튼·슬라이더·텍스트 입력)을 스킨 요소로 넣을까 — `settings` 화면과 선곡 화면 옵션 패널을 스킨으로 그릴지와 같은 질문.
2. 오디오 시각화를 넣을까(오디오 → 화면 데이터 경계 필요).
3. 변환기를 언제 할까(이 문서 의견: CP4 본 범위 밖, 4-0 뒤 손 변환 검증만).
