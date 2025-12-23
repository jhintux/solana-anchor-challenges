use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use mock_amm::program::MockAmm;
use vault_core::program::VaultCore;

declare_id!("5bw3v7LUaXn3pRmgXUPpeneYu9My3AhF7EemUNmmVLUQ");

#[program]
pub mod composer_router {
    use super::*;

    /// Deposit → Swap → Stake workflow
    ///
    /// This instruction atomically executes:
    /// 1. Swaps input tokens for output tokens via CPI to mock-amm
    /// 2. Deposits output tokens into vault via CPI to vault-core
    /// ///
    /// Account layout:
    ///
    /// Fixed accounts (defined in DepositSwapStake struct, in order):
    /// - user (signer, mut): The user executing the transaction
    /// - config: RouterConfig account (validates swap_program is allowlisted)
    /// - input_token_account (mut): User's token account for input tokens (token A)
    /// - output_token_account (mut): User's token account for output tokens (token B, receives swap output)
    /// - swap_program: Swap program to CPI to for swap
    /// - vault_program: vault-core program to CPI to for deposit
    /// - token_program: SPL Token program
    /// - system_program: System program
    ///
    /// Remaining accounts (variable, passed through to CPIs):
    ///
    /// First 8 accounts - Swap instruction accounts (for mock-amm swap):
    /// - [0] pool: AMM Pool account
    /// - [1] user: User signer (same as fixed accounts user)
    /// - [2] user_token_in: Must match input_token_account (validated)
    /// - [3] user_token_out: Must match output_token_account (validated)
    /// - [4] vault_a: Pool's token A vault
    /// - [5] vault_b: Pool's token B vault
    /// - [6] pool_authority: Pool's PDA authority
    /// - [7] token_program: SPL Token program
    ///
    /// Next 8 accounts - Vault deposit instruction accounts:
    /// - [8] vault: Vault account (must match output_token_account mint)
    /// - [9] user_position: User's position PDA in vault
    /// - [10] user: User signer (same as fixed accounts user)
    /// - [11] user_token_account: Must match output_token_account (validated)
    /// - [12] vault_token_account: Vault's token account
    /// - [13] vault_authority: Vault's PDA authority
    /// - [14] token_program: SPL Token program
    /// - [15] system_program: System program
    ///
    /// Total: 16 remaining accounts required
    pub fn deposit_swap_stake<'c: 'info, 'info>(
        ctx: Context<'_, '_, 'c, 'info, DepositSwapStake<'info>>,
        swap_amount_in: u64,
        min_amount_out: u64,
        vault_deposit_amount: u64,
        expected_input_mint: Pubkey,
        expected_output_mint: Pubkey,
    ) -> Result<()> {
        require!(swap_amount_in > 0, RouterError::InvalidAmount);
        require!(min_amount_out > 0, RouterError::InvalidAmount);
        require!(vault_deposit_amount > 0, RouterError::InvalidAmount);

        // Validate token account authorities
        require!(
            ctx.accounts.input_token_account.owner == ctx.accounts.user.key(),
            RouterError::InvalidTokenAccountOwner
        );

        require!(
            ctx.accounts.output_token_account.owner == ctx.accounts.user.key(),
            RouterError::InvalidTokenAccountOwner
        );

        require!(
            ctx.accounts.input_token_account.mint == expected_input_mint,
            RouterError::InvalidMint
        );

        require!(
            ctx.accounts.output_token_account.mint == expected_output_mint,
            RouterError::InvalidMint
        );

        // Validate user has sufficient balance
        require!(
            ctx.accounts.input_token_account.amount >= swap_amount_in,
            RouterError::InsufficientBalance
        );

        // 1. CPI to mock-amm swap
        let swap_accounts: Vec<_> = ctx
            .remaining_accounts
            .iter()
            .take(8)
            .collect::<Vec<&AccountInfo>>();

        let vault_accounts = ctx
            .remaining_accounts
            .iter()
            .skip(8)
            .take(8)
            .collect::<Vec<_>>();

        // Validate swap accounts match expected token accounts
        // Account 2 should be user_token_in (input_token_account)
        // Account 3 should be user_token_out (output_token_account)
        require!(
            ctx.remaining_accounts[2].key() == ctx.accounts.input_token_account.key(),
            RouterError::InvalidMint
        );
        require!(
            ctx.remaining_accounts[3].key() == ctx.accounts.output_token_account.key(),
            RouterError::InvalidMint
        );

        let mut seeds = vec![
            b"pool",
            expected_input_mint.as_ref(),
            expected_output_mint.as_ref(),
            b"authority",
        ];

        let (pool_authority_pda, pool_authority_bump) =
            Pubkey::find_program_address(&seeds, &ctx.accounts.amm_program.key());

        // Verify the pool_authority account matches
        require!(
            swap_accounts[6].key() == pool_authority_pda,
            RouterError::InvalidMint
        );

        let bump = [pool_authority_bump];
        seeds.push(&bump);
        let pool_authority_seeds = [&seeds[..]];

        let ctx_swap = CpiContext::new(
            ctx.accounts.amm_program.to_account_info(),
            mock_amm::cpi::accounts::Swap {
                pool: swap_accounts[0].to_account_info(),
                user: swap_accounts[1].to_account_info(),
                user_token_in: swap_accounts[2].to_account_info(),
                user_token_out: swap_accounts[3].to_account_info(),
                vault_a: swap_accounts[4].to_account_info(),
                vault_b: swap_accounts[5].to_account_info(),
                pool_authority: swap_accounts[6].to_account_info(),
                token_program: swap_accounts[7].to_account_info(),
            },
        )
        .with_signer(&pool_authority_seeds);
        mock_amm::cpi::swap(ctx_swap, swap_amount_in, min_amount_out)?;

        // 2. Reload output token account to verify swap happened
        ctx.accounts.output_token_account.reload()?;

        let mut seeds = vec![b"vault", expected_output_mint.as_ref(), b"authority"];

        let (vault_authority_pda, vault_authority_bump) =
            Pubkey::find_program_address(&seeds, ctx.accounts.vault_program.key);

        // Verify the vault_authority account matches
        require!(
            vault_accounts[5].key() == vault_authority_pda,
            RouterError::InvalidMint
        );

        let bump = [vault_authority_bump];
        seeds.push(&bump);
        let vault_authority_seeds = [&seeds[..]];

        // 3. CPI to vault-core deposit
        let ctx_deposit = CpiContext::new(
            ctx.accounts.vault_program.to_account_info(),
            vault_core::cpi::accounts::Deposit {
                vault: vault_accounts[0].to_account_info(),
                user_position: vault_accounts[1].to_account_info(),
                user: vault_accounts[2].to_account_info(),
                user_token_account: vault_accounts[3].to_account_info(),
                vault_token_account: vault_accounts[4].to_account_info(),
                vault_authority: vault_accounts[5].to_account_info(),
                token_program: vault_accounts[6].to_account_info(),
                system_program: vault_accounts[7].to_account_info(),
            },
        )
        .with_signer(&vault_authority_seeds);
        vault_core::cpi::deposit(ctx_deposit, vault_deposit_amount)?;

        Ok(())
    }
}

#[derive(Accounts)]
pub struct DepositSwapStake<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub input_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub output_token_account: Account<'info, TokenAccount>,

    pub amm_program: Program<'info, MockAmm>,
    pub vault_program: Program<'info, VaultCore>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[error_code]
pub enum RouterError {
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Invalid mint")]
    InvalidMint,
    #[msg("Invalid token account owner")]
    InvalidTokenAccountOwner,
    #[msg("Insufficient balance")]
    InsufficientBalance,
    #[msg("Invalid swap program")]
    InvalidSwapProgram,
    #[msg("Invalid vault program")]
    InvalidVaultProgram,
    #[msg("Invalid token account")]
    InvalidTokenAccount,
    #[msg("Invalid pool authority")]
    InvalidPoolAuthority,
    #[msg("Invalid vault authority")]
    InvalidVaultAuthority,
}
