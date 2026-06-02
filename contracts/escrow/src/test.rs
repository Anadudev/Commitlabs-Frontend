#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env, Map, String, Symbol, TryFromVal, Val, Vec,
};

struct Fixture<'a> {
    env: Env,
    client: EscrowContractClient<'a>,
    token: TokenClient<'a>,
    token_admin: StellarAssetClient<'a>,
    admin: Address,
    fee_recipient: Address,
    asset: Address,
    contract_id: Address,
}

fn setup<'a>() -> Fixture<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let asset = sac.address();
    let token = TokenClient::new(&env, &asset);
    let token_admin = StellarAssetClient::new(&env, &asset);

    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &asset, &fee_recipient, &200u32, &300u32, &500u32);

    Fixture {
        env,
        client,
        token,
        token_admin,
        admin,
        fee_recipient,
        asset,
        contract_id,
    }
}

fn fund_owner(f: &Fixture, owner: &Address, amount: i128) {
    f.token_admin.mint(owner, &amount);
}

fn metadata(env: &Env) -> Map<String, String> {
    let mut metadata = Map::new(env);
    metadata.set(
        String::from_str(env, "source"),
        String::from_str(env, "issue-463"),
    );
    metadata
}

fn assert_contract_event<D>(
    env: &Env,
    contract_id: &Address,
    event_name: &str,
    owner: &Address,
    commitment_id: u64,
    expected_data: D,
) where
    D: TryFromVal<Env, Val> + PartialEq + core::fmt::Debug,
{
    let events = env.events().all();
    let expected_event = Symbol::new(env, event_name);

    let mut index: u32 = 0;
    while index < events.len() {
        let (event_contract, topics, data): (Address, Vec<Val>, Val) = events.get(index).unwrap();
        if event_contract != *contract_id {
            index += 1;
            continue;
        }

        if topics.len() != 3 {
            index += 1;
            continue;
        }

        let actual_event =
            Symbol::try_from_val(env, &topics.get(0).unwrap()).expect("event name topic");
        let actual_owner =
            Address::try_from_val(env, &topics.get(1).unwrap()).expect("owner topic");
        let actual_commitment_id =
            u64::try_from_val(env, &topics.get(2).unwrap()).expect("commitment id topic");

        if actual_event == expected_event
            && actual_owner == *owner
            && actual_commitment_id == commitment_id
        {
            let actual_data =
                D::try_from_val(env, &data).expect("event payload should decode into expected type");
            assert_eq!(actual_data, expected_data, "event payload mismatch");
            return;
        }

        index += 1;
    }

    panic!("expected contract event was not emitted");
}

#[test]
fn create_commitment_emits_stable_indexable_event() {
    let f = setup();
    let owner = Address::generate(&f.env);

    let id = f.client.create_commitment(
        &owner,
        &f.asset,
        &1_000i128,
        &RiskProfile::Balanced,
        &30u32,
        &300u32,
        &metadata(&f.env),
    );

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "create_commitment",
        &owner,
        id,
        CreateCommitmentEventData {
            asset: f.asset.clone(),
            amount: 1_000,
            risk: RiskProfile::Balanced,
            maturity: 30 * 86_400,
            penalty_bps: 300,
        },
    );
}

#[test]
fn default_penalty_creation_keeps_create_commitment_event_name() {
    let f = setup();
    let owner = Address::generate(&f.env);

    let id = f.client.create_commitment_with_default(
        &owner,
        &f.asset,
        &2_000i128,
        &RiskProfile::Safe,
        &15u32,
    );

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "create_commitment",
        &owner,
        id,
        CreateCommitmentEventData {
            asset: f.asset.clone(),
            amount: 2_000,
            risk: RiskProfile::Safe,
            maturity: 15 * 86_400,
            penalty_bps: 200,
        },
    );
}

#[test]
fn fund_escrow_emits_stable_indexable_event() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    let id = f.client.create_commitment(
        &owner,
        &f.asset,
        &1_000i128,
        &RiskProfile::Balanced,
        &30u32,
        &300u32,
        &metadata(&f.env),
    );
    f.client.fund_escrow(&id);

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "fund_escrow",
        &owner,
        id,
        FundEscrowEventData {
            asset: f.asset.clone(),
            amount: 1_000,
            risk: RiskProfile::Balanced,
        },
    );
    assert_eq!(f.token.balance(&owner), 0);
}

#[test]
fn release_emits_stable_indexable_event() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    let id = f.client.create_commitment(
        &owner,
        &f.asset,
        &1_000i128,
        &RiskProfile::Aggressive,
        &10u32,
        &500u32,
        &metadata(&f.env),
    );
    let commitment = f.client.get_commitment(&id);
    f.client.fund_escrow(&id);

    f.token_admin.mint(&f.admin, &commitment.accrued_yield);
    f.client
        .deposit_yield_pool(&f.admin, &commitment.accrued_yield);
    f.env.ledger().set_timestamp(commitment.maturity);

    let payout = f.client.release(&id);
    assert_eq!(payout, commitment.amount + commitment.accrued_yield);

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "release",
        &owner,
        id,
        ReleaseEventData {
            asset: f.asset.clone(),
            amount: commitment.amount,
            accrued_yield: commitment.accrued_yield,
            payout,
            risk: RiskProfile::Aggressive,
        },
    );
}

#[test]
fn refund_emits_stable_indexable_event() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    let id = f.client.create_commitment(
        &owner,
        &f.asset,
        &1_000i128,
        &RiskProfile::Aggressive,
        &30u32,
        &500u32,
        &metadata(&f.env),
    );
    f.client.fund_escrow(&id);

    let refunded_amount = f.client.refund(&id);
    assert_eq!(refunded_amount, 950);

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "refund",
        &owner,
        id,
        RefundEventData {
            asset: f.asset.clone(),
            amount: 1_000,
            refunded_amount: 950,
            penalty: 50,
            risk: RiskProfile::Aggressive,
        },
    );
    assert_eq!(f.token.balance(&f.fee_recipient), 50);
}

#[test]
fn dispute_emits_stable_indexable_event() {
    let f = setup();
    let owner = Address::generate(&f.env);
    fund_owner(&f, &owner, 1_000);

    let id = f.client.create_commitment(
        &owner,
        &f.asset,
        &1_000i128,
        &RiskProfile::Balanced,
        &30u32,
        &300u32,
        &metadata(&f.env),
    );
    f.client.fund_escrow(&id);

    let reason = String::from_str(&f.env, "value mismatch during settlement");
    f.client.dispute(&id, &owner, &reason);

    assert_contract_event(
        &f.env,
        &f.contract_id,
        "dispute",
        &owner,
        id,
        DisputeEventData {
            asset: f.asset.clone(),
            amount: 1_000,
            risk: RiskProfile::Balanced,
            reason_category: DisputeReason::ValueMismatch,
            reason_text: reason,
            disputed_by: owner.clone(),
        },
    );
}
