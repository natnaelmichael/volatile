//! `PollState` — the rolling volatility window and the TWAP sample ring.
//! PLAN.md §4.4, §6.3.
//!
//! All prices are stored in 1e9 fixed point (`SCALE`). The samples are read from
//! the CPMM observation account by the venue adapter at `poll_and_act` time and
//! pushed here; the program never accepts a price from a caller (PLAN.md §5.2
//! step 3).
//!
//! Prices are kept as `u128` fixed point. The ring stores a conversion of
//! [`crate::math::volatility::TwapSample`], duplicated rather than imported so
//! `math/` stays free of Anchor derives.

use anchor_lang::prelude::*;

use crate::constants::POLL_RING_LEN;

/// One point of the TWAP ring: cumulative-average price and when it was read.
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace, Default,
)]
pub struct StoredPriceSample {
    /// When the observation was taken (block timestamp).
    pub timestamp: i64,
    /// The price in 1e9 fixed point. `0` is never a valid price, so a sample
    /// with `timestamp == 0` is the empty sentinel.
    pub price_fixed: u128,
}

impl StoredPriceSample {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timestamp == 0
    }
}

/// The rolling window + the ring that feeds it. PDA `["poll", mint]`.
#[account]
#[derive(InitSpace)]
pub struct PollState {
    /// The mint this window belongs to (defensive).
    pub mint: Pubkey,
    /// Start of the current lookback window.
    pub window_start_ts: i64,
    /// The price at `window_start_ts`, fixed point. The denominator of the
    /// range volatility (6.3).
    pub window_start_price: u128,
    /// Highest price seen in the window, fixed point.
    pub window_high: u128,
    /// Lowest price seen in the window, fixed point.
    pub window_low: u128,
    /// When `poll_and_act` last ran; the cooldown is measured from here (I5).
    pub last_check_ts: i64,
    /// The epoch of `last_check_ts`, for the "one queued change per epoch" rule.
    pub last_check_epoch: u64,
    /// Head of the ring: next push lands at `samples[ring_index]`.
    pub ring_index: u8,
    /// The last `POLL_RING_LEN` TWAP samples, newest at `ring_index - 1`.
    pub samples: [StoredPriceSample; POLL_RING_LEN],
    pub bump: u8,
}

impl PollState {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    /// Number of non-empty samples currently stored.
    pub fn len(&self) -> usize {
        self.samples.iter().filter(|s| !s.is_empty()).count()
    }

    /// True when no sample has ever been observed.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert a sample, evicting the oldest when full.
    pub fn push(&mut self, sample: StoredPriceSample) {
        let i = self.ring_index as usize;
        self.samples[i] = sample;
        self.ring_index = ((self.ring_index as usize + 1) % POLL_RING_LEN) as u8;
    }

    /// A clone of the ring in timestamp order, oldest first, ignoring empties.
    pub fn sorted(&self) -> Vec<StoredPriceSample> {
        let mut v: Vec<StoredPriceSample> =
            self.samples.iter().copied().filter(|s| !s.is_empty()).collect();
        v.sort_by_key(|s| s.timestamp);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: i64, price: u128) -> StoredPriceSample {
        StoredPriceSample { timestamp: ts, price_fixed: price }
    }

    #[test]
    fn ring_pushes_and_evicts_and_sorts() {
        let mut p = PollState {
            mint: Pubkey::default(),
            window_start_ts: 0,
            window_start_price: 0,
            window_high: 0,
            window_low: 0,
            last_check_ts: 0,
            last_check_epoch: 0,
            ring_index: 0,
            samples: [StoredPriceSample::default(); POLL_RING_LEN],
            bump: 0,
        };
        assert_eq!(p.len(), 0);

        // Fill past capacity to force eviction.
        for i in 0..(POLL_RING_LEN + 3) {
            p.push(sample((100 + i) as i64, (1000 + i) as u128));
        }
        assert_eq!(p.len(), POLL_RING_LEN);
        let sorted = p.sorted();
        assert_eq!(sorted.len(), POLL_RING_LEN);
        for w in sorted.windows(2) {
            assert!(w[0].timestamp < w[1].timestamp);
        }
        // The three oldest (100,101,102) were evicted.
        assert_eq!(sorted[0].timestamp, 103);
        assert_eq!(sorted[POLL_RING_LEN - 1].timestamp, (100 + POLL_RING_LEN + 2) as i64);
    }

    #[test]
    fn poll_layout_is_stable() {
        // 32 mint + i64 + 3*u128 + i64 + u64 + u8 + 16*24 + bump.
        let body = 32 + 8 + 3 * 16 + 8 + 8 + 1 + 16 * 24 + 1;
        assert_eq!(PollState::INIT_SPACE, body);
        assert_eq!(PollState::LEN, 8 + body);
    }
}