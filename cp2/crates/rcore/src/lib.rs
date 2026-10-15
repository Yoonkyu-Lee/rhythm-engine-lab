//! CP2 코어. 실행 데이터(컴파일된 채보 + 소리 뱅크)와, 그것을 읽는 처리(믹서·판정 등)를 담는다.
//!
//! 코어는 기존 게임 형식(BMS 채널·소절·WAV ID 등)을 모른다. 기존 곡은 코어 밖 어댑터가 실행 데이터로 바꿔 넣는다.

pub mod mixer;
pub mod resample;
pub mod ring;
pub mod rtc;
pub mod runtime;
