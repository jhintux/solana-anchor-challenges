import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { VaultCore } from "../target/types/vault_core";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";

describe("vault-core", () => {
  // Configure the client to use the local cluster.
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.vaultCore as Program<VaultCore>;
  const connection = provider.connection;

  let authority: Keypair;
  let user1: Keypair;
  let user2: Keypair;
  let tokenMint1: PublicKey;
  let tokenMint2: PublicKey;

  beforeEach(async () => {
    // Create keypairs
    authority = Keypair.generate();
    user1 = Keypair.generate();
    user2 = Keypair.generate();

    // Airdrop SOL to keypairs
    const airdropAmount = 10 * anchor.web3.LAMPORTS_PER_SOL;

    const airdropTxs = await Promise.all([
      connection.requestAirdrop(authority.publicKey, airdropAmount),
      connection.requestAirdrop(user1.publicKey, airdropAmount),
      connection.requestAirdrop(user2.publicKey, airdropAmount),
    ]);

    const blockhash = await connection.getLatestBlockhash();
    // Wait for confirmations
    await Promise.all(
      airdropTxs.map((signature) => {
        return connection.confirmTransaction(
          { signature, ...blockhash },
          "confirmed"
        );
      })
    );

    // Create token mints
    tokenMint1 = await createMint(
      connection,
      authority,
      authority.publicKey,
      null,
      9
    );
    tokenMint2 = await createMint(
      connection,
      authority,
      authority.publicKey,
      null,
      9
    );
  });

  async function getVaultPDA(
    tokenMint: PublicKey
  ): Promise<[PublicKey, number]> {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), tokenMint.toBuffer()],
      program.programId
    );
  }

  async function getVaultAuthorityPDA(
    tokenMint: PublicKey
  ): Promise<[PublicKey, number]> {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), tokenMint.toBuffer(), Buffer.from("authority")],
      program.programId
    );
  }

  async function getUserPositionPDA(
    vault: PublicKey,
    user: PublicKey
  ): Promise<[PublicKey, number]> {
    return PublicKey.findProgramAddressSync(
      [Buffer.from("position"), vault.toBuffer(), user.toBuffer()],
      program.programId
    );
  }

  describe("initialize_vault", () => {
    it("Initializes a vault successfully", async () => {
      const [vault] = await getVaultPDA(tokenMint1);

      await program.methods
        .initializeVault()
        .accounts({
          //vault: vault,
          authority: authority.publicKey,
          tokenMint: tokenMint1,
          //vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([authority])
        .rpc();

      const vaultAccount = await program.account.vault.fetch(vault);
      expect(vaultAccount.authority.toString()).to.equal(
        authority.publicKey.toString()
      );
      expect(vaultAccount.tokenMint.toString()).to.equal(tokenMint1.toString());
      expect(vaultAccount.totalShares.toNumber()).to.equal(0);
    });

    it("Allows multiple vaults for different token mints", async () => {
      // Initialize vault for tokenMint1
      const [vault1] = await getVaultPDA(tokenMint1);

      await program.methods
        .initializeVault()
        .accounts({
          authority: authority.publicKey,
          tokenMint: tokenMint1,
        })
        .signers([authority])
        .rpc();

      // Initialize vault for tokenMint2
      const [vault2] = await getVaultPDA(tokenMint2);

      await program.methods
        .initializeVault()
        .accounts({
          authority: authority.publicKey,
          tokenMint: tokenMint2,
        })
        .signers([authority])
        .rpc();

      // Verify both vaults exist and are different
      const vaultAccount1 = await program.account.vault.fetch(vault1);
      const vaultAccount2 = await program.account.vault.fetch(vault2);
      expect(vaultAccount1.tokenMint.toString()).to.equal(
        tokenMint1.toString()
      );
      expect(vaultAccount2.tokenMint.toString()).to.equal(
        tokenMint2.toString()
      );
      expect(vault1.toString()).to.not.equal(vault2.toString());
    });
  });

  describe("deposit", () => {
    let vault: PublicKey;
    let vaultTokenAccount: PublicKey;
    let userTokenAccount: PublicKey;

    beforeEach(async () => {
      // Initialize vault
      const [vaultPDA] = await getVaultPDA(tokenMint1);
      vault = vaultPDA;
      const [vaultAuthority] = await getVaultAuthorityPDA(tokenMint1);
      const vaultTokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMint1,
        vaultAuthority,
        true
      );
      vaultTokenAccount = vaultTokenAccountInfo.address;

      await program.methods
        .initializeVault()
        .accounts({
          authority: authority.publicKey,
          tokenMint: tokenMint1,
        })
        .signers([authority])
        .rpc();

      // Create user token account and mint tokens
      const userTokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user1,
        tokenMint1,
        user1.publicKey,
        false
      );
      userTokenAccount = userTokenAccountInfo.address;
      await mintTo(
        connection,
        authority,
        tokenMint1,
        userTokenAccount,
        authority,
        1000000 * 10 ** 9 // 1M tokens with 9 decimals
      );
    });

    it("Deposits tokens and mints shares (first deposit)", async () => {
      const depositAmount = new anchor.BN(1000 * 10 ** 9); // 1000 tokens

      const [userPosition] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );

      await program.methods
        .deposit(depositAmount)
        .accounts({
          vault: vault,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
        })
        .signers([user1])
        .rpc();

      // Verify vault state
      const vaultAccount = await program.account.vault.fetch(vault);
      expect(vaultAccount.totalShares.toNumber()).to.equal(
        depositAmount.toNumber()
      );

      // Verify user position
      const positionAccount = await program.account.userPosition.fetch(
        userPosition
      );
      expect(positionAccount.shares.toNumber()).to.equal(
        depositAmount.toNumber()
      );
      expect(positionAccount.user.toString()).to.equal(
        user1.publicKey.toString()
      );
      expect(positionAccount.vault.toString()).to.equal(vault.toString());

      // Verify token balances
      const vaultBalance = await getAccount(connection, vaultTokenAccount);
      expect(vaultBalance.amount.toString()).to.equal(depositAmount.toString());
    });

    it("Deposits tokens and calculates shares correctly (subsequent deposit)", async () => {
      const firstDeposit = new anchor.BN(1000 * 10 ** 9);
      const secondDeposit = new anchor.BN(500 * 10 ** 9);

      const [userPosition] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );

      // First deposit
      await program.methods
        .deposit(firstDeposit)
        .accounts({
          vault: vault,
          //userPosition: userPosition,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      // Second deposit - should get proportional shares
      await program.methods
        .deposit(secondDeposit)
        .accounts({
          vault: vault,
          //userPosition: userPosition,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      const vaultAccount = await program.account.vault.fetch(vault);
      const positionAccount = await program.account.userPosition.fetch(
        userPosition
      );

      // Shares should be: 1000 + (500 * 1000) / 1000 = 1000 + 500 = 1500
      expect(vaultAccount.totalShares.toNumber()).to.equal(1500 * 10 ** 9);
      expect(positionAccount.shares.toNumber()).to.equal(1500 * 10 ** 9);
    });

    it("Handles tiny deposit (1 token) when vault is empty", async () => {
      const tinyDeposit = new anchor.BN(1);
      const [userPosition] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );

      await program.methods
        .deposit(tinyDeposit)
        .accounts({
          vault: vault,
          //userPosition: userPosition,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      const positionAccount = await program.account.userPosition.fetch(
        userPosition
      );
      expect(positionAccount.shares.toNumber()).to.equal(1);
    });

    it("Handles tiny deposit with existing balance (tests rounding)", async () => {
      const largeDeposit = new anchor.BN(999999 * 10 ** 9);
      const tinyDeposit = new anchor.BN(1);

      // Large deposit first
      await program.methods
        .deposit(largeDeposit)
        .accounts({
          vault: vault,
          //userPosition: userPosition,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      // Tiny deposit - may result in 0 shares due to rounding
      // With 1M tokens and 1 token deposit: shares = (1 * 1M) / 1M = 1 share
      // So it should succeed, not fail
      try {
        await program.methods
          .deposit(tinyDeposit)
          .accounts({
            vault: vault,
            //userPosition: userPosition,
            user: user1.publicKey,
            userTokenAccount: userTokenAccount,
            vaultTokenAccount: vaultTokenAccount,
            //vaultAuthority: vaultAuthority,
            //tokenProgram: TOKEN_PROGRAM_ID,
            //systemProgram: SystemProgram.programId,
          })
          .signers([user1])
          .rpc();
        // If it succeeds, that's expected - the calculation results in at least 1 share
        // Verify the position was updated
        const [userPositionPDA] = await getUserPositionPDA(
          vault,
          user1.publicKey
        );
        const positionAccount = await program.account.userPosition.fetch(
          userPositionPDA
        );
        expect(positionAccount.shares.toNumber()).to.be.greaterThan(0);
      } catch (e) {
        // If it fails, it should be due to 0 shares from rounding
        // Check for InvalidAmount error (shares must be > 0)
        const errorStr = e.toString();
        console.log(errorStr);
        expect(
          errorStr.includes("InvalidAmount") ||
            errorStr.includes("Invalid amount") ||
            errorStr.includes("invalid amount") ||
            errorStr.includes("must be greater than zero")
        ).to.be.true;
      }
    });

    it("Supports multiple users depositing", async () => {
      const user1Deposit = new anchor.BN(1000 * 10 ** 9);
      const user2Deposit = new anchor.BN(2000 * 10 ** 9);

      // Create user2 token account
      const user2TokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user2,
        tokenMint1,
        user2.publicKey,
        false
      );
      const user2TokenAccount = user2TokenAccountInfo.address;
      await mintTo(
        connection,
        authority,
        tokenMint1,
        user2TokenAccount,
        authority,
        1000000 * 10 ** 9
      );

      const [user1Position] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );
      const [user2Position] = await getUserPositionPDA(
        vault,
        user2.publicKey
      );

      // User1 deposits
      await program.methods
        .deposit(user1Deposit)
        .accounts({
          vault: vault,
          //userPosition: user1Position,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();

      // User2 deposits
      await program.methods
        .deposit(user2Deposit)
        .accounts({
          vault: vault,
          //userPosition: user2Position,
          user: user2.publicKey,
          userTokenAccount: user2TokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user2])
        .rpc();

      const vaultAccount = await program.account.vault.fetch(vault);
      const user1PositionAccount = await program.account.userPosition.fetch(
        user1Position
      );
      const user2PositionAccount = await program.account.userPosition.fetch(
        user2Position
      );

      // Total shares should be: 1000 + 2000 = 3000
      expect(vaultAccount.totalShares.toNumber()).to.equal(3000 * 10 ** 9);
      expect(user1PositionAccount.shares.toNumber()).to.equal(
        user1Deposit.toNumber()
      );
      expect(user2PositionAccount.shares.toNumber()).to.equal(
        user2Deposit.toNumber()
      );
    });

    it("Fails with zero amount", async () => {
      try {
        await program.methods
          .deposit(new anchor.BN(0))
          .accounts({
            vault: vault,
            //userPosition: userPosition,
            user: user1.publicKey,
            userTokenAccount: userTokenAccount,
            vaultTokenAccount: vaultTokenAccount,
            //vaultAuthority: vaultAuthority,
            //tokenProgram: TOKEN_PROGRAM_ID,
            //systemProgram: SystemProgram.programId,
          })
          .signers([user1])
          .rpc();
        expect.fail("Should have failed with zero amount");
      } catch (e) {
        expect(e.toString()).to.include("InvalidAmount");
      }
    });
  });

  describe("withdraw", () => {
    let vault: PublicKey;
    let vaultAuthority: PublicKey;
    let vaultTokenAccount: PublicKey;
    let userTokenAccount: PublicKey;
    let userPosition: PublicKey;

    beforeEach(async () => {
      // Initialize vault
      const [vaultPDA] = await getVaultPDA(tokenMint1);
      vault = vaultPDA;
      const [vaultAuthorityPDA] = await getVaultAuthorityPDA(tokenMint1);
      vaultAuthority = vaultAuthorityPDA;
      const vaultTokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMint1,
        vaultAuthority,
        true
      );
      vaultTokenAccount = vaultTokenAccountInfo.address;

      await program.methods
        .initializeVault()
        .accounts({
          //vault: vault,
          authority: authority.publicKey,
          tokenMint: tokenMint1,
          //vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([authority])
        .rpc();

      // Create user token account and mint tokens
      const userTokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user1,
        tokenMint1,
        user1.publicKey,
        false
      );
      userTokenAccount = userTokenAccountInfo.address;
      await mintTo(
        connection,
        authority,
        tokenMint1,
        userTokenAccount,
        authority,
        1000000 * 10 ** 9
      );

      // Make initial deposit
      const [userPositionPDA] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );
      userPosition = userPositionPDA;
      await program.methods
        .deposit(new anchor.BN(10000 * 10 ** 9))
        .accounts({
          vault: vault,
          //userPosition: userPosition,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          //vaultAuthority: vaultAuthority,
          //tokenProgram: TOKEN_PROGRAM_ID,
          //systemProgram: SystemProgram.programId,
        })
        .signers([user1])
        .rpc();
    });

    it("Withdraws tokens and burns shares (partial withdraw)", async () => {
      const withdrawShares = new anchor.BN(5000 * 10 ** 9);

      const initialVaultBalance = (
        await getAccount(connection, vaultTokenAccount)
      ).amount;
      const initialUserBalance = (
        await getAccount(connection, userTokenAccount)
      ).amount;

      await program.methods
        .withdraw(withdrawShares)
        .accountsPartial({
          vault,
          user: user1.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
        })
        .signers([user1])
        .rpc();

      // Verify vault state
      const vaultAccount = await program.account.vault.fetch(vault);
      expect(vaultAccount.totalShares.toNumber()).to.equal(5000 * 10 ** 9);

      // Verify user position
      const positionAccount = await program.account.userPosition.fetch(
        userPosition
      );
      expect(positionAccount.shares.toNumber()).to.equal(5000 * 10 ** 9);

      // Verify token balances changed
      const finalVaultBalance = (
        await getAccount(connection, vaultTokenAccount)
      ).amount;
      const finalUserBalance = (await getAccount(connection, userTokenAccount))
        .amount;

      expect(finalVaultBalance.toString()).to.equal(
        (initialVaultBalance - BigInt(5000 * 10 ** 9)).toString()
      );
      expect(finalUserBalance.toString()).to.equal(
        (initialUserBalance + BigInt(5000 * 10 ** 9)).toString()
      );
    });

    it("Withdraws all shares and closes account (full withdraw)", async () => {
      const vaultAccountBefore = await program.account.vault.fetch(vault);
      const totalShares = vaultAccountBefore.totalShares;

      await program.methods
        .withdraw(totalShares)
        .accountsPartial({
          vault,
          user: user1.publicKey,
          userTokenAccount,
          vaultTokenAccount,
        })
        .signers([user1])
        .rpc();

      // Verify vault state
      const vaultAccount = await program.account.vault.fetch(vault);
      expect(vaultAccount.totalShares.toNumber()).to.equal(0);

      // Verify user position account is closed
      try {
        const [userPositionPDA] = await getUserPositionPDA(
          vault,
          user1.publicKey
        );
        await program.account.userPosition.fetch(userPositionPDA);
        expect.fail("Account should be closed");
      } catch (e) {
        // Expected - account should be closed
        const errorStr = e.toString();
        expect(
          errorStr.includes("AccountNotFound") ||
            errorStr.includes("Account not found") ||
            errorStr.includes("does not exist")
        ).to.be.true;
      }
    });

    it("Fails with insufficient shares", async () => {
      const vaultAccount = await program.account.vault.fetch(vault);
      const excessiveShares = vaultAccount.totalShares.add(new anchor.BN(1));

      try {
        await program.methods
          .withdraw(excessiveShares)
          .accountsPartial({
            vault,
            user: user1.publicKey,
            userTokenAccount,
            vaultTokenAccount,
          })
          .signers([user1])
          .rpc();
        expect.fail("Should have failed with insufficient shares");
      } catch (e) {
        expect(e.toString()).to.include("InsufficientShares");
      }
    });

    it("Fails with zero shares", async () => {
      try {
        await program.methods
          .withdraw(new anchor.BN(0))
          .accountsPartial({
            vault,
            user: user1.publicKey,
            userTokenAccount,
            vaultTokenAccount,
          })
          .signers([user1])
          .rpc();
        expect.fail("Should have failed with zero shares");
      } catch (e) {
        expect(e.toString()).to.include("InvalidAmount");
      }
    });
  });

  describe("invariants", () => {
    it("Maintains invariant: total_shares >= sum(user_shares) with multiple users", async () => {
      // Initialize vault
      const [vault,] = await getVaultPDA(tokenMint1);
      const [vaultAuthority,] = await getVaultAuthorityPDA(tokenMint1);
      const vaultTokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        authority,
        tokenMint1,
        vaultAuthority,
        true
      );
      const vaultTokenAccount = vaultTokenAccountInfo.address;

      await program.methods
        .initializeVault()
        .accounts({
          authority: authority.publicKey,
          tokenMint: tokenMint1,
        })
        .signers([authority])
        .rpc();

      // Create token accounts for both users
      const user1TokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user1,
        tokenMint1,
        user1.publicKey,
        false
      );
      const user1TokenAccount = user1TokenAccountInfo.address;
      const user2TokenAccountInfo = await getOrCreateAssociatedTokenAccount(
        connection,
        user2,
        tokenMint1,
        user2.publicKey,
        false
      );
      const user2TokenAccount = user2TokenAccountInfo.address;
      await Promise.all([
        mintTo(
          connection,
          authority,
          tokenMint1,
          user1TokenAccount,
          authority,
          1000000 * 10 ** 9
        ),
        mintTo(
          connection,
          authority,
          tokenMint1,
          user2TokenAccount,
          authority,
          1000000 * 10 ** 9
        ),
      ]);

      const [user1Position] = await getUserPositionPDA(
        vault,
        user1.publicKey
      );
      const [user2Position] = await getUserPositionPDA(
        vault,
        user2.publicKey
      );

      // Multiple deposits and withdrawals
      await program.methods
        .deposit(new anchor.BN(1000 * 10 ** 9))
        .accounts({
          vault: vault,
          user: user1.publicKey,
          userTokenAccount: user1TokenAccount,
          vaultTokenAccount: vaultTokenAccount,
        })
        .signers([user1])
        .rpc();

      await program.methods
        .deposit(new anchor.BN(2000 * 10 ** 9))
        .accounts({
          vault: vault,
          user: user2.publicKey,
          userTokenAccount: user2TokenAccount,
          vaultTokenAccount: vaultTokenAccount,
        })
        .signers([user2])
        .rpc();

      // Verify invariant
      const vaultAccount = await program.account.vault.fetch(vault);
      const user1PositionAccount = await program.account.userPosition.fetch(
        user1Position
      );
      const user2PositionAccount = await program.account.userPosition.fetch(
        user2Position
      );

      const totalUserShares =
        user1PositionAccount.shares.toNumber() +
        user2PositionAccount.shares.toNumber();
      expect(vaultAccount.totalShares.toNumber()).to.be.at.least(
        totalUserShares
      );
      expect(vaultAccount.totalShares.toNumber()).to.equal(totalUserShares);
    });
  });
});
