//! `ActiveAction` — the one in-flight fast-layer action. PLAN.md §4.5, D16.
//!
//! There can be at most one active action. A RaiseFee may be raised further
//! within the weekly cap; nothing else overwrites an active action. With
//! `revert_action` removed (D16), an action is cleared when the weekly
//! `queue_scheduled_fee` writes the new week's base fee over the pending change.

use anchor_lang::prelude::*;

/// What the fast layer decided at `poll_and_act` (PLAN.md §5.2 step 4).
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub enum ActionKind {
    /// Queue a higher transfer fee, effective at `effective_epoch`.
    #[default]
    RaiseFee,
    /// Buy the token with treasury quote to arrest a falling price.
    BuyDefend,
}

/// PDA `["action", mint]`. Present and `active == true` while an action awaits
/// or has recently reached effect; cleared by the weekly fee queue (D16).
#[account]
#[derive(InitSpace)]
pub struct ActiveAction {
    /// The mint this action belongs to (defensive).
    pub mint: Pubkey,
    pub kind: ActionKind,
    /// When `poll_and_act` start-queued the action.
    pub started_ts: i64,
    /// Epoch the action was queued in.
    pub started_epoch: u64,
    /// The fee the mint held before this action, bps.
    pub fee_before_bps: u16,
    /// The fee this action queued, bps.
    pub fee_after_bps: u16,
    /// Epoch the native change activates at: `started_epoch + FEE_CHANGE_DELAY_EPOCHS`.
    pub effective_epoch: u64,
    /// Quote spent so far (BuyDefend), raw units. 0 for a RaiseFee.
    pub quote_used: u64,
    /// Once the new fee is read back from the mint, the `Clock::unix_timestamp`
    /// of the epoch boundary it actually landed on (records the 5.3 scheduling
    /// error). `0` = not yet observed.
    pub observed_effective_ts: i64,
    /// False until the weekly `queue_scheduled_fee` clears the action.
    pub active: bool,
    pub bump: u8,
}

impl ActiveAction {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    #[inline]
    pub fn is_raise(&self) -> bool {
        self.kind == ActionKind::RaiseFee
    }

    #[inline]
    pub fn raises_strictly(&self, fee_bps: u16) -> bool {
        fee_bps > self.fee_after_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_epoch_is_start_plus_delay() {
        let a = ActiveAction {
            mint: Pubkey::default(),
            kind: ActionKind::RaiseFee,
            started_ts: 0,
            started_epoch: 41,
            fee_before_bps: 100,
            fee_after_bps: 700,
            effective_epoch: 41 + crate::constants::FEE_CHANGE_DELAY_EPOCHS,
            quote_used: 0,
            observed_effective_ts: 0,
            active: true,
            bump: 0,
        };
        assert_eq!(a.effective_epoch, 43);
        assert!(a.is_raise());
        assert!(a.raises_strictly(800));
        assert!(!a.raises_strictly(700));
    }

    #[test]
    fn action_layout_is_stable() {
        // 32 mint + enum + i64 + u64 + 2*u16 + u64 + u64 + i64 + bool + bump.
        let body = 32 + 1 + 8 + 8 + 2 + 2 + 8 + 8 + 8 + 1 + 1;
        assert_eq!(ActiveAction::INIT_SPACE, body);
        assert_eq!(ActiveAction::LEN, 8 + body);
    }
}