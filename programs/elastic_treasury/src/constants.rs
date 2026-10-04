//! Compile-time constants and their invariants.
//!
//! PLAN.md §0.3: every bound that can be checked by the compiler is checked by the
//! compiler. Every bound that cannot is checked by a `const` assertion or a unit
//! test in this file, so an out-of-range constant is a build failure rather than
//! an incident.
//!
//! Sources for the values that came from outside the plan are cited inline and
//! summarised in `docs/VERIFIED.md`.

use anchor_lang::prelude::*;

// ---------------------------------------------------------------------------
// Seeds (PLAN.md §4)
// ---------------------------------------------------------------------------

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";
#[constant]
pub const SCHEDULE_SEED: &[u8] = b"schedule";
#[constant]
pub const TREASURY_SEED: &[u8] = b"treasury";
#[constant]
pub const POLL_SEED: &[u8] = b"poll";
#[constant]
pub const ACTION_SEED: &[u8] = b"action";
/// PDA set as the mint's `transfer_fee_config_authority`. Only this program may
/// change the fee, by CPI.
#[constant]
pub const FEE_AUTHORITY_SEED: &[u8] = b"fee_authority";
/// PDA set as the mint's `withdraw_withheld_authority`. Only this program may
/// pull withheld fees out of the mint.
#[constant]
pub const WITHDRAW_AUTHORITY_SEED: &[u8] = b"withdraw_authority";
/// Owner of `token_vault` / `quote_vault`.
#[constant]
pub const TREASURY_VAULT_OWNER: &[u8] = b"treasury_vault_owner";

// ---------------------------------------------------------------------------
// Fee bounds
// ---------------------------------------------------------------------------

/// PLAN.md D10: hard program cap of 50%, on top of the mint's own 10_000 limit.
/// (docs/VERIFIED.md V3: `MAX_FEE_BASIS_POINTS = 10_000` in
/// `spl-token-2022-interface/src/extension/transfer_fee/mod.rs:26`.)
pub const MAX_FEE_BPS: u16 = 5_000;

/// Token-2022's own ceiling. Used only to reject values that the mint would
/// reject anyway, so the failure is a named program error instead of a token
/// program error. Never a source of an accepted fee.
pub const TOKEN_2022_MAX_FEE_BPS: u16 = 10_000;

pub const MIN_FEE_BPS: u16 = 0;

/// Basis-point denominator.
pub const BPS_DENOMINATOR: u128 = 10_000;

// ---------------------------------------------------------------------------
// Schedule (PLAN.md D8, D14)
// ---------------------------------------------------------------------------

pub const WEEK_SECS: i64 = 604_800;

/// Unix time 0 was a Thursday. The first Monday 00:01 UTC after the epoch is
/// therefore 4 days + 60 s in, so `ts` is a valid Monday 00:01 UTC exactly when
/// `(ts - MONDAY_0001_UTC_OFFSET) % WEEK_SECS == 0`.
///
/// PLAN.md T1 specifies this identity; the tests at the bottom of this file
/// check it against real timestamps.
pub const MONDAY_0001_UTC_OFFSET: i64 = 345_660;

/// A published week must be at least this far in the future (D8).
pub const MIN_PUBLISH_LEAD_SECS: i64 = 604_800;

/// At most this many weeks may be visible in the schedule at once. PLAN.md §4.2
/// fixes the array at 4 slots.
pub const MAX_SCHEDULE_SLOTS: usize = 4;

/// How far ahead `publish_weekly_params` may target, in weeks.
pub const MAX_PUBLISH_WEEKS_AHEAD: i64 = 4;

/// Seconds by which Monday 00:01 UTC may be *approximated* when a caller
/// supplies a week start. A published entry must land on the exact grid, so
/// this is a tolerance for `initialize` only, never for `publish`.
pub const WEEK_GRID_TOLERANCE_SECS: i64 = 0;

// ---------------------------------------------------------------------------
// Polling / volatility
// ---------------------------------------------------------------------------

/// Default volatility lookback window (PLAN.md D4): one week.
pub const DEFAULT_POLLING_RATE_HOURS: u32 = 168;
pub const MIN_POLLING_RATE_HOURS: u32 = 1;
pub const MAX_POLLING_RATE_HOURS: u32 = 8_760;

/// Minimum gap between two `poll_and_act` calls (PLAN.md §5.2 step 2, Q1).
pub const ACTION_COOLDOWN_SECS: i64 = 86_400;

/// Number of TWAP samples retained in `PollState`. PLAN.md §4.4 caps this at 16.
pub const POLL_RING_LEN: usize = 16;

/// Reject a spot/TWAP disagreement larger than this (PLAN.md I8).
pub const MAX_ORACLE_DEVIATION_BPS: u32 = 500;

/// Oldest sample the program will accept for the TWAP window, in seconds.
/// Guards against a ring buffer that has gone stale (docs/VERIFIED.md V4 / A6).
pub const MAX_TWAP_SAMPLE_AGE_SECS: i64 = 7 * 60 * 60;

// ---------------------------------------------------------------------------
// Formula
// ---------------------------------------------------------------------------

/// PLAN.md D13: LP loans tokens -> short sells -> re-buys -> returns tokens.
pub const SHORT_HOPS: u8 = 4;

/// Highest `formula_version` this build implements (PLAN.md D9).
pub const MAX_SUPPORTED_FORMULA_VERSION: u8 = 1;

/// Volatility is published in basis points, so `volatility_max_bps` cannot
/// exceed this or the "price fell by 10x" case becomes unrepresentable.
pub const MAX_VOLATILITY_BPS: u16 = 10_000;

// ---------------------------------------------------------------------------
// Slippage / safety
// ---------------------------------------------------------------------------

/// Slippage tolerance applied to every treasury swap (PLAN.md §5.2 step 5).
pub const MAX_SLIPPAGE_BPS: u16 = 1_000; // 10%

/// PLAN.md §5.2 step 5: a single action may spend at most this fraction of the
/// quote vault. Invariant I2 / I7.
pub const SAFETY_LIMIT_BPS: u16 = 1_000; // 10%
pub const MIN_SAFETY_LIMIT_BPS: u16 = 1;
pub const MAX_SAFETY_LIMIT_BPS: u16 = 10_000;

/// Never take a swap whose output would round to zero, and never post a
/// `min_out` of zero (which would remove slippage protection entirely).
pub const MIN_SWAP_QUOTE_AMOUNT: u64 = 1;

// ---------------------------------------------------------------------------
// Epoch scheduling (docs/VERIFIED.md V1, V5, V8)
// ---------------------------------------------------------------------------

/// A native `SetTransferFee` always activates at `current_epoch + 2`. Not a
/// knob: it is a property of `process_set_transfer_fee`
/// (`token-2022/program/src/extension/transfer_fee/processor.rs:106`).
/// Applies to decreases too, which is why `revert_action` is not faster than
/// `poll_and_act` (docs/VERIFIED.md A1).
pub const FEE_CHANGE_DELAY_EPOCHS: u64 = 2;

/// Mainnet `slots_per_epoch`, read live from `getEpochSchedule`. Documented as
/// a constant because a program cannot assume the caller agrees; production
/// epochs are at or past `first_normal_epoch` where this is exact.
pub const MAINNET_SLOTS_PER_EPOCH: u64 = 432_000;

/// Observed mainnet slot time. The nominal value is 400 ms; measured ≈350 ms,
/// giving ≈42 h epochs and ≈273 epochs/year. See docs/VERIFIED.md V5.
pub const EST_SLOT_MS: u64 = 350;

// ---------------------------------------------------------------------------
// Oracle (docs/VERIFIED.md V7 / A8, A9)
// ---------------------------------------------------------------------------

/// Pyth staleness window, in slots. The SDK's own `VALID_SLOT_PERIOD = 25`
/// (pyth-sdk-solana `src/lib.rs:33`) is a devnet-sized constant and far too
/// tight for mainnet.
pub const DEFAULT_ORACLE_MAX_AGE_SLOTS: u64 = 150;

/// Reject a Pyth price whose confidence interval exceeds this fraction of the
/// price. The SDK does not check this at all (docs/VERIFIED.md A9).
pub const DEFAULT_ORACLE_MAX_CONF_BPS: u32 = 100; // 1%

// ---------------------------------------------------------------------------
// Venue (PLAN.md D5, docs/VERIFIED.md V4)
// ---------------------------------------------------------------------------

/// Raydium CPMM. Verified live on mainnet as an executable program owned by
/// `BPFLoaderUpgradeab1e...`.
#[constant]
pub const RAYDIUM_CPMM_PROGRAM_ID: &[u8] = b"CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

/// `AUTH_SEED` in `raydium-cp-swap/programs/cp-swap/src/lib.rs:82`. The CPMM
/// authority that signs vault transfers.
#[constant]
pub const RAYDIUM_CPMM_AUTH_SEED: &[u8] = b"vault_and_lp_mint_auth_seed";

/// CPMM fee denominator, `curve/fees.rs:4`. Not configurable here: sizing that
/// guessed it wrong would under-quote every treasury swap.
pub const CPMM_FEE_RATE_DENOMINATOR: u64 = 1_000_000;

/// `OBSERVATION_NUM` in `states/oracle.rs:10`.
pub const CPMM_OBSERVATION_NUM: usize = 100;

/// `OBSERVATION_UPDATE_DURATION_DEFAULT` in `states/oracle.rs:11`. The CPMM
/// oracle refuses to open a new ring slot more often than this, so a quiet pool
/// produces no samples at all (docs/VERIFIED.md A6).
pub const CPMM_OBSERVATION_UPDATE_SECS: u64 = 15;

/// CPMM's cumulative prices are Q32.32, not Q64.64 (`states/pool.rs:14`).
pub const CPMM_PRICE_SCALE_SHIFT: u32 = 32;

pub const CPMM_POOL_STATE_LEN: usize = 637;
pub const CPMM_OBSERVATION_STATE_LEN: usize = 4_075;
pub const CPMM_AMM_CONFIG_LEN: usize = 236;

// ---------------------------------------------------------------------------
// Fixed point (PLAN.md §6)
// ---------------------------------------------------------------------------

/// Fixed-point scale 1e9.
pub const SCALE: u128 = 1_000_000_000;
pub const SCALE_I64: i64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Compile-time assertions
// ---------------------------------------------------------------------------

// Feasible fee ordering: a published ceiling must be able to hold a raise, and
// the program cap must sit below the token's own cap or §D10 is meaningless.
const _: () = assert!(MIN_FEE_BPS < MAX_FEE_BPS);
const _: () = assert!(MAX_FEE_BPS < TOKEN_2022_MAX_FEE_BPS);
const _: () = assert!(MAX_FEE_BPS as u32 <= BPS_DENOMINATOR as u32);

// The week grid must actually be a week long and the offset must be inside it,
// otherwise the Monday check is either always-true or always-false.
const _: () = assert!(WEEK_SECS == 604_800);
const _: () = assert!(MONDAY_0001_UTC_OFFSET > 0);
const _: () = assert!(MONDAY_0001_UTC_OFFSET < WEEK_SECS);
const _: () = assert!(MIN_PUBLISH_LEAD_SECS == WEEK_SECS);
const _: () = assert!(MAX_PUBLISH_WEEKS_AHEAD * WEEK_SECS == MIN_PUBLISH_LEAD_SECS * 4);
const _: () = assert!(MAX_SCHEDULE_SLOTS <= MAX_PUBLISH_WEEKS_AHEAD as usize);

// Polling window bounds.
const _: () = assert!(MIN_POLLING_RATE_HOURS >= 1);
const _: () = assert!(DEFAULT_POLLING_RATE_HOURS >= MIN_POLLING_RATE_HOURS);
const _: () = assert!(DEFAULT_POLLING_RATE_HOURS <= MAX_POLLING_RATE_HOURS);
// The cooldown must be reachable at least once per lookback window, or the
// fast layer described in PLAN.md §3 could never fire.
const _: () = assert!(ACTION_COOLDOWN_SECS <= DEFAULT_POLLING_RATE_HOURS as i64 * 3600);
// Enough cooldown intervals to cover a window, so a poll can always be taken.
const _: () = assert!(POLL_RING_LEN >= 2);
const _: () = assert!(MAX_ORACLE_DEVIATION_BPS as u128 <= BPS_DENOMINATOR);
const _: () = assert!(
    MAX_TWAP_SAMPLE_AGE_SECS <= DEFAULT_POLLING_RATE_HOURS as i64 * 3600,
    "a sample may never be accepted that predates the window it defines"
);

// Formula.
const _: () = assert!(SHORT_HOPS == 4);
const _: () = assert!(MAX_SUPPORTED_FORMULA_VERSION >= 1);
const _: () = assert!(MAX_VOLATILITY_BPS as u128 <= BPS_DENOMINATOR);

// Slippage and safety. A slippage tolerance at or above 10_000 would make
// `min_out` meaningless, so 10_000 is itself rejected.
const _: () = assert!(MAX_SLIPPAGE_BPS > 0);
const _: () = assert!(MAX_SLIPPAGE_BPS < TOKEN_2022_MAX_FEE_BPS);
const _: () = assert!(MIN_SAFETY_LIMIT_BPS > 0);
const _: () = assert!(SAFETY_LIMIT_BPS >= MIN_SAFETY_LIMIT_BPS);
const _: () = assert!(SAFETY_LIMIT_BPS <= MAX_SAFETY_LIMIT_BPS);

// Epoch scheduling.
const _: () = assert!(FEE_CHANGE_DELAY_EPOCHS == 2);
const _: () = assert!(FEE_CHANGE_DELAY_EPOCHS > 0);
const _: () = assert!(EST_SLOT_MS > 0);
const _: () = assert!(MAINNET_SLOTS_PER_EPOCH > 0);
// An epoch must be long enough that the 2-epoch fee delay is a delay and not
// the same instruction. 2 epochs at 350 ms must exceed the daily cooldown,
// otherwise `poll_and_act` and its own fee change land inside one epoch and
// the "at most one queued change per epoch" rule in PLAN.md §3 is meaningless.
const _: () = assert!(
    FEE_CHANGE_DELAY_EPOCHS * MAINNET_SLOTS_PER_EPOCH * EST_SLOT_MS / 1000
        > ACTION_COOLDOWN_SECS as u64
);

// Oracle.
const _: () = assert!(DEFAULT_ORACLE_MAX_AGE_SLOTS > 0);
const _: () = assert!(DEFAULT_ORACLE_MAX_CONF_BPS > 0);
const _: () = assert!(DEFAULT_ORACLE_MAX_CONF_BPS as u128 <= BPS_DENOMINATOR);

// Fixed point.
const _: () = assert!(SCALE == 1_000_000_000);

// CPMM layouts, cross-checked against the constants the CPMM source asserts
// on its own size (`PoolState::LEN`, `ObservationState::LEN`, `AmmConfig::LEN`).
const _: () = assert!(CPMM_POOL_STATE_LEN == 637);
const _: () = assert!(CPMM_OBSERVATION_STATE_LEN == 4_075);
const _: () = assert!(CPMM_AMM_CONFIG_LEN == 236);
const _: () = assert!(CPMM_FEE_RATE_DENOMINATOR == 1_000_000);
const _: () = assert!(CPMM_PRICE_SCALE_SHIFT == 32);

// ---------------------------------------------------------------------------
// Schedule grid helpers
// ---------------------------------------------------------------------------

/// True when `ts` is exactly a Monday 00:01 UTC boundary.
pub fn is_monday_0001_utc(ts: i64) -> bool {
    ts.rem_euclid(WEEK_SECS) == MONDAY_0001_UTC_OFFSET.rem_euclid(WEEK_SECS)
}

/// The first Monday 00:01 UTC at or after `ts`. Uses `rem_euclid` so pre-1970
/// timestamps cannot produce a negative remainder (Rust's `%` truncates).
pub fn next_monday_0001_utc(ts: i64) -> i64 {
    let rem = ts.rem_euclid(WEEK_SECS);
    let delta = MONDAY_0001_UTC_OFFSET.rem_euclid(WEEK_SECS) - rem;
    if delta == 0 {
        ts
    } else {
        ts.saturating_add(delta.rem_euclid(WEEK_SECS))
    }
}

/// The Monday 00:01 UTC of the week that contains `ts`. A timestamp that *is*
/// on the boundary belongs to the week starting there, not the previous one.
pub fn current_week_start(ts: i64) -> i64 {
    let offset = MONDAY_0001_UTC_OFFSET.rem_euclid(WEEK_SECS);
    // Distance backwards to the grid, i.e. `rem - offset`, not the other way
    // round. Getting the sign wrong floors to the grid point a full week *before*
    // the correct one whenever `rem` is one second past a boundary.
    let delta = (ts.rem_euclid(WEEK_SECS) - offset).rem_euclid(WEEK_SECS);
    ts.saturating_sub(delta)
}

/// Weeks from `origin` to `ts`, floored. `ts` must be on the grid.
pub fn weeks_since(origin: i64, ts: i64) -> i64 {
    (ts - origin) / WEEK_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    // PLAN.md T1: "a test confirms a known Monday 00:01 UTC timestamp passes the
    // check and a Sunday one fails."

    /// 1970-01-05 00:01:00 UTC. Unix time 0 was a Thursday (index 4 with Sunday
    /// = 0), so +4 days lands on Monday and +1 minute makes it 00:01.
    #[test]
    fn first_monday_0001_utc_passes() {
        assert_eq!(MONDAY_0001_UTC_OFFSET, 345_660);
        assert!(is_monday_0001_utc(345_660));
    }

    #[test]
    fn sunday_fails_the_monday_check() {
        // 1970-01-11T00:01:00Z, six days after the first Monday.
        let sunday = MONDAY_0001_UTC_OFFSET + 6 * 86_400 + 60;
        assert!(!is_monday_0001_utc(sunday));

        // Every other weekday at 00:01 must fail too.
        for day in 1..=6i64 {
            let ts = MONDAY_0001_UTC_OFFSET + day * 86_400 + 60;
            assert!(!is_monday_0001_utc(ts), "weekday +{day} accepted");
        }

        // Even one second off the minute boundary must fail.
        assert!(!is_monday_0001_utc(MONDAY_0001_UTC_OFFSET + 1));
        // And midnight Monday is not 00:01.
        assert!(!is_monday_0001_utc(MONDAY_0001_UTC_OFFSET - 60));
        // Nor is 00:02.
        assert!(!is_monday_0001_utc(MONDAY_0001_UTC_OFFSET + 60));
    }

    #[test]
    fn modern_monday_0001_utc_passes() {
        // 2026-01-05T00:01:00Z, cross-checked against `date -u -j -f %s`.
        let ts = 1_767_571_260i64;
        assert_eq!(ts, MONDAY_0001_UTC_OFFSET + 2_922 * WEEK_SECS);
        assert!(is_monday_0001_utc(ts));

        // The Monday immediately before and after.
        assert!(is_monday_0001_utc(ts + WEEK_SECS));
        assert!(is_monday_0001_utc(ts - WEEK_SECS));
    }

    #[test]
    fn every_grid_point_is_exactly_seven_days_apart() {
        let start = MONDAY_0001_UTC_OFFSET;
        for week in 0..1000i64 {
            let ts = start + week * WEEK_SECS;
            assert!(is_monday_0001_utc(ts), "week {week} rejected");
            assert_eq!(next_monday_0001_utc(ts), ts, "week {week} moved");
            assert_eq!(current_week_start(ts), ts, "week {week} wrong floor");
            assert_eq!(current_week_start(ts + 1), ts);
            assert_eq!(current_week_start(ts + WEEK_SECS - 1), ts);
            assert_eq!(current_week_start(ts + WEEK_SECS), ts + WEEK_SECS);
        }
    }

    #[test]
    fn non_grid_timestamps_are_rejected() {
        // Sweep a whole week at an hour granularity. Offset 0 is excluded because
        // it is the grid point itself; no whole-hour offset can land on :01.
        let start = MONDAY_0001_UTC_OFFSET;
        for hours in 1..(WEEK_SECS / 3600) {
            let ts = start + hours * 3600;
            assert!(!is_monday_0001_utc(ts), "hourly offset {hours} accepted");
            assert_eq!(current_week_start(ts), start);
            assert_eq!(next_monday_0001_utc(ts), start + WEEK_SECS);
        }
        // 0 is excluded for the same reason: it is the grid point.
        for seconds in 1..60i64 {
            assert!(!is_monday_0001_utc(start + seconds));
            assert!(!is_monday_0001_utc(start - seconds));
        }
    }

    #[test]
    fn negative_timestamps_do_not_panic() {
        // Rust's `%` truncates toward zero, so a naive `ts % WEEK` would give a
        // negative remainder here and the check would invert.
        assert!(is_monday_0001_utc(-WEEK_SECS + MONDAY_0001_UTC_OFFSET));
        assert!(!is_monday_0001_utc(-1));
        assert_eq!(next_monday_0001_utc(-1), MONDAY_0001_UTC_OFFSET);
        assert_eq!(weeks_since(MONDAY_0001_UTC_OFFSET, MONDAY_0001_UTC_OFFSET + 3 * WEEK_SECS), 3);
    }

    #[test]
    fn lead_time_window_is_exactly_four_weeks() {
        // `publish_weekly_params` accepts [now + 1 week, now + 4 weeks].
        let now = 1_767_571_260i64;
        let target = next_monday_0001_utc(now + MIN_PUBLISH_LEAD_SECS);
        assert!(target - now >= MIN_PUBLISH_LEAD_SECS);
        assert!(target <= now + MAX_PUBLISH_WEEKS_AHEAD * WEEK_SECS);
        assert!(is_monday_0001_utc(target));
    }

    #[test]
    fn constants_are_in_the_documented_ranges() {
        assert_eq!(MAX_FEE_BPS, 5000);
        assert_eq!(SHORT_HOPS, 4);
        assert_eq!(WEEK_SECS, 604_800);
        assert_eq!(MIN_PUBLISH_LEAD_SECS, 604_800);
        assert_eq!(ACTION_COOLDOWN_SECS, 86_400);
        const _: () = assert!(DEFAULT_POLLING_RATE_HOURS == 168);
        const _: () = assert!(POLL_RING_LEN <= 16);
    }
}
