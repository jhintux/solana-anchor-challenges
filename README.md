# Solana DeFi Challenges

This repository contains a series of hands-on projects designed to practice and learn more about **Anchor**, **Solana**, and **DeFi concepts**. Each project builds upon the previous one, gradually introducing more complex patterns and real-world DeFi mechanics.

Everyone is invited to follow along and complete these projects! Whether you're new to Solana development or looking to deepen your understanding of DeFi primitives, these challenges provide a structured path to building production-ready protocols.

---

### Project 1 — **Vault Core: Shares-Based Token Vault**

**Brief:** Build a program-owned SPL token vault where users deposit and withdraw using a **shares** model (not raw balances).
**Goals**

* Understand program-owned custody, PDAs, token CPI, and share accounting.
* Build a “DeFi primitive” you’ll reuse forever.
  **Constraints**
* No looping over users, ever.
* All math must be deterministic and integer-based (use `u128` for intermediate math).
* Deposits/withdrawals must be permissionless and safe under rounding.
  **Tasks**
* Define accounts: `Vault`, `VaultAuthority(PDA)`, `VaultTokenAccount`, `UserPosition`.
* Implement instructions: `initialize_vault`, `deposit`, `withdraw`.
* Implement share math: mint shares on deposit, burn shares on withdraw.
* Add invariants: `total_shares >= sum(user_shares)` (enforced via state transitions).
* Add tests: rounding edge cases (tiny deposits, full withdraw, partial withdraw).
* Add “safe close”: close `UserPosition` when shares hit zero.

---

### Project 2 — **Staking Emissions: Lazy Rewards Engine**

**Brief:** Extend the vault so stakers earn rewards over time, calculated lazily on interaction.
**Goals**

* Learn time-based DeFi accounting patterns (`reward_per_share`, `reward_debt`).
* Build a scalable rewards system with constant-time operations.
  **Constraints**
* No iterating through all stakers.
* Reward accrual must be monotonic and stable across multiple calls in the same slot.
* Rewards precision must be handled explicitly (scaling factor).
  **Tasks**
* Extend `Vault` state: `reward_rate`, `acc_reward_per_share`, `last_update_ts`.
* Extend `UserPosition`: `shares`, `reward_debt`, `pending_rewards`.
* Add instruction: `fund_rewards` (seed the reward vault).
* Add instruction: `claim_rewards`.
* Update `deposit/withdraw` to “settle” rewards before changing shares.
* Integrate `Clock` sysvar and implement `update_rewards()` helper.
* Tests: multiple users, staggered deposits, claim timing, zero-reward periods.

---

### Project 3 — **Composer Router: Multi-Step Transaction Lego**

**Brief:** Create a router program that composes actions like deposit → swap (via CPI) → restake in one instruction.
**Goals**

* Learn Solana transaction composition patterns (remaining accounts, CPI chaining).
* Build a reusable “workflow” layer similar to real DeFi routers.
  **Constraints**
* Router must be generic: it shouldn’t hardcode a single swap program.
* Must handle compute limits and fail safely (atomicity).
* Validate critical accounts; don’t blindly trust `remaining_accounts`.
  **Tasks**
* Define router instruction: `deposit_swap_stake` (single entrypoint).
* Implement CPI into your Vault program (Project 1/2).
* Implement CPI into a “mock AMM” program (simple swap) for learning.
* Design and document the required `remaining_accounts` layout.
* Add guardrails: expected mints, token accounts, and authority checks.
* Add tests: wrong account ordering, wrong mint, swap slippage bounds, missing accounts.

---

### Project 4 — **Flash Loan Vault: Borrow + Callback + Repay**

**Brief:** Add a flash-loan feature: borrow from the vault, run arbitrary logic via callback CPI, enforce repayment + fee before exit.
**Goals**

* Master “end-of-instruction invariants” and atomic safety.
* Learn how protocols safely interact with untrusted code in the same transaction.
  **Constraints**
* Must verify repayment by checking actual vault token balance deltas.
* Callback can be any program: treat it as hostile.
* If invariant fails, entire transaction must revert.
  **Tasks**
* Add state: `flash_fee_bps`, `fee_treasury`.
* Implement `flash_loan(amount, callback_program, callback_ix_data)`.
* Transfer out funds → CPI callback → verify repayment + fee → transfer fee to treasury.
* Add optional allowlist mode for callback programs (configurable).
* Tests: successful repay, under-repay, repay wrong mint, repay to wrong account, callback tries weird stuff.

---

### Project 5 — **Oracle Guardrails: Price-Aware Protocol Controls**

**Brief:** Integrate an oracle (Pyth or Switchboard) and gate actions based on fresh, confident prices.
**Goals**

* Learn safe oracle consumption: freshness + confidence + sanity bounds.
* Build reusable oracle validation utilities for later protocols.
  **Constraints**
* Reject stale prices (define a max age window).
* Reject low-confidence prices (confidence interval too wide).
* Never use oracle values without validation.
  **Tasks**
* Implement `read_price()` + validation helpers: staleness/confidence checks.
* Add an instruction: `oracle_gated_withdraw` or `oracle_gated_swap`.
* Add config: `max_staleness_secs`, `max_confidence_bps`, optional `min_price/max_price`.
* Tests: stale oracle, confidence too wide, valid oracle, manipulated inputs.

---

### Project 6 — **Mini Lending Market: Collateral, Borrow, Liquidate**

**Brief:** Build a minimal lending protocol: deposit collateral, borrow against it using oracle prices, and allow liquidation when health < 1.
**Goals**

* Combine vault custody + oracle truth + risk math into a real protocol shape.
* Learn liquidation mechanics and “don’t go bankrupt” constraints.
  **Constraints**
* No iteration over borrowers.
* Health factor must be enforced on every state-changing action.
* Liquidation must be permissionless and financially incentivized.
  **Tasks**
* Define accounts: `Market`, `Reserve`(borrow vault), `CollateralVault`, `Obligation`(user position).
* Implement instructions:

  * `deposit_collateral`
  * `withdraw_collateral` (health check)
  * `borrow`
  * `repay`
  * `liquidate` (bonus to liquidator)
* Implement interest (optional but recommended): simple per-second accrual using an index.
* Add risk params: `ltv_bps`, `liquidation_threshold_bps`, `liq_bonus_bps`.
* Tests: borrow at max LTV, price drop triggers liquidation, partial liquidation, repay restores health.

---

### Definition of “Done” (for every project)

* ✅ Full Anchor tests covering happy paths + adversarial inputs
* ✅ Documented account model + invariants
* ✅ Deterministic math with explicit rounding strategy
* ✅ Clear error codes and constraints validation
* ✅ Reproducible local validator workflow (scripts)
