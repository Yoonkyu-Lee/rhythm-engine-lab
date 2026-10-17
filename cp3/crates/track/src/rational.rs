//! 작성 형식의 수: 문자열 유리수(`"3"`, `"1.5"`, `"7/3"`, `"-1/3"`)를 정확히 읽고, ns로 한 번 짝수 반올림한다 (명세 2-1, 3-1).

use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};

#[allow(unused_imports)]
use num_integer::Integer as _;

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

/// 유리수 → IEEE 이진 부동소수점 비트, 한 번의 짝수 반올림 (리뷰 3-1 #7).
/// `mant` = 저장 가수 비트 수(f64 52, f32 23), `ebits` = 지수 비트 수(11, 8). 범위를 넘으면 None.
fn to_ieee_bits(x: &Q, mant: u32, ebits: u32) -> Option<u64> {
    let bias: i64 = (1 << (ebits - 1)) - 1;
    let emin = 1 - bias; // 정규 수의 최소 지수
    let emax = bias;
    if x.is_zero() {
        return Some(0);
    }
    let neg = x.is_negative();
    let n = x.numer().abs();
    let d = x.denom().clone();
    // e = floor(log2(n/d))
    let mut e = n.bits() as i64 - d.bits() as i64;
    let ge = |e: i64| -> bool {
        // n/d >= 2^e ?
        if e >= 0 { n >= (&d << e as usize) } else { (&n << (-e) as usize) >= d }
    };
    if !ge(e) {
        e -= 1;
    }
    // 가수 비트 위치: 정규 수는 e, 비정규 수는 emin
    let scale_exp = e.max(emin) - mant as i64; // 값 = m × 2^scale_exp
    let (num, den) = if scale_exp >= 0 { (n.clone(), &d << scale_exp as usize) } else { (&n << (-scale_exp) as usize, d.clone()) };
    let (mut m, r) = num.div_rem(&den);
    let twice = &r * 2u32;
    if twice > den || (twice == den && m.is_odd()) {
        m += BigInt::one();
    }
    let mut m = m.to_u64()?;
    let mut exp = e.max(emin);
    if m >> (mant + 1) != 0 {
        // 반올림으로 자리 올림
        m >>= 1;
        exp += 1;
    }
    let bits = if m >> mant == 0 {
        // 비정규 수
        m
    } else {
        if exp > emax {
            return None;
        }
        (((exp + bias) as u64) << mant) | (m & ((1u64 << mant) - 1))
    };
    Some(bits | if neg { 1u64 << (mant + ebits) } else { 0 })
}

/// 가장 가까운 f64 (짝수 반올림 한 번). 범위 밖이면 Err.
pub fn to_f64_checked(x: &Q) -> Result<f64, String> {
    to_ieee_bits(x, 52, 11).map(f64::from_bits).ok_or_else(|| format!("f64 범위 밖: {x}"))
}

/// 가장 가까운 f32 (f64를 거치지 않고 짝수 반올림 한 번). 범위 밖이면 Err.
pub fn to_f32_checked(x: &Q) -> Result<f32, String> {
    to_ieee_bits(x, 23, 8).map(|b| f32::from_bits(b as u32)).ok_or_else(|| format!("f32 범위 밖: {x}"))
}

/// 범위가 이미 검사된 값용 (범위 밖이면 무한대).
pub fn to_f64(x: &Q) -> f64 {
    to_f64_checked(x).unwrap_or(if x.is_negative() { f64::NEG_INFINITY } else { f64::INFINITY })
}

pub fn to_f32(x: &Q) -> f32 {
    to_f32_checked(x).unwrap_or(if x.is_negative() { f32::NEG_INFINITY } else { f32::INFINITY })
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

    #[test]
    fn float_conversion_single_rounding() {
        let p = |e: i64| if e >= 0 { Q::from_integer(BigInt::from(1) << e as usize) } else { Q::new(BigInt::from(1), BigInt::from(1) << (-e) as usize) };
        assert_eq!(to_f64_checked(&p(120)).unwrap(), 2f64.powi(120));
        assert_eq!(to_f64_checked(&p(-120)).unwrap(), 2f64.powi(-120));
        assert_eq!(to_f32_checked(&p(120)).unwrap(), 2f32.powi(120));
        // 1 + 2^-24 + 2^-80 → f32 1.0000001192092896 (f64를 거치면 1.0이 되는 이중 반올림 사례)
        let x = int(1) + p(-24) + p(-80);
        assert_eq!(to_f32_checked(&x).unwrap(), 1.000_000_119_209_289_6_f32);
        assert_eq!(to_f64_checked(&frac(1, 3)).unwrap(), 1.0 / 3.0);
        assert_eq!(to_f32_checked(&frac(1, 3)).unwrap(), 1.0f32 / 3.0);
        assert_eq!(to_f64_checked(&frac(-7, 2)).unwrap(), -3.5);
        assert_eq!(to_f32_checked(&frac(21, 20)).unwrap(), 1.05f32);
        assert_eq!(to_f64_checked(&parse("179.94").unwrap()).unwrap(), 179.94);
        assert!(to_f32_checked(&p(200)).is_err());
        // 비정규 수와 0
        assert_eq!(to_f64_checked(&p(-1074)).unwrap(), f64::from_bits(1));
        assert_eq!(to_f64_checked(&p(-1076)).unwrap(), 0.0);
        assert_eq!(to_f32_checked(&p(-149)).unwrap(), f32::from_bits(1));
    }
}
