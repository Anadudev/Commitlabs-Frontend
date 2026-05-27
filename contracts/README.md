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

### Lifecycle

```
create_commitment ──► fund_escrow ──► release            (matured: principal back to owner)
                                  └──► refund             (early exit: principal − penalty)
                                  └──► dispute ──► resolve_dispute   (admin adjudication)
```

### Public functions

| Function | Description |
| --- | --- |
| `initialize(admin, token, fee_recipient)` | One-time setup of admin, escrow token (SAC) and penalty fee recipient. |
| `create_commitment(owner, asset, amount, risk, duration_days, penalty_bps)` | Create an unfunded commitment; returns its `id`. |
| `fund_escrow(commitment_id)` | Transfer `amount` from owner into the contract (`Created → Funded`). |
| `release(commitment_id, caller)` | Return principal to owner once matured (`Funded → Released`). |
| `refund(commitment_id)` | Early-exit refund of principal minus `penalty_bps` (`Funded → Refunded`). |
| `dispute(commitment_id, caller, reason)` | Freeze a funded commitment pending admin resolution. |
| `resolve_dispute(commitment_id, release_to_owner)` | Admin-only settlement of a disputed commitment. |
| `record_attestation(commitment_id, attestor, compliance_score)` | Record a 0–100 compliance score. |
| `get_commitment(commitment_id)` | Read a single commitment record. |
| `get_owner_commitments(owner)` | List commitment ids owned by an address. |

### Risk profiles & penalties

`RiskProfile` is `Safe | Balanced | Aggressive`, matching the frontend
`CommitmentType`. The early-exit penalty is supplied at creation time in basis
points (`penalty_bps`, max `10_000`) and is paid to the configured fee
recipient on `refund` / adverse `resolve_dispute`.

### Errors

Stable numeric error codes (`#[contracterror]`) are surfaced so the backend
`normalizeContractError` mapper can translate them into HTTP responses:
`AlreadyInitialized`, `NotInitialized`, `NotFound`, `Unauthorized`,
`InvalidAmount`, `InvalidState`, `NotMatured`, `InvalidDuration`,
`PenaltyTooHigh`.

## Build & test

Requires the `stellar` CLI (v23) and the `wasm32v1-none` / `wasm32-unknown-unknown`
target.

```bash
# from contracts/
cargo test            # run unit tests in escrow/src/test.rs
stellar contract build
```

## Contract upgrade flow

The escrow contract now supports an admin-gated upgrade path through the
`upgrade(new_wasm_hash: BytesN<32>)` entrypoint. Only the configured admin
stored at `DataKey::Admin` may authorize upgrades.

1. Build the updated contract WASM:

```bash
cd contracts/escrow
cargo build --release --target wasm32-unknown-unknown
```

2. Compute the WASM hash from the built artifact. For example:

```bash
wasm-util hash target/wasm32-unknown-unknown/release/commitlabs-escrow.wasm
```

3. Call the upgrade entrypoint with the new hash. The transaction must be
   signed by the admin address currently stored in the escrow contract.

4. Confirm the `upgrade` event was emitted and the contract continues to
   operate normally.

### Security model

- `DataKey::Admin` is the single upgrade authority stored in contract state.
- `upgrade` reads the admin address from storage and calls
  `admin.require_auth()` before any state changes.
- Only an admin-signed transaction may proceed to `env.deployer().update_current_contract_wasm`.
- A zero-valued WASM hash is rejected as invalid and returns `InvalidWasmHash`.
- If admin storage is missing, the call returns `NotInitialized`.

### Safe upgrade notes

- Verify the new WASM hash against your reproducible build output.
- Do not issue upgrades from untrusted tooling or without an audit trail.
- The contract storage remains intact across upgrades; only the implementation changes.
- Keep the admin key securely guarded and use this upgrade path sparingly.

> Note: the upgrade entrypoint is intentionally minimal to reduce attack surface.

> Note: this documentation describes the full operational flow for the new admin-gated upgrade pattern.
