# VERIFIED.md — fact check log for PLAN.md v2

Every claim in PLAN.md §2 (V1–V9) was checked against primary source, not docs prose alone.
Each entry names the file and version that settles it. Anything where the plan turned out to be
**wrong or incomplete** is flagged `PLAN AMENDMENT` and is carried into `QUESTIONS.md`.

Toolchain pinned at time of verification:

| Tool | Version |
|---|---|
| `anchor-cli` | 1.1.2 |
| `solana-cli` | 3.1.10 (`Agave`, `src:7bc9c805`) |
| `rustc` / `cargo` | 1.97.1 |
| `anchor-lang` (target) | 1.1.2 |
| `spl-token-2022` (source read) | `main` @ 2026-09-23, crate `11.1.0` |
| `spl-token-interface` (target) | `3.0.0` |
| `pyth-sdk-solana` (target) | `0.10.6` |
| `raydium-cp-swap` (source read) | `master` |

---

## V1 — Does `SetTransferFee` always take effect at `current_epoch + 2`, including decreases?

**Yes. Always `+2`, no asymmetry.** Fee *decreases* are **not** immediate.

Source: `solana-program/token-2022` → `program/src/extension/transfer_fee/processor.rs`,
`process_set_transfer_fee` (lines 95–112):

```rust
// * newer transfer fee epoch >= next epoch: it was never used, so just
//   overwrite next transfer fee
let epoch = Clock::get()?.epoch;
if u64::from(extension.newer_transfer_fee.epoch) <= epoch {
    extension.older_transfer_fee = extension.newer_transfer_fee;
}
// set two epochs ahead to avoid rug pulls at the end of an epoch
let newer_fee_start_epoch = epoch.saturating_add(2);
extension.newer_transfer_fee = TransferFee { epoch: newer_fee_start_epoch.into(), .. };
```

There is no branch on the sign of the fee change. PLAN.md §5.4's parenthetical
("unless V1 shows decreases are immediate") resolves to **no**: reverting a fee down is *also*
delayed by two epochs. Consequences accepted as-is:

- `revert_action` cannot restore the base fee faster than the raise did.
- PLAN.md §3 "Fast layer (a)" is even slower than the plan assumed; §5.3's
  "treat the fee path as trend control" is the correct framing and is stated in the README.
- The only lever that beats the 2-epoch delay is `BuyDefend`, which does not change the fee.

Corroborated by the official docs page ("updates the next transfer fee configuration, which takes
effect starting two epochs later"): <https://solana.com/docs/tokens/extensions/transfer-fees>

## V2 — Does a second `SetTransferFee` before the first takes effect overwrite the pending one?

**Yes — the pending entry is silently overwritten.** Only one change can ever be queued.

Same function, lines 101–112: when `newer_transfer_fee.epoch > current_epoch` (the "pending" case)
the code *skips* the `older_transfer_fee = newer_transfer_fee` shuffle and assigns
`newer_transfer_fee = TransferFee { epoch: current + 2, .. }` unconditionally. The previously
queued change is destroyed with no error and no return value.

`TransferFeeConfig::get_epoch_fee` (`interface/src/extension/transfer_fee/mod.rs:152`) selects
`newer` iff `epoch >= newer_transfer_fee.epoch`, else `older` — so exactly two configs can ever be
live, and only the newest *call* survives.

**Design consequence, matches PLAN.md §5.3.** Last writer wins. The program must therefore track
in `FeeQueueStatus` (PLAN.md §4.2) which epoch currently holds a pending change and on whose
authority, so that (a) a keeper cannot clobber a scheduled base fee with an intra-week raise,
(b) `queue_scheduled_fee` can be called idempotently without damaging anything, and (c) the
scheduled base fee always takes precedence.

## V3 — `maximum_fee` caps the fee in raw units; is `MAX_FEE_BASIS_POINTS` 10 000?

**Confirmed.**

- `interface/src/extension/transfer_fee/mod.rs:26` — `pub const MAX_FEE_BASIS_POINTS: u16 = 10_000;`
- Enforced on write in both `process_initialize_transfer_fee_config` and
  `process_set_transfer_fee`: `if transfer_fee_basis_points > MAX_FEE_BASIS_POINTS { return Err(TokenError::TransferFeeExceedsMaximum.into()) }`
- `TransferFee::calculate_fee` (`mod.rs:58`) rounds the fee **up**
  (`ceil_div(pre_fee_amount * bps, 10_000)`) and then clamps:
  `Some(cmp::min(raw_fee, u64::from(self.maximum_fee)))`.
- So `maximum_fee = u64::MAX` never clamps for any `u64` amount. Plan's choice is correct.

Rounding details that the calculator must mirror:

- **Fee on a transfer rounds UP** (`ceil_div`). Never assume the pool receives `amount - fee`
  computed with floor division.
- `TransferFee::calculate_pre_fee_amount` (`mod.rs:89`) is the exact gross-up helper: given a
  desired net receipt it returns the smallest `pre_fee_amount` that nets that much
  (`ceil_div(net * 10_000, 10_000 - bps)`), also clamped by `maximum_fee`. Use this rather than
  hand-rolling a divide.

The program's own `MAX_FEE_BPS = 5000` (§D10) is strictly tighter than the mint's 10 000, which is
why the mint-level cap is not a defence.

## V4 — Does Raydium CPMM support Token-2022 mints with `TransferFee`? Layouts?

**Yes — `TransferFeeConfig` is explicitly on CPMM's allow-list.** AMM v4 is not Token-2022 at all.

- Program ID (verified live on mainnet, `BPFLoaderUpgradeab1e...`, executable):
  **`CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C`**
  — `raydium-cp-swap/programs/cp-swap/src/lib.rs:25`
- `is_supported_mint` allow-list (exactly these five extensions; anything else reverts pool
  creation with `NotSupportMint`): `TransferFeeConfig`, `MetadataPointer`, `TokenMetadata`,
  `InterestBearingConfig`, `ScaledUiAmount` — `programs/cp-swap/src/utils/token.rs`.
  A mint created by this program carries **only** `TransferFeeConfig`, so it passes.
  Rejected and therefore forbidden for our mint: `TransferHook`, `NonTransferable`,
  `PermanentDelegate`, `DefaultAccountState`, `Pausable`, `MintCloseAuthority`,
  `GroupPointer`/`GroupMemberPointer`, `ConfidentialTransfer*`. **This is a hard constraint on
  mint creation — recorded in `docs/DESIGN.md` and `docs/THREATS.md`.**
- Bypass: an admin can register an arbitrary mint via the `SupportMintAssociated` PDA
  (`[b"support_mint", mint]`). Not used, not needed.

### Instruction: `swap_base_input(amount_in: u64, minimum_amount_out: u64)`
`programs/cp-swap/src/instructions/swap_base_input.rs`, 13 accounts in this exact order:

| # | Account | W | Notes |
|---|---|---|---|
| 0 | `payer` | | **signer** |
| 1 | `authority` | | PDA, seeds `[b"vault_and_lp_mint_auth_seed"]`, `bump` |
| 2 | `amm_config` | | `address = pool_state.amm_config` |
| 3 | `pool_state` | ✓ | `AccountLoader<PoolState>` |
| 4 | `input_token_account` | ✓ | |
| 5 | `output_token_account` | ✓ | |
| 6 | `input_vault` | ✓ | must be `token_0_vault` or `token_1_vault` |
| 7 | `output_vault` | ✓ | must be `token_0_vault` or `token_1_vault` |
| 8 | `input_token_program` | | `Interface<TokenInterface>` — accepts classic **and** 2022 |
| 9 | `output_token_program` | | ditto |
| 10 | `input_token_mint` | | `address = input_vault.mint` |
| 11 | `output_token_mint` | | `address = output_vault.mint` |
| 12 | `observation_state` | ✓ | `address = pool_state.observation_key` |

`swap_base_output(max_amount_in: u64, amount_out: u64)` uses the **same** account list.

### PLAN AMENDMENT — how the Token-2022 transfer fee interacts with `minimum_amount_out`

The plan (§6.1) says to compute `min_out` "accounting for the transfer fee on pool-to-user
transfers". It does not say *where* that is checked. Source, lines 142–156:

```rust
let amount_out = u64::try_from(result.output_amount).unwrap();
let transfer_fee = get_transfer_fee(&ctx.accounts.output_token_mint.to_account_info(), amount_out)?;
let amount_received = amount_out.checked_sub(transfer_fee).unwrap();
require_gte!(amount_received, minimum_amount_out, ErrorCode::ExceededSlippage);
```

`minimum_amount_out` is compared against the **net** amount the destination account receives, i.e.
**after** the Token-2022 fee is withheld. So the number we pass must be the **net** figure we want
to receive:

```
min_out = floor(dY_net * (10_000 - MAX_SLIPPAGE_BPS) / 10_000)     // net, whole raw units
```

and pass `min_out` as `minimum_amount_out`. There is deliberately **no gross-up with
`calculate_pre_fee_amount`** here: that helper inverts the *input* leg, and grossing up a value the
pool then compares net would make the check strictly tighter than the plan asks for — a correctly
priced swap could revert. The Token-2022 fee on the output leg is already inside the pool's own
`amount_out` arithmetic, so it needs no correction on our side.

The gross-up that *is* required is the inbound one, below. Both directions are encoded once in
`math::amm::min_out_from_net` and `math::amm::assert_min_out_satisfied` and unit-tested, including
the negative case that delivers less than `min_out` after the CPI.

Inbound direction (line 88): `actual_amount_in = amount_in - transfer_fee`. **The pool curve is fed
`amount_in` net of the Token-2022 fee**, so sizing must gross up the quote spent before the CPI.

### CPMM fee + swap math (mirrored exactly in `math::amm`)
- `curve/fees.rs`: `FEE_RATE_DENOMINATOR_VALUE = 1_000_000`. `trading_fee` and `creator_fee` use
  **ceil**; `protocol_fee` and `fund_fee` use **floor**.
- `curve/constant_product.rs::swap_base_input_without_fees`:
  `out = (amount_in * out_vault) / (in_vault + amount_in)`, integer **floor**, `u128`.
- `PoolState::vault_amount_without_fee` subtracts accrued `protocol + fund + creator` fees from both
  vault balances *before* the curve sees them. The venue adapter must subtract the same fees when
  computing price, reserves and `min_out`, or every quote is wrong.
- `PoolState::token_price_x32`: `token1_amount * 2^32 / token_0_amount` (integer floor, `u128`).
  Note the stored cumulative prices are **Q32.32**, not Q64.64.

### Observation (TWAP) account layout
`programs/cp-swap/src/states/oracle.rs`, `#[repr(C, packed)]`, 100-element ring buffer:

```rust
Observation        { block_timestamp: u64, cumulative_token_0_price_x32: u128,
                      cumulative_token_1_price_x32: u128 }                 // 40 bytes
ObservationState   { initialized: bool, observation_index: u16, pool_id: Pubkey,
                      observations: [Observation; 100],
                      last_update_timestamp: u64, padding: [u64; 3] }
```

Total account len `ObservationState::LEN = 8 + 1 + 2 + 32 + 40*100 + 8*4 = 4_075`.

Offsets after the 8-byte Anchor discriminator: `initialized` 0, `observation_index` 1, `pool_id` 3,
`observations` 35, `last_update_timestamp` 4035, `padding` 4043.

> **PLAN AMENDMENT (important).** PLAN.md §5.2 step 3 and §6.3 say to read "the TWAP from its
> observation account". A naive caller will trip over three things, all now recorded:
> 1. The buffer is a **ring**; `observation_index` is the index of the newest written element and
>    wraps. Unwrap it before walking backwards.
> 2. The entries are **cumulative Q32.32 prices x seconds**, not instantaneous prices. TWAP over a
>    window is `Δcumulative / Δtime`, then un-scale by 2^32.
> 3. `ObservationState::update` returns early **without writing a new slot** when
>    `block_timestamp - last_update_timestamp < OBSERVATION_UPDATE_DURATION_DEFAULT` (= **15 s**,
>    `oracle.rs:11,117`). The buffer therefore only advances on swaps more than 15 s apart. A pool
>    with no swaps produces an empty buffer and **no TWAP is available at all**.
>
> Consequence: the program cannot assume a TWAP exists. `poll_and_act` must revert with a named
> error when fewer than `POLL_RING_LEN` in-window samples exist, and it must also bound the staleness
> of the newest sample. This is now INVARIANT I10 in `docs/DESIGN.md`.

### `PoolState` layout (what the venue adapter reads)
`#[account(zero_copy(unsafe))] #[repr(C, packed)]`, `LEN = 8 + 10*32 + 5 + 8*7 + 2 + 6 + 2*8 + 28*8 = 637`.

| offset | field | | offset | field |
|---|---|---|---|---|
| 8 | `amm_config` | | 333 | `lp_supply: u64` |
| 40 | `pool_creator` | | 341 | `protocol_fees_token_0` |
| 72 | `token_0_vault` | | 349 | `protocol_fees_token_1` |
| 104 | `token_1_vault` | | 357 | `fund_fees_token_0` |
| 136 | `lp_mint` | | 365 | `fund_fees_token_1` |
| 168 | `token_0_mint` | | 373 | `open_time` |
| 200 | `token_1_mint` | | 381 | `recent_epoch` |
| 232 | `token_0_program` | | 389 | `creator_fee_on: u8` |
| 264 | `token_1_program` | | 390 | `enable_creator_fee: bool` |
| 296 | `observation_key` | | 391 | `padding1: [u8; 6]` |
| 328 | `auth_bump: u8` | | 397 | `creator_fees_token_0` |
| 329 | `status: u8` | | 405 | `creator_fees_token_1` |
| 330 | `lp_mint_decimals: u8` | | 413 | `padding: [u64; 28]` |
| 331 | `mint_0_decimals: u8` | | | |
| 332 | `mint_1_decimals: u8` | | | |

`status` bit 2 (`4`) = swap disabled (`PoolStatusBitIndex::Swap`); swaps also require
`Clock::unix_timestamp >= open_time`.

> The `u64`/`u128` fields sit at **unaligned** offsets (333, 341, …). They are read with
> `u64::from_le_bytes(slice)` rather than a packed `#[derive(Pod)]` struct, per PLAN.md §0.4's ban
> on transmutes. `AmmConfig` is by contrast an ordinary borsh `#[account]` (`LEN = 236`).

## V5 — Mainnet slots per epoch and epoch duration

- `EpochSchedule` on mainnet, read live:
  `{"slotsPerEpoch":432000,"leaderScheduleSlotOffset":432000,"warmup":false,"firstNormalEpoch":0,"firstNormalSlot":0}`.
- `DEFAULT_SLOTS_PER_EPOCH = 432_000` — `solana-epoch-schedule` `src/lib.rs:31`.
- Live epoch state at time of writing: epoch **1049**, `slotIndex` 70 376, `absoluteSlot`
  453 238 376. Check: `1049 * 432000 = 453_168_000`; `+ 70_376 = 453_238_376`. Exact.
- Observed slot time is currently **≈ 350 ms**, i.e. ≈ **42 h** per epoch
  (≈ 273 epochs/year), not the historical 400 ms / 48 h. Real epochs still stretch to ~3 days when
  slots are missed.

So: **`EST_SLOT_MS` default 350**, and the plan's "expected error of up to roughly one epoch
(about 1-3 days)" is right. Worst case for `queue_scheduled_fee`'s "nearest epoch boundary" choice
is **± half an epoch, ≈ ±21 h**. Stated in the README.

## V6 — Does Anchor ship Token-2022 CPI helpers for the three fee instructions?

**Yes, all three.** `anchor-lang` `spl/src/token_2022_extensions/transfer_fee.rs` (feature
`token_2022_extensions`):

| Plan's need | Anchor helper |
|---|---|
| `initialize_transfer_fee_config` | `transfer_fee_initialize` |
| `SetTransferFee` | `transfer_fee_set` |
| `HarvestWithheldTokensToMint` | `harvest_withheld_tokens_to_mint` |
| `WithdrawWithheldTokensFromMint` | `withdraw_withheld_tokens_from_mint` |
| `WithdrawWithheldTokensFromAccounts` | `withdraw_withheld_tokens_from_accounts` |
| fee-aware transfer (if ever needed) | `transfer_checked_with_fee` |

Each is a `CpiContext`-based `invoke_signed`, i.e. PDA signing works directly. All are re-exported
through `anchor_spl::token_2022_extensions::*`. **No raw `invoke_signed` needed.**

Two caveats that shape the code:

- `harvest_withheld_tokens_to_mint` and `withdraw_withheld_tokens_from_accounts` take a
  `sources: Vec<AccountInfo>` — variable-length, so `remaining_accounts` or a fixed-size array.
  The plan's `harvest_fees` (§5.1) says "harvest to mint, then withdraw to `token_vault`", which is
  the two-step `harvest_withheld_tokens_to_mint` + `withdraw_withheld_tokens_from_mint` path; only
  the mint's own `withheld_amount` moves, so per-user accounts need not be listed.
- `anchor_spl::token_interface::Mint`/`TokenAccount` (used for pool reads) accept **either**
  token program — `impl Owners` returns both IDs — and `get_mint_extension_data::<T>()` reads an
  extension out of a raw `AccountInfo`. That is exactly what the CPMM adapter needs; the 13th
  CPMM account being `Interface<TokenInterface>` is not a problem.

## V7 — Pyth on-chain price feed: crate, layout, staleness, confidence

- Crate: **`pyth-sdk-solana` 0.10.6**. Account type
  `SolanaPriceAccount = GenericPriceAccount<32, ()>` (`src/state.rs:425`), a flat POD struct read
  with `SolanaPriceAccount::account_info_to_feed` / `load_price_account`.
- **PLAN AMENDMENT A11 — the SDK cannot be compiled into this program at all.**
  `pyth-sdk-solana` 0.10.6 depends on `borsh = "0.10.4"` and `solana-program = ">= 1.10"`.
  Every `solana-address` 2.x release (which `solana-program` 3.x pulls in) depends on
  `borsh = "^1.5.5"` and implements only borsh 1.x. The two majors cannot be unified, so the
  SDK's own `BorshDeserialize` derive on `Address` fields fails to compile:

  ```
  error[E0277]: the trait bound `Address: borsh::BorshDeserialize` is not satisfied
    --> pyth-sdk-solana-0.10.6/src/error.rs:2:5
  ```

  Pinning `solana-program` to Anchor's cohort does not help: the conflict is in `borsh`, not
  `solana-program`. 0.10.6 is the newest release and there is no fix upstream.
  **Resolution:** `oracle/pyth.rs` defines the price-account layout locally, transcribed
  field-for-field from `pyth-sdk-solana` 0.10.6 `src/state.rs`, and reads it with `bytemuck`.
  The layout is a flat POD struct — no borsh anywhere on the read path — so nothing is lost, and
  the program no longer depends on a crate carrying borsh 0.10. The transcription carries the
  upstream line numbers in comments so it can be re-checked on upgrade.

- Layout (`src/state.rs:292+`): `magic, ver, atype, size, ptype, expo: i32, num, num_qt,
  last_slot, valid_slot, agg: PriceInfo, ema_price, ema_conf, timestamp: i64, min_pub, drv2..drv4,
  prod, next, prev_slot, prev_price, prev_conf, prev_timestamp, agg_pub_slot`.
  `agg: PriceInfo { status, unused, price: i64, conf: u64, pub_slot }`.
- Read API used: `GenericPriceAccount::get_price_no_older_than(&clock, slot_threshold) -> Option<Price>`
  where `Price { price: i64, conf: u64, expo: i32, publish_time: i64 }`.

> **PLAN AMENDMENT.** PLAN.md §6.5 says "Reject stale (`max_age`)". `get_price_no_older_than`
> measures staleness in **slots, not seconds**, and the SDK's own
> `pub const VALID_SLOT_PERIOD = 25` (`src/lib.rs:33`) is a devnet-sized constant that is far too
> tight for mainnet. `max_age` is therefore implemented as **`max_age_slots: u64`** with a
> documented mainnet default, and confidence is a separate explicit check.

- **Confidence is not checked by the SDK.** `get_price_no_older_than` returns a price with
> `conf = 0` just as happily as a tight one. The program must reject
> `conf * 10_000 > price * MAX_CONF_BPS` itself, and additionally require `price > 0` and a
  `PriceStatus::Trading` aggregate (falling back to `prev_*` values otherwise, which the SDK does).
  Note `price` is **signed**; a negative price must be rejected, not wrapped.
- Only **one** feed is needed. PLAN.md §4.1 stores `quote_usd_feed`; the token's USD price is
  derived on-chain as `pool_price * quote_usd_price` (§6.5), so no second feed and no feed
  validation for the token itself.
- Pythnet vs Solana: `PythnetPriceAccount = GenericPriceAccount<128, PriceAccountExt>`. The program
  reads `SolanaPriceAccount`; a Pythnet account will fail the `load` size/magic check and revert,
  which is the desired behaviour on a Solana cluster.

## V8 — `Clock` fields and estimating a future epoch boundary

`Clock` (`solana-clock` `src/lib.rs:206`) exposes exactly:

```rust
pub struct Clock {
    pub slot: Slot,                  // absolute slot
    pub epoch_start_timestamp: UnixTimestamp,  // first slot of THIS epoch
    pub epoch: Epoch,
    pub leader_schedule_epoch: Epoch,
    pub unix_timestamp: UnixTimestamp,
}
```

All four fields PLAN.md wants are present, and `epoch_start_timestamp` is the key one: it anchors
the current epoch to wall-clock **on-chain**, so no off-chain bookkeeping is needed.

The `EpochSchedule` sysvar is also available on-chain
(`solana_program::sysvar::epoch_schedule::EpochSchedule`), giving `slots_per_epoch`,
`first_normal_epoch`, `first_normal_slot`. For any epoch at or past `first_normal_epoch`
(mainnet: `first_normal_epoch = first_normal_slot = 0`, so simply `epoch * 432000`) the start slot
is **exact**, not estimated:

```
epoch_start_slot(E) = first_normal_slot + (E - first_normal_epoch) * slots_per_epoch
```

So the only remaining error is converting slots to wall-clock:

```
epoch_start_ts(E) ≈ clock.epoch_start_timestamp
                    + (E - clock.epoch) * slots_per_epoch * EST_SLOT_MS / 1000
```

**Recorded error.** Two components:

1. *Modelling error* — `EST_SLOT_MS` is a constant. Observed 350 ms against a nominal 400 ms over
   V5's full epoch; even a perfect estimator is off by the slot-time drift accumulated over
   `2 * slots_per_epoch` slots (the +2 look-ahead is ≈ 84 h ahead at 350 ms). At ±10 % slot time
   that is ≈ **±8 h** on the *estimate*.
2. *Intrinsic error* — epoch boundaries are fixed by slot count, not by the clock, so no estimate
   can do better than half an epoch: **≈ ±21 h**.

Net worst case **±~1 day**; this is the "up to roughly one epoch" the plan promises, and the
observed value is recorded into `observed_effective_ts` by reading back `newer_transfer_fee.epoch`
after the change activates (§5.3), so the realised error is auditable rather than assumed.

> **PLAN AMENDMENT.** PLAN.md §5.3 lets `queue_scheduled_fee` pick the call epoch `E` so that
> epoch `E+2` lands nearest Monday. With the formula above that is
> `E = clock.epoch + round((target_ts - now_ts) / (2 * slots_per_epoch * EST_SLOT_MS))`, clamped to
> `E >= clock.epoch` (you cannot schedule the past). Two guards the plan omits and that are now
> implemented: `E` must be `>= clock.epoch` **and** the resulting `E+2` must be `> clock.epoch`, and
> the function must be **idempotent** — see V2, a re-call overwrites the pending entry, so
> re-publishing the same base fee for the same week must be a no-op rather than a fresh overwrite.

## V9 — Can a Token-2022 transfer hook charge or modify a transfer fee?

**No. A hook can only revert the transfer. Native `TransferFeeConfig` is the only fee mechanism.**

`program/src/processor.rs`, `process_transfer`:

1. `fee` is computed once, *before* any hook runs, straight from the mint's `TransferFeeConfig`
   (line 392–396): `transfer_fee_config.calculate_epoch_fee(Clock::get()?.epoch, amount)`.
2. Balances are mutated *before* the hook CPI (lines 540–556): source `-= amount`,
   destination `+= amount - calculated_fee`, and `calculated_fee` is credited to the destination's
   `TransferFeeAmount.withheld_amount`.
3. Only then `invoke_execute(..., amount)` is called, passing `amount` **by value** in the hook's
   instruction data, with source and destination flagged as already-transferring. The hook's
   **return data is discarded**; only `Result` propagates.
4. The hook holds no privileged handle on the mint, cannot sign for the program, and cannot write
   the `TransferFeeConfig` extension.

A hook cannot raise the fee, cannot redirect the withheld balance (it lands in the destination
account's `withheld_amount` and is only movable by the `withdraw_withheld_authority` — our PDA),
and cannot make a transfer cost more than `amount`. It *can* brick transfers entirely, which is why
V4's allow-list forbids `TransferHook` on our mint entirely. PLAN.md §5's reliance on native
`SetTransferFee` is sound.

---

## T2 notes: reading the CPMM sources and the plan's own reference table

Recorded while transcribing `math/`, because two items are judgement calls a later reader would
otherwise have to reverse-engineer.

### The plan's §6.2 reference table cannot be checked "within 1 bps"

T2's acceptance criterion says the §6.2 reference values must "reproduce within 1 bps", but the plan
states them to one decimal place of a percent — a granularity of 10 bps. The literal criterion is
therefore unsatisfiable against the printed numbers. Both forms are asserted in
`math::fee_deterrent::tests::reference_values_match_the_plan_table`:

| `R = P0/Pf` | plan | exact, this build |
|---|---|---|
| 1.00 | 21.4% | **21.39%** (2 139 bps) |
| 1.10 | 18.6% | **18.65%** (1 865 bps) |
| 1.25 | 14.9% | **14.87%** (1 487 bps) |
| 1.50 | 9.3% | **9.29%** (929 bps) |
| ≥ 2.00 | 0% | **0%** |

Each exact value agrees with the plan's rounded figure to within half of the plan's own 10-bps
rounding step. `R = 1.10` is the only entry where the exact value rounds *up* out of the printed
step (18.65% → 18.6%), which is the direction the plan's "round the required fee up" rule requires.

### The uncapped required fee exceeds the 50% cap for crashes worse than ~69%

Solving `fee_required = 1 - sqrt(u*)` for the price move at which it reaches the 50% program cap
gives `R ≈ 0.3125`, i.e. a **68.75%** price drop. Below that the equation asks for a fee the program
is not permitted to charge. `required_fee_bps_raw` therefore returns the *uncapped* value (and
`None` for `R >= 2`), while `required_fee_bps` / `fee_for_move` — the on-chain path — always return
at most `MAX_FEE_BPS`. Keeping both means "the cap bound" stays distinguishable from "the equation
is satisfied", which the tests rely on. This is the sharpest instance of the §6.2 limitation already
on record: past a 69% crash v1 deters nothing at all.

### CPMM elasticity (§6.4) is a constant, not a function of the reserves

`elasticity` is defined as "bps price move per 1% of quote reserve traded". For a constant-product
curve that quantity does not depend on the reserve ratio at all. With `dy = dx*y/(x+dx)` and token
price `x/y`:

```text
price' = x'/y' = (x+dx)/x * (x+dx)/x   =>   relative move = (1+f)^2 - 1 = 2f + f^2,  f = dx/x
```

At `f = 1%` that is `2 + 0.01 = 2.01%` = **201 bps for every CPMM pool**, and selling instead of
buying gives the same magnitude with the opposite sign. Consequences worth recording:

* `math::amm::ELASTICITY_BPS_PER_ONE_PERCENT` is a constant. There is no reserve input to get wrong
  and no way for a pool operator to make the program's elasticity look better by choosing a ratio.
* It must still be *derived and asserted* rather than assumed, so
  `elasticity_is_reserve_independent` simulates swaps at five reserve ratios (1:1, 100:1, 1:100,
  arbitrary, and a size large enough to avoid truncation) and checks the realised move against the
  closed form. The realised integer move is 200–201 bps: the pool floors `dy`, which can only lose,
  so the closed form is an upper bound.

### TWAP is a difference of cumulative values, not a sum over intervals

The CPMM observation ring stores cumulative price×time. The average over `[t0, t1]` is
`(cum(t1) - cum(t0)) / (t1 - t0)`, using the cumulative values bracketing the window. Because those
values are absolute rather than per-slot, a ring wrap needs no correction — the caller only has to
linearise the ring into timestamp order (T8). `math::volatility::twap` returns the average in the
*input's* price units (Q32.32) so it does not have to know which Q-format it was handed.

---

## Summary of plan amendments carried forward

| # | Amendment | Where it lands |
|---|---|---|
| A1 | Fee *decreases* are also delayed 2 epochs (V1). | `docs/DESIGN.md`, README; `revert_action` has no faster path |
| A2 | A queued `SetTransferFee` is silently overwritten by the next call (V2). | `FeeQueueStatus` in `state/schedule.rs`, idempotent `queue_scheduled_fee` |
| A3 | `minimum_amount_out` is compared **net** of the output transfer fee, so pass the desired **net** receipt with **no** gross-up (V4, re-confirmed against `instructions_swap_base_input.rs:147-153` in T2). | `math::amm::min_out_from_net` + `assert_min_out_satisfied` + unit tests |
| A4 | `SwapBaseInput` feeds the curve `amount_in` **net** of the input transfer fee (V4). | `math::amm` sizing, `venue/raydium_cpmm.rs` |
| A5 | CPMM reserves are net of accrued protocol/fund/creator fees (V4). | `venue/raydium_cpmm.rs` reserve read |
| A6 | Observation buffer is a ring of **cumulative Q32.32 × seconds** and only advances every ≥15 s (V4). | `math::volatility`, invariant I10 |
| A7 | `EST_SLOT_MS = 350`; error ±~1 day (V5/V8). | `constants.rs`, README |
| A8 | Pyth staleness is in **slots**; SDK's `VALID_SLOT_PERIOD = 25` is devnet-sized (V7). | `config.oracle_max_age_slots`, `oracle/pyth.rs` |
| A9 | Pyth confidence is **not** validated by the SDK and `price` is signed (V7). | `oracle/pyth.rs` |
| A10 | Mint must carry **only** `TransferFeeConfig` (+ optionally `MetadataPointer`) or CPMM refuses the pool (V4). | `initialize`, `docs/THREATS.md` |
| A11 | `pyth-sdk-solana` is uncompilable alongside `anchor-lang` 1.1.2 (borsh 0.10 vs 1.x). | `oracle/pyth.rs` reads the layout locally, transcribed with upstream line refs |

None of these contradict a *decision* in §1. None changed D1–D14. Nothing here requires a human
answer to Q1–Q6, which remain open in `QUESTIONS.md` with their stated defaults in force.
