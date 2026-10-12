//! Quote parity: the `CustomAmm` SDK quote must match what the on-chain
//! `custom_amm` program actually returns when it executes the swap.

mod common;

use common::{ensure_fixtures, quote_mint, token_mint, DISC_SWAP};
use custom_amm_sdk::CustomAmm;
use jupiter_amm_interface::Swap;
use jupiter_amm_test_kit::{assert_pool_parity, PoolTest};

#[test]
fn custom_amm_quote_parity() {
    let pool = ensure_fixtures();
    let test = PoolTest::new(pool)
        .add_swap(token_mint(), quote_mint(), 1_000_000)
        .add_swap(quote_mint(), token_mint(), 2_500_000);

    assert_pool_parity::<CustomAmm>(&test, encode_swap);
}

/// Native `custom_amm::swap(amount_in, minimum_amount_out)` instruction data.
fn encode_swap(_swap: &Swap, in_amount: u64) -> Vec<u8> {
    let mut data = DISC_SWAP.to_vec();
    data.extend_from_slice(&in_amount.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes()); // minimum_amount_out
    data
}
