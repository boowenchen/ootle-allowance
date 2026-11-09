use tari_template_test_tooling::{
    TemplateTest,
    byte_type::ToByteType,
    crypto::RistrettoSecretKey,
    engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId},
    support::assert_error::assert_reject_reason,
    template_lib_types::{Amount, ComponentAddress, NonFungibleAddress, ResourceAddress},
    transaction::args,
};
use tari_template_test_tooling::template_lib_types::crypto::RistrettoPublicKeyBytes;

struct Party {
    account: ComponentAddress,
    proof: NonFungibleAddress,
    key: RistrettoSecretKey,
    pk: RistrettoPublicKeyBytes,
}

struct Setup {
    test: TemplateTest,
    owner: Party,
    agent: Party,
    payee: ComponentAddress,
    allowance: ComponentAddress,
    badge: ResourceAddress,
    token: ResourceAddress,
}

fn party(test: &mut TemplateTest) -> Party {
    let (account, proof, key, pk) = test.create_funded_account_with_keypair();
    Party { account, proof, key, pk: pk.to_byte_type() }
}

/// Owner creates a dUSD allowance (cap 100, budget 250, 3 payments per epoch), funds it with 1000,
/// registers one agent and one allowlisted payee.
fn setup() -> Setup {
    let mut test = TemplateTest::new(".", [".", "demo_token"]);
    let owner = party(&mut test);
    let agent = party(&mut test);
    let (payee, _, _) = test.create_empty_account();

    let token_template = test.get_template_address("DemoToken");
    let allowance_template = test.get_template_address("Allowance");

    let res = test.execute_expect_success(
        test.transaction()
            .call_function(token_template, "new", args!["dUSD"])
            .build_and_seal(&owner.key),
        vec![owner.proof.clone()],
    );
    let faucet = find_component(&res, token_template);
    test.execute_expect_success(
        test.transaction().call_method(faucet, "resource", args![]).build_and_seal(&owner.key),
        vec![owner.proof.clone()],
    );
    let token: ResourceAddress = test.expect_return(0);

    let res = test.execute_expect_success(
        test.transaction()
            .call_function(allowance_template, "create", args![token, Amount::new(100), Amount::new(250), 3u32])
            .put_last_instruction_output_on_workspace("badge")
            .call_method(owner.account, "deposit", args![Workspace("badge")])
            .build_and_seal(&owner.key),
        vec![owner.proof.clone()],
    );
    let allowance = find_component(&res, allowance_template);
    let badge: ResourceAddress = test.extract_component_value(allowance, "$.1");

    // Funding is a plain deposit; anyone may do it.
    test.execute_expect_success(
        test.transaction()
            .call_method(faucet, "mint", args![Amount::new(1000)])
            .put_last_instruction_output_on_workspace("funds")
            .call_method(allowance, "fund", args![Workspace("funds")])
            .build_and_seal(&owner.key),
        vec![owner.proof.clone()],
    );

    let mut s = Setup { test, owner, agent, payee, allowance, badge, token };
    let agent_pk = s.agent.pk;
    let payee = s.payee;
    admin(&mut s, "add_agent", args![agent_pk, "research-bot"]);
    admin(&mut s, "allow_payee", args![payee, "inference-api"]);
    s
}

fn find_component(
    res: &tari_template_test_tooling::engine_types::commit_result::ExecuteResult,
    template: tari_template_test_tooling::template_lib_types::TemplateAddress,
) -> ComponentAddress {
    res.finalize
        .result
        .accept()
        .unwrap()
        .up_iter()
        .find(|(id, s)| id.is_component() && *s.substate_value().component().unwrap().template_address() == template)
        .map(|(id, _)| id.as_component_address().unwrap())
        .unwrap()
}

fn admin_tx(
    s: &mut Setup,
    method: &str,
    args: Vec<tari_template_test_tooling::transaction::builder::named_args::NamedArg>,
) -> tari_template_test_tooling::transaction::Transaction {
    s.test
        .transaction()
        .create_proof(s.owner.account, s.badge)
        .put_last_instruction_output_on_workspace("proof")
        .authorize_proof("proof")
        .call_method(s.allowance, method, args)
        .drop_all_proofs_in_workspace()
        .build_and_seal(&s.owner.key)
}

fn admin(s: &mut Setup, method: &str, args: Vec<tari_template_test_tooling::transaction::builder::named_args::NamedArg>) {
    let tx = admin_tx(s, method, args);
    let proof = s.owner.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
}

fn agent_tx(
    s: &mut Setup,
    method: &str,
    args: Vec<tari_template_test_tooling::transaction::builder::named_args::NamedArg>,
) -> tari_template_test_tooling::transaction::Transaction {
    s.test.transaction().call_method(s.allowance, method, args).build_and_seal(&s.agent.key)
}

fn pay(s: &mut Setup, to: ComponentAddress, amount: u64) {
    let tx = agent_tx(s, "pay", args![to, Amount::new(amount as u128), "invoice"]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
}

fn pay_fails(s: &mut Setup, to: ComponentAddress, amount: u64, reason: &str) {
    let tx = agent_tx(s, "pay", args![to, Amount::new(amount as u128), "invoice"]);
    let proof = s.agent.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, reason);
}

fn balance(s: &mut Setup, account: ComponentAddress) -> Amount {
    let token = s.token;
    let tx = s.test.transaction().call_method(account, "balance", args![token]).build_and_seal(&s.owner.key);
    let proof = s.owner.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    s.test.expect_return(0)
}

fn vault_balance(s: &mut Setup) -> Amount {
    let tx = s.test.transaction().call_method(s.allowance, "balance", args![]).build_and_seal(&s.owner.key);
    let proof = s.owner.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    s.test.expect_return(0)
}

fn set_epoch(s: &mut Setup, epoch: u64) {
    s.test.set_virtual_substate(VirtualSubstateId::CurrentEpoch, VirtualSubstate::CurrentEpoch(epoch));
}

#[test]
fn agent_pays_allowlisted_payee() {
    let mut s = setup();
    let payee = s.payee;
    pay(&mut s, payee, 60);
    assert_eq!(balance(&mut s, payee), Amount::new(60));
    assert_eq!(vault_balance(&mut s), Amount::new(940));
}

#[test]
fn per_payment_cap_is_enforced() {
    let mut s = setup();
    let payee = s.payee;
    pay_fails(&mut s, payee, 101, "Amount exceeds the per-payment cap");
    pay(&mut s, payee, 100);
}

#[test]
fn epoch_budget_resets_next_epoch() {
    let mut s = setup();
    let payee = s.payee;
    pay(&mut s, payee, 100);
    pay(&mut s, payee, 100);
    pay_fails(&mut s, payee, 60, "Epoch budget exceeded");
    pay(&mut s, payee, 50);
    set_epoch(&mut s, 1);
    pay(&mut s, payee, 100);
    assert_eq!(balance(&mut s, payee), Amount::new(350));
}

#[test]
fn payment_count_limit_is_enforced() {
    let mut s = setup();
    let payee = s.payee;
    for _ in 0..3 {
        pay(&mut s, payee, 1);
    }
    pay_fails(&mut s, payee, 1, "Payment count limit reached for this epoch");
    set_epoch(&mut s, 7);
    pay(&mut s, payee, 1);
}

#[test]
fn unlisted_payee_is_refused() {
    let mut s = setup();
    let (other, _, _) = s.test.create_empty_account();
    pay_fails(&mut s, other, 10, "Payee is not allowlisted");
}

#[test]
fn non_agent_cannot_pay() {
    let mut s = setup();
    let stranger = party(&mut s.test);
    let payee = s.payee;
    let tx = s
        .test
        .transaction()
        .call_method(s.allowance, "pay", args![payee, Amount::new(10), "x"])
        .build_and_seal(&stranger.key);
    let r = s.test.execute_expect_failure(tx, vec![stranger.proof.clone()]);
    assert_reject_reason(r, "Signer is not an agent");

    // The owner is not an agent either: owning the badge does not let you spend as an agent.
    let tx = s
        .test
        .transaction()
        .call_method(s.allowance, "pay", args![payee, Amount::new(10), "x"])
        .build_and_seal(&s.owner.key);
    let proof = s.owner.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "Signer is not an agent");
}

#[test]
fn removed_agent_cannot_pay() {
    let mut s = setup();
    let pk = s.agent.pk;
    admin(&mut s, "remove_agent", args![pk]);
    let payee = s.payee;
    pay_fails(&mut s, payee, 10, "Signer is not an agent");
}

#[test]
fn request_reserves_funds_until_owner_decides() {
    let mut s = setup();
    let (vendor, _, _) = s.test.create_empty_account();

    // Over the cap and to an unlisted vendor: the agent files a request instead.
    let tx = agent_tx(&mut s, "request", args![vendor, Amount::new(900), "annual licence"]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    let id: u64 = s.test.expect_return(0);
    assert_eq!(id, 1);

    // Only 100 of 1000 is unreserved now.
    let payee = s.payee;
    pay(&mut s, payee, 100);
    pay_fails(&mut s, payee, 1, "Insufficient unreserved balance");
    let tx = admin_tx(&mut s, "withdraw", args![Amount::new(1)]);
    let proof = s.owner.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "Insufficient unreserved balance");

    admin(&mut s, "approve", args![1u64]);
    assert_eq!(balance(&mut s, vendor), Amount::new(900));
    assert_eq!(vault_balance(&mut s), Amount::ZERO);

    // A decided request cannot be replayed.
    let tx = admin_tx(&mut s, "approve", args![1u64]);
    let proof = s.owner.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "No such request");
}

#[test]
fn rejected_request_releases_reserve() {
    let mut s = setup();
    let (vendor, _, _) = s.test.create_empty_account();
    let tx = agent_tx(&mut s, "request", args![vendor, Amount::new(950), "x"]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    let payee = s.payee;
    pay_fails(&mut s, payee, 60, "Insufficient unreserved balance");
    admin(&mut s, "reject", args![1u64]);
    pay(&mut s, payee, 60);
    assert_eq!(balance(&mut s, vendor), Amount::ZERO);
}

#[test]
fn agent_cannot_approve_its_own_request() {
    let mut s = setup();
    let (vendor, _, _) = s.test.create_empty_account();
    let tx = agent_tx(&mut s, "request", args![vendor, Amount::new(500), "x"]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    let tx = agent_tx(&mut s, "approve", args![1u64]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_failure(tx, vec![proof]);
    assert_eq!(balance(&mut s, vendor), Amount::ZERO);
}

#[test]
fn panic_freeze_stops_spending_until_owner_unfreezes() {
    let mut s = setup();
    let tx = agent_tx(&mut s, "panic_freeze", args!["key may have leaked"]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    let payee = s.payee;
    pay_fails(&mut s, payee, 10, "Allowance is frozen");

    // Requests are refused while frozen too.
    let tx = agent_tx(&mut s, "request", args![payee, Amount::new(10), "x"]);
    let proof = s.agent.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "Allowance is frozen");

    // The agent cannot lift the freeze.
    let tx = agent_tx(&mut s, "unfreeze", args![]);
    let proof = s.agent.proof.clone();
    s.test.execute_expect_failure(tx, vec![proof]);

    admin(&mut s, "unfreeze", args![]);
    pay(&mut s, payee, 10);
}

#[test]
fn only_owner_can_withdraw_or_change_policy() {
    let mut s = setup();
    for (method, a) in [
        ("withdraw", args![Amount::new(1)]),
        ("set_policy", args![Amount::new(1000), Amount::new(1000), 100u32]),
        ("allow_payee", args![s.agent.account, "me"]),
        ("add_agent", args![s.owner.pk, "x"]),
        ("freeze", args![]),
    ] {
        let tx = agent_tx(&mut s, method, a);
        let proof = s.agent.proof.clone();
        s.test.execute_expect_failure(tx, vec![proof]);
    }

    let owner_account = s.owner.account;
    let tx = s
        .test
        .transaction()
        .create_proof(s.owner.account, s.badge)
        .put_last_instruction_output_on_workspace("proof")
        .authorize_proof("proof")
        .call_method(s.allowance, "withdraw", args![Amount::new(400)])
        .put_last_instruction_output_on_workspace("out")
        .call_method(owner_account, "deposit", args![Workspace("out")])
        .drop_all_proofs_in_workspace()
        .build_and_seal(&s.owner.key);
    let proof = s.owner.proof.clone();
    s.test.execute_expect_success(tx, vec![proof]);
    assert_eq!(balance(&mut s, owner_account), Amount::new(400));
    assert_eq!(vault_balance(&mut s), Amount::new(600));
}

#[test]
fn policy_change_applies_immediately() {
    let mut s = setup();
    admin(&mut s, "set_policy", args![Amount::new(10), Amount::new(20), 5u32]);
    let payee = s.payee;
    pay_fails(&mut s, payee, 11, "Amount exceeds the per-payment cap");
    pay(&mut s, payee, 10);
    pay(&mut s, payee, 10);
    pay_fails(&mut s, payee, 1, "Epoch budget exceeded");

    let tx = admin_tx(&mut s, "set_policy", args![Amount::new(30), Amount::new(20), 5u32]);
    let proof = s.owner.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "Per-payment cap exceeds the epoch budget");
}

#[test]
fn zero_and_oversized_inputs_are_refused() {
    let mut s = setup();
    let payee = s.payee;
    pay_fails(&mut s, payee, 0, "Amount must be positive");
    let long = "x".repeat(129);
    let tx = agent_tx(&mut s, "pay", args![payee, Amount::new(1), long]);
    let proof = s.agent.proof.clone();
    let r = s.test.execute_expect_failure(tx, vec![proof]);
    assert_reject_reason(r, "Memo too long");
}
