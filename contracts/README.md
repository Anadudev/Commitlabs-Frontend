# CommitLabs Soroban Contracts

Soroban (Rust) smart-contract workspace backing the CommitLabs liquidity commitment protocol. The frontend and Next.js backend service layer (`src/lib/backend/services/contracts.ts`) interact with these contracts via the Stellar Soroban RPC.

## Workspace layout

```text
contracts/
├── Cargo.toml                    # Cargo workspace (members = ["escrow"])
├── escrow/
│   ├── Cargo.toml                # commitlabs-escrow crate (cdylib + rlib)
│   └── src/
│       ├── lib.rs                # EscrowContract implementation
│       └── test.rs               # Unit tests (cfg(test))
└── scripts/
    ├── deploy-testnet.sh         # Build + deploy + initialize helper
    └── deploy-testnet.smoke.mjs  # Dry-run smoke validation
```

## Escrow lifecycle

The escrow contract manages the on-chain lifecycle of a liquidity commitment. Assets are deposited under a chosen risk profile and held in escrow until the commitment matures, is exited early, or is disputed.

### Security: Checks-Effects-Interactions

To prevent reentrancy and similar vulnerabilities when interacting with external tokens, the escrow contract enforces the **Checks-Effects-Interactions** pattern. Specifically, within operations that transfer tokens (`release`, `refund`, and `resolve_dispute`):

1. **Checks**: Validate caller authorization, commitment status, and ledger time.
2. **Effects**: Update the commitment state and persist it to storage.
3. **Interactions**: Perform cross-contract calls to the asset's token contract.

This ordering guarantees contract state is fully resolved before control is handed to external logic.

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

```text
create_commitment ──► fund_escrow ──► release
                   └──► refund
                   └──► dispute ──► resolve_dispute
```

### Marketplace transfer flow

`transfer_ownership(commitment_id, new_owner)` updates ownership for a **funded** commitment.

1. Marketplace buyer proposes `new_owner`.
2. The current commitment owner calls `transfer_ownership` and authorizes it.
3. The contract verifies the commitment is `Funded`.
4. The contract updates ownership and owner indexes.
5. The commitment remains eligible for later lifecycle actions under the new owner.

### Public functions

| Function | Description |
| --- | --- |
| `initialize(admin, token, fee_recipient, safe_default_penalty_bps, balanced_default_penalty_bps, aggressive_default_penalty_bps)` | One-time setup of admin, escrow token (SAC), fee recipient, and default penalties for each risk profile. |
| `create_commitment(owner, asset, amount, risk, duration_days, penalty_bps)` | Create an unfunded commitment with explicit penalty; returns its `id`. |
| `create_commitment_default(owner, asset, amount, risk, duration_days)` | Create an unfunded commitment using the default penalty for the risk profile; returns its `id`. |
| `fund_escrow(commitment_id)` | Transfer `amount` from owner into the contract (`Created → Funded`). |
| `deposit_yield_pool(admin, amount)` | Admin-only deposit of yield tokens into the contract yield pool. |
| `get_yield_pool_balance()` | Read the yield pool balance available for matured release payouts. |
| `release(commitment_id, caller)` | Return principal plus accrued yield to owner once matured (`Funded → Released`). |
| `settle_commitment(commitment_id, caller)` | Alias for `release` that returns a settlement result matching backend ABI expectations. |
| `refund(commitment_id)` | Early-exit refund of principal minus `penalty_bps` (`Funded → Refunded`). |
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

### Attestation history

Compliance scores recorded via `record_attestation` are appended to an on-chain historical log. Use `get_attestations` to retrieve the full timeline.

### `early_exit_commitment` entrypoint

ABI signature:

```rust
pub fn early_exit_commitment(env: Env, commitment_id: u64, caller: Address) -> Result
```

Returned `EarlyExitResult` fields:

- `exitAmount` (`i128`)
- `penaltyAmount` (`i128`)
- `finalStatus` (`EscrowStatus`)

### Grace period behavior

If a funded commitment is refunded within the configured grace period before maturity, the early-exit penalty is waived and the full principal is returned.

## Yield model

Matured `release` payouts return locked principal plus accrued yield. Current annualized rates:

- `Safe`: 5.00%
- `Balanced`: 7.00%
- `Aggressive`: 10.00%

Yield is funded via `deposit_yield_pool(admin, amount)`.

### Risk profiles and penalties

`RiskProfile` is `Safe | Balanced | Aggressive`, matching the frontend `CommitmentType`.

Default penalties are configured once at initialization and automatically applied
to commitments created via `create_commitment_default()`. This
simplifies commitment creation when consistent penalty tiers are desired.

Upper-bound limits enforced in `create_commitment`:

The contract defaults match the CommitLabs backend tier structure:

| Risk Profile | Default Penalty | Basis Points | Use Case |
| --- | --- | --- | --- |
| Safe | 2% | 200 | Low-risk commitments with minimal early-exit cost |
| Balanced | 3% | 300 | Medium-risk commitments with moderate early-exit cost |
| Aggressive | 5% | 500 | High-risk commitments with significant early-exit cost |

#### Two API patterns

The contract provides two ways to create commitments:

1. **Explicit penalty** (`create_commitment`): Set a specific penalty per commitment
   - Allows per-commitment customization
   - Overrides default if needed
   - Useful for custom deal terms

2. **Default penalty** (`create_commitment_default`): Use the profile default
   - Simplifies API calls
   - Ensures consistency across commitments
   - No penalty parameter needed

Example:
```rust
// Use default penalty (e.g., 3% for Balanced risk)
let id = contract.create_commitment_default(
    &owner, &asset, &1000, &RiskProfile::Balanced, &30
)?;

// Or override with custom penalty (e.g., 2% instead of default 3%)
let id = contract.create_commitment(
    &owner, &asset, &1000, &RiskProfile::Balanced, &30, &200
)?;
```

#### Querying defaults

Use `get_default_penalty(risk)` to retrieve the current default for a risk profile.
Useful for frontend/backend UI and verification.

### Dispute categorization & reason storage

When a commitment is disputed via `dispute(commitment_id, caller, reason)`, the
contract automatically categorizes the reason string into a `DisputeReason` enum
using keyword matching. This enables efficient on-chain classification and 
off-chain indexing of disputes.

#### DisputeReason categories

| Category | Keywords | Example |
| --- | --- | --- |
| `ValueMismatch` | value, mismatch, amount, delivered | "actual value delivered was less than promised" |
| `NonCompliance` | compliance, attestation, failed, violation | "compliance violation detected" |
| `FraudSuspicion` | fraud, unauthorized, suspicious | "suspected fraudulent activity" |
| `OperationalFailure` | operational, failure, delivery | "operational failure in delivery" |
| `Other` | (default) | "some other unclassified reason" |

#### Dispute record structure

Each disputed commitment stores a `DisputeRecord` containing:
- `reason_category`: The `DisputeReason` enum value (0–4)
- `reason_text`: The free-form reason string provided by the initiator (for audit)
- `disputed_at`: Ledger timestamp when the dispute was opened
- `disputed_by`: Address that initiated the dispute (owner or admin)

The dispute record is persisted on-chain and can be read at any time via
`get_dispute(commitment_id)`, even after the dispute is resolved. This enables
auditing, analytics, and off-chain verification of dispute history.

### Errors

Stable contract error codes are surfaced for backend mapping, including `AlreadyInitialized`, `NotInitialized`, `NotFound`, `Unauthorized`, `InvalidAmount`, `InvalidState`, `NotMatured`, `InvalidDuration`, `PenaltyTooHigh`, `Paused`, `AssetMismatch`, `InsufficientYieldPool`, `InvalidWasmHash`, and `CommitmentViolated`.

## Testnet deploy flow

This repository now includes a scripted testnet deploy path for the escrow contract.

### What the script does

`contracts/scripts/deploy-testnet.sh`:

1. Builds from `contracts/Cargo.toml` using `stellar contract build`
2. Deploys the compiled WASM to Stellar testnet
3. Invokes `initialize(admin, token, fee_recipient)`
4. Upserts the resulting contract id into the frontend env file

The script updates:

- `NEXT_PUBLIC_COMMITMENT_CORE_CONTRACT`
- `COMMITMENT_CORE_CONTRACT`
- `SOROBAN_COMMITMENT_CORE_CONTRACT`

This keeps the deployed address aligned with `src/lib/backend/config.ts` and `src/lib/backend/services/contracts.ts`.

### Required environment variables

| Variable | Purpose |
| --- | --- |
| `STELLAR_ACCOUNT` | CLI source account used for build/deploy/invoke signing. Prefer an identity alias or secure storage-backed signer. |
| `COMMITLABS_ADMIN_ADDRESS` | Admin `G...` address passed to `initialize` |
| `COMMITLABS_TOKEN_CONTRACT_ID` | Token `C...` contract id passed to `initialize` |
| `COMMITLABS_FEE_RECIPIENT_ADDRESS` | Fee recipient `G...` address passed to `initialize` |

Optional overrides:

- `STELLAR_RPC_URL`
- `STELLAR_NETWORK_PASSPHRASE`
- `COMMITLABS_ENV_FILE`
- `COMMITLABS_CONTRACT_MANIFEST`
- `COMMITLABS_CONTRACT_PACKAGE`
- `COMMITLABS_WASM_PATH`
- `COMMITLABS_CONTRACT_ALIAS`
- `DRY_RUN`

### Usage

Dry run:

```bash
DRY_RUN=1 \
STELLAR_ACCOUNT=deployer \
COMMITLABS_ADMIN_ADDRESS=G... \
COMMITLABS_TOKEN_CONTRACT_ID=C... \
COMMITLABS_FEE_RECIPIENT_ADDRESS=G... \
./contracts/scripts/deploy-testnet.sh
```

Real testnet deploy:

```bash
STELLAR_ACCOUNT=deployer \
COMMITLABS_ADMIN_ADDRESS=G... \
COMMITLABS_TOKEN_CONTRACT_ID=C... \
COMMITLABS_FEE_RECIPIENT_ADDRESS=G... \
./contracts/scripts/deploy-testnet.sh
```

### Security notes

- Keep secrets out of the script and source control; export them only in your shell session.
- The script never writes secret material into `.env.local`.
- Review the target env file before committing anything.

### Verification

Run:

```bash
npm run test:contracts:deploy
```

This dry-run smoke check validates the env-file upsert behavior and the missing-input guardrails without requiring a live deployer account.

## Build and test

Requires the `stellar` CLI and the `wasm32v1-none` / `wasm32-unknown-unknown` targets.

```bash
# from contracts/
cargo test
stellar contract build
```

## Continuous integration

The contracts CI validates contract tests and WebAssembly build output on pushes and pull requests touching the contract workspace.
