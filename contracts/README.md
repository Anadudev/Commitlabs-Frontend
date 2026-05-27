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
| `initialize(admin, token, fee_recipient)` | One-time setup of admin, escrow token (SAC) and penalty fee recipient. |
`create_commitment(owner, asset, amount, risk, duration_days, penalty_bps)` | Create an unfunded commitment; returns its `id`.

Overflow behavior: `duration_days` is converted into an absolute maturity timestamp using checked arithmetic. If the conversion overflows, the call fails with `InvalidDuration`. |

| `fund_escrow(commitment_id)` | Transfer `amount` from owner into the contract (`Created → Funded`). |
| `transfer_ownership(commitment_id, new_owner)` | Transfer marketplace ownership for secondary trading (`Funded` only). Current owner must authorize and the contract updates both `Commitment.owner` and `OwnerIndex`. |
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

> Note: this workspace is scaffolded to ground the contract issue backlog.
> Verify a local toolchain before deploying to testnet/mainnet.
