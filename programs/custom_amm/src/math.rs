//! Pure swap math. No Anchor account types cross this boundary so the functions
//! can be unit-tested on the host.
//!
//! Everything is `u128` with checked ops; no floats, no truncating casts.
//! Rounding is always toward the pool, so a caller can never round in their own
//! favour.

use crate::errors::AmmError;
use anchor_lang::prelude::*;

/// Fixed-point scale for a price (`quote_mint` per `token_mint`).
pub const PRICE_SCALE: u128 = 1_000_000_000;
/// Basis-point denominator.
pub const BPS_DENOMINATOR: u128 = 10_000;
/// Time constant of the rolling TWAP.
pub const TWAP_WINDOW_SECS: u128 = 3_600;
/// Hard cap on the swap fee.
pub const MAX_FEE_BPS: u16 = 1_000;

/// Constant-product output for `amount_in`, with the fee taken off the input.
///
/// `out = reserve_out * in_net / (reserve_in + in_net)`, floor-divided.
pub fn amount_out(
    reserve_in: u128,
    reserve_out: u128,
    amount_in: u128,
    fee_bps: u16,
) -> Result<u128> {
    require!(reserve_in > 0 && reserve_out > 0, AmmError::PoolEmpty);
    let fee_complement = BPS_DENOMINATOR
        .checked_sub(u128::from(fee_bps))
        .ok_or(AmmError::MathOverflow)?;
    let in_net = amount_in
        .checked_mul(fee_complement)
        .ok_or(AmmError::MathOverflow)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or(AmmError::MathOverflow)?;
    require!(in_net > 0, AmmError::SwapOutputZero);

    let numerator = reserve_out
        .checked_mul(in_net)
        .ok_or(AmmError::MathOverflow)?;
    let denominator = reserve_in
        .checked_add(in_net)
        .ok_or(AmmError::MathOverflow)?;
    numerator
        .checked_div(denominator)
        .ok_or_else(|| AmmError::MathOverflow.into())
}

/// A price scaled by [`PRICE_SCALE`], floor-divided.
pub fn price(numerator_amount: u128, denominator_amount: u128) -> Result<u128> {
    require!(denominator_amount > 0, AmmError::PoolEmpty);
    numerator_amount
        .checked_mul(PRICE_SCALE)
        .ok_or(AmmError::MathOverflow)?
        .checked_div(denominator_amount)
        .ok_or_else(|| AmmError::MathOverflow.into())
}

/// `price / twap` expressed in bps.
pub fn ratio_bps(price: u128, twap: u128) -> Result<u128> {
    require!(twap > 0, AmmError::PoolEmpty);
    price
        .checked_mul(BPS_DENOMINATOR)
        .ok_or(AmmError::MathOverflow)?
        .checked_div(twap)
        .ok_or_else(|| AmmError::MathOverflow.into())
}

/// Time-weighted blend of `rolling` toward `observed` over `elapsed` seconds,
/// with a window of [`TWAP_WINDOW_SECS`]. `elapsed` is clamped to the window.
pub fn blend_twap(rolling: u128, observed: u128, elapsed: u128) -> Result<u128> {
    let window = TWAP_WINDOW_SECS;
    let e = elapsed.min(window);
    let remaining = window.checked_sub(e).ok_or(AmmError::MathOverflow)?;
    let aged = rolling
        .checked_mul(remaining)
        .ok_or(AmmError::MathOverflow)?;
    let fresh = observed.checked_mul(e).ok_or(AmmError::MathOverflow)?;
    aged.checked_add(fresh)
        .ok_or(AmmError::MathOverflow)?
        .checked_div(window)
        .ok_or_else(|| AmmError::MathOverflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_reduces_output() {
        let no_fee = amount_out(1_000_000, 1_000_000, 10_000, 0).unwrap();
        let with_fee = amount_out(1_000_000, 1_000_000, 10_000, 30).unwrap();
        assert!(with_fee < no_fee);
        assert_eq!(no_fee, 9_900);
    }

    #[test]
    fn empty_pool_rejected() {
        assert!(amount_out(0, 1, 1, 0).is_err());
    }

    #[test]
    fn twap_stays_between_endpoints() {
        let blended = blend_twap(1_000_000_000, 2_000_000_000, 1_800).unwrap();
        assert!(blended >= 1_000_000_000);
        assert!(blended <= 2_000_000_000);
        assert_eq!(blended, 1_500_000_000);
    }

    #[test]
    fn twap_clamps_long_gap() {
        // elapsed beyond the window collapses to the observed price.
        assert_eq!(
            blend_twap(1_000_000_000, 2_000_000_000, 10_000).unwrap(),
            2_000_000_000
        );
    }
}
