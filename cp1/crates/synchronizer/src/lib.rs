//! 싱크로나이저: 엔진에서 시계를 읽는 유일한 층.
//! 지금 있는 것: QPC, WASAPI 장치 열기, 시계 부품(장치 위치 ↔ QPC 모델), 예약기. 입력 도장은 다음 단계.

pub mod clock;
pub mod qpc;
pub mod scheduler;
pub mod wasapi;
