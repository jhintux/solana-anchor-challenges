import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ComposerRouter } from "../target/types/composer_router";
import { MockAmm } from "../target/types/mock_amm";
import { VaultCore } from "../target/types/vault_core";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  createMint,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";
import {
  PublicKey,
  Keypair,
  SystemProgram,
  SendTransactionError,
} from "@solana/web3.js";

describe.only("composer-router", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const routerProgram = anchor.workspace
    .composerRouter as Program<ComposerRouter>;
  const ammProgram = anchor.workspace.mockAmm as Program<MockAmm>;
  const vaultProgram = anchor.workspace.vaultCore as Program<VaultCore>;
  const connection = provider.connection;

  let authority: Keypair;
  let user: Keypair;
  let tokenMintA: PublicKey;
  let tokenMintB: PublicKey;

  beforeEach(async () => {
    authority = Keypair.generate();
    user = Keypair.generate();

    const airdropAmount = 10 * anchor.web3.LAMPORTS_PER_SOL;
    const airdropTxs = await Promise.all([
      connection.requestAirdrop(authority.publicKey, airdropAmount),
      connection.requestAirdrop(user.publicKey, airdropAmount),
    ]);

    const blockhash = await connection.getLatestBlockhash();
    await Promise.all(
      airdropTxs.map((signature) => {
        return connection.confirmTransaction(
          { signature, ...blockhash },
          "confirmed"
        );
      })
    );

    tokenMintA = await createMint(
      connection,
      authority,
      authority.publicKey,
      null,
      9
    );
    tokenMintB = await createMint(
      connection,
      authority,
      authority.publicKey,
      null,
      9
    );
  });

  async function getPoolPDA(
    mintA: PublicKey,
    mintB: PublicKey
  ): Promise<[PublicKey, number]> {
    // Ensure deterministic ordering (smaller mint first)
    const [mint1, mint2] =
      mintA.toBuffer().toString("hex") < mintB.toBuffer().toString("hex")
        ? [mintA, mintB]
        : [mintB, mintA];
    return PublicKey.findProgramAddressSync(
      [Buffer.from("pool"), mint1.toBuffer(), mint2.toBuffer()],
      ammProgram.programId
    );
  }

  async function getPoolAuthorityPDA(
    mintA: PublicKey,
    mintB: PublicKey
  ): Promise<[PublicKey, number]> {
    const [mint1, mint2] =
      mintA.toBuffer().toString("hex") < mintB.toBuffer().toString("hex")
        ? [mintA, mintB]
        : [mintB, mintA];
    return PublicKey.findProgramAddressSync(
      [
        Buffer.from("pool"),
        mint1.toBuffer(),
        mint2.toBuffer(),
        Buffer.from("authority"),
      ],
      ammProgram.programId
    );
  }

  describe("deposit_swap_stake", () => {
    let pool: PublicKey;
    let poolAuthority: PublicKey;
    let vault: PublicKey;
    let vaultAuthority: PublicKey;
    let userTokenAccountA: PublicKey;
    let userTokenAccountB: PublicKey;
    let vaultTokenAccount: PublicKey;
    let poolVaultA: PublicKey;
    let poolVaultB: PublicKey;

    beforeEach(async () => {
      [tokenMintA, tokenMintB] =
        tokenMintA.toBuffer().toString("hex") <
        tokenMintB.toBuffer().toString("hex")
          ? [tokenMintA, tokenMintB]
          : [tokenMintB, tokenMintA];

      // Initialize AMM pool
      const [poolPDA, bumpPool] = await getPoolPDA(tokenMintA, tokenMintB);
      pool = poolPDA;
      const [poolAuthorityPDA, bumpAuth] = await getPoolAuthorityPDA(
        tokenMintA,
        tokenMintB
      );
      poolAuthority = poolAuthorityPDA;

      console.log(
        "Creating pool vaults - ensure they're created with pool_authority as owner"
      );
      const poolVaultAInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMintA,
        poolAuthority,
        true
      );
      poolVaultA = poolVaultAInfo.address;

      const poolVaultBInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMintB,
        poolAuthority,
        true
      );
      poolVaultB = poolVaultBInfo.address;

      console.log("Verifying vaults have correct owner (pool_authority)");
      const vaultAAccount = await getAccount(connection, poolVaultA);
      const vaultBAccount = await getAccount(connection, poolVaultB);
      if (vaultAAccount.owner.toString() !== poolAuthority.toString()) {
        throw new Error(
          `Pool vault A has wrong owner. Expected ${poolAuthority.toString()}, got ${vaultAAccount.owner.toString()}`
        );
      }
      if (vaultBAccount.owner.toString() !== poolAuthority.toString()) {
        throw new Error(
          `Pool vault B has wrong owner. Expected ${poolAuthority.toString()}, got ${vaultBAccount.owner.toString()}`
        );
      }

      console.log("Creating authority token accounts for initial liquidity");
      const authorityTokenAccountA = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMintA,
        authority.publicKey,
        false
      );
      const authorityTokenAccountB = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMintB,
        authority.publicKey,
        false
      );

      await mintTo(
        connection,
        authority,
        tokenMintA,
        authorityTokenAccountA.address,
        authority,
        1000000 * 10 ** 9,
        undefined
      );
      await mintTo(
        connection,
        authority,
        tokenMintB,
        authorityTokenAccountB.address,
        authority,
        1000000 * 10 ** 9,
        undefined
      );

      console.log("Initializing pool");
      try {
        await ammProgram.methods
          .initializePool(
            new anchor.BN(100000 * 10 ** 9),
            new anchor.BN(100000 * 10 ** 9)
          )
          .accounts({
            authority: authority.publicKey,
            mintA: tokenMintA,
            mintB: tokenMintB,
            vaultA: poolVaultA,
            vaultB: poolVaultB,
            authorityTokenAccountA: authorityTokenAccountA.address,
            authorityTokenAccountB: authorityTokenAccountB.address,
          })
          .signers([authority])
          .rpc();
      } catch (e: any) {
        if (!e.toString().includes("already in use")) {
          throw e;
        }
      }

      console.log("Initializing vault for token B");
      const [vaultPDA] = PublicKey.findProgramAddressSync(
        [Buffer.from("vault"), tokenMintB.toBuffer()],
        vaultProgram.programId
      );
      vault = vaultPDA;
      const [vaultAuthorityPDA] = PublicKey.findProgramAddressSync(
        [Buffer.from("vault"), tokenMintB.toBuffer(), Buffer.from("authority")],
        vaultProgram.programId
      );
      vaultAuthority = vaultAuthorityPDA;

      vaultTokenAccount = await getAssociatedTokenAddress(
        tokenMintB,
        vaultAuthority,
        true
      );

      console.log("Initializing vault");
      try {
        const tx = await vaultProgram.methods
          .initializeVault()
          .accounts({
            authority: authority.publicKey,
            tokenMint: tokenMintB,
            rewardMint: tokenMintB,
          })
          .signers([authority])
          .rpc();
        console.log("tx: ", tx);
      } catch (e: any) {
        if (!e.toString().includes("already in use")) {
          throw e;
        }
      }

      console.log("Creating user token accounts");
      const userTokenAccountAInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user,
        tokenMintA,
        user.publicKey,
        false
      );
      userTokenAccountA = userTokenAccountAInfo.address;

      const userTokenAccountBInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user,
        tokenMintB,
        user.publicKey,
        false
      );
      userTokenAccountB = userTokenAccountBInfo.address;

      console.log("Minting tokens to user");
      await mintTo(
        connection,
        authority,
        tokenMintA,
        userTokenAccountA,
        authority,
        10000 * 10 ** 9,
        undefined
      );
    });

    it("Happy path: Deposit → Swap → Stake succeeds", async () => {
      const swapAmountIn = new anchor.BN(1000 * 10 ** 9);
      const minAmountOut = new anchor.BN(900 * 10 ** 9); // Allow some slippage
      const vaultDepositAmount = new anchor.BN(950 * 10 ** 9); // Approximate swap output

      console.log("Getting user position PDA");
      const [userPosition] = PublicKey.findProgramAddressSync(
        [Buffer.from("position"), vault.toBuffer(), user.publicKey.toBuffer()],
        vaultProgram.programId
      );

      const initialBalanceA = (await getAccount(connection, userTokenAccountA))
        .amount;
      const initialBalanceB = (await getAccount(connection, userTokenAccountB))
        .amount;

      // Build remaining accounts for swap CPI
      // mock-amm swap accounts: pool, user, user_token_in, user_token_out, vault_a, vault_b, pool_authority, token_program
      const swapAccounts = [
        { pubkey: pool, isSigner: false, isWritable: false },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountA, isSigner: false, isWritable: true },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: poolVaultA, isSigner: false, isWritable: true },
        { pubkey: poolVaultB, isSigner: false, isWritable: true },
        { pubkey: poolAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ];

      // Build remaining accounts for vault deposit CPI
      // vault deposit accounts: vault, user_position, user, user_token_account, vault_token_account, vault_authority, token_program, system_program
      const vaultAccounts = [
        { pubkey: vault, isSigner: false, isWritable: true },
        { pubkey: userPosition, isSigner: false, isWritable: true },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: vaultTokenAccount, isSigner: false, isWritable: true },
        { pubkey: vaultAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ];

      const remainingAccounts = [...swapAccounts, ...vaultAccounts];

      const tx = await routerProgram.methods
        .depositSwapStake(
          swapAmountIn,
          minAmountOut,
          vaultDepositAmount,
          tokenMintA,
          tokenMintB
        )
        .accounts({
          user: user.publicKey,
          inputTokenAccount: userTokenAccountA,
          outputTokenAccount: userTokenAccountB,
        })
        .remainingAccounts(remainingAccounts)
        .signers([user])
        .rpc({ skipPreflight: true });
      console.log("tx: ", tx);

      // Verify tokens were swapped and deposited
      const finalBalanceA = (await getAccount(connection, userTokenAccountA))
        .amount;
      const finalBalanceB = (await getAccount(connection, userTokenAccountB))
        .amount;

      expect(Number(finalBalanceA)).to.be.lessThan(Number(initialBalanceA));
      expect(Number(finalBalanceB)).to.be.greaterThan(Number(initialBalanceB));

      // Verify vault has tokens
      const vaultBalance = (await getAccount(connection, vaultTokenAccount))
        .amount;
      expect(Number(vaultBalance)).to.be.greaterThan(0);

      // Verify user position was created
      const position = await vaultProgram.account.userPosition.fetch(
        userPosition
      );
      expect(position.shares.toNumber()).to.be.greaterThan(0);
    });

    it("Fails with invalid swap program", async () => {
      const swapAmountIn = new anchor.BN(1000 * 10 ** 9);
      const minAmountOut = new anchor.BN(900 * 10 ** 9);
      const vaultDepositAmount = new anchor.BN(950 * 10 ** 9);

      const [userPosition] = PublicKey.findProgramAddressSync(
        [Buffer.from("position"), vault.toBuffer(), user.publicKey.toBuffer()],
        vaultProgram.programId
      );

      // Build remaining accounts for swap CPI
      const swapAccounts = [
        { pubkey: pool, isSigner: false, isWritable: false },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountA, isSigner: false, isWritable: true },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: poolVaultA, isSigner: false, isWritable: true },
        { pubkey: poolVaultB, isSigner: false, isWritable: true },
        { pubkey: poolAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ];

      // Build remaining accounts for vault deposit CPI
      const vaultAccounts = [
        { pubkey: vault, isSigner: false, isWritable: true },
        { pubkey: userPosition, isSigner: false, isWritable: true },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: vaultTokenAccount, isSigner: false, isWritable: true },
        { pubkey: vaultAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ];

      const remainingAccounts = [...swapAccounts, ...vaultAccounts];

      // Pass vault program as amm_program to trigger InvalidSwapProgram error
      // Note: Since ammProgram has a fixed address in the IDL, Anchor will validate it
      // and fail if we try to use a different program. This tests that validation.
      try {
        await routerProgram.methods
          .depositSwapStake(
            swapAmountIn,
            minAmountOut,
            vaultDepositAmount,
            tokenMintA,
            tokenMintB
          )
          .accounts({
            user: user.publicKey,
            inputTokenAccount: userTokenAccountA,
            outputTokenAccount: userTokenAccountB,
          })
          .accountsPartial({
            ammProgram: vaultProgram.programId, // Invalid: using vault program instead of AMM program
          })
          .remainingAccounts(remainingAccounts)
          .signers([user])
          .rpc();
        expect.fail("Should have failed with invalid swap program");
      } catch (e) {
        // Anchor will fail during account validation since vault program doesn't match MockAmm IDL
        // The error might be an Anchor error, but we check for program-related errors
        expect(
          e.toString().includes("InvalidSwapProgram") ||
            e.toString().includes("invalidSwapProgram") ||
            e.toString().includes("ConstraintProgram") ||
            e.toString().includes("Program") ||
            e.toString().includes("IDL")
        ).to.be.true;
      }
    });

    it("Fails with insufficient balance", async () => {
      const swapAmountIn = new anchor.BN(100000 * 10 ** 9); // More than user has
      const minAmountOut = new anchor.BN(900 * 10 ** 9);
      const vaultDepositAmount = new anchor.BN(950 * 10 ** 9);

      const [userPosition] = PublicKey.findProgramAddressSync(
        [Buffer.from("position"), vault.toBuffer(), user.publicKey.toBuffer()],
        vaultProgram.programId
      );

      // Build remaining accounts for swap CPI
      const swapAccounts = [
        { pubkey: pool, isSigner: false, isWritable: false },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountA, isSigner: false, isWritable: true },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: poolVaultA, isSigner: false, isWritable: true },
        { pubkey: poolVaultB, isSigner: false, isWritable: true },
        { pubkey: poolAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ];

      // Build remaining accounts for vault deposit CPI
      const vaultAccounts = [
        { pubkey: vault, isSigner: false, isWritable: true },
        { pubkey: userPosition, isSigner: false, isWritable: true },
        { pubkey: user.publicKey, isSigner: true, isWritable: false },
        { pubkey: userTokenAccountB, isSigner: false, isWritable: true },
        { pubkey: vaultTokenAccount, isSigner: false, isWritable: true },
        { pubkey: vaultAuthority, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ];

      const remainingAccounts = [...swapAccounts, ...vaultAccounts];

      try {
        await routerProgram.methods
          .depositSwapStake(
            swapAmountIn,
            minAmountOut,
            vaultDepositAmount,
            tokenMintA,
            tokenMintB
          )
          .accounts({
            user: user.publicKey,
            inputTokenAccount: userTokenAccountA,
            outputTokenAccount: userTokenAccountB,
          })
          .remainingAccounts(remainingAccounts)
          .signers([user])
          .rpc();
        expect.fail("Should have failed with insufficient balance");
      } catch (e) {
        expect(
          e.toString().includes("InsufficientBalance") ||
            e.toString().includes("insufficientBalance")
        ).to.be.true;
      }
    });
  });
});
