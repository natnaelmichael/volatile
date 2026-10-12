//! A line-for-line copy of the on-chain swap math in
//! `programs/custom_amm/src/math.rs`, with `AmmError` in place of Anchor's
//! `ProgramError`.
//!
//! `quote()` must reproduce the executed trade exactly, so any change here must
//! be mirrored on-chain (and vice versa). Everything is checked `u128`; rounding
//! is always toward the pool.

use jupiter_amm_interface::AmmError;

/// Fixed-point scale for a price (`quote_mint` per `token_mint`).
pub const PRICE_SCALE: u128 = 1_000_000_000;
/// Basis-point denominator.
pub const BPS_DENOMINATOR: u128 = 10_000;

fn math_overflow() -> AmmError {
    AmmError::Custom("math overflow".to_string())
}

fn pool_empty() -> AmmError {
    AmmError::Custom("pool empty".to_string())
}

/// Constant-product output for `amount_in`, with the fee taken off the input.
///
/// `out = reserve_out * in_net / (reserve_in + in_net)`, floor-divided.
pub fn amount_out(
    reserve_in: u128,
    reserve_out: u128,
    amount_in: u128,
    fee_bps: u16,
) -> Result<u128, AmmError> {
    if reserve_in == 0 || reserve_out == 0 {
        return Err(pool_empty());
    }
    let in_net = input_net(amount_in, fee_bps)?;
    if in_net == 0 {
        return Err(AmmError::Custom("swap output zero".to_string()));
    }

    let numerator = reserve_out.checked_mul(in_net).ok_or_else(math_overflow)?;
    let denominator = reserve_in.checked_add(in_net).ok_or_else(math_overflow)?;
    numerator.checked_div(denominator).ok_or_else(math_overflow)
}

/// Total fee charged on `amount_in`, in input-mint units: `amount_in - in_net`.
pub fn input_fee(amount_in: u128, fee_bps: u16) -> Result<u128, AmmError> {
    let in_net = input_net(amount_in, fee_bps)?;
    amount_in.checked_sub(in_net).ok_or_else(math_overflow)
}

/// `amount_in` with the fee removed, floor-divided.
fn input_net(amount_in: u128, fee_bps: u16) -> Result<u128, AmmError> {
    let fee_complement = BPS_DENOMINATOR
        .checked_sub(u128::from(fee_bps))
        .ok_or_else(math_overflow)?;
    amount_in
        .checked_mul(fee_complement)
        .ok_or_else(math_overflow)?
        .checked_div(BPS_DENOMINATOR)
        .ok_or_else(math_overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_on_chain_reference() {
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
    fn fee_is_input_side() {
        assert_eq!(input_fee(10_000, 30).unwrap(), 30);
    }
}
