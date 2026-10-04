//! Custom errors.
//!
//! PLAN.md §0.5: "Any invalid input reverts the whole transaction with a named
//! custom error." Every guard in this program resolves to one of these variants;
//! no bare `ErrorCode` from a dependency is ever surfaced to the caller, because
//! an error a client cannot branch on is an error that gets ignored.
//!
//! Codes are grouped by instruction and never renumbered, so the IDL is a stable
//! ABI for the keeper (PLAN.md §9).

use anchor_lang::prelude::*;

/// Number of user-defined errors. Informational only: Anchor assigns the codes
/// positionally from 6000, so adding a variant anywhere but the end renumbers
/// everything after it. `tests/idl_is_stable.rs` fails if this drifts from the
/// generated IDL.
pub const NUM_ERRORS: u8 = 59;

#[error_code]
pub enum ElasticTreasuryError {
    // -- generic (0) --------------------------------------------------------
    /// A value failed a bound check that no more specific variant describes.
    ValueOutOfRange,
    /// Arithmetic that must be exact overflowed or underflowed.
    MathOverflow,
    /// A `u128` intermediate could not be represented, usually from hostile input.
    PrecisionLoss,
    /// An account failed a structural check (owner, size, or layout).
    InvalidAccount,

    // -- initialize (10) ---------------------------------------------------
    /// `polling_rate_hours` was 0 or above `MAX_POLLING_RATE_HOURS`.
    InvalidPollingRateHours,
    /// `activation.value_usd` was 0.
    InvalidActivationThreshold,
    /// The activation metric named a type the program does not implement.
    UnsupportedActivationKind,
    /// Developer, keeper, mint, quote mint, pool or feed was the default pubkey.
    ZeroAddress,
    /// The mint is not owned by the Token-2022 program.
    MintNotToken2022,
    /// The mint lacks a `TransferFeeConfig` extension.
    MintMissingTransferFeeConfig,
    /// The mint is missing `TransferFeeConfig` authorities.
    MintMissingFeeAuthority,
    /// The mint already has a non-zero transfer fee.
    MintFeeAlreadySet,
    /// The mint carries an extension CPMM will refuse (docs/VERIFIED.md A10).
    MintHasDisallowedExtension,
    /// `base_fee_bps` exceeds the program's cap.
    FeeExceedsCap,
    /// The pool account is not owned by the configured venue program.
    PoolWrongOwner,
    /// The pool's mints are not the configured mint and quote mint.
    PoolMintMismatch,
    /// The quote mint is not the configured quote mint.
    QuoteMintMismatch,
    /// The Pyth feed account is the wrong size or has bad magic.
    InvalidOracleAccount,
    /// The quote feed does not price the quote mint.
    QuoteFeedSymbolMismatch,
    /// The vault accounts do not belong to the pool.
    VaultNotPoolOwned,
    /// The token vault's mint is not the program's mint.
    VaultMintMismatch,

    // -- schedule (20) -----------------------------------------------------
    /// `week_effective_ts` is not a Monday 00:01 UTC boundary.
    NotMonday0001Utc,
    /// Fewer than `MIN_PUBLISH_LEAD_SECS` until `week_effective_ts`.
    PublishLeadTooShort,
    /// More than `MAX_PUBLISH_WEEKS_AHEAD` weeks ahead.
    PublishLeadTooLong,
    /// More than `week_effective_ts` is in the past by more than the tolerance.
    WeekInPast,
    /// The schedule has no free slot.
    ScheduleFull,
    /// That week is already published.
    WeekAlreadyPublished,
    /// No entry exists for that week.
    WeekNotPublished,
    /// The caller may not touch this week. The current and past weeks are
    /// immutable (PLAN.md I4).
    WeekImmutable,
    /// `formula_version` is not implemented by this build.
    UnsupportedFormulaVersion,
    /// `volatility_min_bps >= volatility_max_bps`.
    VolatilityBoundsInverted,
    /// `volatility_min_bps` was 0, or `volatility_max_bps` above 10 000.
    VolatilityBoundsOutOfRange,
    /// `safety_limit_bps` was 0 or above 10 000.
    SafetyLimitOutOfRange,
    /// `base_fee_bps > max_fee_bps`.
    BaseFeeAboveMaxFee,
    /// `short_hops` did not match the compiled-in `SHORT_HOPS`.
    ShortHopsMismatch,
    /// The week is out of chronological order for its slot.
    WeekOrderViolated,
    /// Caller is not the developer.
    NotDeveloper,
    /// Caller is not the keeper.
    NotKeeper,
    /// The pending developer rotation does not match.
    RotationMismatch,
    /// A rotation is already pending, so a second proposal would overwrite it.
    RotationAlreadyPending,

    // -- activate (30) -----------------------------------------------------
    /// The activation metric is below the configured threshold.
    ActivationThresholdNotMet,
    /// Already latched. `activated` is one-way (PLAN.md I9).
    AlreadyActivated,
    /// The Pyth price is stale, untrusted, or has too wide a confidence band.
    OraclePriceUnusable,

    // -- fee queue (40) ----------------------------------------------------
    /// Fewer than `FEE_CHANGE_DELAY_EPOCHS` remain before the target.
    FeeDelayTooSoon,
    /// The pending fee change is owned by a source that may not be overwritten.
    FeeQueueOccupied,
    /// The active week's entry is missing; nothing to queue.
    NoActiveWeek,
    /// The call would not change the fee, so queueing is a no-op.
    FeeUnchanged,

    // -- poll_and_act (50) -------------------------------------------------
    /// `poll_and_act` before activation.
    NotActivated,
    /// `now < last_check_ts + ACTION_COOLDOWN_SECS` (PLAN.md I5).
    CooldownActive,
    /// The lookback window has fewer than `POLL_RING_LEN` usable samples.
    InsufficientSamples,
    /// The newest TWAP sample is older than `MAX_TWAP_SAMPLE_AGE_SECS`.
    StaleTwapSample,
    /// Spot and TWAP disagree by more than `MAX_ORACLE_DEVIATION_BPS`.
    OracleDeviation,
    /// The pool reserves are zero or the vault balance is below accrued fees.
    PoolEmpty,
    /// The pool has swaps disabled, or `open_time` is in the future.
    PoolNotSwappable,
    /// The swap would return nothing.
    SwapOutputZero,
    /// The action would spend more than `safety_limit_bps` of the quote vault.
    SafetyLimitExceeded,
    /// An `ActiveAction` already exists and is not re-raisable.
    ActionAlreadyActive,
    /// `revert_action` before the minimum hold time, or while volatility is
    /// still above `volatility_max_bps`.
    RevertNotAllowed,

    // -- harvest (60) ------------------------------------------------------
    /// There is nothing withheld to harvest.
    NothingToHarvest,
}

impl From<MathError> for ElasticTreasuryError {
    fn from(e: MathError) -> Self {
        match e {
            // PLAN.md §0.4 bans silent wraparound, so both directions are the
            // same on-chain failure: the computation could not be done exactly.
            MathError::Overflow
            | MathError::Underflow
            | MathError::DivideByZero
            | MathError::NegativeSqrt => ElasticTreasuryError::MathOverflow,
            // The 1e9 fixed-point grid could not resolve the requested value.
            MathError::Unrepresentable | MathError::LossyConversion => {
                ElasticTreasuryError::PrecisionLoss
            }
        }
    }
}

impl From<MathError> for ProgramError {
    fn from(e: MathError) -> Self {
        match e {
            MathError::DivideByZero => ProgramError::InvalidArgument,
            _ => ProgramError::ArithmeticOverflow,
        }
    }
}

/// Errors from the pure calculator in `math/`. Kept separate from the
/// on-chain error enum so the math module can be unit-tested on the host with no
/// Anchor types in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    Overflow,
    Underflow,
    DivideByZero,
    /// The requested precision is finer than 1e9 fixed point can express.
    Unrepresentable,
    /// A `sqrt` argument was negative.
    NegativeSqrt,
    /// A conversion to the target width would truncate a non-zero remainder.
    LossyConversion,
}
