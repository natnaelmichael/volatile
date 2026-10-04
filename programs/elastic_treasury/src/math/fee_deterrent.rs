//! Fee deterrent, formula v1.
//!
//! PLAN.md D9: "Formula v1 = the original equation exactly as written". This
//! module is frozen — a later model is a new `formula_version`, never an edit
//! here, so that a published week's parameters always mean what they meant.
//!
//! ## The equation
//!
//! Original (PLAN.md §6.2):
//!
//! ```text
//! S*P0 = r^4 * S*Pf + S*r^2*Pf        (the right side factors to S*Pf*r^2*(1+r^2))
//! run as the inequality   S*P0 > S*Pf*r^2*(1+r^2)
//! ```
//!
//! with `r` the per-transfer **retention**, `r = 1 - fee`. `S` is the starting
//! token amount and cancels, which is why it is absent from the implementation.
//!
//! Substituting `R = P0/Pf` and `u = r^2`:
//!
//! ```text
//! R > u*(1+u)   =>   u^2 + u - R < 0   =>   u < u* = (-1 + sqrt(1 + 4R)) / 2
//! r < sqrt(u*)  =>  fee_required = 1 - sqrt(u*)
//! ```
//!
//! If `R >= 2` then `u* >= 1`, retention of 100% already satisfies the
//! inequality, and no additional fee is required.
//!
//! ## Why `short_hops` is a parameter but the exponents are hard-wired
//!
//! v1's exponents are `r^4` and `r^2` as written. PLAN.md §6.2 asks for
//! `short_hops` to stay "in the entry and in the function signature even though
//! v1's exponents are fixed as written, so a v2 can use it", so the signature
//! takes it and this version checks it equals [`SHORT_HOPS`]. A v2 will use it
//! to derive the exponents.
//!
//! ## Integer implementation
//!
//! Everything is 1e9 fixed point (`crate::math::fixed`). The two square roots
//! land as:
//!
//! ```text
//! sqrt_fixed(SCALE + 4*R)          // sqrt(1 + 4R)
//! sqrt_fixed((sqrt_disc - SCALE)/2) // sqrt(u*)
//! ```
//!
//! Rounding is **up** on the fee, which is what the plan requires ("require the
//! higher fee"). `sqrt_fixed` floors its argument's root, and `fee = SCALE - r`
//! inverts that, so the composition rounds the fee up for free. `MAX_FEE_BPS` is
//! asserted on the way out, never trusted from the caller.

use crate::constants::{MAX_FEE_BPS, MAX_SUPPORTED_FORMULA_VERSION, SHORT_HOPS};
use crate::errors::MathError;
use crate::math::fixed::{self, SCALE};

/// Inputs to formula v1.
///
/// Retention is deliberately *not* an input. v1 derives the fee from the price
/// move alone (§6.2), and the caller gets retention back from the fee it
/// receives; carrying a retention here would invite a caller to pass the
/// current one and believe it was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterrentInput {
    /// `R = P0/Pf`, 1e9 fixed point, in units where "no move" is exactly `SCALE`.
    pub price_ratio_fixed: u128,
    /// Number of taxed hops a short must make. v1 requires [`SHORT_HOPS`].
    pub short_hops: u8,
    /// The fee currently in force, in bps. The result is never below this.
    pub current_fee_bps: u16,
    /// The week's published ceiling. Must be `<= MAX_FEE_BPS` (PLAN.md I6); a
    /// larger value is an error, not something to clamp.
    pub max_fee_bps: u16,
}

/// `R` at and above which v1 stops binding, because `u* >= 1` and retention of
/// 100% already satisfies the inequality.
///
/// PLAN.md §6.2: "if `R >= 2` then `u* >= 1` and no extra fee is required
/// (return the current fee)."
pub const R_UNBOUNDED_FIXED: u128 = 2 * SCALE;

/// The uncapped fee required for v1's condition to bind, in bps.
///
/// Returns `None` when the price move is 50% or worse, which is where v1 stops
/// binding — see [`R_UNBOUNDED_FIXED`] and `docs/VERIFIED.md`. The value is
/// *not* clamped to `MAX_FEE_BPS`: for `R < 0.3125` the equation demands more
/// than the program may ever charge, and folding that into a clamp would make
/// "unreachable" indistinguishable from "satisfied". [`fee_for_move`] is the
/// on-chain entry point and never returns more than `MAX_FEE_BPS`.
pub fn required_fee_bps_raw(
    price_ratio_fixed: u128,
    short_hops: u8,
) -> Result<Option<u16>, MathError> {
    if short_hops != SHORT_HOPS {
        // v1's exponents are `r^4` and `r^2`. A different hop count belongs to a
        // different `formula_version`, not to this function.
        return Err(MathError::Unrepresentable);
    }
    if price_ratio_fixed >= R_UNBOUNDED_FIXED {
        return Ok(None);
    }
    if price_ratio_fixed == 0 {
        // P0/Pf of zero means the final price is zero or the inputs overflowed
        // before division. Demand the maximum the cap allows rather than
        // guessing.
        return Ok(Some(u16::MAX));
    }

    // u* = (-1 + sqrt(1 + 4R)) / 2, in 1e9.
    let four_r = price_ratio_fixed.checked_mul(4).ok_or(MathError::Overflow)?;
    let discriminant = fixed::add_one(four_r)?;
    let sqrt_disc = fixed::sqrt_fixed(discriminant)?;
    // sqrt_disc >= SCALE because R > 0, so this cannot underflow.
    let u_star = sqrt_disc
        .checked_sub(SCALE)
        .ok_or(MathError::Underflow)?
        / 2;

    // r = sqrt(u*), floored -> fee = 1 - r is rounded UP.
    let retention_needed = fixed::sqrt_fixed(u_star)?;
    if retention_needed >= SCALE {
        return Ok(Some(0));
    }
    let fee_fixed = SCALE
        .checked_sub(retention_needed)
        .ok_or(MathError::Underflow)?;

    // Round the fee UP to whole bps: we want the smallest fee that still deters.
    let bps = fixed::to_bps_ceil(fee_fixed)?;
    Ok(Some(u16::try_from(bps).unwrap_or(u16::MAX)))
}

/// The fee required for v1's condition to bind, in bps, capped at
/// [`MAX_FEE_BPS`]. Yields `0` once `R` reaches [`R_UNBOUNDED_FIXED`].
pub fn required_fee_bps(price_ratio_fixed: u128, short_hops: u8) -> Result<u16, MathError> {
    let raw = required_fee_bps_raw(price_ratio_fixed, short_hops)?;
    Ok(raw.unwrap_or(0).min(MAX_FEE_BPS))
}

/// Formula v1: the fee to charge, given the price move and the published ceiling.
///
/// Order of operations, per PLAN.md §6.2 and §5.2 step 4:
/// 1. compute `required_fee_bps`
/// 2. clamp to `[current_fee_bps, min(max_fee_bps, MAX_FEE_BPS)]`
///
/// Step 2 is why a *falling* price cannot lower the fee: the clamp's lower
/// bound is the current fee, so `revert_action` is the only path down.
pub fn fee_for_move(input: DeterrentInput) -> Result<u16, MathError> {
    if input.max_fee_bps > MAX_FEE_BPS {
        return Err(MathError::Unrepresentable);
    }
    let ceiling = input.max_fee_bps.min(MAX_FEE_BPS);
    let raw = required_fee_bps(input.price_ratio_fixed, input.short_hops)?;

    if raw >= ceiling {
        return Ok(ceiling);
    }
    Ok(raw.max(input.current_fee_bps))
}

/// Whether v1's deterrence condition holds at a given retention. Used by tests
/// and by `docs/DESIGN.md` examples; not on the critical path.
pub fn is_deterred(retention_fixed: u128, price_ratio_fixed: u128) -> bool {
    if price_ratio_fixed >= R_UNBOUNDED_FIXED {
        return true;
    }
    let r2 = match retention_fixed.checked_mul(retention_fixed) {
        Some(v) => v / SCALE,
        None => return false,
    };
    // R > u*(1+u)  <=>  R*SCALE > u*(SCALE + u)
    r2.checked_add(r2.checked_mul(r2).unwrap_or(0) / SCALE)
        .is_some_and(|rhs| price_ratio_fixed > rhs)
}

/// Highest `formula_version` this build can evaluate.
pub fn supported_formula_version() -> u8 {
    MAX_SUPPORTED_FORMULA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    const BPS: u32 = 10_000;

    fn ratio(n: u64, d: u64) -> u128 {
        fixed::ratio_to_fixed(n as u128, d as u128).unwrap()
    }

    /// The table in PLAN.md §6.2.
    ///
    /// The plan's own numbers are given to 0.1% (10 bps) precision, so they
    /// cannot be checked "within 1 bps" as written. Both forms are asserted:
    /// the exact integer result, and the plan's rounded table.
    #[test]
    fn reference_values_match_the_plan_table() {
        // (P0, Pf, exact bps required, plan's percentage restated in bps)
        let table: &[(u64, u64, u64, u64)] = &[
            (1, 1, 2_139, 2_140),  // 21.4%
            (11, 10, 1_865, 1_860), // 18.6%
            (5, 4, 1_487, 1_490),   // 14.9%
            (3, 2, 929, 930),      //  9.3%
            (2, 1, 0, 0),          //  0%   (R = 2.0)
            (5, 2, 0, 0),          //  0%   (R = 2.5)
        ];
        for &(num, den, expected_bps, plan_bps) in table {
            let r = ratio(num, den);
            let got = required_fee_bps(r, SHORT_HOPS).unwrap();
            assert_eq!(got as u64, expected_bps, "R = {num}/{den}");
            // The plan states these to 0.1% = 10 bps, so the exact integer result
            // must agree with the rounded table to within half of that step.
            assert!(
                (got as i64 - plan_bps as i64).abs() <= 5,
                "R = {num}/{den}: {got} bps is more than 0.1% from {plan_bps} bps"
            );
        }
    }

    #[test]
    fn one_below_the_cap_is_never_exceeded() {
        for num in 1u64..400 {
            for den in 1u64..400 {
                let r = fixed::ratio_to_fixed(num as u128, den as u128).unwrap();
                let fee = required_fee_bps(r, SHORT_HOPS).unwrap();
                assert!(
                    fee as u32 <= MAX_FEE_BPS as u32,
                    "R = {num}/{den} produced {fee} bps"
                );
            }
        }
    }

    /// The documented limitation, made visible rather than hidden (PLAN.md §6.2).
    #[test]
    fn v1_stops_binding_at_fifty_percent_drop() {
        // Exactly R = 2 needs no fee at all.
        assert_eq!(required_fee_bps(2 * SCALE, SHORT_HOPS).unwrap(), 0);
        assert_eq!(required_fee_bps(2 * SCALE + 1, SHORT_HOPS).unwrap(), 0);
        // A 60% drop (Pf/P0 = 0.4 -> R = 2.5) also needs none.
        assert_eq!(required_fee_bps(ratio(5, 2), SHORT_HOPS).unwrap(), 0);
        // ...but the caller still gets `current_fee_bps` back, never less.
        let fee = fee_for_move(DeterrentInput {
            price_ratio_fixed: ratio(5, 2),
            short_hops: SHORT_HOPS,
            current_fee_bps: 250,
            max_fee_bps: 5_000,
        })
        .unwrap();
        assert_eq!(fee, 250);
    }

    /// The fee must decrease as the price drop shrinks, and the curve must be
    /// smooth enough that a 1-bp step in R never moves the answer by much.
    #[test]
    fn fee_is_monotone_decreasing_in_ratio() {
        let mut previous = u16::MAX;
        for step in 0..=2_000u128 {
            let r = SCALE + step * (SCALE / 2_000);
            let fee = required_fee_bps(r, SHORT_HOPS).unwrap();
            assert!(fee <= previous, "fee rose at step {step}: {previous} -> {fee}");
            previous = fee;
        }
        // Exactly at R = 2 the equation is satisfied by a zero fee.
        assert_eq!(previous, 0);
    }

    /// The required fee must actually satisfy v1's inequality at or above the
    /// value returned, and one bps less must not (where it is representable).
    #[test]
    fn returned_fee_is_the_smallest_that_deters() {
        for num in 1u64..60 {
            for den in 1u64..60 {
                let r = ratio(num, den);
                // `raw` is the uncapped answer; skip the moves so violent that the
                // equation wants more than the 50% program cap, since no in-range
                // fee can deter those and minimality is then vacuous.
                let Some(fee) = required_fee_bps_raw(r, SHORT_HOPS).unwrap() else {
                    continue;
                };
                if fee == 0 || fee as u32 >= MAX_FEE_BPS as u32 {
                    continue;
                }
                assert!(
                    is_deterred(retention_of_bps(fee), r),
                    "R={num}/{den}: {fee} bps does not deter"
                );
                if fee > 1 {
                    assert!(
                        !is_deterred(retention_of_bps(fee - 1), r),
                        "R={num}/{den}: {} bps also deters, so {fee} is not minimal",
                        fee - 1
                    );
                }
            }
        }
    }

    fn retention_of_bps(fee_bps: u16) -> u128 {
        SCALE - (fee_bps as u128 * SCALE) / (BPS as u128)
    }

    /// PLAN.md §6.2: clamp to `[current_fee, min(max_fee, 5000)]`.
    #[test]
    fn clamping_is_applied_in_both_directions() {
        // A move that needs less than the current fee never lowers it: the only
        // path down is an explicit `revert_action` (PLAN.md §5.2 step 4).
        assert_eq!(
            fee_for_move(DeterrentInput {
                price_ratio_fixed: ratio(11, 10),
                short_hops: SHORT_HOPS,
                current_fee_bps: 1_900,
                max_fee_bps: 5_000,
            })
            .unwrap(),
            1_900
        );
        // ...and one that needs more is honoured in full.
        assert_eq!(
            fee_for_move(DeterrentInput {
                price_ratio_fixed: ratio(3, 2),
                short_hops: SHORT_HOPS,
                current_fee_bps: 400,
                max_fee_bps: 5_000,
            })
            .unwrap(),
            929
        );

        // Above the ceiling: clamped down to the ceiling, never above.
        assert_eq!(
            fee_for_move(DeterrentInput {
                price_ratio_fixed: ratio(1, 1),
                short_hops: SHORT_HOPS,
                current_fee_bps: 0,
                max_fee_bps: 300,
            })
            .unwrap(),
            300
        );

        // A ceiling above the program cap is refused, not silently honoured.
        assert_eq!(
            fee_for_move(DeterrentInput {
                price_ratio_fixed: SCALE,
                short_hops: SHORT_HOPS,
                current_fee_bps: 0,
                max_fee_bps: 7_777,
            }),
            Err(MathError::Unrepresentable)
        );
    }

    #[test]
    fn wrong_short_hops_is_rejected() {
        assert_eq!(
            required_fee_bps(SCALE, 3),
            Err(MathError::Unrepresentable)
        );
        assert_eq!(required_fee_bps(SCALE, 5), Err(MathError::Unrepresentable));
        assert_eq!(required_fee_bps(SCALE, 0), Err(MathError::Unrepresentable));
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        for &r in &[0u128, 1, SCALE, SCALE + 1, u16::MAX as u128 * SCALE, u128::MAX / 2] {
            let _ = required_fee_bps(r, SHORT_HOPS);
            let _ = fee_for_move(DeterrentInput {
                price_ratio_fixed: r,
                short_hops: SHORT_HOPS,
                current_fee_bps: 0,
                max_fee_bps: MAX_FEE_BPS,
            });
        }
        // An absurd ceiling is refused rather than silently honoured.
        assert_eq!(
            fee_for_move(DeterrentInput {
                price_ratio_fixed: SCALE,
                short_hops: SHORT_HOPS,
                current_fee_bps: 0,
                max_fee_bps: u16::MAX,
            }),
            Err(MathError::Unrepresentable)
        );
        // A price ratio beyond the range a price can take still answers.
        assert!(fee_for_move(DeterrentInput {
            price_ratio_fixed: u16::MAX as u128 * SCALE,
            short_hops: SHORT_HOPS,
            current_fee_bps: 0,
            max_fee_bps: MAX_FEE_BPS,
        })
        .is_ok());
    }
}
