#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env, String,
};

/// Spins up a test environment with a Stellar Asset Contract token and a
/// deployed, initialized escrow contract. Returns the pieces tests need.
struct Fixture<'a> {
    env: Env,
    client: EscrowContractClient<'a>,
    token: TokenClient<'a>,
    token_admin: StellarAssetClient<'a>,
    admin: Address,
    fee_recipient: Address,
    asset: Address,
}

fn setup<'a>() -> Fixture<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);

    // Deploy a SAC token to use as the escrow asset.
    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let asset = sac.address();
    let token = TokenClient::new(&env, &asset);
    let token_admin = StellarAssetClient::new(&env, &asset);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &asset, &fee_recipient);

    Fixture {
        env,
        client,
        token,
        token_admin,
        admin,
        fee_recipient,
        asset,
    }
}

fn fund_owner(f: &Fixture, owner: &Address, amount: i128) {
    f.token_admin.mint(owner, &amount);
}

#[test]
fn initialize_is_one_time() {
    let f = setup();
    let other = Address::generate(&f.env);
    let res = f
        .client
        .try_initialize(&f.admin, &f.asset, &other);
    assert_eq!(res, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn create_and_fund_locks_funds() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    let c = f.client.get_commitment(&id);
    assert_eq!(c.status, EscrowStatus::Created);
    assert_eq!(c.amount, 1_000);

    f.client.fund_escrow(&id);
    assert_eq!(f.token.balance(&owner), 0);
    assert_eq!(f.client.get_commitment(&id).status, EscrowStatus::Funded);
}

#[test]
fn release_after_maturity_returns_principal() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Safe, &10, &200);
    f.client.fund_escrow(&id);

    // Advance ledger time past maturity.
    f.env.ledger().set_timestamp(11 * 86_400);
    let paid = f.client.release(&id, &owner);
    assert_eq!(paid, 1_000);
    assert_eq!(f.token.balance(&owner), 1_000);
    assert_eq!(f.client.get_commitment(&id).status, EscrowStatus::Released);
}

#[test]
fn release_before_maturity_fails() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Safe, &10, &200);
    f.client.fund_escrow(&id);

    let res = f.client.try_release(&id, &owner);
    assert_eq!(res, Err(Ok(Error::NotMatured)));
}

#[test]
fn refund_applies_penalty_to_fee_recipient() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    // 5% penalty.
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Aggressive, &30, &500);
    f.client.fund_escrow(&id);

    let refunded = f.client.refund(&id);
    assert_eq!(refunded, 950);
    assert_eq!(f.token.balance(&owner), 950);
    assert_eq!(f.token.balance(&f.fee_recipient), 50);
    assert_eq!(f.client.get_commitment(&id).status, EscrowStatus::Refunded);
}

#[test]
fn dispute_freezes_then_admin_resolves() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client
        .dispute(&id, &owner, &String::from_str(&f.env, "value mismatch"));
    assert_eq!(f.client.get_commitment(&id).status, EscrowStatus::Disputed);

    // Release/refund are blocked while disputed.
    assert_eq!(
        f.client.try_refund(&id),
        Err(Ok(Error::InvalidState))
    );

    let paid = f.client.resolve_dispute(&id, &true);
    assert_eq!(paid, 1_000);
    assert_eq!(f.token.balance(&owner), 1_000);
}

#[test]
fn create_rejects_invalid_amount() {
    let f = setup();
    let owner = Address::generate(&f.env);
    let res =
        f.client
            .try_create_commitment(&owner, &f.asset, &0, &RiskProfile::Safe, &30, &200);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn create_rejects_excessive_penalty() {
    let f = setup();
    let owner = Address::generate(&f.env);
    let res = f.client.try_create_commitment(
        &owner,
        &f.asset,
        &1_000,
        &RiskProfile::Safe,
        &30,
        &20_000,
    );
    assert_eq!(res, Err(Ok(Error::PenaltyTooHigh)));
}

#[test]
fn record_attestation_clamps_score() {
    let f = setup();
    let owner = Address::generate(&f.env);
    let attestor = Address::generate(&f.env);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.record_attestation(&id, &attestor, &250);
    assert_eq!(f.client.get_commitment(&id).compliance_score, 100);
}

#[test]
fn owner_index_tracks_commitments() {
    let f = setup();
    let owner = Address::generate(&f.env);
    let a = f
        .client
        .create_commitment(&owner, &f.asset, &100, &RiskProfile::Safe, &30, &200);
    let b = f
        .client
        .create_commitment(&owner, &f.asset, &200, &RiskProfile::Balanced, &30, &300);
    let ids = f.client.get_owner_commitments(&owner);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids.get(0).unwrap(), a);
    assert_eq!(ids.get(1).unwrap(), b);
}

#[test]
fn dispute_categorizes_value_mismatch() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    // Test value mismatch keyword detection.
    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "actual value delivered was less than promised"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::ValueMismatch);
    assert_eq!(record.disputed_by, owner);
}

#[test]
fn dispute_categorizes_non_compliance() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "compliance violation detected"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::NonCompliance);
}

#[test]
fn dispute_categorizes_fraud_suspicion() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "suspicious fraud activity detected"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::FraudSuspicion);
}

#[test]
fn dispute_categorizes_operational_failure() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "operational failure in delivery"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::OperationalFailure);
}

#[test]
fn dispute_categorizes_other_reason() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "some unspecified reason"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::Other);
}

#[test]
fn get_dispute_returns_persisted_reason_text() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    let reason_text = String::from_str(&f.env, "detailed explanation of the issue");
    f.client.dispute(&id, &owner, &reason_text);

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_text, reason_text);
}

#[test]
fn dispute_stores_timestamp_and_initiator() {
    let f = setup();
    let owner = Address::generate(&f.env);
    let initiator = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    let initial_timestamp = f.env.ledger().timestamp();
    f.client.dispute(
        &id,
        &initiator,
        &String::from_str(&f.env, "value mismatch"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.disputed_by, initiator);
    assert!(record.disputed_at >= initial_timestamp);
}

#[test]
fn get_dispute_returns_none_for_undisputed_commitment() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_none());
}

#[test]
fn admin_can_open_dispute() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    // Admin initiates dispute.
    f.client.dispute(
        &id,
        &f.admin,
        &String::from_str(&f.env, "value mismatch"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.disputed_by, f.admin);
}

#[test]
fn dispute_reason_case_insensitive() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    f.client.dispute(
        &id,
        &owner,
        &String::from_str(&f.env, "COMPLIANCE VIOLATION DETECTED"),
    );

    let dispute = f.client.get_dispute(&id);
    assert!(dispute.is_some());
    let record = dispute.unwrap();
    assert_eq!(record.reason_category, DisputeReason::NonCompliance);
}

#[test]
fn resolve_dispute_preserves_dispute_record() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);
    let id = f
        .client
        .create_commitment(&owner, &f.asset, &1_000, &RiskProfile::Balanced, &30, &300);
    f.client.fund_escrow(&id);

    let reason = String::from_str(&f.env, "value mismatch issue");
    f.client.dispute(&id, &owner, &reason);

    // Dispute record should exist before resolution.
    let dispute_before = f.client.get_dispute(&id);
    assert!(dispute_before.is_some());

    // Admin resolves the dispute.
    f.client.resolve_dispute(&id, &true);

    // Dispute record should still be accessible after resolution.
    let dispute_after = f.client.get_dispute(&id);
    assert!(dispute_after.is_some());
    let record = dispute_after.unwrap();
    assert_eq!(record.reason_text, reason);
    assert_eq!(record.reason_category, DisputeReason::ValueMismatch);
}
