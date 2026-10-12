//! Pool state.

use anchor_lang::prelude::*;

/// A constant-product pool plus the volatility parameters that the swap
/// instruction enforces internally.
///
/// PDA: `[b"pool", token_mint, quote_mint]`.
#[account]
pub struct PoolState {
    /// The mint whose transfers are gated by the transfer hook.
    pub token_mint: Pubkey,
    /// The other side of the pool. Assumed to carry no transfer hook.
    pub quote_mint: Pubkey,
    /// PDA that owns both vaults (signs swap transfers).
    pub pool_authority: Pubkey,
    /// Vault holding `token_mint`.
    pub token_vault: Pubkey,
    /// Vault holding `quote_mint`.
    pub quote_vault: Pubkey,
    /// Time-weighted price of one `token_mint` in `quote_mint`, scaled by
    /// `math::PRICE_SCALE`. Zero until the first trade seeds it.
    pub rolling_twap: u128,
    /// Unix timestamp of the last TWAP update (0 until the first trade).
    pub last_twap_update: i64,
    /// Lower bound, in bps of `rolling_twap`, that the execution price may take.
    pub lower_bound_bps: u32,
    /// Upper bound, in bps of `rolling_twap`, that the execution price may take.
    pub upper_bound_bps: u32,
    /// Swap fee in basis points.
    pub fee_bps: u16,
    /// PDA bump.
    pub bump: u8,
}

impl PoolState {
    /// Anchor discriminator (8) + five pubkeys + u128 + i64 + two u32 + u16 + u8.
    pub const LEN: usize = 8 + (32 * 5) + 16 + 8 + 4 + 4 + 2 + 1;
}
