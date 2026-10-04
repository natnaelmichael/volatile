//! Raydium CPMM constant-product math, transcribed from the pool program.
//!
//! This module exists so the program can *price* a swap and defend against a
//! manipulated quote without trusting the keeper or the pool's own answer. Every
//! function mirrors `raydium-cp-swap` exactly, including its rounding
//! direction, because a quote that is off by one unit in the wrong direction is
//! either an unnecessary revert or a sandwich.
//!
//! Sources (`docs/VERIFIED.md` V4, V5):
//!
//! * `programs/cp-swap/src/curve/fees.rs` — `ceil_div` / `floor_div` and
//!   `trading_fee`, `protocol_fee`, `fund_fee`, `creator_fee`,
//!   `split_creator_fee`, `calculate_pre_fee_amount`.
//! * `programs/cp-swap/src/curve/curve_calculator.rs` — `swap_base_input`.
//! * `programs/cp-swap/src/curve/curve_constant_product.rs` —
//!   `swap_base_input_without_fees`.
//! * `programs/cp-swap/src/states/pool.rs` — `vault_amount_without_fee`.
//! * `programs/cp-swap/src/instructions/swap_base_input.rs` — the instruction
//!   itself, for the Token-2022 netting and the slippage check.
//!
//! ## Rounding
//!
//! | quantity | direction | why |
//! |---|---|---|
//! | trading fee, creator fee | up (`ceil_div`) | protocol must not be shorted |
//! | protocol fee, fund fee | down (`floor_div`) | as upstream; the dust stays in the vault |
//! | output amount | down | as upstream |
//! | `min_out` | up | plan §6.1: "Round every number in the conservative direction" |
//!
//! ## The one place we deliberately differ from upstream
//!
//! `swap_base_input` in the pool unwraps internally and returns `None` on
//! overflow, which the instruction turns into `ZeroTradingTokens`. Everything
//! here returns [`MathError`] instead, so a bad reserve or a hostile amount
//! reverts as a named error rather than being silently zero.

use crate::errors::MathError;
use crate::math::fixed::{self, SCALE};

/// CPMM fee denominator. Not 10_000: the pool stores rates out of 1e6.
pub const FEE_DENOMINATOR: u64 = 1_000_000;

/// CPMM protocol + fund + creator fee rates, from the pool's `AmmConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeRates {
    pub trade_fee_rate: u64,
    pub creator_fee_rate: u64,
    pub protocol_fee_rate: u64,
    pub fund_fee_rate: u64,
    /// Whether the pool charges the creator fee on the input side.
    pub is_creator_fee_on_input: bool,
}

impl FeeRates {
    /// The pool asserts these bounds on `AmmConfig`; we re-check them so a
    /// malformed config cannot divide by zero or produce a negative split.
    pub fn validate(&self) -> Result<(), MathError> {
        if self
            .trade_fee_rate
            .checked_add(self.creator_fee_rate)
            .ok_or(MathError::Overflow)?
            >= FEE_DENOMINATOR
        {
            return Err(MathError::Unrepresentable);
        }
        if self.protocol_fee_rate > FEE_DENOMINATOR
            || self.fund_fee_rate > FEE_DENOMINATOR
        {
            return Err(MathError::Unrepresentable);
        }
        if self
            .protocol_fee_rate
            .checked_add(self.fund_fee_rate)
            .ok_or(MathError::Overflow)?
            > FEE_DENOMINATOR
        {
            return Err(MathError::Unrepresentable);
        }
        Ok(())
    }

    /// Total rate taken from the input before the curve sees it.
    fn input_rate(&self) -> Result<u64, MathError> {
        if self.is_creator_fee_on_input {
            self.trade_fee_rate
                .checked_add(self.creator_fee_rate)
                .ok_or(MathError::Overflow)
        } else {
            Ok(self.trade_fee_rate)
        }
    }
}

/// `ceil(a * num / den)`, matching the pool's `ceil_div` including its
/// `+ den - 1` form.
pub fn ceil_div_fee(a: u128, num: u64, den: u64) -> Result<u128, MathError> {
    if den == 0 {
        return Err(MathError::DivideByZero);
    }
    a.checked_mul(num as u128)
        .and_then(|v| v.checked_add(den as u128 - 1))
        .ok_or(MathError::Overflow)?
        .checked_div(den as u128)
        .ok_or(MathError::DivideByZero)
}

/// `floor(a * num / den)`, matching the pool's `floor_div`.
pub fn floor_div_fee(a: u128, num: u64, den: u64) -> Result<u128, MathError> {
    if den == 0 {
        return Err(MathError::DivideByZero);
    }
    a.checked_mul(num as u128)
        .ok_or(MathError::Overflow)?
        .checked_div(den as u128)
        .ok_or(MathError::DivideByZero)
}

/// `Fees::trading_fee`.
pub fn trading_fee(amount: u128, trade_fee_rate: u64) -> Result<u128, MathError> {
    ceil_div_fee(amount, trade_fee_rate, FEE_DENOMINATOR)
}

/// `Fees::creator_fee`.
pub fn creator_fee(amount: u128, creator_fee_rate: u64) -> Result<u128, MathError> {
    ceil_div_fee(amount, creator_fee_rate, FEE_DENOMINATOR)
}

/// `Fees::protocol_fee`, floored.
pub fn protocol_fee(amount: u128, protocol_fee_rate: u64) -> Result<u128, MathError> {
    floor_div_fee(amount, protocol_fee_rate, FEE_DENOMINATOR)
}

/// `Fees::fund_fee`, floored.
pub fn fund_fee(amount: u128, fund_fee_rate: u64) -> Result<u128, MathError> {
    floor_div_fee(amount, fund_fee_rate, FEE_DENOMINATOR)
}

/// `Fees::split_creator_fee`: the creator's share of a combined input fee.
pub fn split_creator_fee(
    total_fee: u128,
    trade_fee_rate: u64,
    creator_fee_rate: u64,
) -> Result<u128, MathError> {
    floor_div_fee(
        total_fee,
        creator_fee_rate,
        trade_fee_rate
            .checked_add(creator_fee_rate)
            .ok_or(MathError::Overflow)?,
    )
}

/// `Fees::calculate_pre_fee_amount`: gross up a post-fee amount, rounded up.
pub fn calculate_pre_fee_amount(
    post_fee_amount: u128,
    fee_rate: u64,
) -> Result<u128, MathError> {
    if fee_rate == 0 {
        return Ok(post_fee_amount);
    }
    let numerator = post_fee_amount
        .checked_mul(FEE_DENOMINATOR as u128)
        .ok_or(MathError::Overflow)?;
    let denominator = (FEE_DENOMINATOR as u128)
        .checked_sub(fee_rate as u128)
        .ok_or(MathError::Underflow)?;
    // A rate equal to the denominator means a 100% fee: no finite gross amount
    // nets to `post_fee_amount`, so refuse instead of dividing by zero.
    let round_up = denominator.checked_sub(1).ok_or(MathError::Underflow)?;
    numerator
        .checked_add(round_up)
        .ok_or(MathError::Overflow)?
        .checked_div(denominator)
        .ok_or(MathError::DivideByZero)
}

/// Pool reserves net of accrued protocol, fund and creator fees.
///
/// Transcribed from `PoolState::vault_amount_without_fee`. Reserves that have not
/// had their accrued fees removed would let a swap price against liquidity that
/// is already spoken for, and would misprice `elasticity` (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccruedFees {
    pub protocol_fees_token_0: u64,
    pub fund_fees_token_0: u64,
    pub creator_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub fund_fees_token_1: u64,
    pub creator_fees_token_1: u64,
}

impl AccruedFees {
    fn total_0(&self) -> Result<u128, MathError> {
        (self.protocol_fees_token_0 as u128)
            .checked_add(self.fund_fees_token_0 as u128)
            .and_then(|v| v.checked_add(self.creator_fees_token_0 as u128))
            .ok_or(MathError::Overflow)
    }

    fn total_1(&self) -> Result<u128, MathError> {
        (self.protocol_fees_token_1 as u128)
            .checked_add(self.fund_fees_token_1 as u128)
            .and_then(|v| v.checked_add(self.creator_fees_token_1 as u128))
            .ok_or(MathError::Overflow)
    }

    /// `(vault_0 - fees_0, vault_1 - fees_1)`.
    pub fn net_reserves(
        &self,
        vault_0: u64,
        vault_1: u64,
    ) -> Result<(u128, u128), MathError> {
        let a = (vault_0 as u128)
            .checked_sub(self.total_0()?)
            .ok_or(MathError::Underflow)?;
        let b = (vault_1 as u128)
            .checked_sub(self.total_1()?)
            .ok_or(MathError::Underflow)?;
        Ok((a, b))
    }
}

/// `delta_y = delta_x * y / (x + delta_x)`, floored, as the pool computes it.
pub fn swap_base_input_without_fees(
    input_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
) -> Result<u128, MathError> {
    let numerator = input_amount
        .checked_mul(output_vault_amount)
        .ok_or(MathError::Overflow)?;
    let denominator = input_vault_amount
        .checked_add(input_amount)
        .ok_or(MathError::Overflow)?;
    numerator.checked_div(denominator).ok_or(MathError::DivideByZero)
}

/// Every field of the pool's `SwapResult`, reproduced so callers can assert on
/// the fee split and not just the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SwapResult {
    pub new_input_vault_amount: u128,
    pub new_output_vault_amount: u128,
    pub input_amount: u128,
    pub output_amount: u128,
    pub trade_fee: u128,
    pub protocol_fee: u128,
    pub fund_fee: u128,
    pub creator_fee: u128,
}

/// `CurveCalculator::swap_base_input`, fee for fee.
pub fn swap_base_input(
    input_amount: u128,
    input_vault_amount: u128,
    output_vault_amount: u128,
    rates: &FeeRates,
) -> Result<SwapResult, MathError> {
    rates.validate()?;

    let mut creator_fee = 0u128;
    let trade_fee;

    let input_amount_less_fees = if rates.is_creator_fee_on_input {
        let total_fee = trading_fee(input_amount, rates.input_rate()?)?;
        creator_fee = split_creator_fee(total_fee, rates.trade_fee_rate, rates.creator_fee_rate)?;
        trade_fee = total_fee.checked_sub(creator_fee).ok_or(MathError::Underflow)?;
        input_amount.checked_sub(total_fee).ok_or(MathError::Underflow)?
    } else {
        trade_fee = trading_fee(input_amount, rates.trade_fee_rate)?;
        input_amount.checked_sub(trade_fee).ok_or(MathError::Underflow)?
    };

    let protocol_fee = protocol_fee(trade_fee, rates.protocol_fee_rate)?;
    let fund_fee = fund_fee(trade_fee, rates.fund_fee_rate)?;

    let output_amount_swapped = swap_base_input_without_fees(
        input_amount_less_fees,
        input_vault_amount,
        output_vault_amount,
    )?;

    let output_amount = if rates.is_creator_fee_on_input {
        output_amount_swapped
    } else {
        creator_fee = self::creator_fee(output_amount_swapped, rates.creator_fee_rate)?;
        output_amount_swapped.checked_sub(creator_fee).ok_or(MathError::Underflow)?
    };

    Ok(SwapResult {
        new_input_vault_amount: input_vault_amount
            .checked_add(input_amount_less_fees)
            .ok_or(MathError::Overflow)?,
        new_output_vault_amount: output_vault_amount
            .checked_sub(output_amount_swapped)
            .ok_or(MathError::Underflow)?,
        input_amount,
        output_amount,
        trade_fee,
        protocol_fee,
        fund_fee,
        creator_fee,
    })
}

/// Token-2022 transfer fee withheld when `amount` moves, for one epoch's rate.
///
/// Transcribed from `TransferFee::calculate_fee` (`extension/mod.rs`, `tfi.rs:58`):
///
/// ```text
/// transfer_fee = min(ceil(amount * transfer_fee_basis_points / 10_000), maximum_fee)
/// ```
///
/// Both `epoch_fee_bps` and `maximum_fee` are fields of the *epoch-resolved*
/// `TransferFee` entry, and which entry applies is decided by comparing the
/// mint's `newer_transfer_fee.epoch` / `current_transfer_fee.epoch` against
/// `current_epoch` (PLAN.md V1: a queued change lands at `epoch + 2`). That
/// resolution reads the mint, so it belongs to the venue adapter (T8) and the
/// state layer (T4), not to this pure function — which is why no epoch is
/// accepted here. Pass the rate that epoch selected.
///
/// Note `maximum_fee` is a real cap, not decoration: at a high enough amount
/// `ceil(amount * bps / 10_000)` exceeds it and the fee stops growing.
pub fn transfer_fee_withheld(
    amount: u64,
    epoch_fee_bps: u16,
    maximum_fee: u64,
) -> Result<u64, MathError> {
    if epoch_fee_bps > 10_000 {
        // Token-2022 caps the rate at MAX_FEE_BASIS_POINTS and would misbehave
        // above it; refuse rather than compute a fee it would never charge.
        return Err(MathError::Unrepresentable);
    }
    let raw = ceil_div_fee(amount as u128, epoch_fee_bps as u64, 10_000)?;
    u64::try_from(raw.min(maximum_fee as u128)).map_err(|_| MathError::Unrepresentable)
}

/// Token-2022 `TransferFee::calculate_pre_fee_amount` (`tfi.rs:89`): the gross
/// amount that must move to net `post_fee_amount`.
///
/// This is the *inbound* gross-up the plan requires (A4): the pool feeds
/// `amount_in - transfer_fee` to the curve, so sizing a treasury swap has to
/// invert the fee on the quote leg. It is deliberately **not** applied to
/// `minimum_amount_out` — see [`min_out_from_net`].
///
/// Upstream's documented edge cases are reproduced: a zero rate is the identity,
/// a zero net amount is zero, a 100% rate returns `post_fee + maximum_fee`, and
/// where rounding makes the pre-fee amount non-unique the smaller one is chosen.
pub fn token_2022_pre_fee_amount(
    post_fee_amount: u64,
    epoch_fee_bps: u16,
    maximum_fee: u64,
) -> Result<u64, MathError> {
    if epoch_fee_bps > 10_000 {
        return Err(MathError::Unrepresentable);
    }
    let bps = epoch_fee_bps as u128;
    let max = maximum_fee as u128;
    match (bps, post_fee_amount) {
        (0, _) => Ok(post_fee_amount),
        (_, 0) => Ok(0),
        (10_000, _) => {
            // A 100% fee means the transfer never succeeds; upstream returns the
            // post-fee amount plus the maximum fee. Reproduced for completeness,
            // though the plan caps our own fee at 50% so this is unreachable.
            post_fee_amount
                .checked_add(maximum_fee)
                .ok_or(MathError::Overflow)
        }
        _ => {
            let numerator = (post_fee_amount as u128)
                .checked_mul(10_000)
                .ok_or(MathError::Overflow)?;
            let denominator = (10_000u128)
                .checked_sub(bps)
                .ok_or(MathError::Underflow)?;
            let raw = numerator
                .checked_add(denominator - 1)
                .ok_or(MathError::Overflow)?
                .checked_div(denominator)
                .ok_or(MathError::DivideByZero)?;
            if raw
                .checked_sub(post_fee_amount as u128)
                .ok_or(MathError::Underflow)?
                >= max
            {
                post_fee_amount.checked_add(maximum_fee).ok_or(MathError::Overflow)
            } else {
                u64::try_from(raw).map_err(|_| MathError::Overflow)
            }
        }
    }
}

/// Amount that actually lands in a destination after the Token-2022 fee.
///
/// The pool does `amount_in - fee` with a saturating subtraction, so we do too.
pub fn net_after_transfer_fee(amount: u64, withheld: u64) -> Result<u64, MathError> {
    Ok(amount.saturating_sub(withheld))
}

/// `minimum_amount_out` for a swap, from the target net receipt.
///
/// This settles the gross-up question A3 asked. The pool source computes
/// `amount_received = amount_out - output_transfer_fee` and compares *that*
/// against `minimum_amount_out`, so the threshold is a **net** figure. Grossing
/// it up with [`token_2022_pre_fee_amount`] would make the check strictly
/// tighter than the plan asks for, and a correctly priced swap could revert.
///
/// The desired net is already a whole number of raw units, so there is nothing to
/// round. Solvency does not rest on this call site anyway:
/// [`assert_min_out_satisfied`] recomputes the fee and re-checks after the CPI.
pub fn min_out_from_net(desired_net_out: u128) -> Result<u64, MathError> {
    u64::try_from(desired_net_out).map_err(|_| MathError::Unrepresentable)
}

/// Re-check a pool-reported `amount_out` against our own quote.
///
/// Called after the CPI. Two failure modes, both closed here:
///
/// * the pool delivered less net than `min_out` required, which the pool should
///   have caught but which we do not take on trust;
/// * the pool delivered *more* than we priced, which means the reserves the quote
///   was built from are no longer the reserves it settled against. The swap may
///   have been better than advertised, but the price we sized the treasury
///   defence on was not the price that traded, so the decision is unsound.
pub fn assert_min_out_satisfied(
    quoted_output_amount: u128,
    realised_amount_out: u64,
    epoch_fee_bps: u16,
    maximum_fee: u64,
    min_out_net: u64,
) -> Result<(), MathError> {
    if realised_amount_out < min_out_net {
        return Err(MathError::MinOutViolated);
    }
    let withheld = transfer_fee_withheld(realised_amount_out, epoch_fee_bps, maximum_fee)?;
    let net = net_after_transfer_fee(realised_amount_out, withheld)?;
    if net < min_out_net {
        return Err(MathError::MinOutViolated);
    }
    if (realised_amount_out as u128) > quoted_output_amount {
        return Err(MathError::StaleQuote);
    }
    Ok(())
}

/// bps of price move caused by trading 1% of the input reserve (PLAN.md §6.4).
///
/// `elasticity` in the plan. For a constant-product curve the answer turns out
/// to be a **constant**, independent of the reserve ratio, which is worth knowing
/// because it means this number can never be gamed by choosing a pool:
///
/// ```text
/// dy = dx*y/(x+dx)                      so y' = y*x/(x+dx)
/// token price (quote per token) = x/y so price' = x'/y' = (x+dx)/x * (x+dx)/x
/// relative move = (1+f)^2 - 1 = 2f + f^2,  f = dx/x
/// ```
///
/// At `f = 1%` that is `2 + 0.01 = 2.01%`, i.e. **201 bps**, for every CPMM
/// pool. Selling the token instead of buying it gives the same magnitude with the
/// opposite sign. `docs/VERIFIED.md` records this; `elasticity_is_reserve_independent`
/// checks the closed form against real simulated swaps at four reserve ratios.
pub const ELASTICITY_BPS_PER_ONE_PERCENT: u32 = 201;

/// Price move, in bps, from trading `input_reserve_fraction_bps` of the input
/// reserve: `2f + f^2` in bps, rounded up.
pub fn price_impact_bps(input_reserve_fraction_bps: u32) -> Result<u32, MathError> {
    let bps = input_reserve_fraction_bps as u128;
    // A trade of the whole reserve would be f = 1; refuse rather than report a
    // meaningless number for an input no pool would accept.
    if bps >= 10_000 {
        return Err(MathError::Unrepresentable);
    }
    let linear = bps.checked_mul(2).ok_or(MathError::Overflow)?;
    let quadratic = fixed::ceil_div(
        bps.checked_mul(bps).ok_or(MathError::Overflow)?,
        10_000,
    )
    .ok_or(MathError::DivideByZero)?;
    u32::try_from(linear.checked_add(quadratic).ok_or(MathError::Overflow)?)
        .map_err(|_| MathError::Unrepresentable)
}

/// §6.4's stored `elasticity`. A constant, per the derivation above; the
/// signature stays reserve-free so a reader does not assume otherwise.
pub fn elasticity() -> u32 {
    ELASTICITY_BPS_PER_ONE_PERCENT
}

/// Q32.32 observation price to 1e9 fixed point, for the volatility module.
///
/// Lossy in the direction of the floor: an observation that rounds down makes the
/// range look marginally narrower, so [`crate::math::volatility`] still rounds its
/// own result up.
pub fn q32_32_to_fixed(value: u128) -> Result<u128, MathError> {
    let scaled = value.checked_mul(SCALE).ok_or(MathError::Overflow)?;
    scaled.checked_div(1u128 << 32).ok_or(MathError::DivideByZero)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A standard Raydium CPMM pool: 0.025% trade fee, no creator fee, 0.12%
    /// protocol, 0.5% fund. These are the rates of a mainstream pool and the
    /// ones any mirror of the math must reproduce. Rates are out of 1e6, so
    /// `1200` is 0.12%.
    fn standard_rates() -> FeeRates {
        FeeRates {
            trade_fee_rate: 250,
            creator_fee_rate: 0,
            protocol_fee_rate: 1_200,
            fund_fee_rate: 5_000,
            is_creator_fee_on_input: false,
        }
    }

    #[test]
    fn fee_helpers_round_the_way_upstream_does() {
        // ceil: 1000 * 250 / 1e6 = 0.25 -> 1
        assert_eq!(trading_fee(1_000, 250).unwrap(), 1);
        // ceil: 3999 * 250 / 1e6 = 0.99975 -> 1
        assert_eq!(trading_fee(3_999, 250).unwrap(), 1);
        // ceil: 4000 * 250 / 1e6 = 1.0 -> 1
        assert_eq!(trading_fee(4_000, 250).unwrap(), 1);
        // floor: 4000 * 250 / 1e6 = 1.0 -> 1
        assert_eq!(protocol_fee(4_000, 250).unwrap(), 1);
        // floor: 3999 * 250 / 1e6 = 0.99975 -> 0
        assert_eq!(protocol_fee(3_999, 250).unwrap(), 0);
    }

    #[test]
    fn pre_fee_amount_grosses_up() {
        // 1000 post-fee at 250/1e6 needs 1000 * 1e6 / 999750 = 1000.25 -> 1001
        assert_eq!(calculate_pre_fee_amount(1_000, 250).unwrap(), 1_001);
        // Zero fee is the identity, not a division.
        assert_eq!(calculate_pre_fee_amount(1_000, 0).unwrap(), 1_000);
        // Rate == denominator would divide by zero; rejected.
        assert_eq!(
            calculate_pre_fee_amount(1_000, FEE_DENOMINATOR),
            Err(MathError::Underflow)
        );
    }

    #[test]
    fn constant_product_matches_the_closed_form() {
        // x = y = 1000, dx = 100 -> dy = 100*1000/1100 = 90.9 -> 90
        assert_eq!(swap_base_input_without_fees(100, 1_000, 1_000).unwrap(), 90);
        // x = 2000, y = 1000, dx = 500 -> dy = 500*1000/2500 = 200
        assert_eq!(swap_base_input_without_fees(500, 2_000, 1_000).unwrap(), 200);
        // Sweep a range against the identity (x+dx)(y-dy) == x*y + remainder.
        for dx in 0u128..500 {
            for (x, y) in [(1_000u128, 1_000u128), (7_777, 31_337), (10u128.pow(12), 10u128.pow(13))] {
                let dy = swap_base_input_without_fees(dx, x, y).unwrap();
                assert!(dy <= y, "drained the pool");
                // Never over-delivers: dy*(x+dx) <= dx*y is the invariant that
                // the k-th invariant in the plan rests on.
                assert!(
                    dy.checked_mul(x + dx).unwrap() <= dx * y,
                    "over-delivered at dx={dx}"
                );
            }
        }
    }

    #[test]
    fn swap_base_input_reproduces_a_hand_computed_swap() {
        // 10 tokens into a 1000/1000 pool, no creator fee. Every number below is
        // the upstream formula written out by hand, independently of this module.
        let r = swap_base_input(10_000_000, 1_000_000_000, 1_000_000_000, &standard_rates()).unwrap();
        assert_eq!(r.input_amount, 10_000_000);
        // trade = ceil(10_000_000 * 250 / 1_000_000) = 2_500 exactly
        assert_eq!(r.trade_fee, 2_500);
        // protocol = floor(2_500 * 1_200 / 1_000_000) = 3
        assert_eq!(r.protocol_fee, 3);
        // fund = floor(2_500 * 5_000 / 1_000_000) = 12 (12.5, floored)
        assert_eq!(r.fund_fee, 12);
        assert_eq!(r.creator_fee, 0);
        // The curve sees 10_000_000 - 2_500 = 9_997_500.
        // dy = floor(9_997_500 * 1e9 / 1_009_997_500) = 9_898_539
        assert_eq!(r.output_amount, 9_898_539);
        assert_eq!(r.new_input_vault_amount, 1_009_997_500);
        assert_eq!(r.new_output_vault_amount, 990_101_461);
        // The protocol and fund fees are withheld from the vault, so the vault
        // only grew by the fee-less input.
        assert_eq!(
            r.protocol_fee + r.fund_fee + r.trade_fee,
            2_500 + 3 + 12
        );
    }

    /// Rounding is a real behavioural difference, so pin it: the pool ceils the
    /// trade fee and floors the protocol fee, and the dust from that floor stays
    /// in the vault rather than going to the protocol.
    #[test]
    fn fee_rounding_matches_upstream_at_the_boundary() {
        // trade fee 1 unit on 4_000 in: 4_000 * 250 / 1e6 = 1.0 -> 1
        assert_eq!(trading_fee(4_000, 250).unwrap(), 1);
        // ...and the protocol's 1.2% of that 1 unit rounds down to nothing.
        assert_eq!(protocol_fee(1, 1_200).unwrap(), 0);
        // One unit more of trade fee still floors to zero: the pool pays the
        // protocol nothing until 2_501 units of trade fee accumulate.
        assert_eq!(protocol_fee(2_500, 1_200).unwrap(), 3);
    }

    #[test]
    fn creator_fee_on_the_output_side_is_subtracted_from_output() {
        let rates = FeeRates {
            trade_fee_rate: 200,
            creator_fee_rate: 100,
            protocol_fee_rate: 0,
            fund_fee_rate: 0,
            is_creator_fee_on_input: false,
        };
        let r = swap_base_input(100_000, 1_000_000, 1_000_000, &rates).unwrap();
        let dy = swap_base_input_without_fees(r.input_amount - r.trade_fee, 1_000_000, 1_000_000).unwrap();
        // creator = ceil(dy * 100 / 1e6)
        let expected_creator = ceil_div_fee(dy, 100, FEE_DENOMINATOR).unwrap();
        assert_eq!(r.creator_fee, expected_creator);
        assert_eq!(r.output_amount, dy - expected_creator);
    }

    #[test]
    fn creator_fee_on_the_input_side_is_split_out_of_the_trade_fee() {
        let rates = FeeRates {
            trade_fee_rate: 200,
            creator_fee_rate: 100,
            protocol_fee_rate: 0,
            fund_fee_rate: 0,
            is_creator_fee_on_input: true,
        };
        let r = swap_base_input(100_000, 1_000_000, 1_000_000, &rates).unwrap();
        // total = ceil(100_000 * 300 / 1e6) = 30; creator = floor(30 * 100/300) = 10
        assert_eq!(r.trade_fee + r.creator_fee, 30);
        assert_eq!(r.creator_fee, 10);
        assert_eq!(r.trade_fee, 20);
        // The creator fee is not taken again from the output.
        assert_eq!(
            r.output_amount,
            swap_base_input_without_fees(100_000 - 30, 1_000_000, 1_000_000).unwrap()
        );
    }

    #[test]
    fn invariant_is_never_broken_and_k_holds() {
        for &is_creator_on_input in &[false, true] {
            for dx in [1u128, 100, 1_000, 50_000, 900_000] {
                let rates = FeeRates {
                    trade_fee_rate: 250,
                    creator_fee_rate: if is_creator_on_input { 0 } else { 50 },
                    protocol_fee_rate: 120,
                    fund_fee_rate: 50,
                    is_creator_fee_on_input: is_creator_on_input,
                };
                let x = 1_000_000u128;
                let y = 1_000_000u128;
                let r = swap_base_input(dx, x, y, &rates).unwrap();
                let k_before = x * y;
                let k_after = r.new_input_vault_amount * r.new_output_vault_amount;
                // Fees accrue to the pool, so k only ever grows.
                assert!(k_after >= k_before, "k shrank: {k_before} -> {k_after}");
                assert!(r.output_amount < y, "output exceeded the reserve");
            }
        }
    }

    #[test]
    fn net_reserves_subtract_every_accrued_fee() {
        let fees = AccruedFees {
            protocol_fees_token_0: 100,
            fund_fees_token_0: 20,
            creator_fees_token_0: 5,
            protocol_fees_token_1: 1,
            fund_fees_token_1: 2,
            creator_fees_token_1: 3,
        };
        assert_eq!(fees.net_reserves(1_000, 1_000).unwrap(), (875, 994));
        // Accrued fees larger than the vault mean the pool state is inconsistent;
        // underflow is reported, never wrapped into a huge reserve.
        assert_eq!(fees.net_reserves(100, 1_000), Err(MathError::Underflow));
        assert_eq!(
            AccruedFees::default().net_reserves(0, 0).unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn transfer_fee_matches_token_2022_rounding() {
        // ceil(amount * bps / 10_000), with no cap in play.
        let uncapped = u64::MAX;
        assert_eq!(transfer_fee_withheld(1_000, 100, uncapped).unwrap(), 10);
        assert_eq!(transfer_fee_withheld(999, 100, uncapped).unwrap(), 10); // rounds up
        assert_eq!(transfer_fee_withheld(10_000, 1_000, uncapped).unwrap(), 1_000);
        assert_eq!(transfer_fee_withheld(1, 5_000, uncapped).unwrap(), 1); // 0.5 -> 1
        assert_eq!(transfer_fee_withheld(0, 5_000, uncapped).unwrap(), 0);
        // A rate Token-2022 would reject is refused here too.
        assert_eq!(
            transfer_fee_withheld(1_000, 10_001, uncapped),
            Err(MathError::Unrepresentable)
        );
    }

    /// `maximum_fee` is a real cap: past it the fee stops growing. Reproducing the
    /// cap is what keeps a quote honest for very large transfers.
    #[test]
    fn maximum_fee_caps_the_transfer_fee() {
        // 1 bps on 1_000_000 tokens would be 100_000 raw, but the cap is 5_000.
        assert_eq!(transfer_fee_withheld(1_000_000_000, 1, 5_000).unwrap(), 5_000);
        // Just under the cap, the rate still governs.
        assert_eq!(transfer_fee_withheld(50_000_000, 1, 5_000).unwrap(), 5_000);
        assert_eq!(transfer_fee_withheld(49_999_999, 1, 5_000).unwrap(), 5_000);
        assert_eq!(transfer_fee_withheld(49_990_000, 1, 5_000).unwrap(), 4_999);
    }

    /// The inbound gross-up, including upstream's three documented edge cases.
    #[test]
    fn token_2022_pre_fee_amount_inverts_the_fee() {
        let uncapped = u64::MAX;
        // To net 1_000 through a 1% fee, 1_011 must move.
        assert_eq!(token_2022_pre_fee_amount(1_000, 100, uncapped).unwrap(), 1_011);
        // No fee is the identity; a zero net is zero.
        assert_eq!(token_2022_pre_fee_amount(1_000, 0, uncapped).unwrap(), 1_000);
        assert_eq!(token_2022_pre_fee_amount(0, 100, uncapped).unwrap(), 0);
        // Where rounding makes the answer non-unique, the smaller one is chosen:
        // both 10 and 11 net 9 at 10%, and upstream picks 10.
        assert_eq!(token_2022_pre_fee_amount(9, 1_000, uncapped).unwrap(), 10);
        // 100% returns post-fee + maximum fee.
        assert_eq!(token_2022_pre_fee_amount(500, 10_000, 77).unwrap(), 577);
        // Below the cap, the rate alone governs: 1_000_000 * 10_000/9_999.
        assert_eq!(token_2022_pre_fee_amount(1_000_000, 1, 5_000).unwrap(), 1_000_101);
        // Once the implied fee would reach the cap, the gross-up is pinned at
        // net + maximum_fee instead of growing further.
        assert_eq!(token_2022_pre_fee_amount(50_000_000, 1, 5_000).unwrap(), 50_005_000);
        assert_eq!(
            token_2022_pre_fee_amount(1_000, 10_001, uncapped),
            Err(MathError::Unrepresentable)
        );
    }

    /// The two legs use opposite helpers, and the reason is worth pinning: the
    /// output leg must not be grossed up, the input leg must.
    #[test]
    fn inbound_and_outbound_gross_up_are_not_interchangeable() {
        // Outbound: min_out is net, exactly as asked, no gross-up.
        assert_eq!(min_out_from_net(1_000_000).unwrap(), 1_000_000);
        // Inbound: to actually spend 1_000_000 of quote through the pool we must
        // move more, because the curve is fed the net amount.
        assert_eq!(
            token_2022_pre_fee_amount(1_000_000, 100, u64::MAX).unwrap(),
            1_010_102
        );
    }

    #[test]
    fn min_out_is_a_net_threshold_and_post_state_is_rechecked() {
        let bps = 2_000u16; // 20% Token-2022 fee on the output
        let cap = u64::MAX;
        let desired_net = 1_000_000u128;
        // Passing a net threshold: the pool compares net receipt against it.
        let min_out = min_out_from_net(desired_net).unwrap();
        assert_eq!(min_out, 1_000_000);

        // To net 1_000_000 through a 20% Token-2022 fee the pool must move
        // 1_250_000 gross. That gross figure is the pool's problem, not ours.
        let gross = transfer_fee_withheld(1_250_000, bps, cap).unwrap();
        assert_eq!(gross, 250_000);
        assert_eq!(net_after_transfer_fee(1_250_000, gross).unwrap(), min_out);

        // A realised swap that nets below the threshold reverts.
        assert_eq!(
            assert_min_out_satisfied(1_250_000, 1_000_000, bps, cap, min_out),
            Err(MathError::MinOutViolated)
        );
        // The healthy case passes.
        assert!(assert_min_out_satisfied(1_250_000, 1_250_000, bps, cap, min_out).is_ok());
        // Delivering more than we priced means our reserves were stale.
        assert_eq!(
            assert_min_out_satisfied(1_250_000, 1_300_000, bps, cap, min_out),
            Err(MathError::StaleQuote)
        );
    }

    #[test]
    fn malformed_fee_rates_are_refused() {
        assert_eq!(
            FeeRates {
                trade_fee_rate: FEE_DENOMINATOR,
                ..standard_rates()
            }
            .validate(),
            Err(MathError::Unrepresentable)
        );
        assert_eq!(
            FeeRates {
                protocol_fee_rate: FEE_DENOMINATOR + 1,
                ..standard_rates()
            }
            .validate(),
            Err(MathError::Unrepresentable)
        );
        let ok = standard_rates();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn extreme_reserves_do_not_panic() {
        let rates = standard_rates();
        for &(dx, x, y) in &[
            (0u128, 0u128, 0u128),
            (u128::MAX, 1, 1),
            (1, u128::MAX, u128::MAX),
            (u64::MAX as u128, u64::MAX as u128, 1),
            (u128::MAX / 2, u128::MAX / 2, u128::MAX / 3),
        ] {
            let _ = swap_base_input(dx, x, y, &rates);
            let _ = swap_base_input_without_fees(dx, x, y);
        }
    }

    /// §6.4's elasticity is a constant for a constant-product pool. Rather than
    /// assert the formula against itself, drive it from real swaps at four very
    /// different reserve ratios.
    #[test]
    fn elasticity_is_reserve_independent() {
        let closed_form = price_impact_bps(100).unwrap();
        assert_eq!(closed_form, 201);
        assert_eq!(elasticity(), ELASTICITY_BPS_PER_ONE_PERCENT);

        for &(x, y) in &[
            (1_000_000u128, 1_000_000u128),   // 1:1
            (100_000_000, 1_000_000),        // 100:1
            (1_000_000, 100_000_000),        // 1:100
            (12_345_678, 98_765_432),        // arbitrary
            (10_000_000_000_000u128, 10_000_000_000_000), // large enough not to truncate
        ] {
            let dx = x / 100;
            let dy = swap_base_input_without_fees(dx, x, y).unwrap();

            // Price of the token in quote, before and after, as an exact rational
            // difference so no intermediate rounding can hide the answer:
            //   move = (x+dx)/(y-dy) / (x/y) - 1
            //        = ((x+dx)*y - x*(y-dy)) / (x*(y-dy))
            let numerator = (x + dx) * y - x * (y - dy);
            let denominator = x * (y - dy);
            let move_bps = (numerator * 10_000 / denominator) as u32;

            // The realised move never exceeds the closed form (the pool floors
            // `dy`, which can only lose), and is within one bps of it.
            assert!(
                (200..=201).contains(&move_bps),
                "x={x} y={y}: realised {move_bps} bps, closed form {closed_form}"
            );
        }
    }

    /// `2f + f^2`, rounded up, and the edges of the domain.
    #[test]
    fn price_impact_is_two_f_plus_f_squared() {
        assert_eq!(price_impact_bps(0).unwrap(), 0);
        assert_eq!(price_impact_bps(50).unwrap(), 101); // 1% + 0.25%
        assert_eq!(price_impact_bps(100).unwrap(), 201); // 2% + 1%
        assert_eq!(price_impact_bps(200).unwrap(), 404); // 4% + 4%
        assert_eq!(price_impact_bps(9_999).unwrap(), 19_998 + 9_999);
        // Trading the entire reserve is not a price impact any pool expresses.
        assert_eq!(price_impact_bps(10_000), Err(MathError::Unrepresentable));
        assert_eq!(price_impact_bps(u32::MAX), Err(MathError::Unrepresentable));
    }

    #[test]
    fn q32_32_conversion_round_trips_known_values() {
        // 1.0 in Q32.32 is 1 << 32.
        assert_eq!(q32_32_to_fixed(1u128 << 32).unwrap(), SCALE);
        assert_eq!(q32_32_to_fixed(1u128 << 31).unwrap(), SCALE / 2);
        // Values below the fixed-point resolution floor to zero rather than wrap.
        assert_eq!(q32_32_to_fixed(0).unwrap(), 0);
        assert_eq!(q32_32_to_fixed(1).unwrap(), 0);
    }

    proptest! {
        /// The fee helpers must never produce a fee larger than the amount, and
        /// must never panic on any rate the pool could legally hold.
        #[test]
        fn fees_are_bounded_by_the_amount(
            amount in 0u128..(1u128 << 64),
            rate in 0u64..=FEE_DENOMINATOR,
        ) {
            let fee = trading_fee(amount, rate).unwrap();
            prop_assert!(fee <= amount);
        }

        /// Protocol and fund fees are floors, so they never exceed the trade fee.
        #[test]
        fn protocol_fee_never_exceeds_trade_fee(
            amount in 0u128..(1u128 << 40),
            trade_rate in 0u64..100_000,
            protocol_rate in 0u64..100_000,
        ) {
            let trade = trading_fee(amount, trade_rate).unwrap();
            let proto = protocol_fee(trade, protocol_rate).unwrap();
            prop_assert!(proto <= trade);
        }

        /// No input may panic, whatever the reserves.
        #[test]
        fn swap_never_panics(
            dx in any::<u128>(),
            x in any::<u128>(),
            y in any::<u128>(),
            creator_rate in 0u64..1_000,
            on_input in any::<bool>(),
        ) {
            let _ = swap_base_input_without_fees(dx, x, y);
            let rates = FeeRates {
                trade_fee_rate: 250,
                creator_fee_rate: creator_rate,
                protocol_fee_rate: 120,
                fund_fee_rate: 50,
                is_creator_fee_on_input: on_input,
            };
            let _ = swap_base_input(dx, x, y, &rates);
        }

        /// Constant product never over-delivers relative to its own invariant.
        #[test]
        fn constant_product_never_over_delivers(
            dx in 0u128..10_000,
            x in 1u128..10_000,
            y in 1u128..10_000,
        ) {
            let dy = swap_base_input_without_fees(dx, x, y).unwrap();
            prop_assume!(dx > 0);
            prop_assert!(dy <= y);
            prop_assert!(dy * (x + dx) <= dx * y);
        }

        /// Larger input never yields a smaller output (monotonicity).
        #[test]
        fn output_is_monotone_in_input(
            small in 1u128..10_000,
            large in 1u128..10_000,
        ) {
            prop_assume!(large > small);
            let x = 1_000_000u128;
            let y = 1_000_000u128;
            let a = swap_base_input_without_fees(small, x, y).unwrap();
            let b = swap_base_input_without_fees(large, x, y).unwrap();
            prop_assert!(b >= a);
        }

        /// The Token-2022 fee is never more than the amount moved.
        #[test]
        fn transfer_fee_bounded(amount in 0u64..(1u64 << 50), bps in 0u16..=10_000) {
            let fee = transfer_fee_withheld(amount, bps, u64::MAX).unwrap();
            prop_assert!(fee <= amount);
            // And it is at least the floor, i.e. rounding is up, never down.
            let floor = (amount as u128 * bps as u128) / 10_000;
            prop_assert!((fee as u128) >= floor);
        }

        /// The cap never produces a fee larger than the cap or the amount.
        #[test]
        fn transfer_fee_respects_the_cap(
            amount in 0u64..(1u64 << 50),
            bps in 0u16..=10_000,
            cap in 0u64..(1u64 << 40),
        ) {
            let fee = transfer_fee_withheld(amount, bps, cap).unwrap();
            prop_assert!(fee <= amount);
            prop_assert!(fee <= cap);
        }

        /// Grossing a net amount up and then taking the fee off it can never land
        /// short of the requested net.
        #[test]
        fn pre_fee_amount_always_covers_the_net(
            net in 1u64..(1u64 << 40),
            bps in 1u16..10_000,
            cap in 1u64..(1u64 << 30),
        ) {
            let gross = token_2022_pre_fee_amount(net, bps, cap).unwrap();
            let withheld = transfer_fee_withheld(gross, bps, cap).unwrap();
            prop_assert!(gross - withheld >= net);
        }
    }
}