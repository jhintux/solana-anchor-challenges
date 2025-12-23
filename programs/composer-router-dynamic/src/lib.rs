use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::{Token, TokenAccount};
use solana_program::hash;

declare_id!("Df2vmmXUbtYRyPRiXdyFWYf2PiwYQo5vMAxTbHz2WH1y");

#[program]
pub mod composer_router_dynamic {
    use super::*;

    pub fn initialize_router_config(
        ctx: Context<InitializeRouterConfig>,
        swap_programs: Vec<Pubkey>,
    ) -> Result<()> {
        require!(
            swap_programs.len() <= RouterConfig::MAX_SWAP_PROGRAMS,
            RouterError::TooManyPrograms
        );

        let config = &mut ctx.accounts.config;
        config.authority = ctx.accounts.authority.key();
        config.swap_programs = swap_programs;
        config.bump = ctx.bumps.config;

        Ok(())
    }

    pub fn update_swap_programs(
        ctx: Context<UpdateRouterConfig>,
        swap_programs: Vec<Pubkey>,
    ) -> Result<()> {
        require!(
            swap_programs.len() <= RouterConfig::MAX_SWAP_PROGRAMS,
            RouterError::TooManyPrograms
        );

        let config = &mut ctx.accounts.config;
        config.swap_programs = swap_programs;

        Ok(())
    }

    /// Deposit → Swap → Stake workflow
    /// 
    /// This instruction atomically executes:
    /// 1. Swaps input tokens for output tokens via CPI to swap_program
    /// 2. Deposits output tokens into vault via CPI to vault_program
    /// 
    /// Account layout:
    /// 
    /// Fixed accounts (defined in DepositSwapStake struct, in order):
    /// - user (signer, mut): The user executing the transaction
    /// - config: RouterConfig account (validates swap_program is allowlisted)
    /// - input_token_account (mut): User's token account for input tokens (token A)
    /// - output_token_account (mut): User's token account for output tokens (token B, receives swap output)
    /// - swap_program: Program ID to CPI to for swap (must be in allowlist)
    /// - vault_program: vault-core program ID to CPI to for deposit
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
    pub fn deposit_swap_stake(
        ctx: Context<DepositSwapStake>,
        swap_amount_in: u64,
        min_amount_out: u64,
        vault_deposit_amount: u64,
        expected_input_mint: Pubkey,
        expected_output_mint: Pubkey,
    ) -> Result<()> {
        require!(swap_amount_in > 0, RouterError::InvalidAmount);
        require!(min_amount_out > 0, RouterError::InvalidAmount);
        require!(vault_deposit_amount > 0, RouterError::InvalidAmount);

        let config = &ctx.accounts.config;

        // 1. Validate swap program is in allowlist
        require!(
            config.swap_programs.contains(&ctx.accounts.swap_program.key()),
            RouterError::SwapProgramNotAllowlisted
        );

        // 2. Validate token accounts have expected mints
        require!(
            ctx.accounts.input_token_account.mint == expected_input_mint,
            RouterError::InvalidMint
        );
        require!(
            ctx.accounts.output_token_account.mint == expected_output_mint,
            RouterError::InvalidMint
        );

        // 3. Validate token account authorities
        require!(
            ctx.accounts.input_token_account.owner == ctx.accounts.user.key(),
            RouterError::InvalidTokenAccountOwner
        );
        require!(
            ctx.accounts.output_token_account.owner == ctx.accounts.user.key(),
            RouterError::InvalidTokenAccountOwner
        );

        // 4. Validate user has sufficient balance
        require!(
            ctx.accounts.input_token_account.amount >= swap_amount_in,
            RouterError::InsufficientBalance
        );

        // 5. CPI to swap program
        // For mock-amm swap instruction:
        // Accounts: pool, user, user_token_in, user_token_out, vault_a, vault_b, pool_authority, token_program (8 accounts)
        // Instruction: swap(amount_in: u64, min_amount_out: u64)
        
        // Calculate Anchor instruction discriminator: first 8 bytes of sha256("global:swap")
        let swap_discriminator = hash::hash(b"global:swap").to_bytes()[..8].to_vec();
        
        let mut swap_ix_data = swap_discriminator;
        swap_ix_data.extend_from_slice(&swap_amount_in.to_le_bytes());
        swap_ix_data.extend_from_slice(&min_amount_out.to_le_bytes());

        // Extract swap accounts from remaining_accounts
        // mock-amm swap needs 8 accounts: pool, user, user_token_in, user_token_out, vault_a, vault_b, pool_authority, token_program
        const MOCK_AMM_SWAP_ACCOUNT_COUNT: usize = 8;
        
        if ctx.remaining_accounts.len() < MOCK_AMM_SWAP_ACCOUNT_COUNT {
            return Err(RouterError::InsufficientAccounts.into());
        }

        let swap_accounts: Vec<_> = ctx.remaining_accounts
            .iter()
            .take(MOCK_AMM_SWAP_ACCOUNT_COUNT)
            .collect();

        // Validate swap accounts match expected token accounts
        // Account 2 should be user_token_in (input_token_account)
        // Account 3 should be user_token_out (output_token_account)
        if swap_accounts.len() >= 4 {
            require!(
                swap_accounts[2].key() == ctx.accounts.input_token_account.key(),
                RouterError::InvalidMint
            );
            require!(
                swap_accounts[3].key() == ctx.accounts.output_token_account.key(),
                RouterError::InvalidMint
            );
        }

        // Derive pool_authority PDA for signing
        // Pool authority seeds: [b"pool", mint_a, mint_b, b"authority"]
        // Mints must be in deterministic order (smaller first)
        let (mint1, mint2) = if expected_input_mint < expected_output_mint {
            (expected_input_mint, expected_output_mint)
        } else {
            (expected_output_mint, expected_input_mint)
        };
        
        msg!("swap_program: {}", ctx.accounts.swap_program.key);
        let (pool_authority_pda, pool_authority_bump) = Pubkey::find_program_address(
            &[
                b"pool",
                mint1.as_ref(),
                mint2.as_ref(),
                b"authority",
            ],
            ctx.accounts.swap_program.key,
        );
        msg!("pool_authority_pda: {}", pool_authority_pda);
        msg!("pool_authority_bump: {}", pool_authority_bump);
        
        // Verify the pool_authority account matches
        require!(
            swap_accounts[6].key() == pool_authority_pda,
            RouterError::InvalidMint
        );

        // Build swap instruction
        let swap_ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: ctx.accounts.swap_program.key(),
            accounts: swap_accounts.iter().map(|acc| {
                anchor_lang::solana_program::instruction::AccountMeta {
                    pubkey: acc.key(),
                    is_signer: acc.is_signer,
                    is_writable: acc.is_writable,
                }
            }).collect(),
            data: swap_ix_data,
        };

        // Use invoke_signed to allow swap program to sign with pool_authority PDA
        let swap_account_infos: Vec<_> = swap_accounts.iter().map(|acc| (*acc).clone()).collect();
        let pool_authority_seeds = &[
            b"pool",
            mint1.as_ref(),
            mint2.as_ref(),
            b"authority",
            &[pool_authority_bump],
        ];
        invoke_signed(
            &swap_ix,
            &swap_account_infos,
            &[&pool_authority_seeds[..]],
        )?;

        // 6. Reload output token account to verify swap happened
        ctx.accounts.output_token_account.reload()?;

        // Validate that vault deposit will use the output token account
        // The vault deposit's user_token_account (4th account in remaining_accounts after swap accounts)
        // should be the output_token_account
        if ctx.remaining_accounts.len() > MOCK_AMM_SWAP_ACCOUNT_COUNT + 3 {
            let vault_user_token_account = &ctx.remaining_accounts[MOCK_AMM_SWAP_ACCOUNT_COUNT + 3];
            require!(
                vault_user_token_account.key() == ctx.accounts.output_token_account.key(),
                RouterError::InvalidMint
            );
        }

        // 7. CPI to vault-core deposit
        // Vault deposit instruction: deposit(amount: u64)
        // Accounts: vault, user_position, user, user_token_account, vault_token_account, vault_authority, token_program, system_program (8 accounts)
        
        // Calculate Anchor instruction discriminator: first 8 bytes of sha256("global:deposit")
        let deposit_discriminator = hash::hash(b"global:deposit").to_bytes()[..8].to_vec();
        
        let mut vault_ix_data = deposit_discriminator;
        vault_ix_data.extend_from_slice(&vault_deposit_amount.to_le_bytes());

        // Extract vault accounts from remaining_accounts (after swap accounts)
        // Vault deposit needs: vault, user_position, user, user_token_account, vault_token_account, vault_authority, token_program, system_program
        const VAULT_DEPOSIT_ACCOUNT_COUNT: usize = 8;
        
        if ctx.remaining_accounts.len() < MOCK_AMM_SWAP_ACCOUNT_COUNT + VAULT_DEPOSIT_ACCOUNT_COUNT {
            return Err(RouterError::InsufficientAccounts.into());
        }

        let vault_accounts: Vec<_> = ctx.remaining_accounts
            .iter()
            .skip(MOCK_AMM_SWAP_ACCOUNT_COUNT)
            .take(VAULT_DEPOSIT_ACCOUNT_COUNT)
            .collect();

        // Derive vault_authority PDA for signing
        // Vault authority seeds: [b"vault", token_mint, b"authority"]
        let (vault_authority_pda, vault_authority_bump) = Pubkey::find_program_address(
            &[
                b"vault",
                expected_output_mint.as_ref(),
                b"authority",
            ],
            ctx.accounts.vault_program.key,
        );
        msg!("vault_authority_pda: {}", vault_authority_pda);
        msg!("vault_authority_bump: {}", vault_authority_bump);
        
        // Verify the vault_authority account matches (5th account in vault_accounts)
        require!(
            vault_accounts[5].key() == vault_authority_pda,
            RouterError::InvalidMint
        );

        // Build vault deposit instruction
        let vault_ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: ctx.accounts.vault_program.key(),
            accounts: vault_accounts.iter().map(|acc| {
                anchor_lang::solana_program::instruction::AccountMeta {
                    pubkey: acc.key(),
                    is_signer: acc.is_signer,
                    is_writable: acc.is_writable,
                }
            }).collect(),
            data: vault_ix_data,
        };
        msg!("check2");
        // Use invoke_signed to allow vault program to sign with vault_authority PDA
        let vault_account_infos: Vec<_> = vault_accounts.iter().map(|acc| (*acc).clone()).collect();
        //msg!("vault_authority: {}", vault_authority_pda);
        msg!("expected_output_mint: {}", expected_output_mint);
        msg!("vault_program: {}", ctx.accounts.vault_program.key());
        let vault_authority_seeds = &[
            b"vault",
            expected_output_mint.as_ref(),
            b"authority",
            //&[vault_authority_bump],
        ];
        invoke_signed(
            &vault_ix,
            &vault_account_infos,
            &[&vault_authority_seeds[..]],
        )?;

        Ok(())
    }
}

#[account]
pub struct RouterConfig {
    pub authority: Pubkey,
    pub swap_programs: Vec<Pubkey>,
    pub bump: u8,
}

impl RouterConfig {
    pub const MAX_SWAP_PROGRAMS: usize = 10;
    pub const LEN: usize = 8 + // discriminator
        32 + // authority
        4 + (32 * Self::MAX_SWAP_PROGRAMS) + // Vec<Pubkey> max size
        1; // bump
}

#[derive(Accounts)]
pub struct InitializeRouterConfig<'info> {
    #[account(
        init,
        payer = authority,
        space = RouterConfig::LEN,
        seeds = [b"router_config"],
        bump
    )]
    pub config: Account<'info, RouterConfig>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateRouterConfig<'info> {
    #[account(
        mut,
        seeds = [b"router_config"],
        bump = config.bump,
        has_one = authority @ RouterError::Unauthorized
    )]
    pub config: Account<'info, RouterConfig>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct DepositSwapStake<'info> {
    #[account(
        seeds = [b"router_config"],
        bump = config.bump
    )]
    pub config: Account<'info, RouterConfig>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub input_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub output_token_account: Account<'info, TokenAccount>,

    /// CHECK: Swap program to CPI to
    pub swap_program: UncheckedAccount<'info>,

    /// CHECK: Vault program to CPI to
    pub vault_program: UncheckedAccount<'info>,

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
    #[msg("Swap program not in allowlist")]
    SwapProgramNotAllowlisted,
    #[msg("Too many swap programs")]
    TooManyPrograms,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Insufficient accounts provided")]
    InsufficientAccounts,
}
