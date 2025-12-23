use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount, Transfer};

declare_id!("8TN4YaBrKm5WZAcFTxzEBTA1i8AXxwnnYWTFxYF5PsSU");

#[program]
pub mod mock_amm {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        initial_amount_a: u64,
        initial_amount_b: u64,
    ) -> Result<()> {
        require!(initial_amount_a > 0, AmmError::InvalidAmount);
        require!(initial_amount_b > 0, AmmError::InvalidAmount);

        // Verify vault token accounts match mints
        require!(
            ctx.accounts.vault_a.mint == ctx.accounts.mint_a.key(),
            AmmError::InvalidMint
        );
        require!(
            ctx.accounts.vault_b.mint == ctx.accounts.mint_b.key(),
            AmmError::InvalidMint
        );
        // Verify vault authorities are the pool_authority PDA
        require!(
            ctx.accounts.vault_a.owner == ctx.accounts.pool_authority.key(),
            AmmError::InvalidMint
        );
        require!(
            ctx.accounts.vault_b.owner == ctx.accounts.pool_authority.key(),
            AmmError::InvalidMint
        );

        let pool = &mut ctx.accounts.pool;
        pool.mint_a = ctx.accounts.mint_a.key();
        pool.mint_b = ctx.accounts.mint_b.key();
        pool.vault_a = ctx.accounts.vault_a.key();
        pool.vault_b = ctx.accounts.vault_b.key();
        pool.authority = ctx.accounts.authority.key();

        // Transfer initial liquidity from authority
        // Transfer token A
        let cpi_accounts_a = Transfer {
            from: ctx.accounts.authority_token_account_a.to_account_info(),
            to: ctx.accounts.vault_a.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program.clone(), cpi_accounts_a);
        anchor_spl::token::transfer(cpi_ctx, initial_amount_a)?;

        // Transfer token B
        let cpi_accounts_b = Transfer {
            from: ctx.accounts.authority_token_account_b.to_account_info(),
            to: ctx.accounts.vault_b.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts_b);
        anchor_spl::token::transfer(cpi_ctx, initial_amount_b)?;

        Ok(())
    }

    pub fn swap(ctx: Context<Swap>, amount_in: u64, min_amount_out: u64) -> Result<()> {
        require!(amount_in > 0, AmmError::InvalidAmount);

        let pool = &ctx.accounts.pool;
        
        // Validate token accounts match pool
        require!(
            ctx.accounts.user_token_in.mint == pool.mint_a
                || ctx.accounts.user_token_in.mint == pool.mint_b,
            AmmError::InvalidMint
        );
        require!(
            ctx.accounts.user_token_out.mint == pool.mint_a
                || ctx.accounts.user_token_out.mint == pool.mint_b,
            AmmError::InvalidMint
        );
        require!(
            ctx.accounts.user_token_in.mint != ctx.accounts.user_token_out.mint,
            AmmError::SameMint
        );

        // Determine which vault is input and which is output
        let (vault_in, vault_out) = if ctx.accounts.user_token_in.mint == pool.mint_a {
            (&ctx.accounts.vault_a, &ctx.accounts.vault_b)
        } else {
            (&ctx.accounts.vault_b, &ctx.accounts.vault_a)
        };

        // Get current reserves
        let reserve_in = vault_in.amount;
        let reserve_out = vault_out.amount;

        require!(reserve_in > 0 && reserve_out > 0, AmmError::InsufficientLiquidity);

        // Calculate output using constant product formula: (x + dx) * (y - dy) = x * y
        // dy = (y * dx) / (x + dx)
        // Using u128 to prevent overflow
        let amount_out = ((amount_in as u128)
            .checked_mul(reserve_out as u128)
            .ok_or(AmmError::MathOverflow)?
            .checked_div(reserve_in.checked_add(amount_in).ok_or(AmmError::MathOverflow)? as u128)
            .ok_or(AmmError::DivisionByZero)?) as u64;

        require!(amount_out >= min_amount_out, AmmError::SlippageExceeded);
        require!(amount_out > 0, AmmError::InvalidAmount);

        // Transfer tokens from user to pool (input)
        let cpi_accounts_in = Transfer {
            from: ctx.accounts.user_token_in.to_account_info(),
            to: vault_in.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts_in);
        anchor_spl::token::transfer(cpi_ctx, amount_in)?;

        // Transfer tokens from pool to user (output)
        let seeds = &[
            b"pool",
            pool.mint_a.as_ref(),
            pool.mint_b.as_ref(),
            b"authority",
            &[ctx.bumps.pool_authority],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts_out = Transfer {
            from: vault_out.to_account_info(),
            to: ctx.accounts.user_token_out.to_account_info(),
            authority: ctx.accounts.pool_authority.to_account_info(),
        };

        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts_out, signer);
        anchor_spl::token::transfer(cpi_ctx, amount_out)?;

        Ok(())
    }
}

#[account]
pub struct Pool {
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub authority: Pubkey,
}

impl Pool {
    pub const LEN: usize = 8 + std::mem::size_of::<Self>();
}

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = authority,
        space = Pool::LEN,
        seeds = [b"pool", mint_a.key().as_ref(), mint_b.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub mint_a: Account<'info, Mint>,
    pub mint_b: Account<'info, Mint>,

    #[account(mut)]
    pub vault_a: Account<'info, TokenAccount>,

    #[account(mut)]
    pub vault_b: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for pool token accounts
    #[account(
        seeds = [b"pool", mint_a.key().as_ref(), mint_b.key().as_ref(), b"authority"],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub authority_token_account_a: Account<'info, TokenAccount>,
    #[account(mut)]
    pub authority_token_account_b: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(
        seeds = [b"pool", pool.mint_a.as_ref(), pool.mint_b.as_ref()],
        bump
    )]
    pub pool: Account<'info, Pool>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub user_token_in: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_out: Account<'info, TokenAccount>,

    #[account(
        mut,
        address = pool.vault_a
    )]
    pub vault_a: Account<'info, TokenAccount>,

    #[account(
        mut,
        address = pool.vault_b
    )]
    pub vault_b: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for pool token accounts
    #[account(
        seeds = [b"pool", pool.mint_a.as_ref(), pool.mint_b.as_ref(), b"authority"],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

#[error_code]
pub enum AmmError {
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Invalid mint")]
    InvalidMint,
    #[msg("Same mint used for input and output")]
    SameMint,
    #[msg("Insufficient liquidity")]
    InsufficientLiquidity,
    #[msg("Slippage exceeded")]
    SlippageExceeded,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Division by zero")]
    DivisionByZero,
}
