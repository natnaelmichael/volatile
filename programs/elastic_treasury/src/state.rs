//! Program-owned accounts. PLAN.md §4.
//!
//! Layouts here are a stable ABI (the keeper reads them, T3 freezes them), so
//! every account also has a literal size test and a serialization round-trip
//! test in the `tests` module at the bottom of this file.

pub mod action;
pub mod config;
pub mod poll;
pub mod schedule;
pub mod treasury;

#[cfg(test)]
mod tests {
    use anchor_lang::prelude::*;

    use super::action::{ActiveAction, ActionKind};
    use super::config::{ActivationKind, Bumps, Config};
    use super::poll::{PollState, StoredPriceSample};
    use super::schedule::{FeeQueueSource, FeeQueueStatus, Schedule, WeeklyParams};
    use super::treasury::TreasuryState;
    use crate::constants::{MAX_SCHEDULE_SLOTS, MONDAY_0001_UTC_OFFSET, POLL_RING_LEN};

    use borsh::de::BorshDeserialize;
    use borsh::ser::BorshSerialize;

    /// An account struct must survive `serialize -> deserialize -> serialize`
    /// with the bytes unchanged and a correct discriminator prefix. This is the
    /// T3 acceptance criterion.
    fn round_trip_account<T: AccountSerialize + AccountDeserialize + Discriminator>(value: &T) {
        let mut buf = Vec::new();
        value.try_serialize(&mut buf).unwrap();
        assert_eq!(&buf[..8], T::DISCRIMINATOR, "discriminator must prefix the data");
        // `try_deserialize` rejects a wrong discriminator and must not lose data;
        // it unwraps here, so a mismatch panics.
        let mut rest = buf.as_slice();
        let back = T::try_deserialize(&mut rest).unwrap();
        // Byte-for-byte stable re-serialization of the deserialized value.
        let mut buf2 = Vec::new();
        back.try_serialize(&mut buf2).unwrap();
        assert_eq!(buf, buf2, "serialization must round trip byte-for-byte");
    }

    /// Value-type (embedded) data survives a borsh round trip.
    fn round_trip_value<T: BorshSerialize + BorshDeserialize>(value: &T) {
        let bytes = borsh::to_vec(value).unwrap();
        let back: T = borsh::from_slice(&bytes).unwrap();
        let bytes2 = borsh::to_vec(&back).unwrap();
        assert_eq!(bytes, bytes2, "value must round trip byte-for-byte");
    }

    #[test]
    fn config_round_trips() {
        round_trip_account(&Config {
            developer: Pubkey::from([1; 32]),
            keeper: Pubkey::from([2; 32]),
            mint: Pubkey::from([3; 32]),
            pool: Pubkey::from([4; 32]),
            quote_mint: Pubkey::from([5; 32]),
            quote_usd_feed: Pubkey::from([6; 32]),
            polling_rate_hours: 168,
            activation_kind: ActivationKind::MarketCap,
            activation_value_usd: 100_000,
            activated: false,
            oracle_max_age_slots: 150,
            oracle_max_conf_bps: 100,
            formula_version: 1,
            last_window_volatility_bps: 0,
            high_water_mark: 1_000_000_000,
            bumps: Bumps::default(),
        });
        round_trip_value(&Bumps::default());
        round_trip_value(&ActivationKind::Liquidity);
    }

    #[test]
    fn schedule_round_trips() {
        let week = |w: i64, fee: u16| WeeklyParams {
            week_effective_ts: MONDAY_0001_UTC_OFFSET + w * 86_400 * 7,
            published_ts: MONDAY_0001_UTC_OFFSET + w * 86_400 * 7 - 86_400 * 7,
            base_fee_bps: fee,
            max_fee_bps: 5000,
            volatility_min_bps: 100,
            volatility_max_bps: 3000,
            safety_limit_bps: 1000,
            formula_version: 1,
            short_hops: 4,
        };
        let mut entries = [WeeklyParams::default(); MAX_SCHEDULE_SLOTS];
        entries[0] = week(4, 100);
        entries[1] = week(5, 150);
        round_trip_account(&Schedule {
            mint: Pubkey::from([1; 32]),
            current_index: 1,
            entries,
            fee_queue: FeeQueueStatus {
                pending_fee_bps: 700,
                pending_fee_epoch: 12,
                pending_fee_source: FeeQueueSource::IntraWeekRaise,
                last_queued_epoch: 12,
            },
            bump: 1,
        });
        round_trip_value(&WeeklyParams::default());
        round_trip_value(&FeeQueueStatus::default());
        round_trip_value(&FeeQueueSource::ScheduledBase);
    }

    #[test]
    fn treasury_round_trips() {
        round_trip_account(&TreasuryState {
            mint: Pubkey::from([1; 32]),
            token_vault: Pubkey::from([2; 32]),
            quote_vault: Pubkey::from([3; 32]),
            total_fees_harvested: 123,
            total_quote_spent: 456,
            total_quote_gained: 0,
            bump: 1,
            token_vault_bump: 2,
            quote_vault_bump: 3,
        });
    }

    #[test]
    fn poll_round_trips() {
        let mut samples = [StoredPriceSample::default(); POLL_RING_LEN];
        samples[0] = StoredPriceSample { timestamp: 1_000, price_fixed: 1_000_000_000 };
        round_trip_account(&PollState {
            mint: Pubkey::from([1; 32]),
            window_start_ts: 1_000,
            window_start_price: 1_000_000_000,
            window_high: 1_010_000_000,
            window_low: 950_000_000,
            last_check_ts: 2_000,
            last_check_epoch: 3,
            ring_index: 1,
            samples,
            bump: 4,
        });
        round_trip_value(&StoredPriceSample::default());
    }

    #[test]
    fn action_round_trips() {
        round_trip_account(&ActiveAction {
            mint: Pubkey::from([1; 32]),
            kind: ActionKind::BuyDefend,
            started_ts: 1_000,
            started_epoch: 7,
            fee_before_bps: 100,
            fee_after_bps: 100,
            effective_epoch: 9,
            quote_used: 50_000,
            observed_effective_ts: 0,
            active: true,
            bump: 5,
        });
    }
}