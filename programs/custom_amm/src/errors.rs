//! Custom errors for the AMM.

use anchor_lang::prelude::*;

#[error_code]
pub enum AmmError {
    /// An amount or parameter was zero or otherwise out of its allowed range.
    ValueOutOfRange,
    /// Arithmetic that must be exact overflowed.
    MathOverflow,
    /// A reserve was zero, so the curve is undefined.
    PoolEmpty,
    /// `token_mint == quote_mint`.
    MintsEqual,
    /// A supplied mint did not match the pool's configured mints.
    MintMismatch,
    /// A vault did not belong to the pool or held the wrong mint.
    InvalidVault,
    /// A vault's authority was not the pool authority PDA.
    VaultAuthorityMismatch,
    /// The curve returned less than `minimum_amount_out`.
    SlippageExceeded,
    /// The execution price left the configured volatility band.
    VolatilityOutOfBounds,
    /// The curve would deliver nothing.
    SwapOutputZero,
    /// A pubkey argument was the default pubkey.
    ZeroAddress,
    /// `fee_bps` exceeded the program cap.
    FeeExceedsCap,
    /// `lower_bound_bps == 0` or `lower_bound_bps >= upper_bound_bps`.
    BoundsInverted,
    /// The supplied hook program / registry / meta-list do not line up.
    HookAccountsMismatch,
}
