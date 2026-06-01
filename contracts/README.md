### Admin and Fee Recipient Rotation

The contract supports secure rotation of the admin and fee recipient addresses after initialization:

| Function | Description |
| --- | --- |
| `set_admin(new_admin)` | Admin-only. Rotates the contract admin to `new_admin`. Emits an event. |
| `set_fee_recipient(new_fee_recipient)` | Admin-only. Rotates the protocol fee recipient. Emits an event. |

Both functions require the current admin to authorize the call. Rotation is rejected if the contract is not initialized. Events are emitted for auditability.
# CommitLabs Soroban Contracts

Soroban (Rust) smart-contract workspace backing the CommitLabs liquidity
commitment protocol. The frontend and Next.js backend service layer
(`src/lib/backend/services/contracts.ts`) interact with these contracts via the
Stellar Soroban RPC.

## Workspace layout

```
contracts/
├── Cargo.toml          # Cargo workspace (members = ["escrow"])
└── escrow/
    ├── Cargo.toml      # commitlabs-escrow crate (cdylib + rlib)
    └── src/
        ├── lib.rs      # EscrowContract implementation
        └── test.rs     # Unit tests (cfg(test))
```

## `escrow` contract

The escrow contract manages the on-chain lifecycle of a liquidity commitment.
Assets are deposited under a chosen risk profile and held in escrow until the
commitment matures, is exited early, or is disputed.

### Security: Checks-Effects-Interactions

To prevent reentrancy and similar vulnerabilities when interacting with external tokens, the escrow contract enforces the **Checks-Effects-Interactions** pattern. Specifically, within operations that transfer tokens (`release`, `refund`, and `resolve_dispute`):
1. **Checks**: Validate caller authorization, commitment status, and ledger time.
2. **Effects**: Update the commitment state (e.g., transition `Funded` -> `Released` or `Refunded`) and persist it to storage.
3. **Interactions**: Perform cross-contract calls to the asset's token contract.

This strict ordering guarantees the contract's internal state is fully resolved before execution control is temporarily handed over to external logic.

## EscrowStatus State Machine

### States

| State | Description |
|-------|-------------|
| `Created` | Commitment created but not yet funded. Awaiting owner to deposit assets. |
| `Funded` | Assets locked in escrow. Commitment is actively held and can be released, refunded, or disputed. |
| `Released` | Matured and released to the owner. Principal plus accrued yield returned. Terminal state. |
| `Refunded` | Exited early or resolved via dispute. Principal minus penalty returned. Terminal state. |
| `Disputed` | Under dispute; all transfers frozen pending admin resolution. Intermediate state. |
| `Violated` | Compliance score dropped below violation threshold. Transfers frozen until resolved. Intermediate state. |

### Transition Diagram (ASCII)

```
                    ┌─────────────┐
                    │   CREATED   │
                    └──────┬──────┘
                           │ fund_escrow()
                           ▼
                    ┌─────────────┐
                    │   FUNDED    │◄─────────────────────────────┐
                    └──┬──┬──┬────┘                              │
                       │  │  │                                   │
        ┌──────────────┘  │  └──────────────┐                   │
        │                 │                 │                   │
        │ release()       │ refund()        │ dispute()         │
        │ (matured)       │ (early exit)    │ (frozen)          │
        │                 │                 │                   │
        ▼                 ▼                 ▼                   │
    ┌─────────┐      ┌─────────┐      ┌──────────┐             │
    │RELEASED │      │REFUNDED │      │ DISPUTED │             │
    └─────────┘      └─────────┘      └────┬─────┘             │
                                            │                   │
                                            │ resolve_dispute() │
                                            │                   │
                                            └───────────────────┘
                                                (release or refund)

    record_attestation() with low score:
    FUNDED ──────────────────────► VIOLATED ──► resolve_dispute() ──► FUNDED or RELEASED/REFUNDED
```

### Transition Table

| From State | To State | Triggered By | Authorized | Preconditions |
|------------|----------|--------------|-----------|---------------|
| `Created` | `Funded` | `fund_escrow()` | Owner | Owner has sufficient balance; asset matches configured token |
| `Funded` | `Released` | `release()` | Any | Ledger time ≥ maturity; yield pool has sufficient balance |
| `Funded` | `Refunded` | `refund()` | Owner | Before maturity (or within grace period); not violated |
| `Funded` | `Refunded` | `refund_partial()` | Owner | Partial withdrawal; remainder stays funded or becomes refunded |
| `Funded` | `Disputed` | `dispute()` | Owner or Admin | Commitment is funded |
| `Funded` | `Violated` | `record_attestation()` | Attestor | Compliance score < violation threshold |
| `Disputed` | `Released` | `resolve_dispute(release_to_owner=true)` | Admin | Dispute exists; yield pool sufficient if matured |
| `Disputed` | `Refunded` | `resolve_dispute(release_to_owner=false)` | Admin | Dispute exists |
| `Violated` | `Released` | `resolve_dispute(release_to_owner=true)` | Admin | Violation exists; yield pool sufficient if matured |
| `Violated` | `Refunded` | `resolve_dispute(release_to_owner=false)` | Admin | Violation exists |

### Lifecycle

```
create_commitment ──► fund_escrow ──► release            (matured: principal back to owner)
                                  └──► refund             (early exit: principal − penalty)
                                  └──► dispute ──► resolve_dispute   (admin adjudication)
```

## Authorization Matrix

### Role Definitions

| Role | Description | How Verified |
|------|-------------|--------------|
| **Owner** | The address that created or currently owns a commitment. | Stored in `Commitment.owner`; verified via `require_auth()` |
| **Admin** | The contract administrator, set at initialization. | Stored in `DataKey::Admin`; verified via `require_auth()` |
| **Attestor** | Any address authorized to record compliance scores. | Verified via `require_auth()` on `record_attestation()` |
| **Any** | Permissionless; no authorization required. | No `require_auth()` call |

### Entrypoint Authorization

| Entrypoint | Owner | Admin | Attestor | Any | Notes |
|------------|-------|-------|----------|-----|-------|
| `initialize()` | ❌ | ✅ | ❌ | ❌ | One-time setup; admin must authorize |
| `create_commitment()` | ✅ | ❌ | ❌ | ❌ | Owner creates and must authorize |
| `create_commitment_with_default_penalty()` | ✅ | ❌ | ❌ | ❌ | Owner creates and must authorize |
| `fund_escrow()` | ✅ | ❌ | ❌ | ❌ | Owner funds and must authorize |
| `release()` | ❌ | ❌ | ❌ | ✅ | Permissionless post-maturity; funds always go to stored owner |
| `refund()` | ✅ | ❌ | ❌ | ❌ | Owner refunds and must authorize |
| `refund_partial()` | ✅ | ❌ | ❌ | ❌ | Owner refunds and must authorize |
| `early_exit_commitment()` | ✅ | ❌ | ❌ | ❌ | Owner exits and must authorize |
| `dispute()` | ✅ | ✅ | ❌ | ❌ | Owner or admin can open dispute |
| `resolve_dispute()` | ❌ | ✅ | ❌ | ❌ | Admin only; resolves disputes |
| `transfer_ownership()` | ✅ | ❌ | ❌ | ❌ | Current owner must authorize transfer |
| `record_attestation()` | ❌ | ❌ | ✅ | ❌ | Attestor must authorize |
| `deposit_yield_pool()` | ❌ | ✅ | ❌ | ❌ | Admin only; funds yield pool |
| `pause()` | ❌ | ✅ | ❌ | ❌ | Admin only; emergency halt |
| `unpause()` | ❌ | ✅ | ❌ | ❌ | Admin only; resume operations |
| `set_grace_period()` | ❌ | ✅ | ❌ | ❌ | Admin only; configures grace window |
| `set_violation_threshold()` | ❌ | ✅ | ❌ | ❌ | Admin only; configures auto-violation |
| `upgrade()` | ❌ | ✅ | ❌ | ❌ | Admin only; contract upgrade |
| `set_admin()` | ❌ | ✅ | ❌ | ❌ | Current admin only; rotates admin |
| `set_fee_recipient()` | ❌ | ✅ | ❌ | ❌ | Current admin only; rotates fee recipient |

### Read-Only Functions (No Authorization)

| Entrypoint | Description |
|------------|-------------|
| `get_commitment()` | Read a single commitment record |
| `get_owner_commitments()` | List commitment ids owned by an address |
| `get_dispute()` | Read the dispute record for a commitment |
| `get_attestations()` | Retrieve attestation history for a commitment |
| `get_default_penalty()` | Read default penalty for a risk profile |
| `get_grace_period()` | Read the configured grace period |
| `get_violation_threshold()` | Read the configured violation threshold |
| `get_yield_pool_balance()` | Read the yield pool balance |
| `is_paused()` | Read the current paused state |

### Authorization Notes

- **Permissionless Release**: `release()` is intentionally permissionless post-maturity to avoid liveness issues (e.g., owner loses key). Funds always transfer to the stored `Commitment.owner`, preventing fund diversion.
- **Owner Authorization**: Functions that modify a commitment (fund, refund, dispute, transfer) require the owner to sign via `require_auth()`.
- **Admin Authority**: Only the admin can resolve disputes, manage yield pool, pause/unpause, and upgrade the contract.
- **Attestor Authority**: Any address can record compliance attestations if they authorize the call. The attestor address is stored in the `AttestationRecord` for audit purposes.
- **No Multi-Sig**: The contract uses single-signature authorization. Multi-sig is handled at the transaction level by the Stellar network.

### Marketplace transfer flow (secondary trading)

`transfer_ownership(commitment_id, new_owner)` updates ownership for a **funded** commitment.

**Flow**
1. Marketplace buyer proposes `new_owner`.
2. The current commitment owner calls `transfer_ownership` and must authorize via `require_auth()`.
3. The contract verifies the commitment is `Funded` (transfers are blocked for non-funded states).
4. The contract updates:
   - `Commitment.owner`
   - `OwnerIndex` for both `old_owner` and `new_owner`
5. The commitment is now eligible for subsequent `release` / `refund` / dispute handling under the new owner.


### Public functions

| Function | Description |
| --- | --- |
| `initialize(admin, token, fee_recipient, safe_default_penalty_bps, balanced_default_penalty_bps, aggressive_default_penalty_bps)` | One-time setup of admin, escrow token (SAC), fee recipient, and default penalties for each risk profile. |
| `create_commitment(owner, asset, amount, risk, duration_days, penalty_bps)` | Create an unfunded commitment with explicit penalty; returns its `id`. |
| `create_commitment_with_default_penalty(owner, asset, amount, risk, duration_days)` | Create an unfunded commitment using the default penalty for the risk profile; returns its `id`. |
| `fund_escrow(commitment_id)` | Transfer `amount` from owner into the contract (`Created → Funded`). |
| `transfer_ownership(commitment_id, new_owner)` | Transfer marketplace ownership for secondary trading (`Funded` only). Current owner must authorize and the contract updates both `Commitment.owner` and `OwnerIndex`. |
| `release(commitment_id, caller)` | Return principal to owner once matured (`Funded → Released`). |
| `refund(commitment_id)` | Early-exit refund of principal minus `penalty_bps` (`Funded → Refunded`). |
| `dispute(commitment_id, caller, reason)` | Freeze a funded commitment pending admin resolution. |

| `deposit_yield_pool(admin, amount)` | Admin-only deposit of yield tokens into the contract yield pool. |
| `get_yield_pool_balance()` | Read the yield pool balance available for matured release payouts. |
| `release(commitment_id, caller)` | Return principal plus accrued yield to owner once matured (`Funded → Released`). |
| `refund(commitment_id)` | Early-exit refund of principal minus `penalty_bps` (`Funded → Refunded`). |
| `set_grace_period(admin, grace_period_seconds)` | Admin-only configuration of the penalty-free grace window before maturity. |
| `get_grace_period()` | Read the currently configured penalty-free grace period in seconds. |
| `dispute(commitment_id, caller, reason)` | Freeze a funded commitment pending admin resolution. The reason is automatically categorized. |
| `resolve_dispute(commitment_id, release_to_owner)` | Admin-only settlement of a disputed commitment. |
| `get_dispute(commitment_id)` | Read the dispute record for a commitment (category, reason, timestamp, initiator). |
| `get_default_penalty(risk)` | Read the default penalty for a specific risk profile. |
| `record_attestation(commitment_id, attestor, compliance_score)` | Record a 0–100 compliance score. |
| `pause()` | Admin-only emergency pause for write operations. |
| `unpause()` | Admin-only resume for paused contract writes. |
| `is_paused()` | Read the current paused state. |
| `get_commitment(commitment_id)` | Read a single commitment record. |
| `get_owner_commitments(owner)` | List commitment ids owned by an address. |
| `get_attestations(commitment_id)` | Retrieve the timeline of `AttestationRecord`s for a commitment. |
| `refund_partial(commitment_id, amount)` | Partial early-exit: withdraw `amount` from the principal, apply the proportional penalty to that portion, keep the remainder escrowed. |
| `set_violation_threshold(threshold)` | Admin-only. Set the compliance score threshold (0–100) below which a funded commitment is auto-violated. 0 disables auto-violation. |
| `get_violation_threshold()` | Read the current violation threshold. |

### Attestation History

Compliance scores recorded via `record_attestation` are appended to an on-chain historical log. This allows clients to query the timeline of scores for a given commitment rather than just reading the latest value. Use `get_attestations` to retrieve a list of `AttestationRecord` structures, each containing the attestor address, the compliance score, and the timestamp.

### `early_exit_commitment` entrypoint details

#### ABI Signature
```rust
pub fn early_exit_commitment(
    env: Env,
    commitment_id: u64,
    caller: Address,
) -> Result<EarlyExitResult, Error>
```

#### Response Struct Format (`EarlyExitResult`)
When returned from the contract, the result is serialized as a map/object containing:
* **`exitAmount`** (`i128`): The final amount returned to the commitment owner (principal minus penalty).
* **`penaltyAmount`** (`i128`): The penalty fee amount deducted and paid to the fee recipient.
* **`finalStatus`** (`EscrowStatus`): The final status of the commitment (always `Refunded`).

#### Field Descriptions
| Field | Type | Description |
| --- | --- | --- |
| `exitAmount` | `i128` | The absolute quantity of tokens transferred back to the commitment owner. |
| `penaltyAmount` | `i128` | The absolute quantity of tokens transferred to the fee recipient as an early-exit penalty. |
| `finalStatus` | `EscrowStatus` | The post-exit state of the escrow commitment, represented as `Refunded`. |

#### Example Usage
An invocator (e.g., the backend service layer) calls this entrypoint and retrieves the structured receipt:
```typescript
const result = await invokeContractMethod(
  contractId,
  "early_exit_commitment",
  [commitmentId, ownerAddress],
  "write"
);
console.log(`Exit Amount: ${result.exitAmount}, Penalty: ${result.penaltyAmount}`);
```

#### Grace period behavior
The contract supports a configurable penalty-free window before commitment maturity. If a funded commitment is refunded while the ledger time is within the configured grace period before maturity, the early-exit penalty is waived and the full principal is returned.

### Yield model

Matured `release` payouts now return the locked principal plus the commitment's accrued yield. Yield is calculated at commitment creation using a simple annualized model based on the selected `RiskProfile` and the commitment duration.

- `Safe`: 5.00% annualized
- `Balanced`: 7.00% annualized
- `Aggressive`: 10.00% annualized

Yield is funded by the admin through `deposit_yield_pool(admin, amount)`. The contract maintains a dedicated yield pool balance, and a matured release will fail if the pool has insufficient funds to pay the accrued yield.

### Risk profiles & penalties

`RiskProfile` is `Safe | Balanced | Aggressive`, matching the frontend
`CommitmentType`. The early-exit penalty is supplied at creation time in basis
points (`penalty_bps`, max `10_000`) and is paid to the configured fee
recipient on `refund` / adverse `resolve_dispute`.

### Commitment limits

To prevent arithmetic overflow (e.g. during maturity timestamp calculations) and ensure input sanity, the following upper-bound limits are enforced in `create_commitment`:
- **Maximum Amount (`MAX_AMOUNT`)**: `1_000_000_000_000` (1T units)
- **Maximum Duration (`MAX_DURATION_DAYS`)**: `365` days (1 year)
- **Maximum Penalty (`MAX_PENALTY_BPS`)**: `10_000` bps (100%)

Attempts to exceed these limits will return `InvalidAmount` or `InvalidDuration` errors, respectively.


### Errors

Stable numeric error codes (`#[contracterror]`) are surfaced so the backend
`normalizeContractError` mapper can translate them into HTTP responses.

| Code | Variant | Triggered When |
|------|---------|----------------|
| 1 | `AlreadyInitialized` | `initialize()` called more than once |
| 2 | `NotInitialized` | Contract not initialized; admin or token not set |
| 3 | `NotFound` | Commitment id does not exist |
| 4 | `Unauthorized` | Caller not authorized for the operation (e.g., non-owner calling `refund()`) |
| 5 | `InvalidAmount` | Amount is ≤ 0, exceeds `MAX_AMOUNT`, or insufficient balance |
| 6 | `InvalidState` | Commitment in wrong state for the operation (e.g., `refund()` on `Released`) |
| 7 | `NotMatured` | `release()` called before maturity timestamp |
| 8 | `InvalidDuration` | Duration is 0, exceeds `MAX_DURATION_DAYS`, or causes timestamp overflow |
| 9 | `PenaltyTooHigh` | Penalty exceeds `MAX_PENALTY_BPS` (10,000 basis points = 100%) |
| 10 | `Paused` | Contract is paused; write operations blocked |
| 11 | `AssetMismatch` | Commitment asset does not match configured escrow token |
| 12 | `InsufficientYieldPool` | Yield pool balance insufficient to pay matured commitment yield |
| 13 | `InvalidWasmHash` | WASM hash provided for upgrade is zero or invalid |
| 14 | `CommitmentViolated` | Commitment in `Violated` status; release and refund blocked until resolved |

### Error Handling Best Practices

- **InvalidState**: Check commitment status before calling state-transition functions. Use `get_commitment()` to verify current state.
- **NotMatured**: For `release()`, check the commitment's maturity timestamp against the current ledger time.
- **InsufficientYieldPool**: Ensure the admin has deposited sufficient yield via `deposit_yield_pool()` before matured commitments are released.
- **CommitmentViolated**: If a commitment is violated, the admin must call `resolve_dispute()` to transition it back to a usable state.
- **Paused**: If the contract is paused, wait for the admin to call `unpause()` before retrying write operations.

## Keeping This Document in Sync

This README documents the escrow contract's state machine, authorization model, and error codes. It must be updated whenever:

- A new `EscrowStatus` variant is added or removed
- A new public entrypoint is added or removed
- Authorization rules change (e.g., a function becomes admin-only)
- New error codes are added to the `#[contracterror]` enum
- State transitions change (e.g., a function now transitions to a different state)

**Cross-reference**: `contracts/escrow/src/lib.rs` (source of truth for all contract logic)  
**Test coverage**: `contracts/escrow/src/test.rs` (validates state transitions and authorization)

## Build & test

Requires the `stellar` CLI (v23) and the `wasm32v1-none` / `wasm32-unknown-unknown`
target.

```bash
# from contracts/
cargo test            # run unit tests in escrow/src/test.rs
stellar contract build
```

> Note: this workspace is scaffolded to ground the contract issue backlog.
> Verify a local toolchain before deploying to testnet/mainnet.

## Continuous Integration

A GitHub Actions CI workflow is configured in `.github/workflows/contracts.yml`.
On every push and pull request touching the `contracts/` directory or the workflow file, the CI will:
1. Set up the stable Rust toolchain with the `wasm32-unknown-unknown` target.
2. Cache Cargo registries and dependency builds via `Swatinem/rust-cache` to ensure fast execution.
3. Install the required version of the `stellar-cli` (v23.0.0).
4. Run `cargo test --locked` to execute the escrow contract unit tests.
5. Execute `stellar contract build` to verify smart contract compilation to WebAssembly.
