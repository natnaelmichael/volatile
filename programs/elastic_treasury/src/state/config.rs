//! `Config` — the immutable identity + tuned bounds of the program. PLAN.md §4.1.
//!
//! Created once at `initialize`, never mutated after that (developer rotation in
//! T6 uses the two-step flow described in the plan and touches `developer` only).
//! Two fields are program-set at `poll_and_act` time and are kept here for the
//! keeper to read in one account instead of having to reconstruct them.
//!
//! Everything user-supplied is validated against the constants in
//! [`crate::constants`] before it is written; an invalid value reverts with a
//! named error and zero state change (PLAN.md §0.5, I1).

use anchor_lang::prelude::*;

/// Which USD metric gates activation (PLAN.md §4.1, D12, Q5/Q6).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum ActivationKind {
    /// Quote-side reserve value in USD, `quote_vault_balance * quote_usd_price`.
    Liquidity,
    /// `total_mint_supply * token_usd_price`.
    MarketCap,
}

impl Default for ActivationKind {
    fn default() -> Self {
        Self::Liquidity
    }
}

/// The bump bytes the program signs with. Every PDA this program owns is listed
/// here so `signer_seeds` can be rebuilt from one struct (B4; PLAN.md §3).
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub struct Bumps {
    pub config: u8,
    pub schedule: u8,
    pub treasury: u8,
    pub poll: u8,
    pub action: u8,
    pub token_vault: u8,
    pub quote_vault: u8,
    /// The mint's `transfer_fee_config_authority` (the only signer allowed to
    /// call `SetTransferFee`). Verified by `process_set_transfer_fee`.
    pub fee_authority: u8,
    /// The mint's `withdraw_withheld_authority`.
    pub withdraw_authority: u8,
}

/// The whole program identity. PDA `["config", mint]`.
#[account]
#[derive(InitSpace)]
pub struct Config {
    // -- identity ------------------------------------------------------------
    /// The deployer; the only signer for `publish_weekly_params` /
    /// `cancel_weekly_params` (I3). Rotation via the T6 two-step flow.
    pub developer: Pubkey,
    /// The untrusted, locally-hosted agent key; the only signer for
    /// `poll_and_act` (D15, I3). A plain key, recomputed-and-bounded like every
    /// other input.
    pub keeper: Pubkey,
    /// The Token-2022 mint whose `transfer_fee_config_authority` is our PDA.
    pub mint: Pubkey,
    /// The Raydium CPMM pool traded through.
    pub pool: Pubkey,
    pub quote_mint: Pubkey,
    /// Pyth USD feed that prices `quote_mint`.
    pub quote_usd_feed: Pubkey,

    // -- tuning, fixed at initialize -----------------------------------------
    /// Volatility lookback window in hours (D4: default 168), `1..=8760`.
    pub polling_rate_hours: u32,
    /// Which USD metric activates the program (D12).
    pub activation_kind: ActivationKind,
    /// Activation threshold in USD, `> 0`.
    pub activation_value_usd: u64,
    /// One-way latch: `activate` may only set false -> true (I9).
    pub activated: bool,
    /// Pyth staleness in **slots**, not seconds (A7/A8). Default
    /// `DEFAULT_ORACLE_MAX_AGE_SLOTS` (B1).
    pub oracle_max_age_slots: u64,
    /// Reject a quote whose confidence exceeds this fraction of the price
    /// (A9/B1). Default `DEFAULT_ORACLE_MAX_CONF_BPS`.
    pub oracle_max_conf_bps: u32,
    /// The `formula_version` of the *active* week, copied at the Monday
    /// boundary. Program-set; mirrored from `WeeklyParams`.
    pub formula_version: u8,

    // -- program-set, reported for the keeper (PLAN.md §4.1) ------------------
    /// Volatility of the last completed lookback window (6.3).
    pub last_window_volatility_bps: u32,
    /// Token price high-water mark in 1e9 fixed point. Tracked and reported
    /// only; gates nothing (Q4).
    pub high_water_mark: u64,

    /// PDAs this program signs with (B4).
    pub bumps: Bumps,
}

impl Config {
    pub const LEN: usize = 8 + Self::INIT_SPACE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_layout_is_stable() {
        // Anchored account prefix (8) + the fields in declaration order.
        // 6 pubkeys, polling_rate u32, kind enum u8, activation u64, activated
        // bool, oracle_max_age u64, oracle_max_conf u32, formula u8,
        // last_volatility u32, high_water_mark u64, bumps (9 bytes).
        let body = 6 * 32 + 4 + 1 + 8 + 1 + 8 + 4 + 1 + 4 + 8 + 9;
        assert_eq!(Config::INIT_SPACE, body);
        assert_eq!(Config::LEN, 8 + body);
    }

    #[test]
    fn small_types_are_one_byte_enums() {
        assert_eq!(ActivationKind::INIT_SPACE, 1);
        assert_eq!(Bumps::INIT_SPACE, 9);
    }
}