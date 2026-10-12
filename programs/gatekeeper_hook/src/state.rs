//! Registry state.

use anchor_lang::prelude::*;

/// A single global registry (PDA `[b"registry"]`).
///
/// The hook is deliberately stateless per-mint: the `ExtraAccountMetaList`
/// lives at the canonical `[b"extra-account-metas", mint]` address and this
/// registry is the only mutable state. Rotating the approved venue is a single
/// instruction, gated by `admin`.
#[account]
pub struct Registry {
    /// Account allowed to rotate `approved_amm`.
    pub admin: Pubkey,
    /// The only program allowed to mediate transfers on a hooked mint.
    pub approved_amm: Pubkey,
    /// PDA bump.
    pub bump: u8,
}

impl Registry {
    /// Anchor discriminator (8) + three fields.
    pub const LEN: usize = 8 + 32 + 32 + 1;
}
