pub mod constants;
pub mod errors;
pub mod instructions;
pub mod math;
pub mod oracle;
pub mod state;
pub mod venue;

use anchor_lang::prelude::*;

pub use constants::*;
pub use errors::*;
pub use instructions::*;

declare_id!("8UEBfsW4pD4VoGcyRiA2qnkuyh1s3DpwfzCphacXef6w");

#[program]
pub mod elastic_treasury {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        crate::instructions::initialize::handle_initialize(ctx)
    }
}
