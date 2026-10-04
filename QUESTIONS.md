# QUESTIONS.md

Open items from PLAN.md §10 plus the questions raised by the T0 verification pass.
**Rule (PLAN.md §0.6): every question below is currently running on its stated default.**

## From PLAN.md §10

| # | Question | Default in force | Raised | Answered |
|---|---|---|---|---|
| Q1 | With a weekly lookback, are `poll_and_act` checks allowed every `ACTION_COOLDOWN_SECS` (24 h) on a rolling window, or only once per `polling_rate_hours`? | Every 24 h on a rolling window | T9 | |
| Q2 | Schedule timezone. | **UTC**, no DST (D14) | T1 | |
| Q3 | When the fund safety limit blocks a BuyDefend: revert the tx, or succeed as a logged no-op? | **Revert** the tx (T9) | T9 | |
| Q4 | What does `high_water_mark` gate? | Tracked and reported only, gates nothing (T3) | T3 | |
| Q5 | Is "liquidity" the quote-side USD value only, or both sides of the pool? | **Quote side only** (T5) | T5 | |
| Q6 | Is market cap total mint supply, or supply net of treasury/burned/locked? | **Total mint supply** (T5) | T5 | |

## Raised by T0 verification (all resolved by evidence, none blocking)

These are **not** questions — they are decisions forced by the sources in `docs/VERIFIED.md`, which
contradict or complete the plan text. They are listed so the reasoning stays visible and reviewable.

| # | Question the plan left open | Resolution | Evidence |
|---|---|---|---|
| A1 | Do fee *decreases* also wait two epochs? (PLAN §5.4: "unless V1 shows decreases are immediate") | **Yes they do.** No faster revert path exists. | V1 |
| A2 | Does a queued fee change survive a second `SetTransferFee`? | **No, it is silently overwritten.** Need `FeeQueueStatus` + idempotent `queue_scheduled_fee`. | V2 |
| A3 | Is CPMM's `minimum_amount_out` net or gross of the Token-2022 fee? | **Net, and pass it as-is.** No `calculate_pre_fee_amount` gross-up: the pool compares `amount_received` (already net) against the threshold, so grossing up would be stricter than the plan. The gross-up applies to the *inbound* leg only. | V4 |
| A4 | Is the CPMM curve fed the input amount gross or net of the Token-2022 fee? | **Net.** Gross up before the CPI. | V4 |
| A5 | Do CPMM vault balances need adjustment before computing price? | **Yes** — accrued protocol + fund + creator fees are excluded from the curve. | V4 |
| A6 | Is the CPMM observation account a price series or cumulative prices? | **Cumulative Q32.32 × seconds in a 100-slot ring, advancing only every ≥15 s.** No TWAP may exist at all. | V4 |
| A7 | What `EST_SLOT_MS`? | **350 ms**, epoch ≈ 42 h, error ±~1 day. | V5, V8 |
| A8 | How is Pyth `max_age` expressed? | **Slots, not seconds.** `max_age_slots` configurable; SDK's `VALID_SLOT_PERIOD = 25` is devnet-sized. | V7 |
| A9 | Does the Pyth SDK validate confidence? | **No**, and `price` is `i64`. Check `conf` and positivity ourselves. | V7 |
| A10 | Which extensions may our mint carry? | **Only `TransferFeeConfig`** (optionally `MetadataPointer`). Anything else makes CPMM refuse to create the pool. | V4 |
| A11 | Which Pyth crate reads the price account? | **Not `pyth-sdk-solana` — it cannot compile.** It needs borsh 0.10 while Anchor's Solana cohort needs borsh 1.x, and no pin resolves it. `oracle/pyth.rs` reads the POD layout locally. | V7 |

## Newly raised, needing a decision before T4

| # | Question | Interim handling |
|---|---|---|
| B1 | PLAN §4.1 does not list `oracle_max_age_slots` / `max_conf_bps` in `Config`, but A8/A9 require them as tunable values. | Add both to `Config`, defaulted in `initialize` (T4). Flagged for a §4.1 amendment. |
| B2 | PLAN §4.2 mentions `FeeQueueStatus` but never defines it. | Define in `state/schedule.rs` as: `pending_fee_bps`, `pending_fee_epoch`, `pending_fee_source { ScheduledBase, IntraWeekRaise, Revert }`, `last_queued_epoch`. Needed for A2. |
| B3 | PLAN §5.1 `harvest_fees` says "harvest to mint, then withdraw to `token_vault`" but does not say who pays the rent/top-up if `token_vault` does not exist. | `token_vault` is created by the program at `initialize` and funded by the developer in the same tx. |
| B4 | PLAN §3 says fee-config / withdraw-withheld authorities are PDAs. §4.1 does not store their bumps. | Store `fee_authority_bump` and `withdraw_authority_bump` in `Config`. |
| B5 | Should `publish_weekly_params` be able to modify an entry that is published but not yet effective? | **No** — I4 makes the entry immutable once published; use `cancel` then re-publish. Keeps "what the market saw" auditable. |
| B6 | `poll_and_act` hint (§9) — is it even accepted? | Accepted and logged, then **entirely ignored** by the decision logic. Present so the keeper's tx shape is stable, per §0.7. |
