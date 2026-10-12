//! Gatekeeper transfer hook.
//!
//! A Token-2022 transfer hook that only allows a mint's transfers to be
//! *mediated by a program* when that program is the approved AMM. Plain
//! wallet-to-wallet transfers (where Token-2022 is the transaction-level
//! instruction) pass through untouched.
//!
//! ## How the gate works
//!
//! Token-2022 always invokes the hook as a CPI, so by the time this code runs
//! the instruction stack looks like one of:
//!
//! ```text
//! P2P:   [Token-2022 transfer_checked] -> [this hook]              (depth 2)
//! AMM:   [AMM swap] -> [Token-2022 transfer_checked] -> [this hook] (depth 3+)
//! ```
//!
//! `get_stack_height()` distinguishes the two. The Instructions sysvar only
//! exposes top-level instructions, so a CPI ancestor (the AMM) is *not*
//! introspectable. Instead, a mediated transfer is authorized by *what it moves*:
//! one side of the transfer must be a token account owned by the approved AMM's
//! pool authority. That authority is a PDA the AMM alone can sign for, so it
//! cannot be forged, and a foreign venue (a different AMM, a bot contract) that
//! tries to move the mint is rejected with `UnauthorizedMarketAccess`.
//!
//! The mint's canonical `ExtraAccountMetaList` pins the approved AMM's
//! `PoolState` account. At transfer time the hook checks that the pool state is
//! owned by the registered `approved_amm` and reads its `pool_authority`, then
//! requires the transfer's source or destination to be controlled by it.

use anchor_lang::{
    prelude::*,
    solana_program::instruction::get_stack_height,
    system_program::{create_account, CreateAccount},
};
use anchor_spl::token_interface::{Mint, TokenAccount};
use anchor_spl::token_2022::spl_token_2022::{
    extension::{
        transfer_hook::{TransferHook as TransferHookExtension, TransferHookAccount},
        BaseStateWithExtensions, StateWithExtensions,
    },
    state::{Account as Token2022Account, Mint as Token2022Mint},
};
use spl_tlv_account_resolution::{account::ExtraAccountMeta, state::ExtraAccountMetaList};
use spl_transfer_hook_interface::instruction::{ExecuteInstruction, TransferHookInstruction};

pub mod errors;
pub mod state;

pub use errors::*;
pub use state::*;

declare_id!("9nwuM7iM2L4rqMFRXZosraLt8kYXoqKZaavPi8DvvgRU");

/// Transaction-level instructions execute at this stack height; each CPI adds one.
const TRANSACTION_LEVEL_STACK_HEIGHT: usize = 1;

/// Byte range of `PoolState::pool_authority` inside the approved AMM's serialized
/// pool account: an 8-byte Anchor discriminator, then `token_mint` (32) and
/// `quote_mint` (32). Kept in lock-step with `custom_amm::PoolState`.
const POOL_AUTHORITY_RANGE: core::ops::Range<usize> = 72..104;

#[program]
pub mod gatekeeper_hook {
    use super::*;

    /// Create the singleton registry. `admin` becomes the only account able to
    /// rotate the approved venue.
    pub fn initialize_registry(
        ctx: Context<InitializeRegistry>,
        approved_amm: Pubkey,
    ) -> Result<()> {
        require_keys_neq!(approved_amm, Pubkey::default(), GatekeeperError::ZeroAddress);
        let registry = &mut ctx.accounts.registry;
        registry.admin = ctx.accounts.admin.key();
        registry.approved_amm = approved_amm;
        registry.bump = ctx.bumps.registry;
        Ok(())
    }

    /// Rotate the approved AMM. Admin-only.
    pub fn set_approved_amm(ctx: Context<SetApprovedAmm>, approved_amm: Pubkey) -> Result<()> {
        require_keys_neq!(approved_amm, Pubkey::default(), GatekeeperError::ZeroAddress);
        ctx.accounts.registry.approved_amm = approved_amm;
        Ok(())
    }

    /// Create and initialize the canonical `ExtraAccountMetaList` for `mint`.
    ///
    /// The list declares the two accounts the hook needs at transfer time: the
    /// registry PDA (to read `approved_amm`) and the approved AMM's `PoolState`
    /// (to read `pool_authority`). Token-2022 resolves these out of the
    /// transfer's remaining accounts and appends them, in this order, after the
    /// four base accounts.
    ///
    /// `pool_state` must already exist and be owned by the registered AMM; the
    /// hook re-checks that ownership on every gated transfer.
    pub fn initialize_extra_account_meta_list(
        ctx: Context<InitializeExtraAccountMetaList>,
    ) -> Result<()> {
        // The mint must actually point its transfer hook at this program, or the
        // list would be dead weight that any mint could attach.
        {
            let mint_info = ctx.accounts.mint.to_account_info();
            let data = mint_info.try_borrow_data()?;
            let mint = StateWithExtensions::<Token2022Mint>::unpack(&data)?;
            let hook = mint.get_extension::<TransferHookExtension>()?;
            let program_id = Option::<Pubkey>::from(hook.program_id);
            require!(
                program_id == Some(crate::ID),
                GatekeeperError::MintHookMismatch
            );
        }

        // Pin only an account the approved AMM owns, so a rotate of the venue
        // automatically invalidates any stale pin.
        require_keys_eq!(
            ctx.accounts.pool_state.to_account_info().owner.key(),
            ctx.accounts.registry.approved_amm,
            GatekeeperError::UnauthorizedMarketAccess
        );

        let account_metas = vec![
            // index 5: the registry PDA.
            ExtraAccountMeta::new_with_pubkey(&ctx.accounts.registry.key(), false, false)?,
            // index 6: the approved AMM's pool state.
            ExtraAccountMeta::new_with_pubkey(&ctx.accounts.pool_state.key(), false, false)?,
        ];

        let account_size = ExtraAccountMetaList::size_of(account_metas.len())? as u64;
        let lamports = Rent::get()?.minimum_balance(account_size as usize);

        let mint_key = ctx.accounts.mint.key();
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"extra-account-metas",
            mint_key.as_ref(),
            &[ctx.bumps.extra_account_meta_list],
        ]];

        create_account(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                CreateAccount {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.extra_account_meta_list.to_account_info(),
                },
            )
            .with_signer(signer_seeds),
            lamports,
            account_size,
            ctx.program_id,
        )?;

        ExtraAccountMetaList::init::<ExecuteInstruction>(
            &mut ctx.accounts.extra_account_meta_list.try_borrow_mut_data()?,
            &account_metas,
        )?;

        Ok(())
    }

    /// The hook body. Called by Token-2022 through the `fallback` below.
    pub fn transfer_hook(ctx: Context<TransferHook>, _amount: u64) -> Result<()> {
        // Fail closed if this is not a real, in-progress Token-2022 transfer.
        assert_transferring(&ctx.accounts.source.to_account_info())?;
        assert_transferring(&ctx.accounts.destination.to_account_info())?;

        let depth = get_stack_height();
        // depth == 2 means Token-2022 is the transaction-level instruction: a
        // plain P2P transfer. Anything deeper is program-mediated and must be a
        // legitimate AMM operation.
        if depth > TRANSACTION_LEVEL_STACK_HEIGHT + 1 {
            let pool_info = ctx.accounts.pool_state.to_account_info();
            // The pinned pool state must belong to the approved AMM. A foreign
            // program cannot own an account of the AMM, so it cannot pass this.
            require_keys_eq!(
                pool_info.owner.key(),
                ctx.accounts.registry.approved_amm,
                GatekeeperError::UnauthorizedMarketAccess
            );

            let authority = pool_authority(&pool_info)?;
            // One side of the transfer must be controlled by the pool authority
            // (the AMM's vault). The authority is a PDA only the AMM can sign
            // for, so it cannot be spoofed by a foreign venue.
            require!(
                ctx.accounts.source.owner == authority
                    || ctx.accounts.destination.owner == authority,
                GatekeeperError::UnauthorizedMarketAccess
            );
        }

        Ok(())
    }

    /// Fallback dispatch for the transfer-hook interface's `Execute` instruction.
    ///
    /// Anchor discriminators differ from the SPL transfer-hook interface's, so
    /// Token-2022's CPI would not match any handler. We validate the interface
    /// discriminator and forward the trailing 8 bytes (the borsh-encoded `u64`
    /// amount) to the wrapped `transfer_hook` handler.
    pub fn fallback<'info>(
        program_id: &'info Pubkey,
        accounts: &'info [AccountInfo<'info>],
        data: &'info [u8],
    ) -> Result<()> {
        match TransferHookInstruction::unpack(data)? {
            TransferHookInstruction::Execute { .. } => {
                // Discriminator is 8 bytes; the remaining 8 are the amount. The
                // slice borrows from `data`, which lives for `'info`.
                let amount_data = data
                    .get(8..16)
                    .ok_or(ProgramError::InvalidInstructionData)?;
                __private::__global::transfer_hook(program_id, accounts, amount_data)
            }
            _ => Err(ProgramError::InvalidInstructionData.into()),
        }
    }
}

/// Reject if the token account's `TransferHookAccount.transferring` flag is not
/// set, which proves the account is mid-transfer and not merely being passed in.
fn assert_transferring(account: &AccountInfo) -> Result<()> {
    let data = account.try_borrow_data()?;
    let token = StateWithExtensions::<Token2022Account>::unpack(&data)?;
    let extension = token
        .get_extension::<TransferHookAccount>()
        .map_err(|_| error!(GatekeeperError::NotTransferring))?;
    require!(
        bool::from(extension.transferring),
        GatekeeperError::NotTransferring
    );
    Ok(())
}

/// Read the `pool_authority` field out of a serialized `custom_amm::PoolState`.
fn pool_authority(pool_state: &AccountInfo) -> Result<Pubkey> {
    let data = pool_state.try_borrow_data()?;
    let bytes = data
        .get(POOL_AUTHORITY_RANGE)
        .ok_or_else(|| error!(GatekeeperError::UnauthorizedMarketAccess))?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| error!(GatekeeperError::UnauthorizedMarketAccess))?;
    Ok(Pubkey::new_from_array(array))
}

#[derive(Accounts)]
pub struct InitializeRegistry<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = Registry::LEN, seeds = [b"registry"], bump)]
    pub registry: Account<'info, Registry>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetApprovedAmm<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [b"registry"], bump = registry.bump, has_one = admin)]
    pub registry: Account<'info, Registry>,
}

#[derive(Accounts)]
pub struct InitializeExtraAccountMetaList<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub admin: Signer<'info>,
    #[account(seeds = [b"registry"], bump = registry.bump, has_one = admin)]
    pub registry: Account<'info, Registry>,
    pub mint: InterfaceAccount<'info, Mint>,
    /// CHECK: PDA, checked against the canonical seeds below.
    #[account(
        mut,
        seeds = [b"extra-account-metas", mint.key().as_ref()],
        bump
    )]
    pub extra_account_meta_list: UncheckedAccount<'info>,
    /// CHECK: the approved AMM's pool state; only its owner is read here.
    pub pool_state: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// Order is load-bearing: Token-2022 passes the four base transfer accounts
/// first, then the `ExtraAccountMetaList` PDA, then the extras in the order the
/// list declares them (registry, approved AMM pool state).
#[derive(Accounts)]
pub struct TransferHook<'info> {
    #[account(token::mint = mint, token::authority = owner)]
    pub source: InterfaceAccount<'info, TokenAccount>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(token::mint = mint)]
    pub destination: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: source owner or delegate; may be a system account or a PDA.
    pub owner: UncheckedAccount<'info>,
    /// CHECK: ExtraAccountMetaList PDA.
    #[account(seeds = [b"extra-account-metas", mint.key().as_ref()], bump)]
    pub extra_account_meta_list: UncheckedAccount<'info>,
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Account<'info, Registry>,
    /// CHECK: the approved AMM's pool state; its owner is checked in the handler.
    pub pool_state: UncheckedAccount<'info>,
}
