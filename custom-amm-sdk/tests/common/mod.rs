//! Local fixture generator for the `custom_amm` parity test.
//!
//! Instead of snapshotting a live mainnet pool, this builds the pool in a
//! LiteSVM from scratch: it initializes a Token-2022 mint carrying the
//! gatekeeper transfer hook, a plain Token-2022 quote mint, the pool and its
//! vaults (via `custom_amm::initialize`), the hook registry and the mint's
//! `ExtraAccountMetaList`, then mints liquidity. The resulting accounts are
//! saved with [`PoolSnapshot::save_dir`] in the layout `jupiter-amm-test-kit`
//! expects.
//!
//! Run with `REGEN=1 cargo test -p custom-amm-sdk` to rebuild the snapshot.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use custom_amm_sdk::{CUSTOM_AMM_ID, GATEKEEPER_HOOK_ID, TOKEN_2022_ID};
use jupiter_amm_test_kit::PoolSnapshot;
use litesvm::LiteSVM;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;

pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");
pub const CLOCK_SYSVAR_ID: Pubkey = pubkey!("SysvarC1ock11111111111111111111111111111111");

/// Lamports seeded into each raw account; far above rent-exemption.
const FUND: u64 = 1_000_000_000;
const DECIMALS: u8 = 6;
const TOKEN_RESERVE: u64 = 1_000_000_000_000;
const QUOTE_RESERVE: u64 = 1_000_000_000_000;
const FEE_BPS: u16 = 30;
const LOWER_BPS: u32 = 8_000;
const UPPER_BPS: u32 = 10_000;

// Anchor discriminator = sha256("<namespace>:<name>")[..8].
const DISC_INITIALIZE: [u8; 8] = [0xaf, 0xaf, 0x6d, 0x1f, 0x0d, 0x98, 0x9b, 0xed];
const DISC_INITIALIZE_REGISTRY: [u8; 8] = [0xbd, 0xb5, 0x14, 0x11, 0xae, 0x39, 0xf9, 0x3b];
const DISC_INITIALIZE_META_LIST: [u8; 8] = [0x5c, 0xc5, 0xae, 0xc5, 0x29, 0x7c, 0x13, 0x03];
pub const DISC_SWAP: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];

/// Deterministic mint keys (they never need to sign).
pub fn token_mint() -> Pubkey {
    Pubkey::new_from_array([1u8; 32])
}

pub fn quote_mint() -> Pubkey {
    Pubkey::new_from_array([2u8; 32])
}

pub fn pool_pda(token_mint: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"pool", token_mint.as_ref(), quote_mint.as_ref()],
        &CUSTOM_AMM_ID,
    )
    .0
}

pub fn pool_authority_pda(token_mint: &Pubkey, quote_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"pool_authority", token_mint.as_ref(), quote_mint.as_ref()],
        &CUSTOM_AMM_ID,
    )
    .0
}

pub fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &GATEKEEPER_HOOK_ID).0
}

pub fn extra_account_meta_list_pda(token_mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"extra-account-metas", token_mint.as_ref()],
        &GATEKEEPER_HOOK_ID,
    )
    .0
}

/// The pool key for the fixed mints, generating fixtures if missing.
pub fn ensure_fixtures() -> Pubkey {
    let token_mint = token_mint();
    let quote_mint = quote_mint();
    let pool = pool_pda(&token_mint, &quote_mint);
    let dir = fixture_dir(&pool);
    if std::env::var_os("REGEN").is_some() || !dir.exists() {
        generate(&dir, token_mint, quote_mint);
    }
    pool
}

fn fixture_dir(pool: &Pubkey) -> PathBuf {
    PathBuf::from("tests/fixtures/accounts").join(pool.to_string())
}

fn read_program(name: &str) -> Vec<u8> {
    let path = format!("tests/fixtures/{name}.so");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("read {path}: {e} (copy target/deploy/{name}.so into tests/fixtures/)")
    })
}

fn send(svm: &mut LiteSVM, payer: &Keypair, signers: &[&Keypair], ixs: &[Instruction]) {
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    svm.send_transaction(tx).expect("transaction should succeed");
}

fn set_raw_account(svm: &mut LiteSVM, key: Pubkey, len: usize) {
    svm.set_account(
        key,
        Account {
            lamports: FUND,
            data: vec![0u8; len],
            owner: TOKEN_2022_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .expect("set raw account");
}

fn generate(dir: &Path, token_mint: Pubkey, quote_mint: Pubkey) {
    let mut svm = LiteSVM::new();
    svm.add_program(CUSTOM_AMM_ID, &read_program("custom_amm"))
        .expect("add custom_amm");
    svm.add_program(GATEKEEPER_HOOK_ID, &read_program("gatekeeper_hook"))
        .expect("add gatekeeper_hook");

    let admin = Keypair::new();
    svm.airdrop(&admin.pubkey(), 100 * FUND).expect("airdrop admin");

    // --- mints -----------------------------------------------------------------
    // Mint with TransferHook extension: base 82 + padding 83 + account-type 1 +
    // TLV(4 + 64) = 234 bytes.
    set_raw_account(&mut svm, token_mint, 234);
    // Plain Token-2022 mint: base only.
    set_raw_account(&mut svm, quote_mint, 82);

    // The transfer-hook extension must be initialized before the mint.
    send(
        &mut svm,
        &admin,
        &[&admin],
        &[Instruction {
            program_id: TOKEN_2022_ID,
            accounts: vec![AccountMeta::new(token_mint, false)],
            data: transfer_hook_initialize(&admin.pubkey(), &GATEKEEPER_HOOK_ID),
        }],
    );
    send(
        &mut svm,
        &admin,
        &[&admin],
        &[
            initialize_mint2(&token_mint, &admin.pubkey()),
            initialize_mint2(&quote_mint, &admin.pubkey()),
        ],
    );

    // --- pool + vaults ---------------------------------------------------------
    let pool = pool_pda(&token_mint, &quote_mint);
    let authority = pool_authority_pda(&token_mint, &quote_mint);
    let token_vault = Keypair::new();
    let quote_vault = Keypair::new();

    send(
        &mut svm,
        &admin,
        &[&admin, &token_vault, &quote_vault],
        &[Instruction {
            program_id: CUSTOM_AMM_ID,
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new_readonly(token_mint, false),
                AccountMeta::new_readonly(quote_mint, false),
                AccountMeta::new(pool, false),
                AccountMeta::new_readonly(authority, false),
                AccountMeta::new(token_vault.pubkey(), true),
                AccountMeta::new(quote_vault.pubkey(), true),
                AccountMeta::new_readonly(TOKEN_2022_ID, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: initialize_data(),
        }],
    );

    // --- hook registry + extra account meta list -------------------------------
    let registry = registry_pda();
    send(
        &mut svm,
        &admin,
        &[&admin],
        &[Instruction {
            program_id: GATEKEEPER_HOOK_ID,
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new(registry, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: {
                let mut data = DISC_INITIALIZE_REGISTRY.to_vec();
                data.extend_from_slice(CUSTOM_AMM_ID.as_ref());
                data
            },
        }],
    );

    let meta_list = extra_account_meta_list_pda(&token_mint);
    send(
        &mut svm,
        &admin,
        &[&admin],
        &[Instruction {
            program_id: GATEKEEPER_HOOK_ID,
            accounts: vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(registry, false),
                AccountMeta::new_readonly(token_mint, false),
                AccountMeta::new(meta_list, false),
                AccountMeta::new_readonly(pool, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: DISC_INITIALIZE_META_LIST.to_vec(),
        }],
    );

    // --- liquidity -------------------------------------------------------------
    send(
        &mut svm,
        &admin,
        &[&admin],
        &[
            Instruction {
                program_id: TOKEN_2022_ID,
                accounts: vec![
                    AccountMeta::new(token_mint, false),
                    AccountMeta::new(token_vault.pubkey(), false),
                    AccountMeta::new_readonly(admin.pubkey(), true),
                ],
                data: mint_to_checked_data(TOKEN_RESERVE),
            },
            Instruction {
                program_id: TOKEN_2022_ID,
                accounts: vec![
                    AccountMeta::new(quote_mint, false),
                    AccountMeta::new(quote_vault.pubkey(), false),
                    AccountMeta::new_readonly(admin.pubkey(), true),
                ],
                data: mint_to_checked_data(QUOTE_RESERVE),
            },
        ],
    );

    // --- snapshot --------------------------------------------------------------
    let mut accounts = HashMap::new();
    for key in [
        token_mint,
        quote_mint,
        pool,
        token_vault.pubkey(),
        quote_vault.pubkey(),
        registry,
        meta_list,
        CLOCK_SYSVAR_ID,
    ] {
        let account = svm
            .get_account(&key)
            .unwrap_or_else(|| panic!("account {key} missing after setup"));
        accounts.insert(key, account);
    }
    PoolSnapshot::from_accounts(accounts)
        .save_dir(dir)
        .expect("save snapshot");
}

// --- instruction encoders ------------------------------------------------------

fn initialize_mint2(mint: &Pubkey, mint_authority: &Pubkey) -> Instruction {
    let mut data = vec![20u8, DECIMALS]; // TokenInstruction::InitializeMint2
    data.extend_from_slice(mint_authority.as_ref());
    data.push(0); // freeze authority: None
    Instruction {
        program_id: TOKEN_2022_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

fn transfer_hook_initialize(authority: &Pubkey, program_id: &Pubkey) -> Vec<u8> {
    let mut data = vec![36u8, 0u8]; // TransferHookExtension, Initialize
    data.extend_from_slice(authority.as_ref());
    data.extend_from_slice(program_id.as_ref());
    data
}

fn initialize_data() -> Vec<u8> {
    let mut data = DISC_INITIALIZE.to_vec();
    data.extend_from_slice(&FEE_BPS.to_le_bytes());
    data.extend_from_slice(&LOWER_BPS.to_le_bytes());
    data.extend_from_slice(&UPPER_BPS.to_le_bytes());
    data
}

fn mint_to_checked_data(amount: u64) -> Vec<u8> {
    let mut data = vec![14u8]; // TokenInstruction::MintToChecked
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(DECIMALS);
    data
}
