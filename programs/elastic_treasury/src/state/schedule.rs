//! `Schedule` — the published weekly parameter array and the fee queue. PLAN.md
//! §4.2, §5.3; B2.
//!
//! A week is immutable once published (I4). The program writes entries only in
//! `publish_weekly_params` / `cancel_weekly_params` and moves `current_index`
//! forward by the clock (the entry with the latest `week_effective_ts <= now`).
//!
//! The fee-queue status exists because a second `SetTransferFee` **silently
//! overwrites** a still-pending one (docs/VERIFIED.md V2/A2), so the program
//! must know what it already queued before deciding to queue again.

use anchor_lang::prelude::*;

use crate::constants::MAX_SCHEDULE_SLOTS;

/// A single published week. Empty when `week_effective_ts == 0`, which is not a
/// valid Monday 00:01 UTC timestamp and therefore unambiguous.
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub struct WeeklyParams {
    /// Monday 00:01 UTC when this entry starts (D8/D14). The empty-slot marker.
    pub week_effective_ts: i64,
    /// When `publish_weekly_params` wrote the entry, for audit.
    pub published_ts: i64,
    /// Fee the fast layer returns *towards* on a weekly reset. `0..=5000`
    /// (D16: this is the only down-ramp).
    pub base_fee_bps: u16,
    /// Weekly ceiling; `<= MAX_FEE_BPS`. Nothing may exceed it (I6).
    pub max_fee_bps: u16,
    /// Reported only (D11); triggers no spending. `0 < min`.
    pub volatility_min_bps: u16,
    /// The trigger: `vol > volatility_max` starts RaiseFee / BuyDefend.
    pub volatility_max_bps: u16,
    /// Max fraction of the quote vault a single action may spend (I2/I7).
    pub safety_limit_bps: u16,
    /// Must be a version the build implements (v1 at launch, D9).
    pub formula_version: u8,
    /// `= SHORT_HOPS` for now (D13); v2 may vary it.
    pub short_hops: u8,
}

impl WeeklyParams {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.week_effective_ts == 0
    }
}

/// Where a queued fee change came from. Only one pending change exists at a
/// time (V2), so the source decides whether it may be overwritten (B2).
///
/// `Revert` existed here until v3 and was removed with `revert_action` (D16).
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub enum FeeQueueSource {
    /// The weekly crank writing `base_fee_bps`. Always allowed to overwrite.
    #[default]
    ScheduledBase,
    /// An intra-week `poll_and_act` raise. May be raised further, but a pending
    /// `ScheduledBase` takes precedence (PLAN.md §5.3).
    IntraWeekRaise,
}

/// The program's memory of the pending native fee change (B2). Empty when
/// `pending_fee_epoch == 0`.
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub struct FeeQueueStatus {
    /// The queued fee, bps. Meaningless when empty; the written
    /// `newer_transfer_fee.transfer_fee_basis_points`.
    pub pending_fee_bps: u16,
    /// The epoch the change will activate at (`current_epoch + 2`). `0` = empty.
    pub pending_fee_epoch: u64,
    pub pending_fee_source: FeeQueueSource,
    /// Epoch of the last successful queue, so `poll_and_act` knows whether an
    /// intra-week raise can be re-queued this epoch (§5.3 "at most one queued
    /// change per epoch").
    pub last_queued_epoch: u64,
}

impl FeeQueueStatus {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pending_fee_epoch == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Fixed 4-slot window of published weeks + the fee-queue memory. PDA
/// `["schedule", mint]`.
#[account]
#[derive(InitSpace)]
pub struct Schedule {
    /// The mint this schedule belongs to (defensive; the PDA seed is enough).
    pub mint: Pubkey,
    /// Index of the *active* entry, bumped by the clock. Slots below or equal to
    /// `current_index` are the past and are immutable (I4).
    pub current_index: u8,
    /// Array of published weeks, slot 0 being the oldest within the window.
    /// Empty slots are `week_effective_ts == 0`.
    pub entries: [WeeklyParams; MAX_SCHEDULE_SLOTS],
    /// What fee the program has already queued into the mint (A2/B2).
    pub fee_queue: FeeQueueStatus,
    pub bump: u8,
}

impl Schedule {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    /// Index of the entry whose Monday is the latest one at or before `ts`.
    /// Returns `None` for anything before the first entry's Monday.
    pub fn active_slot(&self, ts: i64) -> Option<usize> {
        let mut active: Option<usize> = None;
        for (i, entry) in self.entries.iter().enumerate() {
            if !entry.is_empty() && entry.week_effective_ts <= ts {
                active = Some(i);
            } else {
                break;
            }
        }
        active
    }

    /// The active week at `ts`, if any.
    pub fn active_entry(&self, ts: i64) -> Option<&WeeklyParams> {
        self.active_slot(ts).map(|i| &self.entries[i])
    }

    /// Index of the first empty slot, `None` when the schedule is full.
    pub fn first_free_slot(&self) -> Option<usize> {
        self.entries.iter().position(|e| e.is_empty())
    }

    /// Index of a week that is published but not yet effective, by its Monday.
    pub fn slot_of(&self, ts: i64) -> Option<usize> {
        self.entries.iter().position(|e| e.week_effective_ts == ts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(week_offset: i64) -> WeeklyParams {
        WeeklyParams {
            week_effective_ts: crate::constants::MONDAY_0001_UTC_OFFSET + week_offset * 604_800,
            published_ts: 42,
            base_fee_bps: 100,
            max_fee_bps: 5000,
            volatility_min_bps: 100,
            volatility_max_bps: 3000,
            safety_limit_bps: 1000,
            formula_version: 1,
            short_hops: 4,
        }
    }

    #[test]
    fn empty_slot_is_week_effective_ts_zero() {
        let empty = WeeklyParams::default();
        assert!(empty.is_empty());
        assert!(!slot(0).is_empty());
    }

    #[test]
    fn fee_queue_empty_sentinel_is_epoch_zero() {
        let q = FeeQueueStatus::default();
        assert!(q.is_empty());
        let mut q = q;
        q.pending_fee_epoch = 7;
        assert!(!q.is_empty());
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn active_slot_selects_latest_published_monday() {
        let s = Schedule {
            mint: Pubkey::default(),
            current_index: 0,
            entries: [slot(8), slot(9), WeeklyParams::default(), WeeklyParams::default()],
            fee_queue: FeeQueueStatus::default(),
            bump: 0,
        };
        assert_eq!(s.active_slot(0), None, "before first week has no active entry");
        assert_eq!(s.active_slot(8 * 604_800 + crate::constants::MONDAY_0001_UTC_OFFSET), Some(0));
        assert_eq!(s.active_slot(9 * 604_800 + crate::constants::MONDAY_0001_UTC_OFFSET - 1), Some(0));
        assert_eq!(s.active_slot(9 * 604_800 + crate::constants::MONDAY_0001_UTC_OFFSET), Some(1));
        assert_eq!(s.first_free_slot(), Some(2));
        assert_eq!(s.slot_of(9 * 604_800 + crate::constants::MONDAY_0001_UTC_OFFSET), Some(1));
    }

    #[test]
    fn schedule_layout_is_stable() {
        // 32 mint + 1 current_index + 4 * 28 + 19 queue + 1 bump.
        assert_eq!(Schedule::INIT_SPACE, 32 + 1 + 4 * 28 + 19 + 1);
        assert_eq!(Schedule::LEN, 8 + Schedule::INIT_SPACE);
    }
}