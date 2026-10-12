//! Off-chain `jupiter-amm-interface` integration for the `custom_amm` program.
//!
//! The on-chain swap recomputes the constant-product output and enforces the
//! volatility band itself, so this SDK must reproduce exactly the integer math in
//! `programs/custom_amm/src/math.rs` for `quote()` to agree with the executed
//! trade. The [`crate::math`] module is a line-for-line copy of the on-chain math
//! with the Anchor `require!`/`Result` plumbing swapped for `AmmError`.
//!
//! `custom_amm` swaps carry a gatekeeper transfer hook on the `token_mint` side,
//! so `get_swap_and_account_metas` appends the hook's `ExtraAccountMetaList` PDA
//! plus the registry and pool-state accounts.

use jupiter_amm_interface::{
    AccountProvider, Amm, AmmContext, AmmError, AmmLabel, KeyedAccount, Quote, QuoteParams, Swap,
    SwapAndAccountMetas, SwapMode, SwapParams,
};
use solana_account::ReadableAccount;
use solana_instruction::AccountMeta;
use solana_pubkey::{pubkey, Pubkey};

mod math;

pub use math::{amount_out, PRICE_SCALE};

/// The `custom_amm` program that owns the pool.
pub const CUSTOM_AMM_ID: Pubkey = pubkey!("3UZvnosY9kcVrPWXdk4pbxJpezkgpNXkG6L7VugS7Vf7");
/// The gatekeeper transfer-hook program attached to the pool's `token_mint`.
pub const GATEKEEPER_HOOK_ID: Pubkey = pubkey!("9nwuM7iM2L4rqMFRXZosraLt8kYXoqKZaavPi8DvvgRU");
/// Token-2022, which hosts the hooked mint and both vaults.
pub const TOKEN_2022_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

/// Byte range of each `PoolState` field, measured from the start of the account
/// (8-byte Anchor discriminator first). Mirrors `custom_amm::PoolState`.
const OFF_TOKEN_MINT: usize = 8;
const OFF_QUOTE_MINT: usize = 40;
const OFF_POOL_AUTHORITY: usize = 72;
const OFF_TOKEN_VAULT: usize = 104;
const OFF_QUOTE_VAULT: usize = 136;
const OFF_FEE_BPS: usize = 200;
/// 8 + 5*32 + 16 + 8 + 4 + 4 + 2 + 1.
const POOL_STATE_LEN: usize = 203;
/// `amount` lives at this offset in a Token-2022 account (after mint + owner).
const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;

/// A constant-product pool view, hydrated from its on-chain `PoolState` and vaults.
#[derive(Clone)]
pub struct CustomAmm {
    key: Pubkey,
    token_mint: Pubkey,
    quote_mint: Pubkey,
    pool_authority: Pubkey,
    token_vault: Pubkey,
    quote_vault: Pubkey,
    fee_bps: u16,
    token_reserve: u128,
    quote_reserve: u128,
}

impl Amm for CustomAmm {
    fn from_keyed_account(
        keyed_account: &KeyedAccount,
        _amm_context: &AmmContext,
    ) -> Result<Self, AmmError> {
        let data = keyed_account.account.data.as_slice();
        if data.len() < POOL_STATE_LEN {
            return Err(AmmError::Custom(format!(
                "custom_amm pool state too short: {}",
                data.len()
            )));
        }
        Ok(Self {
            key: keyed_account.key,
            token_mint: read_pubkey(data, OFF_TOKEN_MINT)?,
            quote_mint: read_pubkey(data, OFF_QUOTE_MINT)?,
            pool_authority: read_pubkey(data, OFF_POOL_AUTHORITY)?,
            token_vault: read_pubkey(data, OFF_TOKEN_VAULT)?,
            quote_vault: read_pubkey(data, OFF_QUOTE_VAULT)?,
            fee_bps: read_u16(data, OFF_FEE_BPS)?,
            token_reserve: 0,
            quote_reserve: 0,
        })
    }

    fn label(&self) -> AmmLabel {
        "Custom AMM"
    }

    fn program_id(&self) -> Pubkey {
        CUSTOM_AMM_ID
    }

    fn key(&self) -> Pubkey {
        self.key
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        vec![self.token_mint, self.quote_mint]
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        vec![self.token_vault, self.quote_vault]
    }

    fn update(&mut self, account_provider: impl AccountProvider) -> Result<(), AmmError> {
        let token = account_provider
            .get(&self.token_vault)
            .ok_or_else(|| AmmError::Custom("token vault missing".to_string()))?;
        let quote = account_provider
            .get(&self.quote_vault)
            .ok_or_else(|| AmmError::Custom("quote vault missing".to_string()))?;
        self.token_reserve = read_amount(token.data())?;
        self.quote_reserve = read_amount(quote.data())?;
        Ok(())
    }

    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote, AmmError> {
        if quote_params.swap_mode != SwapMode::ExactIn {
            return Err(AmmError::Custom("only ExactIn is supported".to_string()));
        }
        let (reserve_in, reserve_out) = self.reserves_for(
            quote_params.input_mint,
            quote_params.output_mint,
        )?;
        let out = math::amount_out(
            reserve_in,
            reserve_out,
            u128::from(quote_params.amount),
            self.fee_bps,
        )?;
        let out_amount = u64::try_from(out)
            .map_err(|_| AmmError::Custom("quote exceeds u64".to_string()))?;

        let fee_amount = math::input_fee(u128::from(quote_params.amount), self.fee_bps)?;
        let fee_amount = u64::try_from(fee_amount)
            .map_err(|_| AmmError::Custom("fee exceeds u64".to_string()))?;

        Ok(Quote {
            in_amount: quote_params.amount,
            out_amount,
            fee_amount,
            fee_mint: quote_params.input_mint,
            ..Default::default()
        })
    }

    fn get_swap_and_account_metas(
        &self,
        swap_params: &SwapParams,
    ) -> Result<SwapAndAccountMetas, AmmError> {
        // The hook's accounts are resolved from the canonical PDAs; the on-chain
        // program re-derives and validates them.
        let (hook_registry, _) =
            Pubkey::find_program_address(&[b"registry"], &GATEKEEPER_HOOK_ID);
        let (extra_account_meta_list, _) = Pubkey::find_program_address(
            &[b"extra-account-metas", self.token_mint.as_ref()],
            &GATEKEEPER_HOOK_ID,
        );

        // Order is load-bearing: it must match `custom_amm::Swap`.
        let account_metas = vec![
            AccountMeta::new(swap_params.user, true),
            AccountMeta::new(self.key, false),
            AccountMeta::new_readonly(self.token_mint, false),
            AccountMeta::new_readonly(self.quote_mint, false),
            AccountMeta::new(self.token_vault, false),
            AccountMeta::new(self.quote_vault, false),
            AccountMeta::new(swap_params.source_token_account, false),
            AccountMeta::new(swap_params.destination_token_account, false),
            AccountMeta::new_readonly(self.pool_authority, false),
            AccountMeta::new_readonly(TOKEN_2022_ID, false),
            AccountMeta::new_readonly(GATEKEEPER_HOOK_ID, false),
            AccountMeta::new_readonly(hook_registry, false),
            AccountMeta::new_readonly(extra_account_meta_list, false),
        ];

        Ok(SwapAndAccountMetas {
            swap: Swap::Placeholder { data: None },
            account_metas,
        })
    }

    fn program_dependencies(&self) -> Vec<(Pubkey, String)> {
        vec![(GATEKEEPER_HOOK_ID, "gatekeeper_hook".to_string())]
    }

    fn get_accounts_len(&self) -> usize {
        13
    }
}

impl CustomAmm {
    /// Reserves for the given direction, or an error if the mints are not this pool.
    fn reserves_for(&self, input_mint: Pubkey, output_mint: Pubkey) -> Result<(u128, u128), AmmError> {
        if input_mint == self.token_mint && output_mint == self.quote_mint {
            Ok((self.token_reserve, self.quote_reserve))
        } else if input_mint == self.quote_mint && output_mint == self.token_mint {
            Ok((self.quote_reserve, self.token_reserve))
        } else {
            Err(AmmError::Custom("mint pair does not belong to this pool".to_string()))
        }
    }
}

fn read_pubkey(data: &[u8], offset: usize) -> Result<Pubkey, AmmError> {
    let bytes: [u8; 32] = data
        .get(offset..offset + 32)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| AmmError::Custom("pool state field out of range".to_string()))?;
    Ok(Pubkey::new_from_array(bytes))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, AmmError> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| AmmError::Custom("pool state u16 out of range".to_string()))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_amount(data: &[u8]) -> Result<u128, AmmError> {
    let bytes: [u8; 8] = data
        .get(TOKEN_ACCOUNT_AMOUNT_OFFSET..TOKEN_ACCOUNT_AMOUNT_OFFSET + 8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| AmmError::Custom("token account amount out of range".to_string()))?;
    Ok(u128::from(u64::from_le_bytes(bytes)))
}
