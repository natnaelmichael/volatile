//! `TreasuryState` — counters and the vault accounts the program owns. PLAN.md
//! §4.3.
//!
//! `token_vault` / `quote_vault` are created at `initialize` and funded by the
//! developer in the same transaction (B3); this account only needs their
//! addresses and the bumps the program signs with when moving funds.

use anchor_lang::prelude::*;

/// PDA `["treasury", mint]`.
#[account]
#[derive(InitSpace)]
pub struct TreasuryState {
    /// The mint this treasury belongs to (defensive).
    pub mint: Pubkey,
    /// The program's token account (`["treasury_vault_owner", mint]`),
    /// destination of harvested fees and of BuyDefend purchases.
    pub token_vault: Pubkey,
    /// The program's quote account, source of BuyDefend funds.
    pub quote_vault: Pubkey,
    /// Cumulative fees harvested (mint -> `token_vault`), raw units.
    pub total_fees_harvested: u64,
    /// Cumulative quote spent on BuyDefend, raw units.
    pub total_quote_spent: u64,
    /// Cumulative quote received on any disposal, raw units. Kept even though
    /// v1 never disposes, so an audit trail exists from day one.
    pub total_quote_gained: u64,
    pub bump: u8,
    /// Bump of the signer for transfers out of `token_vault`.
    pub token_vault_bump: u8,
    /// Bump of the signer for transfers out of `quote_vault`.
    pub quote_vault_bump: u8,
}

impl TreasuryState {
    pub const LEN: usize = 8 + Self::INIT_SPACE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treasury_layout_is_stable() {
        // 3 pubkeys + 3 u64 counters + 3 bumps.
        assert_eq!(TreasuryState::INIT_SPACE, 3 * 32 + 3 * 8 + 3);
        assert_eq!(TreasuryState::LEN, 8 + TreasuryState::INIT_SPACE);
    }
}