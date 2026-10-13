//! CP1-2 준비: ASIO 드라이버 목록과 정보 조회 (소리를 내지 않음). 직접 적은 IASIO 함수표가 맞는지 검증한다.
//!
//!   testbench asioprobe [--driver TOPPING]

use synchronizer::asio::{self, error_name, Driver};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

use crate::arg;

fn type_name(t: i32) -> &'static str {
    match t {
        asio::ASIOST_INT16_LSB => "Int16LSB",
        asio::ASIOST_INT24_LSB => "Int24LSB",
        asio::ASIOST_INT32_LSB => "Int32LSB",
        asio::ASIOST_FLOAT32_LSB => "Float32LSB",
        _ => "기타",
    }
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let list = asio::drivers();
    println!("등록된 ASIO 드라이버 {}개:", list.len());
    for d in &list {
        println!("  {}  {:?}", d.name, d.clsid);
    }
    let want = arg(args, "--driver", "TOPPING");
    let entry = list.iter().find(|d| d.name.contains(&want)).ok_or(format!("'{want}' 드라이버 없음"))?;
    let drv = Driver::open(entry)?;
    let e = |x: i32| format!("오류 {x} ({})", error_name(x));
    println!("\n[{}]", entry.name);
    println!("  드라이버 이름: {}  버전: {}", drv.name(), drv.version());
    match drv.channels() {
        Ok((i, o)) => println!("  채널: 입력 {i}, 출력 {o}"),
        Err(x) => println!("  채널: {}", e(x)),
    }
    match drv.buffer_size() {
        Ok((a, b, c, g)) => println!("  버퍼: 최소 {a}, 최대 {b}, 선호 {c}, 간격 {g}"),
        Err(x) => println!("  버퍼: {}", e(x)),
    }
    match drv.sample_rate() {
        Ok(r) => println!("  샘플레이트: {r} (44100 가능 {}, 48000 가능 {})", drv.can_sample_rate(44100.0), drv.can_sample_rate(48000.0)),
        Err(x) => println!("  샘플레이트: {}", e(x)),
    }
    match drv.latencies() {
        Ok((i, o)) => println!("  보고 지연: 입력 {i} 샘플, 출력 {o} 샘플"),
        Err(x) => println!("  보고 지연: {}", e(x)),
    }
    match drv.sample_position() {
        Ok((s, t)) => println!("  재생 위치 (시작 전): {s} 샘플, 시각 {t} ns"),
        Err(x) => println!("  재생 위치 (시작 전): {}", e(x)),
    }
    if let Ok((ni, no)) = drv.channels() {
        for (is_in, n, label) in [(true, ni, "입력"), (false, no, "출력")] {
            for c in 0..n {
                match drv.channel_info(c, is_in) {
                    Ok((name, t, active)) => println!("  {label} {c:2}: {name:<24} {} {}", type_name(t), if active { "사용 중" } else { "" }),
                    Err(x) => println!("  {label} {c:2}: {}", e(x)),
                }
            }
        }
    }
    Ok(())
}
