//! Freeze the error ABI against the generated IDL.
//!
//! PLAN.md §0.5 and errors.rs both state that translated error codes are added
//! positionally from 6000 and never renumbered once frozen. Anchor derives the
//! codes from variant order, so inserting or removing a variant anywhere but at
//! the very end silently renumbers every later error — exactly the kind of
//! break a client cannot see until it is in production. `NUM_ERRORS` in
//! `errors.rs` refers to this test; this test fails if the two drift.
//!
//! The order below is not reconstructed from the enum (Rust cannot enumerate
//! variants); it is the frozen ABI and must only ever be appended to.

use std::path::PathBuf;

use elastic_treasury::NUM_ERRORS;

/// The frozen variant order, matching `ElasticTreasuryError` in `errors.rs`.
const ERROR_NAMES: &[&str] = &[
    // -- generic (0) -----------------------------------------------------
    "ValueOutOfRange",
    "MathOverflow",
    "PrecisionLoss",
    "InvalidAccount",
    // -- initialize (10) -------------------------------------------------
    "InvalidPollingRateHours",
    "InvalidActivationThreshold",
    "UnsupportedActivationKind",
    "ZeroAddress",
    "MintNotToken2022",
    "MintMissingTransferFeeConfig",
    "MintMissingFeeAuthority",
    "MintFeeAlreadySet",
    "MintHasDisallowedExtension",
    "FeeExceedsCap",
    "PoolWrongOwner",
    "PoolMintMismatch",
    "QuoteMintMismatch",
    "InvalidOracleAccount",
    "QuoteFeedSymbolMismatch",
    "VaultNotPoolOwned",
    "VaultMintMismatch",
    // -- schedule (20) ---------------------------------------------------
    "NotMonday0001Utc",
    "PublishLeadTooShort",
    "PublishLeadTooLong",
    "WeekInPast",
    "ScheduleFull",
    "WeekAlreadyPublished",
    "WeekNotPublished",
    "WeekImmutable",
    "UnsupportedFormulaVersion",
    "VolatilityBoundsInverted",
    "VolatilityBoundsOutOfRange",
    "SafetyLimitOutOfRange",
    "BaseFeeAboveMaxFee",
    "ShortHopsMismatch",
    "WeekOrderViolated",
    "NotDeveloper",
    "NotKeeper",
    "RotationMismatch",
    "RotationAlreadyPending",
    // -- activate (30) ---------------------------------------------------
    "ActivationThresholdNotMet",
    "AlreadyActivated",
    "OraclePriceUnusable",
    // -- fee queue (40) --------------------------------------------------
    "FeeDelayTooSoon",
    "FeeQueueOccupied",
    "NoActiveWeek",
    "FeeUnchanged",
    // -- poll_and_act (50) ------------------------------------------------
    "NotActivated",
    "CooldownActive",
    "InsufficientSamples",
    "StaleTwapSample",
    "OracleDeviation",
    "PoolEmpty",
    "PoolNotSwappable",
    "SwapOutputZero",
    "SafetyLimitExceeded",
    "ActionAlreadyActive",
    // -- harvest (60) ----------------------------------------------------
    "NothingToHarvest",
    // -- appended after T2 (60) ------------------------------------------
    "SlippageExceeded",
    "StalePoolState",
];

#[test]
fn idl_errors_match_errors_rs() {
    // Integration tests run with the crate directory as CWD under `anchor test`
    // and the workspace root under `cargo test`; resolve from the manifest so
    // neither is assumed. Requires `anchor build` to have run first.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let idl_path = manifest.join("../../target/idl/elastic_treasury.json");
    let text = std::fs::read_to_string(&idl_path).unwrap_or_else(|e| {
        panic!(
            "could not read generated IDL at {} (run `anchor build` first): {e}",
            idl_path.display()
        )
    });
    let idl: serde_json::Value = serde_json::from_str(&text).expect("IDL is valid JSON");

    let errors = idl["errors"].as_array().expect("IDL has an errors array");
    let names: Vec<&str> = errors
        .iter()
        .map(|e| e["name"].as_str().expect("error has a name"))
        .collect();

    assert_eq!(
        names,
        ERROR_NAMES,
        "IDL error order diverged from errors.rs: renumbering breaks every client"
    );
    assert_eq!(
        names.len() as u8,
        NUM_ERRORS,
        "the exported NUM_ERRORS constant is out of date"
    );

    // Anchor assigns codes sequentially from 6000 in declaration order. Freeze
    // that too, so a client can trust the stable IDs, not just the names.
    for (i, e) in errors.iter().enumerate() {
        assert_eq!(
            e["code"].as_u64().unwrap(),
            6000 + i as u64,
            "error codes are not sequential from 6000"
        );
    }
}