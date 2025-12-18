use anchor_lang::prelude::*;
use anchor_spl::{token::{Mint, Token, TokenAccount, Transfer}, associated_token::AssociatedToken};

declare_id!("A4nGMAE6j5xty4a5PALzz7nYnWQcB59mYcLptZMoYkfN");

// Precision scaling factor for reward calculations (1e12)
pub const REWARD_PRECISION: u128 = 1_000_000_000_000;

#[program]
pub mod vault_core {
    use super::*;

    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let clock = Clock::get()?;
        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.token_mint = ctx.accounts.token_mint.key();
        vault.total_shares = 0;
        vault.reward_rate = 0;
        vault.acc_reward_per_share = 0;
        vault.last_update_ts = clock.unix_timestamp;
        vault.reward_mint = ctx.accounts.reward_mint.key();
        vault.reward_vault = ctx.accounts.reward_vault.key();
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

        // Update rewards before processing deposit
        let clock = Clock::get()?;
        update_rewards(vault, clock.unix_timestamp)?;

        // Initialize or update user position
        // Protect against re-initialization: if account exists, verify it matches
        let is_new_position = user_position.shares == 0;
        if !is_new_position {
            // Account already exists - verify it matches
            require!(
                user_position.user == ctx.accounts.user.key(),
                VaultError::InvalidUserPosition
            );
            require!(user_position.vault == vault.key(), VaultError::InvalidVault);
        } else {
            // New account - initialize fields
            user_position.user = ctx.accounts.user.key();
            user_position.vault = vault.key();
            user_position.reward_debt = 0;
        }

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

        user_position.shares = user_position
            .shares
            .checked_add(shares)
            .ok_or(VaultError::MathOverflow)?;

        // Update reward_debt: user's new debt = new_shares * acc_reward_per_share (stored scaled)
        let new_shares = user_position.shares;
        user_position.reward_debt = (new_shares as u128)
            .checked_mul(vault.acc_reward_per_share)
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

        // Update rewards before processing withdraw
        let clock = Clock::get()?;
        update_rewards(vault, clock.unix_timestamp)?;

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

        // Calculate pending rewards BEFORE updating shares
        // This uses the current (old) shares value
        let total_owed_scaled_before_withdrawal = (user_position.shares as u128)
            .checked_mul(vault.acc_reward_per_share)
            .ok_or(VaultError::MathOverflow)?;
        
        let old_reward_debt = user_position.reward_debt;
        let pending_rewards_scaled = total_owed_scaled_before_withdrawal
            .saturating_sub(old_reward_debt);
        
        // Calculate new shares
        let new_shares = user_position
            .shares
            .checked_sub(shares)
            .ok_or(VaultError::MathOverflow)?;
        
        // Update shares
        user_position.shares = new_shares;

        // Update reward_debt based on new shares
        if new_shares > 0 {
            // Normal case: update reward_debt to match new shares
            user_position.reward_debt = (new_shares as u128)
                .checked_mul(vault.acc_reward_per_share)
                .ok_or(VaultError::MathOverflow)?;
        } else {
            // Shares reached 0
            if pending_rewards_scaled > 0 {
                // Store the pending rewards amount directly in reward_debt
                // This is a special case: when shares=0, reward_debt represents pending rewards (scaled)
                // In claim_rewards, we'll detect shares=0 and use reward_debt directly as pending
                user_position.reward_debt = pending_rewards_scaled;
            } else {
                // No pending rewards, set to 0
                user_position.reward_debt = 0;
            }
        }

        // Close account only if shares reach zero AND no pending rewards
        // If there are pending rewards, account remains open for claiming
        // After claiming, claim_rewards will close the account
        if new_shares == 0 && pending_rewards_scaled == 0 {
            // No pending rewards, safe to close
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

    pub fn fund_rewards(ctx: Context<FundRewards>, amount: u64, reward_rate: u64) -> Result<()> {
        require!(amount > 0, VaultError::InvalidAmount);

        let vault = &mut ctx.accounts.vault;
        let reward_vault = &ctx.accounts.reward_vault;

        // Validate reward_vault matches vault's reward_vault
        require!(
            vault.reward_vault == reward_vault.key(),
            VaultError::RewardVaultMismatch
        );

        // Validate reward mint matches
        require!(
            vault.reward_mint == reward_vault.mint,
            VaultError::InvalidRewardMint
        );
        require!(
            vault.reward_mint == ctx.accounts.funder_token_account.mint,
            VaultError::InvalidRewardMint
        );

        // Transfer tokens from funder to reward vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.funder_token_account.to_account_info(),
            to: reward_vault.to_account_info(),
            authority: ctx.accounts.funder.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        anchor_spl::token::transfer(cpi_ctx, amount)?;

        // Update reward rate if provided
        if reward_rate > 0 {
            vault.reward_rate = reward_rate;
        }

        Ok(())
    }

    pub fn claim_rewards(ctx: Context<ClaimRewards>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let user_position = &mut ctx.accounts.user_position;
        let reward_vault = &ctx.accounts.reward_vault;

        // Validate reward_vault matches vault's reward_vault
        require!(
            vault.reward_vault == reward_vault.key(),
            VaultError::RewardVaultMismatch
        );

        // Validate reward mint matches
        require!(
            vault.reward_mint == reward_vault.mint,
            VaultError::InvalidRewardMint
        );
        require!(
            vault.reward_mint == ctx.accounts.user_reward_token_account.mint,
            VaultError::InvalidRewardMint
        );

        // Verify user position matches
        require!(user_position.vault == vault.key(), VaultError::InvalidVault);
        require!(
            user_position.user == ctx.accounts.user.key(),
            VaultError::InvalidVault
        );

        // Update rewards before calculating pending
        let clock = Clock::get()?;
        update_rewards(vault, clock.unix_timestamp)?;

        // Calculate pending rewards
        // Special case: if shares == 0, reward_debt stores pending_rewards_scaled (from withdraw)
        // In this case, pending = reward_debt directly (since shares * acc_reward_per_share = 0)
        let pending_scaled = if user_position.shares == 0 {
            // When shares are 0, reward_debt stores the pending rewards amount (scaled)
            // This was set in withdraw when shares became 0 and there were pending rewards
            user_position.reward_debt
        } else {
            // Normal case: pending = (shares * acc_reward_per_share) - reward_debt
            let total_owed_scaled = (user_position.shares as u128)
                .checked_mul(vault.acc_reward_per_share)
                .ok_or(VaultError::MathOverflow)?;
            total_owed_scaled
                .saturating_sub(user_position.reward_debt)
        };
        
        let pending = pending_scaled
            .checked_div(REWARD_PRECISION)
            .ok_or(VaultError::DivisionByZero)?;

        // Transfer rewards if there are any pending
        if pending > 0 {
            let pending_u64 = pending.min(u64::MAX as u128) as u64;
            
            // Check sufficient balance in reward vault
            require!(
                reward_vault.amount >= pending_u64,
                VaultError::InsufficientRewardBalance
            );

            // Transfer tokens from reward vault to user
            let seeds = &[
                b"vault",
                vault.token_mint.as_ref(),
                b"authority",
                &[ctx.bumps.vault_authority],
            ];
            let signer = &[&seeds[..]];

            let cpi_accounts = Transfer {
                from: reward_vault.to_account_info(),
                to: ctx.accounts.user_reward_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
            anchor_spl::token::transfer(cpi_ctx, pending_u64)?;

            // Update reward_debt: recalculate based on current shares (stored scaled)
            if user_position.shares > 0 {
                user_position.reward_debt = (user_position.shares as u128)
                    .checked_mul(vault.acc_reward_per_share)
                    .ok_or(VaultError::MathOverflow)?;
            } else {
                // Shares are 0, no more rewards can accrue, set reward_debt to 0
                user_position.reward_debt = 0;
                
                // Close the account since rewards are claimed and shares are 0
                let user = ctx.accounts.user.to_account_info();
                let user_position_account = ctx.accounts.user_position.to_account_info();
                let dest_starting_lamports = user.lamports();
                **user.lamports.borrow_mut() = dest_starting_lamports
                    .checked_add(user_position_account.lamports())
                    .ok_or(VaultError::MathOverflow)?;
                **user_position_account.lamports.borrow_mut() = 0;
            }
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

// Helper function to update rewards based on elapsed time
// This is idempotent - multiple calls in the same slot produce the same result
fn update_rewards(vault: &mut Vault, current_ts: i64) -> Result<()> {
    let delta_ts = current_ts.saturating_sub(vault.last_update_ts);

    // If same slot (delta_ts == 0) or no shares, skip update → idempotent!
    if delta_ts == 0 || vault.total_shares == 0 {
        vault.last_update_ts = current_ts;
        return Ok(());
    }

    // Calculate rewards to distribute: reward_rate * delta_time
    let rewards = (vault.reward_rate as u128)
        .checked_mul(delta_ts as u128)
        .ok_or(VaultError::MathOverflow)?;

    // Update accumulated rewards per share
    // acc_reward_per_share += (rewards * REWARD_PRECISION) / total_shares
    if rewards > 0 && vault.total_shares > 0 {
        let acc_increment = (rewards * REWARD_PRECISION)
            .checked_div(vault.total_shares as u128)
            .ok_or(VaultError::DivisionByZero)?;

        vault.acc_reward_per_share = vault
            .acc_reward_per_share
            .checked_add(acc_increment)
            .ok_or(VaultError::MathOverflow)?;
    }

    vault.last_update_ts = current_ts;
    Ok(())
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

    pub reward_mint: Account<'info, Mint>,

    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = reward_mint,
        associated_token::authority = vault_authority,
        associated_token::token_program = token_program
    )]
    pub reward_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,

    /// CHECK: Clock sysvar for timestamp
    pub clock: Sysvar<'info, Clock>,
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

    /// CHECK: Clock sysvar for timestamp
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
pub struct FundRewards<'info> {
    #[account(mut)]
    pub vault: Account<'info, Vault>,

    #[account(mut)]
    pub funder: Signer<'info>,

    #[account(mut)]
    pub funder_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub reward_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[account]
pub struct Vault {
    pub authority: Pubkey,
    pub token_mint: Pubkey,
    pub total_shares: u64,
    pub reward_rate: u64,
    pub acc_reward_per_share: u128,
    pub last_update_ts: i64,
    pub reward_mint: Pubkey,
    pub reward_vault: Pubkey,
}

impl Vault {
    pub const LEN: usize = 8 + std::mem::size_of::<Self>();
}

#[account]
pub struct UserPosition {
    pub user: Pubkey,
    pub vault: Pubkey,
    pub shares: u64,
    pub reward_debt: u128,
}

impl UserPosition {
    pub const LEN: usize = 8 + std::mem::size_of::<Self>();
}

#[derive(Accounts)]
pub struct ClaimRewards<'info> {
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
    pub user_reward_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub reward_vault: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for the vault token account
    #[account(
        seeds = [b"vault", vault.token_mint.as_ref(), b"authority"],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,

    /// CHECK: Clock sysvar for timestamp
    pub clock: Sysvar<'info, Clock>,
}

#[error_code]
pub enum VaultError {
    #[msg("Insufficient shares to withdraw")]
    InsufficientShares,
    #[msg("Invalid vault account")]
    InvalidVault,
    #[msg("Invalid user position")]
    InvalidUserPosition,
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
    #[msg("Reward rate not set")]
    RewardRateNotSet,
    #[msg("Insufficient reward balance")]
    InsufficientRewardBalance,
    #[msg("Reward vault mismatch")]
    RewardVaultMismatch,
    #[msg("Invalid reward mint")]
    InvalidRewardMint,
}
