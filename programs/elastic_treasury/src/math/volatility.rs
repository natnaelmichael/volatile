//! Volatility, per PLAN.md §6.3, plus the TWAP average it feeds on.
//!
//! ```text
//! vol_bps = (window_high - window_low) * 10_000 / window_start_price
//! direction from the sign of (price_now - window_start_price)
//! ```
//!
//! This is a *range*, not a standard deviation. That is deliberate and it is the
//! cheapest statistic available from the CPMM observation ring: the ring stores
//! cumulative price×time, so a high and a low over a window cost one pass and no
//! extra samples, and a range never needs a mean to be well defined.
//!
//! Two properties matter more than the statistic itself:
//!
//! * **Samples are program-read, never keeper-supplied.** Nothing in this module
//!   accepts a caller-provided price. [`RollingWindow`] is built from a slice of
//!   TWAP samples the venue adapter read from the pool (T8), and `start_price`
//!   comes from the pool too.
//! * **Rounding is up.** Volatility selects between three behaviours (idle, raise
//!   the fee, spend treasury funds). Understating the range could pick "idle"
//!   when the plan says "act", so the answer is rounded toward action.
//!
//! `window_high`/`window_low`/`window_start_price`/`price_now` are 1e9 fixed
//! point. The CPMM observation ring is Q32.32; `amm.rs` converts on the way in.

use crate::errors::MathError;
use crate::math::fixed;

/// Lookback window statistics for one `polling_rate_hours` span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceWindow {
    /// `window_start_price`, from the published week's anchor. Must be non-zero.
    pub start_price: u128,
    /// Highest TWAP sample in the window, inclusive of the endpoints.
    pub high: u128,
    /// Lowest TWAP sample in the window, inclusive of the endpoints.
    pub low: u128,
    /// Latest TWAP sample. Only its *sign* against `start_price` is used.
    pub now_price: u128,
}

impl PriceWindow {
    /// Validate the invariants the range formula depends on.
    pub fn new(
        start_price: u128,
        high: u128,
        low: u128,
        now_price: u128,
    ) -> Result<Self, MathError> {
        if start_price == 0 {
            return Err(MathError::Unrepresentable);
        }
        if high < low {
            return Err(MathError::Unrepresentable);
        }
        Ok(Self {
            start_price,
            high,
            low,
            now_price,
        })
    }

    /// `(window_high - window_low) * 10_000 / window_start_price`, rounded up.
    pub fn volatility_bps(&self) -> Result<u32, MathError> {
        let range = self.high.checked_sub(self.low).ok_or(MathError::Underflow)?;
        let scaled = range
            .checked_mul(10_000)
            .ok_or(MathError::Overflow)?;
        let bps = fixed::ceil_div(scaled, self.start_price).ok_or(MathError::DivideByZero)?;
        u32::try_from(bps).map_err(|_| MathError::Unrepresentable)
    }

    /// Which way the price has moved since the window anchor.
    pub fn direction(&self) -> Direction {
        Direction::of(self.now_price, self.start_price)
    }

    /// Signed move since the anchor, in bps, rounded up in magnitude. Positive
    /// is a rise. Used for reporting and for sizing a `BuyDefend`.
    pub fn move_bps(&self) -> Result<i64, MathError> {
        let delta = self.now_price.abs_diff(self.start_price);
        let magnitude = fixed::ceil_div(
            delta
                .checked_mul(10_000)
                .ok_or(MathError::Overflow)?,
            self.start_price,
        )
        .ok_or(MathError::DivideByZero)?;
        let magnitude = i64::try_from(magnitude).map_err(|_| MathError::Unrepresentable)?;
        if self.now_price >= self.start_price {
            Ok(magnitude)
        } else {
            magnitude
                .checked_neg()
                .ok_or(MathError::Unrepresentable)
        }
    }
}

/// Sign of the move since the window anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Rising,
    Falling,
    Flat,
}

impl Direction {
    pub fn of(now_price: u128, start_price: u128) -> Self {
        match now_price.cmp(&start_price) {
            core::cmp::Ordering::Greater => Direction::Rising,
            core::cmp::Ordering::Less => Direction::Falling,
            core::cmp::Ordering::Equal => Direction::Flat,
        }
    }
}

/// One TWAP sample: Unix timestamp and price in 1e9 fixed point.
///
/// Assumed sorted ascending by timestamp and de-duplicated, which is what the
/// CPMM observation ring yields. [`RollingWindow`] relies on that rather than
/// re-sorting, because sorting on-chain costs CU for no benefit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwapSample {
    pub timestamp: i64,
    pub price_fixed: u128,
}

/// A single price reading, used when the observation ring cannot supply a TWAP.
///
/// §6.3 sources volatility from the TWAP. The ring only advances every 15
/// seconds and may be brand new, so a freshly created pool genuinely has no TWAP
/// history. Falling back to spot for that case is recorded rather than hidden:
/// [`RollingWindow::spot_fallback`] returns `self.is_twap = false` so the caller
/// can refuse to act on it if it wants to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollingWindow {
    pub start_price: u128,
    pub high: u128,
    pub low: u128,
    pub now_price: u128,
    pub sample_count: u8,
    pub is_twap: bool,
}

impl RollingWindow {
    pub fn window(&self) -> Result<PriceWindow, MathError> {
        PriceWindow::new(self.start_price, self.high, self.low, self.now_price)
    }

    /// Build the window from TWAP samples covering the last `lookback_secs`.
    ///
    /// Samples strictly older than `now_ts - lookback_secs` are excluded. The
    /// anchor price is taken from the published week, not from the samples, so a
    /// window that starts mid-history still measures from the right origin.
    ///
    /// Returns [`MathError::InsufficientSamples`] when fewer than two samples
    /// fall in the window: with one sample the range is zero and volatility would
    /// read a perfect 0% on a pool that may have moved a lot.
    pub fn from_twap_samples(
        samples: &[TwapSample],
        start_price: u128,
        now_ts: i64,
        lookback_secs: i64,
    ) -> Result<Self, MathError> {
        if lookback_secs <= 0 {
            return Err(MathError::Unrepresentable);
        }
        if start_price == 0 {
            return Err(MathError::Unrepresentable);
        }
        // Saturating: a negative `now_ts - lookback` just means "everything".
        let cutoff = now_ts.saturating_sub(lookback_secs);

        let mut high = 0u128;
        let mut low = u128::MAX;
        let mut count: u8 = 0;
        let mut now_price = 0u128;

        for sample in samples {
            if sample.timestamp < cutoff || sample.timestamp > now_ts {
                continue;
            }
            if sample.price_fixed == 0 {
                // A zero price in the ring means an empty observation slot.
                return Err(MathError::Unrepresentable);
            }
            high = high.max(sample.price_fixed);
            low = low.min(sample.price_fixed);
            now_price = sample.price_fixed; // ascending order: last in wins
            count = count.saturating_add(1);
        }

        if count < 2 {
            return Err(MathError::InsufficientSamples);
        }
        Ok(Self {
            start_price,
            high,
            low,
            now_price,
            sample_count: count,
            is_twap: true,
        })
    }

    /// Single-slot window from spot. See [`RollingWindow`] for the caveat.
    pub fn spot_fallback(start_price: u128, spot_price: u128) -> Result<Self, MathError> {
        if start_price == 0 || spot_price == 0 {
            return Err(MathError::Unrepresentable);
        }
        Ok(Self {
            start_price,
            high: spot_price,
            low: spot_price,
            now_price: spot_price,
            sample_count: 1,
            is_twap: false,
        })
    }
}

/// Time-weighted average price over `[start_ts, end_ts]`.
///
/// Each observation stores a *cumulative* price×time, which is monotone in its
/// timestamp. The average over a window is therefore just the difference of the
/// two cumulative values that bracket it:
///
/// ```text
/// twap = (cum(end_ts) - cum(start_ts)) / (end_ts - start_ts)
/// ```
///
/// This holds across a ring wrap because the cumulative values are absolute, not
/// per-slot, so the caller only has to linearise the ring into timestamp order
/// (T8); no interval-by-interval accumulation is needed here.
///
/// The result is in the *same units as the input prices*, so a Q32.32 ring yields
/// a Q32.32 average which `amm::q32_32_to_fixed` converts to 1e9 fixed point.
/// Returning the input units rather than 1e9 keeps this function from having to
/// know which Q-format it was handed.
///
/// `cum_start` is taken as zero when the ring holds no observation at or before
/// `start_ts`, which is correct for a ring that was seeded at zero.
pub fn twap(
    samples: &[(i64, u128)],
    start_ts: i64,
    end_ts: i64,
) -> Result<u128, MathError> {
    if end_ts <= start_ts {
        return Err(MathError::Unrepresentable);
    }
    if samples.len() < 2 {
        return Err(MathError::InsufficientSamples);
    }

    let mut cumulative_at_start: Option<u128> = None;
    let mut cumulative_at_end: Option<u128> = None;
    let mut previous_ts = i64::MIN;

    for &(ts, cumulative) in samples {
        if ts < previous_ts {
            // The caller forgot to linearise the circular ring.
            return Err(MathError::Unrepresentable);
        }
        previous_ts = ts;
        if ts <= start_ts {
            cumulative_at_start = Some(cumulative);
        }
        if ts <= end_ts {
            cumulative_at_end = Some(cumulative);
        }
    }

    let cumulative_at_end = cumulative_at_end.ok_or(MathError::InsufficientSamples)?;
    let cumulative_at_start = cumulative_at_start.unwrap_or(0);
    let accumulated = cumulative_at_end
        .checked_sub(cumulative_at_start)
        .ok_or(MathError::Underflow)?;

    Ok(accumulated / ((end_ts as i128 - start_ts as i128) as u128))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::fixed::SCALE;

    fn price(x: u64) -> u128 {
        x as u128 * SCALE
    }

    #[test]
    fn range_formula_matches_the_plan() {
        // Start 1.00, high 1.10, low 0.90 -> 20% range.
        let w = PriceWindow::new(price(1), price(11) / 10, price(9) / 10, price(1)).unwrap();
        assert_eq!(w.volatility_bps().unwrap(), 2_000);

        // Start 2.00, high 2.20, low 1.80 -> 0.40 / 2.00 = 20%.
        let w = PriceWindow::new(price(2), price(22) / 10, price(18) / 10, price(2)).unwrap();
        assert_eq!(w.volatility_bps().unwrap(), 2_000);

        // Flat window is exactly zero.
        let w = PriceWindow::new(price(1), price(1), price(1), price(1)).unwrap();
        assert_eq!(w.volatility_bps().unwrap(), 0);
    }

    #[test]
    fn a_range_that_does_not_divide_evenly_rounds_up() {
        // (3 - 2) / 3 = 33.33% -> 3334 bps, not 3333.
        let w = PriceWindow::new(price(3), price(3), price(2), price(3)).unwrap();
        assert_eq!(w.volatility_bps().unwrap(), 3_334);

        // 1/7 of a price move.
        let w = PriceWindow::new(price(7), price(7), price(6), price(7)).unwrap();
        assert_eq!(w.volatility_bps().unwrap(), 1_429);
    }

    #[test]
    fn direction_follows_the_sign_of_the_move() {
        assert_eq!(Direction::of(price(11) / 10, price(1)), Direction::Rising);
        assert_eq!(Direction::of(price(9) / 10, price(1)), Direction::Falling);
        assert_eq!(Direction::of(price(1), price(1)), Direction::Flat);
    }

    #[test]
    fn move_bps_is_signed() {
        let up = PriceWindow::new(price(1), price(11) / 10, price(1), price(11) / 10).unwrap();
        assert_eq!(up.move_bps().unwrap(), 1_000);
        let down = PriceWindow::new(price(1), price(1), price(9) / 10, price(9) / 10).unwrap();
        assert_eq!(down.move_bps().unwrap(), -1_000);
        let flat = PriceWindow::new(price(1), price(1), price(1), price(1)).unwrap();
        assert_eq!(flat.move_bps().unwrap(), 0);
    }

    #[test]
    fn zero_anchor_and_inverted_range_are_rejected() {
        assert_eq!(
            PriceWindow::new(0, price(1), price(1), price(1)),
            Err(MathError::Unrepresentable)
        );
        assert_eq!(
            PriceWindow::new(price(1), price(1), price(2), price(1)),
            Err(MathError::Unrepresentable)
        );
    }

    #[test]
    fn extreme_prices_do_not_panic() {
        for &(start, high, low, now) in &[
            (1u128, u128::MAX, 1, 1),
            (u128::MAX, u128::MAX, 1, u128::MAX),
            (u128::MAX, u128::MAX, u128::MAX, u128::MAX),
            (1, 1, 1, u128::MAX),
        ] {
            let w = PriceWindow::new(start, high, low, now).unwrap();
            // Either a number or a named error, never a panic.
            let _ = w.volatility_bps();
            let _ = w.move_bps();
            let _ = w.direction();
        }
    }

    fn samples() -> Vec<TwapSample> {
        vec![
            TwapSample { timestamp: 1_000, price_fixed: price(1) },
            TwapSample { timestamp: 2_000, price_fixed: price(12) / 10 },
            TwapSample { timestamp: 3_000, price_fixed: price(8) / 10 },
            TwapSample { timestamp: 4_000, price_fixed: price(11) / 10 },
        ]
    }

    #[test]
    fn window_uses_only_samples_inside_the_lookback() {
        let all = samples();
        // 3_000s lookback ending at 4_000 excludes nothing.
        let w = RollingWindow::from_twap_samples(&all, price(1), 4_000, 3_000).unwrap();
        assert_eq!(w.high, price(12) / 10);
        assert_eq!(w.low, price(8) / 10);
        assert_eq!(w.now_price, price(11) / 10);
        assert_eq!(w.sample_count, 4);

        // A 1_500s lookback keeps the last two samples only, dropping the 1.20 high.
        let w = RollingWindow::from_twap_samples(&all, price(1), 4_000, 1_500).unwrap();
        assert_eq!(w.high, price(11) / 10);
        assert_eq!(w.low, price(8) / 10);
        assert_eq!(w.sample_count, 2);

        // Samples after `now_ts` are ignored rather than trusted.
        let w = RollingWindow::from_twap_samples(&all, price(1), 2_500, 10_000).unwrap();
        assert_eq!(w.sample_count, 2);
        assert_eq!(w.now_price, price(12) / 10);
    }

    #[test]
    fn a_single_sample_is_not_a_window() {
        let one = vec![TwapSample { timestamp: 1_000, price_fixed: price(1) }];
        assert_eq!(
            RollingWindow::from_twap_samples(&one, price(1), 1_000, 1_000),
            Err(MathError::InsufficientSamples)
        );
        // A brand new pool reports 0% range off spot and says so.
        let fallback = RollingWindow::spot_fallback(price(1), price(1)).unwrap();
        assert!(!fallback.is_twap);
        assert_eq!(fallback.window().unwrap().volatility_bps().unwrap(), 0);
    }

    #[test]
    fn malformed_inputs_are_refused() {
        let all = samples();
        assert_eq!(
            RollingWindow::from_twap_samples(&all, 0, 4_000, 1_000),
            Err(MathError::Unrepresentable)
        );
        assert_eq!(
            RollingWindow::from_twap_samples(&all, price(1), 4_000, 0),
            Err(MathError::Unrepresentable)
        );
        let with_zero = [
            TwapSample { timestamp: 1_000, price_fixed: 0 },
            TwapSample { timestamp: 2_000, price_fixed: price(1) },
        ];
        assert_eq!(
            RollingWindow::from_twap_samples(&with_zero, price(1), 2_000, 1_000),
            Err(MathError::Unrepresentable)
        );
        assert_eq!(RollingWindow::spot_fallback(0, price(1)), Err(MathError::Unrepresentable));
        assert_eq!(RollingWindow::spot_fallback(price(1), 0), Err(MathError::Unrepresentable));
    }

    /// The whole window comes from the program, so a keeper cannot move it.
    #[test]
    fn volatility_tracks_the_sample_range() {
        let w = RollingWindow::from_twap_samples(&samples(), price(1), 4_000, 3_000)
            .unwrap()
            .window()
            .unwrap();
        // high 1.20, low 0.80 -> 40%.
        assert_eq!(w.volatility_bps().unwrap(), 4_000);
        assert_eq!(w.direction(), Direction::Rising);
    }

    #[test]
    fn twap_is_the_time_weighted_average() {
        // Two observations of equal length at 1.00 and 3.00 -> 2.00.
        let ring = [(0i64, 0u128), (10, 10 << 32), (20, 40 << 32)];
        assert_eq!(twap(&ring, 0, 20).unwrap(), 2 << 32);

        // Weighting: 9s at 1.00 then 1s at 10.00 -> 1.90.
        let ring = [(0i64, 0u128), (9, 9 << 32), (10, 19 << 32)];
        assert_eq!(twap(&ring, 0, 10).unwrap(), (19 << 32) / 10);

        // A sub-window slices the same ring.
        assert_eq!(twap(&ring, 9, 10).unwrap(), 10 << 32);

        // A window that starts before the first observation still uses the
        // absolute cumulative values, which is what makes a ring wrap harmless.
        assert_eq!(twap(&ring, -10, 0).unwrap(), 0);
    }

    #[test]
    fn twap_rejects_degenerate_requests() {
        let ring = [(0i64, 0u128), (10, 10 << 32)];
        assert_eq!(twap(&ring, 10, 10), Err(MathError::Unrepresentable));
        assert_eq!(twap(&ring, 20, 10), Err(MathError::Unrepresentable));
        assert_eq!(twap(&ring[..1], 0, 10), Err(MathError::InsufficientSamples));
        // Reversed timestamps mean the caller forgot to linearise the ring.
        let reversed = [(10i64, 0u128), (0, 10 << 32)];
        assert_eq!(twap(&reversed, 0, 10), Err(MathError::Unrepresentable));
        // A window entirely before the first sample has no weight.
        assert_eq!(twap(&ring, -100, -50), Err(MathError::InsufficientSamples));
    }
}