//! 작성 형식의 수: 문자열 유리수(`"3"`, `"1.5"`, `"7/3"`, `"-1/3"`)를 정확히 읽고, ns로 한 번 짝수 반올림한다 (명세 2-1, 3-1).

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

pub type Q = BigRational;

pub fn int(n: i64) -> Q {
    Q::from_integer(BigInt::from(n))
}

pub fn frac(n: i64, d: i64) -> Q {
    Q::new(BigInt::from(n), BigInt::from(d))
}

/// 문자열 유리수. 정수, 소수(`1.5`, `-0.25`), 분수(`7/3`). 지수 표기·공백 안쪽·빈 문자열은 거부.
pub fn parse(s: &str) -> Result<Q, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(format!("빈 수: {s:?}"));
    }
    if let Some((a, b)) = t.split_once('/') {
        let n: BigInt = parse_dec_int(a)?;
        let d: BigInt = parse_dec_int(b)?;
        if d.is_zero() {
            return Err(format!("분모 0: {s:?}"));
        }
        return Ok(Q::new(n, d));
    }
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (ip, fp) = body.split_once('.').unwrap_or((body, ""));
    if ip.is_empty() && fp.is_empty() || !ip.chars().all(|c| c.is_ascii_digit()) || !fp.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("유리수가 아님: {s:?}"));
    }
    let digits = format!("{}{}", if ip.is_empty() { "0" } else { ip }, fp);
    let n: BigInt = digits.parse().map_err(|_| format!("유리수가 아님: {s:?}"))?;
    let d = num_traits::pow(BigInt::from(10), fp.len());
    let q = Q::new(n, d);
    Ok(if neg { -q } else { q })
}

fn parse_dec_int(s: &str) -> Result<BigInt, String> {
    let t = s.trim();
    let body = t.strip_prefix('-').unwrap_or(t);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("정수가 아님: {s:?}"));
    }
    t.parse().map_err(|_| format!("정수가 아님: {s:?}"))
}

/// 짝수 쪽 반올림 (round half to even). 결과가 i64 밖이면 Err.
pub fn round_half_even(x: &Q) -> Result<i64, String> {
    let (q, r) = x.numer().div_mod_floor(x.denom()); // r ∈ [0, denom)
    let twice = &r * 2;
    let d = x.denom();
    let mut n = q;
    if twice > *d || (twice == *d && n.is_odd()) {
        n += BigInt::one();
    }
    n.to_i64().ok_or_else(|| format!("i64 범위 밖: {x}"))
}

/// 초(유리수) → ns (한 번 짝수 반올림).
pub fn sec_to_ns(sec: &Q) -> Result<i64, String> {
    round_half_even(&(sec * int(1_000_000_000)))
}

/// 가장 가까운 f64 (IEEE 짝수 반올림에 가깝게: 분자·분모를 f64로 나누되 큰 수는 비트 정렬 후 나눗셈).
pub fn to_f64(x: &Q) -> f64 {
    // 분자·분모가 f64로 정확하면 한 번의 나눗셈이 정확한 반올림이다.
    if let (Some(n), Some(d)) = (x.numer().to_i64(), x.denom().to_i64()) {
        if n.unsigned_abs() < (1u64 << 53) && (d as u64) < (1u64 << 53) {
            return n as f64 / d as f64;
        }
    }
    // 큰 수: 120비트 정밀도로 줄인 뒤 나눈다 (오차는 f64 반올림보다 훨씬 작음)
    let shift = x.numer().bits().max(x.denom().bits()) as i64 - 120;
    let (n, d) = if shift > 0 {
        (x.numer() >> shift as usize, x.denom() >> shift as usize)
    } else {
        (x.numer().clone(), x.denom().clone())
    };
    n.to_f64().unwrap_or(f64::NAN) / d.to_f64().unwrap_or(f64::NAN)
}

pub fn to_f32(x: &Q) -> f32 {
    to_f64(x) as f32
}

pub fn is_neg(x: &Q) -> bool {
    x.is_negative()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        assert_eq!(parse("3").unwrap(), int(3));
        assert_eq!(parse("1.5").unwrap(), frac(3, 2));
        assert_eq!(parse("-0.25").unwrap(), frac(-1, 4));
        assert_eq!(parse("7/3").unwrap(), frac(7, 3));
        assert_eq!(parse("-1/3").unwrap(), frac(-1, 3));
        assert_eq!(parse("13000/3").unwrap(), frac(13000, 3));
        assert!(parse("1e3").is_err());
        assert!(parse("").is_err());
        assert!(parse("1/0").is_err());
        assert!(parse("abc").is_err());
    }

    #[test]
    fn half_even() {
        assert_eq!(round_half_even(&frac(1, 2)).unwrap(), 0);
        assert_eq!(round_half_even(&frac(3, 2)).unwrap(), 2);
        assert_eq!(round_half_even(&frac(5, 2)).unwrap(), 2);
        assert_eq!(round_half_even(&frac(-1, 2)).unwrap(), 0);
        assert_eq!(round_half_even(&frac(-3, 2)).unwrap(), -2);
        assert_eq!(round_half_even(&frac(-5, 2)).unwrap(), -2);
        assert_eq!(round_half_even(&frac(7, 3)).unwrap(), 2);
        assert_eq!(round_half_even(&frac(-7, 3)).unwrap(), -2);
        assert_eq!(sec_to_ns(&frac(13, 3)).unwrap(), 4_333_333_333);
    }
}
