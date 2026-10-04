# PLAN.md: Elastic Treasury Token Program (Anchor / Rust, Token-2022)  [v2]

Scope: **on-chain program only.** The off-chain keeper/AI agent is a separate project; this plan only defines the interface it will use (Section 9).

**Changes in v2:** weekly published parameter schedule (Mondays 00:01 UTC); formula v1 = original equation as written; treasury stimulus removed; fee cap 50%; activation in USD (Pyth) and latched; short hop count k=4 fixed from the lender/short-seller model.

---

## 0. Rules for the coding agent (read first)

1. Work one task at a time, in order. Do not start a task until the previous task's acceptance criteria pass.
2. Never invent APIs. If unsure about an Anchor, `spl-token-2022`, Pyth, or Raydium call, read the crate source or docs and cite the file/version in a code comment. Pin all versions.
3. After every task: `anchor build` and `cargo test` must pass, then commit with message `T<number>: <title>`.
4. All arithmetic uses checked ops or `u128` intermediates. No truncating `as` casts. No floats in on-chain code.
5. Any invalid input reverts the whole transaction with a named custom error. Never partially apply state.
6. Do not add features that are not in this plan. Put questions in `QUESTIONS.md` instead of guessing.
7. Treat every keeper-supplied number as untrusted. The program recomputes or bounds-checks everything itself.

---

## 1. Decisions log

| # | Decision | Status |
|---|----------|--------|
| D1 | Program and keeper are separate projects. This repo = program only. | Final |
| D2 | Token standard = **Token-2022 with TransferFeeConfig**. | Final |
| D3 | Fee changes use native `SetTransferFee` via CPI, signed by a program PDA that is the `transfer_fee_config_authority`. Fee changes are **queued and take effect at an epoch boundary**, never at an exact wall-clock time. | Final (verify V1, V2, V8) |
| D4 | `polling_rate_hours` default = 168 (one week), `u32 >= 1`, fixed at `initialize`. It is the volatility lookback window. | Final |
| D5 | v1 DEX = **Raydium CPMM** behind a `Venue` adapter; more pools later (T13). | Final |
| D6 | Order-book analysis lives in the keeper. The program reads only on-chain pool state. | Final |
| D7 | Strict validation at `initialize` and on every schedule publish. Compile-time checks apply to constants only. | Final |
| D8 | **Weekly schedule:** parameters and the formula version change only on Mondays 00:01 UTC, and each week's entry must be published on-chain at least 7 days ahead. | Final |
| D9 | **Formula v1 = the original equation exactly as written** (Section 6.2). Later versions are added as new `formula_version` values; v1 is never edited. | Final |
| D10 | **Fee cap = 50%** (`MAX_FEE_BPS = 5000`), a hard program constant. | Final |
| D11 | **Treasury stimulus is removed** from v1. `volatility_min` is stored and reported but triggers no spending (reserved for a future decision). | Final |
| D12 | Activation thresholds are **USD equivalents**, read via a Pyth price feed. Once activated, the program **stays activated** even if the metric later falls below the threshold. | Final (verify V7) |
| D13 | Short hop count `k = 4` (LP loans tokens to short seller, short seller sells, re-buys, returns tokens to LP). Stored as a constant `SHORT_HOPS = 4`. | Final for v1 |
| D14 | Timezone for the schedule = **UTC** (no DST shifts). | Assumed (Q2) |

---

## 2. Facts to verify before relying on them (Task T0)

Create `docs/VERIFIED.md`; record each result with a source link or file path.

- **V1:** `SetTransferFee` takes effect at `current_epoch + 2` (confirmed in Solana docs). Check whether a fee *decrease* behaves differently in `process_set_transfer_fee`.
- **V2:** Does a second `SetTransferFee` call, made before the first has taken effect, **overwrite** the pending ("newer") entry? Only one pending change exists at a time, so this drives the cancel/queue design.
- **V3:** `maximum_fee` caps the fee per transfer in raw units; set it to `u64::MAX` so it never clamps. Confirm max `transfer_fee_basis_points` = 10_000 (the program enforces its own 5_000 cap on top).
- **V4:** Raydium **CPMM** supports Token-2022 mints with TransferFee (and which other extensions it rejects). Assume AMM v4 does not. Record the program ID, swap accounts, and observation (TWAP) account layout.
- **V5:** Mainnet slots-per-epoch and approximate epoch duration (about 2-3 days).
- **V6:** Whether Anchor's current release has `token_interface`/`token_2022` CPI helpers for `set_transfer_fee`, `harvest_withheld_tokens_to_mint`, `withdraw_withheld_tokens_from_mint`; otherwise use raw `invoke_signed`.
- **V7:** Pyth on-chain price feed usage for SOL/USD (and the token price if needed): current SDK crate, account layout, staleness (`max_age`) and confidence checks.
- **V8:** Which `Clock` fields are available (`unix_timestamp`, `epoch`, `epoch_start_timestamp`) and how to estimate the wall-clock time of a future epoch boundary. Record the estimation error.
- **V9:** Confirm that a Token-2022 transfer hook **cannot** charge or modify a transfer fee (it cannot change the transfer amount), so native `TransferFeeConfig` is the only fee mechanism.

**Acceptance:** all nine answered. Any answer that contradicts this plan goes to `QUESTIONS.md` before T1.

---

## 3. Architecture

```
programs/elastic_treasury/src/
  lib.rs, constants.rs, errors.rs
  state/      config.rs treasury.rs poll.rs action.rs schedule.rs
  instructions/
    initialize.rs  activate.rs
    publish_weekly_params.rs  cancel_weekly_params.rs
    queue_scheduled_fee.rs  harvest_fees.rs
    poll_and_act.rs  revert_action.rs
  venue/      mod.rs (trait Venue)  raydium_cpmm.rs
  oracle/     pyth.rs
  math/       mod.rs amm.rs fee_deterrent.rs volatility.rs fixed.rs
tests/        (Anchor TS + LiteSVM/Bankrun Rust)
docs/ VERIFIED.md  DESIGN.md  THREATS.md
QUESTIONS.md
```

**Two layers of control (important to document publicly):**
- **Slow layer (weekly, published):** a `WeeklyParams` entry fixes the base fee, fee ceiling, volatility bounds, safety limit and formula version for one Monday-to-Monday week. Nothing here can change mid-week.
- **Fast layer (inside the week, bounded by the published values):** (a) the fee can be raised in response to volatility, but at most one queued change per epoch, effective two epochs later and never above the published ceiling or 50%; (b) treasury buy defense (BuyDefend), which is the only path that can react faster than the epoch delay.

**Trust model**
- `developer` can only publish/cancel future weekly entries. It cannot touch the current week.
- `keeper` is the only signer for `poll_and_act`; it can only pick actions the program validates.
- Fee-config authority, withdraw-withheld authority and treasury token accounts are **PDAs** owned by this program.

---

## 4. State accounts

### 4.1 `Config` (PDA `["config", mint]`), set at `initialize`, immutable except where stated
- `developer: Pubkey` (rotation via two-step propose/accept, see T6), `keeper: Pubkey`
- `polling_rate_hours: u32` (default 168)
- `activation: { kind: enum { Liquidity, MarketCap }, value_usd: u64 }`  (exactly one kind)
- `activated: bool` (latched, never reset to false)
- `mint`, `pool`, `quote_mint`, `quote_usd_feed: Pubkey`, bumps
- `formula_version: u8` (current active version, copied from the active week)
- Program-set: `elasticity_scaled: u64`, `last_window_volatility_bps: u32`, `high_water_mark: u64` (see Q4)

### 4.2 `WeeklyParams` (entries stored in `Schedule`, PDA `["schedule", mint]`, fixed array of 4 slots)
Each entry: `week_effective_ts: i64` (a Monday 00:01 UTC), `published_ts: i64`, and
- `base_fee_bps: u16`, `max_fee_bps: u16` (<= 5000)
- `volatility_min_bps: u16`, `volatility_max_bps: u16` (`0 < min < max <= 10_000`)
- `safety_limit_bps: u16` (`1..=10_000`)
- `formula_version: u8` (must be a version the program supports; v1 only at launch)
- `short_hops: u8` (= `SHORT_HOPS` constant for now)

Plus `current_index` and the `FeeQueueStatus` (see 5.3).

### 4.3 `TreasuryState` (PDA `["treasury", mint]`)
`token_vault`, `quote_vault`, `total_fees_harvested`, `total_quote_spent`, `total_quote_gained`.

### 4.4 `PollState` (PDA `["poll", mint]`)
`window_start_ts`, `window_start_price`, `window_high`, `window_low`, `last_check_ts`, `last_check_epoch`, ring buffer of the last N <= 16 TWAP samples.

### 4.5 `ActiveAction` (PDA `["action", mint]`)
`kind: enum { RaiseFee, BuyDefend }`, `started_ts`, `started_epoch`, `fee_before_bps`, `fee_after_bps`, `effective_epoch` (= start + 2), `quote_used`, `observed_effective_ts` (filled once seen).

---

## 5. Instructions and guardrails

### 5.1 Instruction set

| Instruction | Signer | Purpose |
|---|---|---|
| `initialize` | developer | Validate and create Config, Schedule (week 0), Treasury, Poll. Set fee-config and withdraw-withheld authorities to program PDAs. |
| `activate` | anyone | Compute liquidity or market cap in USD from pool + Pyth; if >= threshold set `activated = true` permanently. |
| `publish_weekly_params` | developer | Append a validated entry. `week_effective_ts` must be a Monday 00:01 UTC at least `MIN_PUBLISH_LEAD_SECS` (7 days) in the future and at most 4 weeks ahead. |
| `cancel_weekly_params` | developer | Remove a future entry that has not started. |
| `queue_scheduled_fee` | anyone (crank) | CPI `SetTransferFee` for the next week's `base_fee_bps`, called in the epoch chosen by 5.3 so the change lands closest to Monday 00:01 UTC. |
| `harvest_fees` | anyone | CPI harvest to mint, then withdraw to `token_vault`. |
| `poll_and_act` | keeper | Core logic, 5.2. |
| `revert_action` | keeper/anyone | Restore base fee, 5.4. |

### 5.2 `poll_and_act`
1. Require `activated`; signer == `keeper`; resolve the active `WeeklyParams` from the clock (the entry with the latest `week_effective_ts <= now`).
2. Require one full lookback window of samples exists (`polling_rate_hours`) and `now >= last_check_ts + ACTION_COOLDOWN_SECS` (const, default 24h; see Q1).
3. Read price/reserves from Raydium CPMM on-chain and the TWAP from its observation account. **Never trust a keeper price.** Revert if spot vs TWAP deviates by more than `MAX_ORACLE_DEVIATION_BPS`.
4. Compute rolling-window volatility (6.3), then:
   - `vol <= volatility_max`: no action (record state; `volatility_min` is only reported).
   - `vol > volatility_max` and price rising: **RaiseFee** to the required fee from Section 6.2, clamped to `[current_fee, min(week.max_fee_bps, 5000)]`.
   - `vol > volatility_max` and price falling: **BuyDefend**: buy the token with treasury quote, sized with 6.1 to bring the move back within `volatility_max`.
5. **Fund safety check:** `quote_used * 10_000 <= safety_limit_bps * quote_vault_balance`; otherwise cancel (see Q3). Enforce `MAX_SLIPPAGE_BPS` on every swap.
6. At most one unreverted `ActiveAction` at a time (a RaiseFee may be raised further within the cap).
7. Emit `ActionTaken` (all inputs/outputs) for off-chain audit.

### 5.3 Fee scheduling vs the epoch delay
- Native fee changes are queued for `epoch + 2` and cannot be timed to a minute. **Exact Monday 00:01 timing is achievable only for program logic** (parameter switch, volatility bounds, safety limit, which formula runs), because those read `Clock::unix_timestamp`. **The fee itself changes at the epoch boundary nearest Monday**, an expected error of up to roughly one epoch (about 1-3 days). State this in the public README.
- `queue_scheduled_fee` picks the call epoch `E` such that the estimated start time of epoch `E+2` is closest to the target Monday (use V8; `EST_SLOT_MS` constant; record error in `observed_effective_ts` once the new fee is read back from the mint).
- Only one pending change exists at a time (V2). If an intra-week RaiseFee is pending when a scheduled change is due, the scheduled base fee takes precedence and overwrites it.
- Because the fee lags by two epochs, treat the fee path as **trend control**, not real-time control.

### 5.4 Reverting actions
A RaiseFee ends when BOTH hold: (a) the next weekly boundary has passed or one full `polling_rate_hours` window has elapsed since it started, whichever is later, and (b) the latest check shows `vol <= volatility_max`. Revert queues the active week's `base_fee_bps` (also subject to the 2-epoch delay unless V1 shows decreases are immediate).

### 5.5 Invariants (all unit-tested)
- I1: All schedule/init values valid; invalid input reverts with zero state change.
- I2: A single action never uses more than `safety_limit_bps` of the quote vault.
- I3: Only `developer` publishes/cancels; only `keeper` calls `poll_and_act`.
- I4: A published entry cannot take effect before its Monday 00:01 UTC, and cannot be published less than 7 days ahead. The current week's entry is immutable.
- I5: `poll_and_act` obeys the cooldown.
- I6: Fee never exceeds `min(week.max_fee_bps, 5000)` and never goes below 0.
- I7: Treasury value cannot fall by more than `safety_limit_bps` in one action.
- I8: Spot vs TWAP deviation guard.
- I9: `activated` can only go false to true.

---

## 6. Calculator module (pure functions, heavily unit-tested)

Fixed-point scale 1e9, `u128` intermediates, no floats. Round conservatively (spend less, require the higher fee).

### 6.1 Constant-product trade sizing
Pool quote reserve `Y`, token reserve `X`, price `P = Y/X`.
- To move price to `P_t`: `Y' = Y * sqrt(P_t / P)`; quote needed `dY = Y * (sqrt(P_t/P) - 1)`, grossed up for the pool fee: `dY_gross = dY / (1 - pool_fee)`.
- Tokens to sell for a downward move: `dX = X * (sqrt(P/P_t) - 1)`.
- Integer `isqrt` (Newton) with tests. Compute `min_out` from `dY` and `MAX_SLIPPAGE_BPS`, accounting for the transfer fee on pool-to-user transfers.

### 6.2 Fee deterrent: formula v1 (the original equation, as written)
Notation: `S` starting tokens, `P0` starting price, `Pf` final price, `r` = per-transfer **retention** (`r = 1 - fee`). The original equation uses its `f` as the retention factor (the only reading under which higher fees help).

Original: `S*P0 = r^4 * S*Pf + S*r^2*Pf`, whose right side factors to `S*Pf*r^2*(1+r^2)`. Run as the inequality `S*P0 > S*Pf*r^2*(1+r^2)`.

Let `R = P0/Pf` and `u = r^2`. Then `R > u*(1+u)`, so `u^2 + u - R < 0`, giving
- `u* = (-1 + sqrt(1 + 4R)) / 2`
- retention needed `r < sqrt(u*)`, so **`fee_required = 1 - sqrt(u*)`**
- if `R >= 2` then `u* >= 1` and no extra fee is required (return the current fee).

Implementation: two integer square roots on the fixed-point scale; round the required fee **up**; clamp to `[current_fee, min(max_fee_bps, 5000)]`. Keep `short_hops` in the entry and in the function signature even though v1's exponents are fixed as written, so a v2 can use it.

Reference values (for tests): `R=1.0 -> 21.4%`, `R=1.1 -> 18.6%`, `R=1.25 -> 14.9%`, `R=1.5 -> 9.3%`, `R>=2 -> 0%`.

Documented properties of v1 (add as tests so they are visible, not hidden):
- It stops binding at `R >= 2` (price drops of 50% or more), which are the cases where a short is most profitable. This is a known limitation to revisit in v2.
- It deters only shorts that pass through this token's taxed transfers (loan, sell, re-buy, return). Perpetual-futures shorts never move this token and are unaffected.
- The fee applies 2 epochs after queuing, so fast crashes can finish first.

### 6.3 Volatility
- Range: `vol_bps = (window_high - window_low) * 10_000 / window_start_price`; direction from the sign of `(price_now - window_start_price)`.
- Samples come from the CPMM observation account (TWAP), never from the keeper. Lookback = `polling_rate_hours`.

### 6.4 Elasticity (program-set)
`elasticity` = bps price move per 1% of quote reserve traded; derived from reserves via 6.1; stored for the keeper's use.

### 6.5 Activation metrics (USD)
- `liquidity_usd` = quote-side reserve value in USD (quote reserve * Pyth quote/USD price). Q5 asks whether this should be both sides instead.
- `market_cap_usd` = circulating supply * token USD price (token price = pool price * quote/USD price). Circulating supply definition: Q6.
- Reject stale (`max_age`) or low-confidence Pyth prices.

---

## 7. Task list (each ends with acceptance criteria)

**T0: Verification.** Section 2 into `docs/VERIFIED.md`. *Accept:* V1-V9 answered with sources.

**T1: Scaffold.** `anchor init`, pinned versions, module layout, `constants.rs`: `MAX_FEE_BPS = 5000`, `MAX_POLL_HOURS`, `MAX_SLIPPAGE_BPS`, `MAX_ORACLE_DEVIATION_BPS`, `SHORT_HOPS = 4`, `WEEK_SECS = 604_800`, `MONDAY_0001_UTC_OFFSET = 345_660` (Unix time 0 was a Thursday; first Monday 00:01 UTC is 345_660 seconds in, so a valid Monday 00:01 satisfies `(ts - 345_660) % 604_800 == 0`), `MIN_PUBLISH_LEAD_SECS = 604_800`, `ACTION_COOLDOWN_SECS = 86_400`, `const` assertions on all bounds. *Accept:* builds; `cargo clippy -- -D warnings` clean; a test confirms a known Monday 00:01 UTC timestamp passes the check and a Sunday one fails.

**T2: Math module.** `fixed.rs`, `amm.rs`, `fee_deterrent.rs` (formula v1), `volatility.rs` with unit and property tests (`proptest`). *Accept:* reference values in 6.2 reproduce within 1 bps; `R>=2` never raises the fee; fee output is always <= 5000; no panics on extreme inputs.

**T3: State and errors.** All accounts in Section 4 with `InitSpace`, size assertions, full `errors.rs`. *Accept:* serialization round-trips.

**T4: `initialize`.** Validate everything (I1); set PDAs as authorities (document in `docs/DESIGN.md` whether the program creates the mint via CPI or an authority transfer is required). *Accept:* every invalid input reverts with a specific error and zero state change.

**T5: `activate`.** Pyth + pool read, latch behavior. *Accept:* fails below threshold, succeeds above, stays activated after the metric falls (I9), rejects stale/low-confidence oracle data.

**T6: Weekly schedule.** `publish_weekly_params`, `cancel_weekly_params`, plus two-step developer rotation. *Accept:* I3, I4 tests; rejects non-Monday timestamps, <7-day lead, >4 weeks ahead, fee > 5000, `min >= max`; current week's entry cannot be changed.

**T7: `queue_scheduled_fee` + `harvest_fees`.** Epoch-targeting logic from 5.3. *Accept:* with a test validator at short epochs, the base fee queues in the intended epoch, `newer_transfer_fee.epoch = current + 2`, and `observed_effective_ts` is recorded; harvest increases `token_vault` by exactly the withheld amount.

**T8: Venue adapter.** `trait Venue` and Raydium CPMM implementation, TWAP read. *Accept:* swap with a Token-2022 fee mint on a cloned pool respects `min_out`.

**T9: `poll_and_act`.** Section 5.2 using T2, T8. *Accept:* I2, I5-I8; scenario tests for no-action, high-vol rise to fee, high-vol fall to buy, safety cancel, cooldown rejection. No code path spends treasury funds when `vol < volatility_min`.

**T10: `revert_action`.** Section 5.4. *Accept:* cannot revert early or while volatility is still above max.

**T11: Adversarial tests.** Flash manipulation before a check, forged accounts, sandwiching the treasury swap, rapid repeated calls, stale oracle, schedule publish at the last second, zero-liquidity pool, wrong pool/mint. *Accept:* each attack reverts or costs <= `safety_limit_bps` of treasury; findings in `docs/THREATS.md`.

**T12: Devnet deploy and docs.** Scripted multi-week scenario (local validator with short epochs first). README must plainly state: parameters change only Mondays 00:01 UTC, published 7+ days ahead; the fee lands at the nearest epoch boundary, not the exact minute; the 50% fee cap; the formula-v1 limitations. *Accept:* README reviewed against Sections 5-6.

**T13 (later): Multi-venue.** Second `Venue` (Orca/Meteora), allow-listed pool registry, median price / summed depth. *Accept:* aggregation survives one manipulated or paused pool.

**Future (not in this plan):** treasury stimulus when `vol < volatility_min` (needs a decision and legal review); formula v2 (candidate: closed form `fee = 1 - (Pf/P0)^(1/k)`); minute-level fee control (impossible with native TransferFeeConfig).

---

## 8. Testing strategy
Unit and property tests for `math/`; LiteSVM/Bankrun with clock and epoch warping; Anchor TS end-to-end on a local validator with CPMM cloned; fuzz validators and calculator; external audit before any mainnet funds.

---

## 9. Keeper interface (for the later agent project)
- Read: `Config`, `Schedule`, `PollState`, `ActiveAction`, `TreasuryState`, `ActionTaken` events.
- Write: `poll_and_act(hint)` where the hint (action kind, size, fee) is only a suggestion; the program recomputes and bounds-checks everything. The keeper also calls `harvest_fees`, `queue_scheduled_fee`, `revert_action`.
- Keeper owns: order-book analysis, short-horizon prediction, call timing.

---

## 10. Open questions (defaults apply if unanswered)

- **Q1 (T9):** Default assumed: with a weekly lookback, checks are allowed every `ACTION_COOLDOWN_SECS` (24h) using a rolling window, so the program can act day to day. Alternative: check only once per `polling_rate_hours`. Confirm.
- **Q2 (T1):** Schedule timezone: UTC (default). A local zone with DST would shift the on-chain Monday offset twice a year.
- **Q3 (T9):** When the safety limit blocks an action: revert (default) or succeed as a logged no-op?
- **Q4 (T3):** What is the high-water mark for (token price vs treasury value) and what does it gate? Default: tracked and reported only, gates nothing.
- **Q5 (T5):** "Liquidity" = quote-side USD value only (default, matches your example of $24,000 of SOL) or total pool value (both sides)?
- **Q6 (T5):** Market-cap supply = total mint supply (default) or supply minus treasury/burned/locked amounts?
