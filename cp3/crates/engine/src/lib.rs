//! CP3 엔진 코어: Track 묶음을 불러 실행 데이터로 만들고(load), 섞고(mixer), 판정한다(judge).
//! CP2 rcore에서 갈라져 나왔다 — 여러 레인, 차분 안 소리 표, 소리 형식 i16/f32, 끊기·choke, 변형 선택.

pub mod calib;
pub mod judge;
pub mod load;
pub mod mixer;
pub mod preview;
pub mod resample;
pub mod ring;
pub mod runtime;
