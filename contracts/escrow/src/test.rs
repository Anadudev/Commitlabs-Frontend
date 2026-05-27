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
fn transfer_ownership_updates_commitment_and_indices() {
    let f = setup();
    let old_owner = Address::generate(&f.env);
    let new_owner = Address::generate(&f.env);

    fund_owner(&f, &old_owner, 1_000);
    let id = f
        .client
        .create_commitment(&old_owner, &f.asset, &1_000, &RiskProfile::Safe, &30, &200);
    f.client.fund_escrow(&id);

    let before_old = f.client.get_owner_commitments(&old_owner);
    assert_eq!(before_old.len(), 1);
    assert_eq!(before_old.get(0).unwrap(), id);

    f.client.transfer_ownership(&id, &new_owner);

    let c = f.client.get_commitment(&id);
    assert_eq!(c.owner, new_owner);

    let after_old = f.client.get_owner_commitments(&old_owner);
    assert_eq!(after_old.len(), 0);

    let after_new = f.client.get_owner_commitments(&new_owner);
    assert_eq!(after_new.len(), 1);
    assert_eq!(after_new.get(0).unwrap(), id);
}

#[test]
fn transfer_ownership_rejects_non_funded_commitments() {
    let f = setup();
    let old_owner = Address::generate(&f.env);
    let new_owner = Address::generate(&f.env);

    fund_owner(&f, &old_owner, 1_000);
    let id = f
        .client
        .create_commitment(&old_owner, &f.asset, &1_000, &RiskProfile::Safe, &30, &200);
    // Intentionally do NOT call fund_escrow; status remains Created.

    let res = f.client.try_transfer_ownership(&id, &new_owner);
    assert_eq!(res, Err(Ok(Error::InvalidState)));
}


#[test]
fn create_rejects_duration_seconds_overflow() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    // duration_seconds = duration_days * SECONDS_PER_DAY (checked_mul)
    // Force overflow in the multiplication stage.
    let duration_days: u32 = u32::MAX;

    let res = f.client.try_create_commitment(
        &owner,
        &f.asset,
        &1_000,
        &RiskProfile::Safe,
        &duration_days,
        &200,
    );
    assert_eq!(res, Err(Ok(Error::InvalidDuration)));
}

#[test]
fn create_rejects_maturity_timestamp_overflow() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    // Force overflow in the addition stage: maturity = now + duration_seconds.
    // Set now close to u64::MAX, then choose a non-zero duration that will overflow.
    let near_max = u64::MAX - (SECONDS_PER_DAY - 1);
    f.env.ledger().set_timestamp(near_max);

    let duration_days: u32 = 1;
    let res = f.client.try_create_commitment(
        &owner,
        &f.asset,
        &1_000,
        &RiskProfile::Safe,
        &duration_days,
        &200,
    );

    assert_eq!(res, Err(Ok(Error::InvalidDuration)));
}

