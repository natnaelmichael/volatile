//! Fixed-point arithmetic on `u128`, scale `1e9`.
//!
//! PLAN.md §6: "Fixed-point scale 1e9, `u128` intermediates, no floats. Round
//! conservatively (spend less, require the higher fee)."
//!
//! Two rules hold everywhere in this module:
//!
//! * Every operation is checked. Nothing wraps, nothing panics on hostile input;
//!   overflow surfaces as [`MathError`] and becomes a named on-chain error.
//! * `u128` is wide enough that the intermediate never has to be split.
//!   The largest value used here is `x * SCALE` with `x` a 1e9-scaled price,
//!   which is ~1e27 for any price a CPMM pool can hold. `u128::MAX` is ~3.4e38.

use crate::errors::MathError;

/// Fixed-point scale. `SCALE` itself represents the real number 1.0.
pub const SCALE: u128 = 1_000_000_000;
/// Same scale as an `i64`, for the few places a signed fixed-point value is
/// more natural than a sign flag.
pub const SCALE_I128: i128 = 1_000_000_000;

/// Convert a real ratio expressed as `n / D` into fixed point, rounding down.
pub fn ratio_to_fixed(n: u128, d: u128) -> Result<u128, MathError> {
    if d == 0 {
        return Err(MathError::DivideByZero);
    }
    n.checked_mul(SCALE)
        .ok_or(MathError::Overflow)?
        .checked_div(d)
        .ok_or(MathError::DivideByZero)
}

/// Convert a real ratio into fixed point, rounding up. Use when under-estimating
/// would cost the protocol money.
pub fn ratio_to_fixed_ceil(n: u128, d: u128) -> Result<u128, MathError> {
    ceil_div(n.checked_mul(SCALE).ok_or(MathError::Overflow)?, d).ok_or(MathError::DivideByZero)
}

/// `(a * b) / SCALE`, rounding down.
pub fn mul_fixed(a: u128, b: u128) -> Result<u128, MathError> {
    a.checked_mul(b)
        .ok_or(MathError::Overflow)?
        .checked_div(SCALE)
        .ok_or(MathError::DivideByZero)
}

/// `(a * b) / SCALE`, rounding up.
pub fn mul_fixed_ceil(a: u128, b: u128) -> Result<u128, MathError> {
    ceil_div(
        a.checked_mul(b).ok_or(MathError::Overflow)?,
        SCALE,
    )
    .ok_or(MathError::DivideByZero)
}

/// `(a * SCALE) / b`, rounding down.
pub fn div_fixed(a: u128, b: u128) -> Result<u128, MathError> {
    if b == 0 {
        return Err(MathError::DivideByZero);
    }
    a.checked_mul(SCALE)
        .ok_or(MathError::Overflow)?
        .checked_div(b)
        .ok_or(MathError::DivideByZero)
}

/// `(a * SCALE) / b`, rounding up.
pub fn div_fixed_ceil(a: u128, b: u128) -> Result<u128, MathError> {
    if b == 0 {
        return Err(MathError::DivideByZero);
    }
    ceil_div(a.checked_mul(SCALE).ok_or(MathError::Overflow)?, b)
        .ok_or(MathError::DivideByZero)
}

/// `(a * b) / D`, rounding up. Returns `None` on division by zero so the caller
/// cannot forget to check, which is why this is not a `Result`.
pub fn ceil_div(a: u128, b: u128) -> Option<u128> {
    if b == 0 {
        return None;
    }
    let q = a / b;
    if a % b == 0 {
        Some(q)
    } else {
        q.checked_add(1)
    }
}

/// `(a * b) / D`, rounding down.
pub fn floor_div(a: u128, b: u128) -> Option<u128> {
    if b == 0 {
        return None;
    }
    Some(a / b)
}

/// Fixed-point addition, `None` on overflow. Kept as an `Option` rather than a
/// `Result` because callers chain it inside larger expressions and the `?`
/// would obscure the arithmetic.
pub fn add_fixed(a: u128, b: u128) -> Option<u128> {
    a.checked_add(b)
}

/// Fixed-point subtraction, `None` on underflow.
pub fn sub_fixed(a: u128, b: u128) -> Option<u128> {
    a.checked_sub(b)
}

/// `SCALE + a`, saturating nowhere: overflow is an error, not a clamp.
pub fn add_one(a: u128) -> Result<u128, MathError> {
    a.checked_add(SCALE).ok_or(MathError::Overflow)
}

/// A 1e9-scaled value as whole basis points, rounding up.
pub fn to_bps_ceil(fixed: u128) -> Result<u32, MathError> {
    let v = ceil_div(
        fixed.checked_mul(10_000).ok_or(MathError::Overflow)?,
        SCALE,
    )
    .ok_or(MathError::DivideByZero)?;
    u32::try_from(v).map_err(|_| MathError::Unrepresentable)
}

/// A 1e9-scaled value as whole basis points, rounding down.
pub fn to_bps_floor(fixed: u128) -> Result<u32, MathError> {
    let v = fixed
        .checked_mul(10_000)
        .ok_or(MathError::Overflow)?
        .checked_div(SCALE)
        .ok_or(MathError::DivideByZero)?;
    u32::try_from(v).map_err(|_| MathError::Unrepresentable)
}

/// Multiply by `bps / 10_000`, rounding down.
pub fn apply_bps(amount: u128, bps: u32) -> Result<u128, MathError> {
    amount
        .checked_mul(bps as u128)
        .ok_or(MathError::Overflow)?
        .checked_div(10_000)
        .ok_or(MathError::DivideByZero)
}

/// Multiply by `bps / 10_000`, rounding up.
pub fn apply_bps_ceil(amount: u128, bps: u32) -> Result<u128, MathError> {
    ceil_div(
        amount.checked_mul(bps as u128).ok_or(MathError::Overflow)?,
        10_000,
    )
    .ok_or(MathError::DivideByZero)
}

/// `floor(sqrt(n))` by Newton's method.
///
/// The initial guess is one bit above the true root so every iterate is an
/// over-estimate, which is what makes the `y >= x` termination exact rather than
/// an approximation. Converges in at most 7 iterations for any `u128`.
pub fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let bit_len = 128 - n.leading_zeros();
    let mut x: u128 = 1u128 << (bit_len + 1).div_ceil(2);
    // `x` is strictly decreasing while it is above the root, so this terminates.
    while x > 0 {
        let y = (x + n / x) / 2;
        if y >= x {
            break;
        }
        x = y;
    }
    x
}

/// `ceil(sqrt(n))`.
pub fn isqrt_ceil(n: u128) -> u128 {
    let r = isqrt(n);
    if r.saturating_mul(r) < n {
        r.saturating_add(1)
    } else {
        r
    }
}

/// `floor(sqrt(x))` for a 1e9-scaled `x`, returned in the same scale.
///
/// Derivation: `x` represents `x/SCALE`, so the root in real units is
/// `sqrt(x/SCALE)` and in fixed point that is `sqrt(x/SCALE) * SCALE`, which is
/// exactly `sqrt(x * SCALE)`. Rounding down is the conservative direction for a
/// root that ends up in a denominator.
pub fn sqrt_fixed(x: u128) -> Result<u128, MathError> {
    let scaled = x.checked_mul(SCALE).ok_or(MathError::Overflow)?;
    Ok(isqrt(scaled))
}

/// `ceil(sqrt(x))` for a 1e9-scaled `x`, returned in the same scale.
pub fn sqrt_fixed_ceil(x: u128) -> Result<u128, MathError> {
    let scaled = x.checked_mul(SCALE).ok_or(MathError::Overflow)?;
    Ok(isqrt_ceil(scaled))
}

/// Narrow a fixed-point or raw value to `u64`, refusing any lossy conversion.
pub fn to_u64_exact(v: u128) -> Result<u64, MathError> {
    u64::try_from(v).map_err(|_| MathError::LossyConversion)
}

/// Fixed-point value from raw token units, rounding down.
pub fn from_raw_ceil(raw: u128, decimals: u8) -> Result<u128, MathError> {
    let d = 10u128
        .checked_pow(decimals as u32)
        .ok_or(MathError::Overflow)?;
    ceil_div(raw.checked_mul(SCALE).ok_or(MathError::Overflow)?, d).ok_or(MathError::DivideByZero)
}

/// Raw token units from a fixed-point value, rounding down.
///
/// Inverse of [`from_raw_ceil`]: `to_raw_floor(from_raw_ceil(x, d), d) <= x`.
pub fn to_raw_floor(fixed: u128, decimals: u8) -> Result<u64, MathError> {
    let d = 10u128
        .checked_pow(decimals as u32)
        .ok_or(MathError::Overflow)?;
    to_u64_exact(
        fixed
            .checked_mul(d)
            .ok_or(MathError::Overflow)?
            .checked_div(SCALE)
            .ok_or(MathError::DivideByZero)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn isqrt_matches_naive() {
        for n in 0u128..10_000 {
            let expected = (n as f64).sqrt().floor() as u128;
            assert_eq!(isqrt(n), expected, "n={n}");
        }
        // Around the u128 range boundaries where a naive seed would overflow.
        for &n in &[
            u128::MAX,
            u128::MAX - 1,
            1u128 << 127,
            (1u128 << 127) - 1,
            (1u128 << 64) + 7,
            u64::MAX as u128,
        ] {
            let r = isqrt(n);
            assert!(
                r.checked_mul(r).is_none_or(|sq| sq <= n),
                "root too large for {n}"
            );
            assert!(
                r.checked_add(1)
                    .and_then(|s| s.checked_mul(s))
                    .is_none_or(|sq| sq > n),
                "root too small for {n}"
            );
        }
    }

    #[test]
    fn isqrt_ceil_rounds_up_only_when_needed() {
        for n in 0u128..1_000 {
            let e = isqrt(n);
            let c = isqrt_ceil(n);
            assert_eq!(c, if e * e < n { e + 1 } else { e });
            assert!(c * c >= n);
        }
    }

    #[test]
    fn sqrt_fixed_round_trips_known_values() {
        // sqrt(4) == 2
        assert_eq!(sqrt_fixed(4 * SCALE).unwrap(), 2 * SCALE);
        // sqrt(0.25) == 0.5
        assert_eq!(sqrt_fixed(SCALE / 4).unwrap(), SCALE / 2);
        // sqrt(2), rounded down.
        let s = sqrt_fixed(2 * SCALE).unwrap();
        assert!(s * s <= 2 * SCALE * SCALE);
        assert!((s + 1) * (s + 1) > 2 * SCALE * SCALE);
    }

    #[test]
    fn sqrt_fixed_rejects_overflow_instead_of_wrapping() {
        assert_eq!(sqrt_fixed(u128::MAX), Err(MathError::Overflow));
        assert_eq!(sqrt_fixed(u128::MAX / SCALE + 1), Err(MathError::Overflow));
    }

    #[test]
    fn division_by_zero_is_an_error_not_a_panic() {
        assert_eq!(div_fixed(1, 0), Err(MathError::DivideByZero));
        assert_eq!(div_fixed_ceil(1, 0), Err(MathError::DivideByZero));
        assert_eq!(ratio_to_fixed(1, 0), Err(MathError::DivideByZero));
        assert_eq!(ceil_div(1, 0), None);
        assert_eq!(floor_div(1, 0), None);
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        assert_eq!(mul_fixed(u128::MAX, 2), Err(MathError::Overflow));
        assert_eq!(apply_bps(u128::MAX, 10_000), Err(MathError::Overflow));
    }

    #[test]
    fn rounding_directions_are_correct() {
        // 3 / 2 == 1.5 -> 1 down, 2 up.
        assert_eq!(ceil_div(3, 2), Some(2));
        assert_eq!(floor_div(3, 2), Some(1));
        assert_eq!(ceil_div(4, 2), Some(2));
        // Fixed point: 0.5 * SCALE / SCALE.
        assert_eq!(mul_fixed(SCALE / 2, SCALE / 2).unwrap(), SCALE / 4);
        assert_eq!(mul_fixed_ceil(SCALE / 3, SCALE / 2).unwrap(), SCALE / 6 + 1);
        assert_eq!(apply_bps(10_000, 1).unwrap(), 1);
        assert_eq!(apply_bps_ceil(10_001, 1).unwrap(), 2);
    }

    #[test]
    fn raw_conversion_round_trips() {
        // 9 decimals: 1 token == SCALE.
        assert_eq!(from_raw_ceil(1_000_000_000, 9).unwrap(), SCALE);
        // 6 decimals: 1 token is 1e6 raw, which is also SCALE in fixed point.
        assert_eq!(from_raw_ceil(1_000_000, 6).unwrap(), SCALE);
        assert_eq!(from_raw_ceil(1_500_000, 6).unwrap(), SCALE * 3 / 2);
        assert_eq!(to_raw_floor(SCALE, 9).unwrap(), 1_000_000_000);
        assert_eq!(to_raw_floor(SCALE + 1_000, 9).unwrap(), 1_000_001_000);
        assert_eq!(to_u64_exact(u64::MAX as u128 + 1), Err(MathError::LossyConversion));
        // Round-tripping raw -> fixed -> raw must never invent tokens, which is
        // what makes it safe to quote against a vault balance.
        for raw in [1u128, 7, 999, 1_000_000_000, 1_234_567_891] {
            let fixed = from_raw_ceil(raw, 9).unwrap();
            assert!(
                to_raw_floor(fixed, 9).unwrap() as u128 <= raw,
                "raw={raw} round-tripped up"
            );
        }
    }

    proptest! {
        /// `isqrt` must satisfy `r^2 <= n < (r+1)^2` for every `u128`.
        #[test]
        fn isqrt_is_exact_floor(n in any::<u128>()) {
            let r = isqrt(n);
            prop_assert!(r.saturating_mul(r) <= n || n == u128::MAX);
            if let Some(s) = r.checked_add(1) {
                if let Some(ss) = s.checked_mul(s) {
                    prop_assert!(ss > n || n == u128::MAX);
                }
            }
        }

        /// Every multiplication rounds toward the value a real 1e9-scaled product
        /// would produce, never past it by a whole unit.
        #[test]
        fn mul_fixed_never_overshoots(a in 0u128..(1u128 << 64), b in 0u128..SCALE) {
            let got = mul_fixed(a, b).unwrap();
            // exact = a*b/SCALE; floor <= exact < floor+1
            prop_assert!(got <= a.checked_mul(b).unwrap() / SCALE);
        }

        #[test]
        fn mul_fixed_ceil_never_undershoots(a in 0u128..(1u128 << 64), b in 0u128..SCALE) {
            let got = mul_fixed_ceil(a, b).unwrap();
            prop_assert!(got >= a.checked_mul(b).unwrap() / SCALE);
        }

        /// No arithmetic helper may panic, for any input.
        #[test]
        fn helpers_never_panic(a in any::<u128>(), b in any::<u128>()) {
            let _ = mul_fixed(a, b);
            let _ = mul_fixed_ceil(a, b);
            let _ = div_fixed(a, b);
            let _ = div_fixed_ceil(a, b);
            let _ = ceil_div(a, b);
            let _ = floor_div(a, b);
            let _ = sqrt_fixed(a);
            let _ = sqrt_fixed_ceil(a);
            let _ = to_u64_exact(a);
            let _ = ratio_to_fixed(a, b);
        }

        /// `sqrt_fixed` is monotone: a >= b implies sqrt(a) >= sqrt(b).
        #[test]
        fn sqrt_fixed_is_monotone(a in 0u128..(1u128 << 40), b in 0u128..(1u128 << 40)) {
            prop_assume!(a >= b);
            let sa = sqrt_fixed(a).unwrap();
            let sb = sqrt_fixed(b).unwrap();
            prop_assert!(sa >= sb);
        }
    }
}
