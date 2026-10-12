//! Custom errors for the gatekeeper transfer hook.
//!
//! Every guard resolves to a named variant so a client can branch on the
//! failure. Codes are positional from 6000; never renumber, only append.

use anchor_lang::prelude::*;

#[error_code]
pub enum GatekeeperError {
    /// The transfer was mediated by a program that is not the approved AMM.
    /// This is the whole point of the hook: liquidity must route through the
    /// registered venue, not through an arbitrary program.
    UnauthorizedMarketAccess,
    /// The source or destination token account does not have its
    /// `TransferHookAccount.transferring` flag set, which means the hook is
    /// being invoked outside a real Token-2022 transfer.
    NotTransferring,
    /// The caller never set an approved AMM.
    AmmNotSet,
    /// Caller is not the registry admin.
    NotAdmin,
    /// A pubkey argument was the default pubkey.
    ZeroAddress,
    /// The mint attached to this hook is missing the `TransferHook` extension
    /// or points at a different hook program.
    MintHookMismatch,
    /// Arithmetic that must be exact overflowed.
    MathOverflow,
}
