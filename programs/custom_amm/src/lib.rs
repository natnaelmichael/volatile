//! Custom constant-product AMM.
//!
//! Swaps are priced and validated entirely on-chain: `swap` recomputes the
//! constant-product output, enforces the caller's `minimum_amount_out`, and
//! checks the execution price against the pool's rolling TWAP band. The caller
//! supplies no volatility parameters.
//!
//! The `token_mint` side is assumed to carry the gatekeeper transfer hook, so
//! every transfer of that mint (both the user->vault and vault->user legs) is
//! built with the hook's `ExtraAccountMetaList` PDAs attached. The hook accepts
//! the transfer because one side is a vault owned by this pool's authority,
//! which is registered in the mint's canonical hook account list.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::{invoke, invoke_signed};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use anchor_spl::token_2022::spl_token_2022;
use spl_transfer_hook_interface::{
    get_extra_account_metas_address, onchain::add_extra_accounts_for_execute_cpi,
};

pub mod errors;
pub mod math;
pub mod state;

pub use errors::*;
pub use math::*;
pub use state::*;

declare_id!("3UZvnosY9kcVrPWXdk4pbxJpezkgpNXkG6L7VugS7Vf7");

#[program]
pub mod custom_amm {
    use super::*;

    /// Create a pool and its two vaults. `rolling_twap` is left at zero and is
    /// seeded by the first swap (the first trade is exempt from the band check).
    pub fn initialize(
        ctx: Context<Initialize>,
        fee_bps: u16,
        lower_bound_bps: u32,
        upper_bound_bps: u32,
    ) -> Result<()> {
        require!(fee_bps <= MAX_FEE_BPS, AmmError::FeeExceedsCap);
        require!(
            lower_bound_bps > 0 && upper_bound_bps > lower_bound_bps,
            AmmError::BoundsInverted
        );
        require!(
            u128::from(upper_bound_bps) <= BPS_DENOMINATOR,
            AmmError::ValueOutOfRange
        );

        let pool = &mut ctx.accounts.pool_state;
        pool.token_mint = ctx.accounts.token_mint.key();
        pool.quote_mint = ctx.accounts.quote_mint.key();
        pool.pool_authority = ctx.accounts.pool_authority.key();
        pool.token_vault = ctx.accounts.token_vault.key();
        pool.quote_vault = ctx.accounts.quote_vault.key();
        pool.rolling_twap = 0;
        pool.last_twap_update = 0;
        pool.lower_bound_bps = lower_bound_bps;
        pool.upper_bound_bps = upper_bound_bps;
        pool.fee_bps = fee_bps;
        pool.bump = ctx.bumps.pool_state;
        Ok(())
    }

    /// Swap `amount_in` of one side for at least `minimum_amount_out` of the
    /// other.
    pub fn swap(ctx: Context<Swap>, amount_in: u64, minimum_amount_out: u64) -> Result<()> {
        require!(amount_in > 0, AmmError::ValueOutOfRange);

        // Snapshot the pool config (all Copy) so no borrow is held across CPIs.
        let token_mint_key = ctx.accounts.pool_state.token_mint;
        let quote_mint_key = ctx.accounts.pool_state.quote_mint;
        let pool_authority_key = ctx.accounts.pool_state.pool_authority;
        let token_vault_key = ctx.accounts.pool_state.token_vault;
        let quote_vault_key = ctx.accounts.pool_state.quote_vault;
        let fee_bps = ctx.accounts.pool_state.fee_bps;
        let lower_bps = ctx.accounts.pool_state.lower_bound_bps;
        let upper_bps = ctx.accounts.pool_state.upper_bound_bps;
        let rolling_before = ctx.accounts.pool_state.rolling_twap;
        let last_update = ctx.accounts.pool_state.last_twap_update;

        // -- structural checks --------------------------------------------
        require_keys_eq!(
            ctx.accounts.token_mint.key(),
            token_mint_key,
            AmmError::MintMismatch
        );
        require_keys_eq!(
            ctx.accounts.quote_mint.key(),
            quote_mint_key,
            AmmError::MintMismatch
        );
        require_keys_eq!(
            ctx.accounts.token_vault.key(),
            token_vault_key,
            AmmError::InvalidVault
        );
        require_keys_eq!(
            ctx.accounts.quote_vault.key(),
            quote_vault_key,
            AmmError::InvalidVault
        );
        require_keys_eq!(
            ctx.accounts.token_vault.mint,
            token_mint_key,
            AmmError::MintMismatch
        );
        require_keys_eq!(
            ctx.accounts.quote_vault.mint,
            quote_mint_key,
            AmmError::MintMismatch
        );
        require_keys_eq!(
            ctx.accounts.token_vault.owner,
            pool_authority_key,
            AmmError::VaultAuthorityMismatch
        );
        require_keys_eq!(
            ctx.accounts.quote_vault.owner,
            pool_authority_key,
            AmmError::VaultAuthorityMismatch
        );
        require_keys_eq!(
            ctx.accounts.pool_authority.key(),
            pool_authority_key,
            AmmError::InvalidVault
        );

        // The hook program, its registry, and the meta-list PDA must line up.
        let expected_meta_list =
            get_extra_account_metas_address(&token_mint_key, ctx.accounts.hook_program.key);
        require_keys_eq!(
            ctx.accounts.extra_account_meta_list.key(),
            expected_meta_list,
            AmmError::HookAccountsMismatch
        );
        let (expected_registry, _) =
            Pubkey::find_program_address(&[b"registry"], ctx.accounts.hook_program.key);
        require_keys_eq!(
            ctx.accounts.hook_registry.key(),
            expected_registry,
            AmmError::HookAccountsMismatch
        );

        // -- direction & reserves -----------------------------------------
        let user_src_mint = ctx.accounts.user_source_token.mint;
        let user_dst_mint = ctx.accounts.user_destination_token.mint;
        require!(user_src_mint != user_dst_mint, AmmError::MintsEqual);
        let token_to_quote = user_src_mint == token_mint_key;
        require!(
            (token_to_quote && user_dst_mint == quote_mint_key)
                || (!token_to_quote
                    && user_src_mint == quote_mint_key
                    && user_dst_mint == token_mint_key),
            AmmError::MintMismatch
        );

        let (input_vault, output_vault, in_mint, out_mint) = if token_to_quote {
            (
                &ctx.accounts.token_vault,
                &ctx.accounts.quote_vault,
                &ctx.accounts.token_mint,
                &ctx.accounts.quote_mint,
            )
        } else {
            (
                &ctx.accounts.quote_vault,
                &ctx.accounts.token_vault,
                &ctx.accounts.quote_mint,
                &ctx.accounts.token_mint,
            )
        };

        let reserve_in = u128::from(input_vault.amount);
        let reserve_out = u128::from(output_vault.amount);
        let amount_in_u128 = u128::from(amount_in);

        let out = math::amount_out(reserve_in, reserve_out, amount_in_u128, fee_bps)?;
        require!(
            out >= u128::from(minimum_amount_out),
            AmmError::SlippageExceeded
        );
        let amount_out_u64 = u64::try_from(out).map_err(|_| AmmError::MathOverflow)?;

        // Execution price as quote-per-token, scaled.
        let exec_price = if token_to_quote {
            math::price(out, amount_in_u128)?
        } else {
            math::price(amount_in_u128, out)?
        };

        // -- volatility band & TWAP ---------------------------------------
        let now = Clock::get()?.unix_timestamp;
        let rolling_after = if rolling_before == 0 || last_update == 0 {
            exec_price
        } else {
            let elapsed = u128::try_from(now.saturating_sub(last_update).max(0))
                .map_err(|_| AmmError::MathOverflow)?;
            let ratio = math::ratio_bps(exec_price, rolling_before)?;
            require!(
                ratio >= u128::from(lower_bps) && ratio <= u128::from(upper_bps),
                AmmError::VolatilityOutOfBounds
            );
            math::blend_twap(rolling_before, exec_price, elapsed)?
        };
        {
            let pool = &mut ctx.accounts.pool_state;
            pool.rolling_twap = rolling_after;
            pool.last_twap_update = now;
        }

        // -- transfers -----------------------------------------------------
        let token_program_info = ctx.accounts.token_program.to_account_info();
        let hook_program_info = ctx.accounts.hook_program.to_account_info();
        let hook_registry_info = ctx.accounts.hook_registry.to_account_info();
        let meta_list_info = ctx.accounts.extra_account_meta_list.to_account_info();
        let hook_pool_state_info = ctx.accounts.pool_state.to_account_info();

        // Leg 1: user -> input vault, authority = user.
        let hook_on_input = if token_to_quote {
            Some((
                &hook_program_info,
                &hook_registry_info,
                &meta_list_info,
                &hook_pool_state_info,
            ))
        } else {
            None
        };
        transfer_checked_maybe_hooked(
            &token_program_info,
            &ctx.accounts.user_source_token.to_account_info(),
            &in_mint.to_account_info(),
            &input_vault.to_account_info(),
            &ctx.accounts.user.to_account_info(),
            amount_in,
            in_mint.decimals,
            hook_on_input,
            None,
        )?;

        // Leg 2: output vault -> user, authority = pool authority PDA.
        let authority_bump = ctx.bumps.pool_authority;
        let authority_seeds: &[&[u8]] = &[
            b"pool_authority",
            token_mint_key.as_ref(),
            quote_mint_key.as_ref(),
            &[authority_bump],
        ];
        let signer_seeds: &[&[&[u8]]] = &[authority_seeds];

        let hook_on_output = if token_to_quote {
            None
        } else {
            Some((
                &hook_program_info,
                &hook_registry_info,
                &meta_list_info,
                &hook_pool_state_info,
            ))
        };
        transfer_checked_maybe_hooked(
            &token_program_info,
            &output_vault.to_account_info(),
            &out_mint.to_account_info(),
            &ctx.accounts.user_destination_token.to_account_info(),
            &ctx.accounts.pool_authority.to_account_info(),
            amount_out_u64,
            out_mint.decimals,
            hook_on_output,
            Some(signer_seeds),
        )?;

        emit!(SwapEvent {
            user: ctx.accounts.user.key(),
            token_to_quote,
            amount_in,
            amount_out: amount_out_u64,
            price: exec_price,
            twap: rolling_after,
        });

        Ok(())
    }
}

/// The four extra accounts a hooked transfer needs: the hook program itself,
/// its registry, the mint's `ExtraAccountMetaList`, and this pool's state (the
/// hook reads `pool_authority` from it).
type HookAccounts<'a, 'info> = (
    &'a AccountInfo<'info>,
    &'a AccountInfo<'info>,
    &'a AccountInfo<'info>,
    &'a AccountInfo<'info>,
);

/// Perform a `TransferChecked`, appending the hook's resolved extra accounts when
/// the transferred mint is hooked.
#[allow(clippy::too_many_arguments)]
fn transfer_checked_maybe_hooked<'info>(
    token_program: &AccountInfo<'info>,
    source: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    destination: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    decimals: u8,
    hook: Option<HookAccounts<'_, 'info>>,
    signer_seeds: Option<&[&[&[u8]]]>,
) -> Result<()> {
    let mut ix = spl_token_2022::instruction::transfer_checked(
        token_program.key,
        source.key,
        mint.key,
        destination.key,
        authority.key,
        &[],
        amount,
        decimals,
    )?;

    let mut infos = vec![
        source.clone(),
        mint.clone(),
        destination.clone(),
        authority.clone(),
    ];

    if let Some((program, registry, meta_list, pool_state)) = hook {
        let additional = vec![
            program.clone(),
            meta_list.clone(),
            registry.clone(),
            pool_state.clone(),
        ];
        add_extra_accounts_for_execute_cpi(
            &mut ix,
            &mut infos,
            program.key,
            source.clone(),
            mint.clone(),
            destination.clone(),
            authority.clone(),
            amount,
            &additional,
        )?;
    }

    match signer_seeds {
        Some(seeds) => invoke_signed(&ix, &infos, seeds),
        None => invoke(&ix, &infos),
    }
    .map_err(Into::into)
}

#[event]
pub struct SwapEvent {
    pub user: Pubkey,
    pub token_to_quote: bool,
    pub amount_in: u64,
    pub amount_out: u64,
    pub price: u128,
    pub twap: u128,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    pub token_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(
        init,
        payer = admin,
        space = PoolState::LEN,
        seeds = [b"pool", token_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump
    )]
    pub pool_state: Account<'info, PoolState>,
    /// CHECK: pool authority PDA.
    #[account(
        seeds = [b"pool_authority", token_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = admin,
        token::mint = token_mint,
        token::authority = pool_authority
    )]
    pub token_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init,
        payer = admin,
        token::mint = quote_mint,
        token::authority = pool_authority
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        seeds = [b"pool", pool_state.token_mint.as_ref(), pool_state.quote_mint.as_ref()],
        bump = pool_state.bump
    )]
    pub pool_state: Account<'info, PoolState>,
    pub token_mint: Box<InterfaceAccount<'info, Mint>>,
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(mut)]
    pub token_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub quote_vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub user_source_token: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut)]
    pub user_destination_token: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: pool authority PDA.
    #[account(
        seeds = [b"pool_authority", pool_state.token_mint.as_ref(), pool_state.quote_mint.as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: gatekeeper transfer-hook program; validated against the meta list.
    pub hook_program: UncheckedAccount<'info>,
    /// CHECK: gatekeeper registry PDA; validated by derivation.
    pub hook_registry: UncheckedAccount<'info>,
    /// CHECK: `ExtraAccountMetaList` PDA; validated by derivation.
    pub extra_account_meta_list: UncheckedAccount<'info>,
}
