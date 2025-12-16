use anchor_lang::prelude::*;
use anchor_spl::{token::{Mint, Token, TokenAccount, Transfer}, associated_token::AssociatedToken};

declare_id!("A4nGMAE6j5xty4a5PALzz7nYnWQcB59mYcLptZMoYkfN");

#[program]
pub mod vault_core {
    use super::*;

    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.token_mint = ctx.accounts.token_mint.key();
        vault.total_shares = 0;
        Ok(())
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, VaultError::InvalidAmount);

        let vault = &mut ctx.accounts.vault;
        let vault_token_account = &ctx.accounts.vault_token_account;
        let user_position = &mut ctx.accounts.user_position;

        // Verify token mint matches
        require!(
            vault.token_mint == vault_token_account.mint,
            VaultError::InvalidTokenMint
        );
        require!(
            vault.token_mint == ctx.accounts.user_token_account.mint,
            VaultError::InvalidTokenMint
        );

        // Get current vault balance
        let vault_balance = vault_token_account.amount;

        // Calculate shares to mint
        let shares = calculate_shares_for_deposit(amount, vault_balance, vault.total_shares)?;

        require!(shares > 0, VaultError::InvalidAmount);

        // Transfer tokens from user to vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        anchor_spl::token::transfer(cpi_ctx, amount)?;

        // Update state
        vault.total_shares = vault
            .total_shares
            .checked_add(shares)
            .ok_or(VaultError::MathOverflow)?;

        // Initialize or update user position
        // Protect against re-initialization: if account exists, verify it matches
        if user_position.shares > 0 {
            // Account already exists - verify it matches
            require!(
                user_position.user == ctx.accounts.user.key(),
                VaultError::InvalidVault
            );
            require!(user_position.vault == vault.key(), VaultError::InvalidVault);
        } else {
            // New account - initialize fields
            user_position.user = ctx.accounts.user.key();
            user_position.vault = vault.key();
        }
        user_position.shares = user_position
            .shares
            .checked_add(shares)
            .ok_or(VaultError::MathOverflow)?;

        Ok(())
    }

    pub fn withdraw(ctx: Context<Withdraw>, shares: u64) -> Result<()> {
        require!(shares > 0, VaultError::InvalidAmount);

        let vault = &mut ctx.accounts.vault;
        let user_position = &mut ctx.accounts.user_position;
        let vault_token_account = &ctx.accounts.vault_token_account;

        // Verify user position matches
        require!(user_position.vault == vault.key(), VaultError::InvalidVault);
        require!(
            user_position.user == ctx.accounts.user.key(),
            VaultError::InvalidVault
        );

        // Verify sufficient shares
        require!(
            user_position.shares >= shares,
            VaultError::InsufficientShares
        );

        // Get current vault balance
        let vault_balance = vault_token_account.amount;

        // Calculate tokens to withdraw
        let tokens = calculate_tokens_for_withdraw(shares, vault_balance, vault.total_shares)?;

        // Transfer tokens from vault to user
        let seeds = &[
            b"vault",
            vault.token_mint.as_ref(),
            b"authority",
            &[ctx.bumps.vault_authority],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: vault_token_account.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        anchor_spl::token::transfer(cpi_ctx, tokens)?;

        // Update state
        vault.total_shares = vault
            .total_shares
            .checked_sub(shares)
            .ok_or(VaultError::MathOverflow)?;

        let new_shares = user_position
            .shares
            .checked_sub(shares)
            .ok_or(VaultError::MathOverflow)?;
        user_position.shares = new_shares;

        // Close account if shares reach zero (safe close)
        if new_shares == 0 {
            let user = ctx.accounts.user.to_account_info();
            let user_position_account = ctx.accounts.user_position.to_account_info();
            let dest_starting_lamports = user.lamports();
            **user.lamports.borrow_mut() = dest_starting_lamports
                .checked_add(user_position_account.lamports())
                .ok_or(VaultError::MathOverflow)?;
            **user_position_account.lamports.borrow_mut() = 0;
        }

        Ok(())
    }
}

// Helper function to calculate shares for deposit
fn calculate_shares_for_deposit(
    deposit_amount: u64,
    vault_balance: u64,
    total_shares: u64,
) -> Result<u64> {
    if vault_balance == 0 {
        // First deposit: 1:1 ratio
        Ok(deposit_amount)
    } else {
        // shares = (deposit_amount * total_shares) / vault_balance
        // Use u128 to prevent overflow
        // Note: Integer division rounds down (truncates), which is correct for vault security
        // This prevents share inflation and ensures the vault can always honor withdrawals
        let shares = (deposit_amount as u128)
            .checked_mul(total_shares as u128)
            .ok_or(VaultError::MathOverflow)?
            .checked_div(vault_balance as u128)
            .ok_or(VaultError::DivisionByZero)?;

        Ok(shares as u64)
    }
}

// Helper function to calculate tokens for withdraw
fn calculate_tokens_for_withdraw(
    shares: u64,
    vault_balance: u64,
    total_shares: u64,
) -> Result<u64> {
    require!(total_shares > 0, VaultError::DivisionByZero);

    // tokens = (shares * vault_balance) / total_shares
    // Use u128 to prevent overflow
    let tokens = (shares as u128)
        .checked_mul(vault_balance as u128)
        .ok_or(VaultError::MathOverflow)?
        .checked_div(total_shares as u128)
        .ok_or(VaultError::DivisionByZero)?;

    let tokens_u64 = tokens as u64;
    require!(tokens_u64 > 0, VaultError::InvalidAmount);

    Ok(tokens_u64)
}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = authority,
        space = Vault::LEN,
        seeds = [b"vault", token_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_mint: Account<'info, Mint>,

    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = token_mint,
        associated_token::authority = vault_authority,
        associated_token::token_program = token_program
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for the vault token account
    #[account(
        seeds = [b"vault", token_mint.key().as_ref(), b"authority"],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub vault: Account<'info, Vault>,

    #[account(
        init_if_needed,
        payer = user,
        space = UserPosition::LEN,
        seeds = [b"position", vault.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for the vault token account
    #[account(
        seeds = [b"vault", vault.token_mint.as_ref(), b"authority"],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"position", vault.key().as_ref(), user.key().as_ref()],
        bump,
        has_one = vault @ VaultError::InvalidVault,
        has_one = user @ VaultError::InvalidVault
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for the vault token account
    #[account(
        seeds = [b"vault", vault.token_mint.as_ref(), b"authority"],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[account]
pub struct Vault {
    pub authority: Pubkey,
    pub token_mint: Pubkey,
    pub total_shares: u64,
}

impl Vault {
    pub const LEN: usize = 8 + std::mem::size_of::<Self>(); // discriminator + authority + token_mint + total_shares
}

#[account]
pub struct UserPosition {
    pub user: Pubkey,
    pub vault: Pubkey,
    pub shares: u64,
}

impl UserPosition {
    pub const LEN: usize = 8 + std::mem::size_of::<Self>(); // discriminator + user + vault + shares
}

#[error_code]
pub enum VaultError {
    #[msg("Insufficient shares to withdraw")]
    InsufficientShares,
    #[msg("Invalid vault account")]
    InvalidVault,
    #[msg("Invalid token mint")]
    InvalidTokenMint,
    #[msg("Invalid amount (must be greater than zero)")]
    InvalidAmount,
    #[msg("Division by zero")]
    DivisionByZero,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Insufficient vault balance")]
    InsufficientVaultBalance,
}
