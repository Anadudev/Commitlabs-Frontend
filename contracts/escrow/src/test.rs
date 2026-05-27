#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Bytes, BytesN, Env, String,
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
fn upgrade_succeeds_for_admin() {
    let f = setup();
    let wasm_bytes = Bytes::from_array(&f.env, &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    // Use the hash of the empty-wasm placeholder already present in the
    // test ledger (sha256 of empty string). This ensures the hash exists in
    // ledger so `update_current_contract_wasm` can succeed in the host.
    let new_hash = BytesN::from_array(
        &f.env,
        &[
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ],
    );
    let res = f.client.try_upgrade(&new_hash);
    assert_eq!(res, Ok(Ok(())));
}

#[test]
fn upgrade_rejects_zero_hash() {
    let f = setup();
    let zero_hash = BytesN::from_array(&f.env, &[0u8; 32]);
    let res = f.client.try_upgrade(&zero_hash);
    assert_eq!(res, Err(Ok(Error::InvalidWasmHash)));
}

#[test]
fn upgrade_rejects_when_admin_missing() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);

    let hash = BytesN::from_array(&env, &[2u8; 32]);
    let res = client.try_upgrade(&hash);
    assert_eq!(res, Err(Ok(Error::NotInitialized)));
}

#[test]
fn upgrade_rejects_without_admin_auth() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Admin, &admin);
    });
    // Avoid uploading a wasm blob in this test host; use a precomputed hash.
    let hash = BytesN::from_array(&env, &[3u8; 32]);
    let res = client.try_upgrade(&hash);
    assert!(res.is_err(), "expected unauthorized upgrade to be rejected");
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
