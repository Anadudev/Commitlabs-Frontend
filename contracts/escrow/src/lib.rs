#![no_std]
//! CommitLabs Escrow Contract
//!
//! Implements the on-chain escrow lifecycle backing CommitLabs liquidity
//! commitments. A commitment locks a depositor's assets for a fixed duration
//! under a chosen risk profile (Safe / Balanced / Aggressive). Funds are held
//! in escrow until the commitment matures (release), is exited early (refund
//! minus penalty), or is disputed (frozen pending resolution).
//!
//! Lifecycle:
//!   create_commitment -> fund_escrow -> {release | refund | dispute -> resolve_dispute}
//!
//! This contract mirrors the methods the backend service layer
//! (`src/lib/backend/services/contracts.ts`) expects to call: `create_commitment`,
//! `fund_escrow`, `release`, `refund`, and `dispute`.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

/// Storage keys for persistent contract state.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Contract administrator (can resolve disputes, set token).
    Admin,
    /// The token (SAC) address used for all escrow transfers.
    Token,
    /// Monotonic counter used to mint new commitment ids.
    NextId,
    /// A single commitment record keyed by its id.
    Commitment(u64),
    /// List of commitment ids owned by an address.
    OwnerIndex(Address),
    /// Protocol fee recipient.
    FeeRecipient,
    /// Dispute record for a commitment, keyed by commitment id.
    Dispute(u64),
    /// Default penalty in basis points for each RiskProfile.
    DefaultPenalty(RiskProfile),
}

/// Risk profile chosen at creation time. Determines the early-exit penalty
/// applied during `refund`.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiskProfile {
    Safe,
    Balanced,
    Aggressive,
}

/// Lifecycle status of a commitment escrow.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscrowStatus {
    /// Created but not yet funded.
    Created,
    /// Funded and actively held in escrow.
    Funded,
    /// Matured and released to the owner.
    Released,
    /// Exited early; refunded minus penalty.
    Refunded,
    /// Under dispute; transfers are frozen.
    Disputed,
}

/// Categorized dispute reason enum. Enables efficient on-chain classification
/// and off-chain indexing of disputes by category.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisputeReason {
    /// Actual value delivered did not match the promised or agreed value.
    ValueMismatch = 0,
    /// Reported compliance violation or attestation failure.
    NonCompliance = 1,
    /// Suspected fraudulent activity or unauthorized access.
    FraudSuspicion = 2,
    /// Operational failure or delivery failure.
    OperationalFailure = 3,
    /// Other reasons not covered by the above categories.
    Other = 4,
}

/// Dispute record: stores both the categorized reason and the free-form
/// reason string for audit and detailed context.
#[contracttype]
#[derive(Clone)]
pub struct DisputeRecord {
    /// Categorized reason for the dispute.
    pub reason_category: DisputeReason,
    /// Free-form reason string for detailed explanation and audit.
    pub reason_text: String,
    /// Timestamp when the dispute was opened.
    pub disputed_at: u64,
    /// Address that initiated the dispute (owner or admin).
    pub disputed_by: Address,
}

/// A single escrow / commitment record.
#[contracttype]
#[derive(Clone)]
pub struct Commitment {
    pub id: u64,
    pub owner: Address,
    pub asset: Address,
    pub amount: i128,
    pub risk: RiskProfile,
    pub status: EscrowStatus,
    /// Ledger timestamp (seconds) at which the commitment may be released.
    pub maturity: u64,
    /// Early-exit penalty in basis points (e.g. 200 = 2%).
    pub penalty_bps: u32,
    /// Compliance score 0..=100 recorded by the attestation engine.
    pub compliance_score: u32,
    pub created_at: u64,
}

/// Errors returned to the caller. Numeric codes are stable and surfaced by the
/// backend `normalizeContractError` mapper.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotFound = 3,
    Unauthorized = 4,
    InvalidAmount = 5,
    InvalidState = 6,
    NotMatured = 7,
    InvalidDuration = 8,
    PenaltyTooHigh = 9,
}

const MAX_PENALTY_BPS: u32 = 10_000;
const SECONDS_PER_DAY: u64 = 86_400;

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    /// One-time initialization. Sets the admin, the escrow token, fee recipient,
    /// and default penalty rates for each risk profile. Default penalties should
    /// match the risk tier (e.g., Safe 200 bps [2%], Balanced 300 bps [3%],
    /// Aggressive 500 bps [5%]).
    ///
    /// # Arguments
    /// * `admin` - Administrator address (can resolve disputes)
    /// * `token` - Escrow token (SAC) address
    /// * `fee_recipient` - Address that receives early-exit penalties
    /// * `safe_default_penalty_bps` - Default penalty for Safe risk profile (in basis points)
    /// * `balanced_default_penalty_bps` - Default penalty for Balanced risk profile
    /// * `aggressive_default_penalty_bps` - Default penalty for Aggressive risk profile
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        fee_recipient: Address,
        safe_default_penalty_bps: u32,
        balanced_default_penalty_bps: u32,
        aggressive_default_penalty_bps: u32,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();

        // Validate penalty values.
        if safe_default_penalty_bps > MAX_PENALTY_BPS
            || balanced_default_penalty_bps > MAX_PENALTY_BPS
            || aggressive_default_penalty_bps > MAX_PENALTY_BPS
        {
            return Err(Error::PenaltyTooHigh);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage().instance().set(&DataKey::NextId, &0u64);

        // Store default penalties for each risk profile.
        env.storage()
            .instance()
            .set(&DataKey::DefaultPenalty(RiskProfile::Safe), &safe_default_penalty_bps);
        env.storage()
            .instance()
            .set(&DataKey::DefaultPenalty(RiskProfile::Balanced), &balanced_default_penalty_bps);
        env.storage()
            .instance()
            .set(&DataKey::DefaultPenalty(RiskProfile::Aggressive), &aggressive_default_penalty_bps);

        Ok(())
    }

    /// Create a new (unfunded) commitment escrow. Returns the new commitment id.
    ///
    /// `duration_days` is converted to an absolute maturity timestamp using the
    /// current ledger time. `penalty_bps` is the early-exit penalty applied on
    /// `refund`.
    pub fn create_commitment(
        env: Env,
        owner: Address,
        asset: Address,
        amount: i128,
        risk: RiskProfile,
        duration_days: u32,
        penalty_bps: u32,
    ) -> Result<u64, Error> {
        Self::require_init(&env)?;
        owner.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if duration_days == 0 {
            return Err(Error::InvalidDuration);
        }
        if penalty_bps > MAX_PENALTY_BPS {
            return Err(Error::PenaltyTooHigh);
        }

        let id = Self::next_id(&env);
        let now = env.ledger().timestamp();

        // Guard against overflow when converting duration_days into an absolute
        // maturity timestamp. Overflow must never wrap, otherwise commitments
        // could be released/refunded at incorrect times.
        let duration_seconds = (duration_days as u64)
            .checked_mul(SECONDS_PER_DAY)
            .ok_or(Error::InvalidDuration)?;
        let maturity = now
            .checked_add(duration_seconds)
            .ok_or(Error::InvalidDuration)?;

        let commitment = Commitment {
            id,
            owner: owner.clone(),
            asset,
            amount,
            risk,
            status: EscrowStatus::Created,
            maturity,
            penalty_bps,
            compliance_score: 100,
            created_at: now,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Commitment(id), &commitment);
        Self::index_owner(&env, &owner, id);

        env.events().publish(
            (Symbol::new(&env, "create_commitment"), owner),
            (id, amount, maturity),
        );
        Ok(id)
    }

    /// Create a new (unfunded) commitment escrow using the default penalty for
    /// the specified risk profile. Returns the new commitment id.
    ///
    /// This function inherits the penalty_bps from the risk profile defaults
    /// configured at initialization time. If an explicit penalty override is
    /// needed, use `create_commitment()` instead.
    ///
    /// `duration_days` is converted to an absolute maturity timestamp using the
    /// current ledger time.
    pub fn create_commitment_with_default_penalty(
        env: Env,
        owner: Address,
        asset: Address,
        amount: i128,
        risk: RiskProfile,
        duration_days: u32,
    ) -> Result<u64, Error> {
        Self::require_init(&env)?;
        owner.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if duration_days == 0 {
            return Err(Error::InvalidDuration);
        }

        // Retrieve the default penalty for this risk profile.
        let penalty_bps = Self::get_default_penalty_internal(&env, risk)?;

        let id = Self::next_id(&env);
        let now = env.ledger().timestamp();
        let maturity = now + (duration_days as u64) * SECONDS_PER_DAY;

        let commitment = Commitment {
            id,
            owner: owner.clone(),
            asset,
            amount,
            risk,
            status: EscrowStatus::Created,
            maturity,
            penalty_bps,
            compliance_score: 100,
            created_at: now,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Commitment(id), &commitment);
        Self::index_owner(&env, &owner, id);

        env.events().publish(
            (Symbol::new(&env, "create_commitment"), owner),
            (id, amount, maturity),
        );
        Ok(id)
    }

    /// Move tokens from the owner into the contract, transitioning the
    /// commitment from `Created` to `Funded`.
    pub fn fund_escrow(env: Env, commitment_id: u64) -> Result<(), Error> {
        Self::require_init(&env)?;
        let mut c = Self::load(&env, commitment_id)?;
        c.owner.require_auth();

        if c.status != EscrowStatus::Created {
            return Err(Error::InvalidState);
        }

        let token = Self::token_client(&env);
        token.transfer(&c.owner, &env.current_contract_address(), &c.amount);

        c.status = EscrowStatus::Funded;
        Self::save(&env, &c);

        env.events().publish(
            (Symbol::new(&env, "fund_escrow"), c.owner.clone()),
            (commitment_id, c.amount),
        );
        Ok(())
    }

    /// Release the escrowed funds back to the owner once the commitment has
    /// matured. Only callable on a `Funded` commitment at/after maturity.
    pub fn release(env: Env, commitment_id: u64, caller: Address) -> Result<i128, Error> {
        Self::require_init(&env)?;
        caller.require_auth();
        let mut c = Self::load(&env, commitment_id)?;

        if c.status != EscrowStatus::Funded {
            return Err(Error::InvalidState);
        }
        if env.ledger().timestamp() < c.maturity {
            return Err(Error::NotMatured);
        }

        let token = Self::token_client(&env);
        token.transfer(&env.current_contract_address(), &c.owner, &c.amount);

        c.status = EscrowStatus::Released;
        Self::save(&env, &c);

        env.events().publish(
            (Symbol::new(&env, "release"), c.owner.clone()),
            (commitment_id, c.amount),
        );
        Ok(c.amount)
    }

    /// Early-exit refund. Returns the principal minus the early-exit penalty;
    /// the penalty is sent to the fee recipient. Only the owner may refund and
    /// only while the commitment is `Funded` and before maturity.
    pub fn refund(env: Env, commitment_id: u64) -> Result<i128, Error> {
        Self::require_init(&env)?;
        let mut c = Self::load(&env, commitment_id)?;
        c.owner.require_auth();

        if c.status != EscrowStatus::Funded {
            return Err(Error::InvalidState);
        }

        let penalty = (c.amount * c.penalty_bps as i128) / MAX_PENALTY_BPS as i128;
        let refund_amount = c.amount - penalty;

        let token = Self::token_client(&env);
        let contract = env.current_contract_address();
        if penalty > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .ok_or(Error::NotInitialized)?;
            token.transfer(&contract, &fee_recipient, &penalty);
        }
        token.transfer(&contract, &c.owner, &refund_amount);

        c.status = EscrowStatus::Refunded;
        Self::save(&env, &c);

        env.events().publish(
            (Symbol::new(&env, "refund"), c.owner.clone()),
            (commitment_id, refund_amount, penalty),
        );
        Ok(refund_amount)
    }

    /// Flag a funded commitment as disputed, freezing release/refund until an
    /// admin resolves it. Either the owner or the admin may open a dispute.
    /// The reason string is automatically categorized based on keywords.
    pub fn dispute(env: Env, commitment_id: u64, caller: Address, reason: String) -> Result<(), Error> {
        Self::require_init(&env)?;
        caller.require_auth();
        let mut c = Self::load(&env, commitment_id)?;

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        if caller != c.owner && caller != admin {
            return Err(Error::Unauthorized);
        }
        if c.status != EscrowStatus::Funded {
            return Err(Error::InvalidState);
        }

        // Categorize the dispute reason based on keywords in the reason string.
        let reason_category = Self::categorize_dispute_reason(&reason);
        let now = env.ledger().timestamp();

        let dispute_record = DisputeRecord {
            reason_category,
            reason_text: reason.clone(),
            disputed_at: now,
            disputed_by: caller.clone(),
        };

        c.status = EscrowStatus::Disputed;
        Self::save(&env, &c);

        // Persist the dispute record.
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(commitment_id), &dispute_record);

        env.events().publish(
            (Symbol::new(&env, "dispute"), caller),
            (commitment_id, reason_category as u32, reason),
        );
        Ok(())
    }

    /// Admin-only resolution of a dispute. `release_to_owner = true` pays the
    /// owner the full principal; `false` refunds principal minus penalty.
    pub fn resolve_dispute(
        env: Env,
        commitment_id: u64,
        release_to_owner: bool,
    ) -> Result<i128, Error> {
        Self::require_init(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let mut c = Self::load(&env, commitment_id)?;
        if c.status != EscrowStatus::Disputed {
            return Err(Error::InvalidState);
        }

        let token = Self::token_client(&env);
        let contract = env.current_contract_address();
        let paid;
        if release_to_owner {
            token.transfer(&contract, &c.owner, &c.amount);
            c.status = EscrowStatus::Released;
            paid = c.amount;
        } else {
            let penalty = (c.amount * c.penalty_bps as i128) / MAX_PENALTY_BPS as i128;
            paid = c.amount - penalty;
            token.transfer(&contract, &c.owner, &paid);
            c.status = EscrowStatus::Refunded;
        }
        Self::save(&env, &c);

        env.events().publish(
            (Symbol::new(&env, "resolve_dispute"), admin),
            (commitment_id, release_to_owner, paid),
        );
        Ok(paid)
    }

    /// Record a compliance attestation (0..=100) against a commitment. Mirrors
    /// the attestation engine integration used by the backend.
    pub fn record_attestation(
        env: Env,
        commitment_id: u64,
        attestor: Address,
        compliance_score: u32,
    ) -> Result<(), Error> {
        Self::require_init(&env)?;
        attestor.require_auth();
        let mut c = Self::load(&env, commitment_id)?;
        let score = if compliance_score > 100 { 100 } else { compliance_score };
        c.compliance_score = score;
        Self::save(&env, &c);
        env.events().publish(
            (Symbol::new(&env, "record_attestation"), attestor),
            (commitment_id, score),
        );
        Ok(())
    }

    /// Read a single commitment record.
    pub fn get_commitment(env: Env, commitment_id: u64) -> Result<Commitment, Error> {
        Self::load(&env, commitment_id)
    }

    /// Return the list of commitment ids owned by an address.
    pub fn get_owner_commitments(env: Env, owner: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::OwnerIndex(owner))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Retrieve the dispute record for a commitment. Returns `None` if no
    /// dispute has been recorded.
    pub fn get_dispute(env: Env, commitment_id: u64) -> Option<DisputeRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(commitment_id))
    }

    /// Retrieve the default penalty (in basis points) for a specific risk profile.
    /// Configured at initialization time and used by
    /// `create_commitment_with_default_penalty()`. Useful for querying the
    /// current penalty configuration.
    pub fn get_default_penalty(env: Env, risk: RiskProfile) -> Result<u32, Error> {
        env.storage()
            .instance()
            .get(&DataKey::DefaultPenalty(risk))
            .ok_or(Error::NotInitialized)
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    fn require_init(env: &Env) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        Ok(())
    }

    /// Retrieve the default penalty for a risk profile (internal use).
    /// Returns NotInitialized if the contract has not been initialized.
    fn get_default_penalty_internal(env: &Env, risk: RiskProfile) -> Result<u32, Error> {
        env.storage()
            .instance()
            .get(&DataKey::DefaultPenalty(risk))
            .ok_or(Error::NotInitialized)
    }

    fn next_id(env: &Env) -> u64 {
        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(0);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));
        id
    }

    fn load(env: &Env, id: u64) -> Result<Commitment, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Commitment(id))
            .ok_or(Error::NotFound)
    }

    fn save(env: &Env, c: &Commitment) {
        env.storage()
            .persistent()
            .set(&DataKey::Commitment(c.id), c);
    }

    fn index_owner(env: &Env, owner: &Address, id: u64) {
        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::OwnerIndex(owner.clone()))
            .unwrap_or_else(|| Vec::new(env));
        ids.push_back(id);
        env.storage()
            .persistent()
            .set(&DataKey::OwnerIndex(owner.clone()), &ids);
    }

    fn token_client(env: &Env) -> soroban_sdk::token::Client {
        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .expect("token not configured");
        soroban_sdk::token::Client::new(env, &token)
    }

    /// Categorize a free-form dispute reason string into a DisputeReason enum.
    /// Uses keyword matching to detect common dispute categories.
    fn categorize_dispute_reason(reason: &String) -> DisputeReason {
        let reason_lower = reason.to_lowercase();
        
        // Check for keywords in order of specificity.
        if reason_lower.contains("value") || reason_lower.contains("mismatch") 
            || reason_lower.contains("amount") || reason_lower.contains("delivered") {
            DisputeReason::ValueMismatch
        } else if reason_lower.contains("compliance") || reason_lower.contains("attestation")
            || reason_lower.contains("failed") || reason_lower.contains("violation") {
            DisputeReason::NonCompliance
        } else if reason_lower.contains("fraud") || reason_lower.contains("unauthorized")
            || reason_lower.contains("suspicious") || reason_lower.contains("suspicious") {
            DisputeReason::FraudSuspicion
        } else if reason_lower.contains("operational") || reason_lower.contains("failure")
            || reason_lower.contains("delivery") {
            DisputeReason::OperationalFailure
        } else {
            DisputeReason::Other
        }
    }
}

mod test;
