use anchor_lang::prelude::*;

pub fn handle_initialize(_ctx: Context<Initialize>) -> Result<()> {
    Ok(())
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// CHECK: placeholder until T4.
    pub payer: Signer<'info>,
}
