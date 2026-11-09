//! Allowance: a spend-limited account that an AI agent can pay from.
//!
//! The owner funds the account and holds an admin badge. Agents are plain signing keys: they can pay
//! allowlisted payees within a per-payment cap, a per-epoch budget and a per-epoch payment count. A
//! payment that does not fit the policy can be filed as a request; the funds are reserved until the
//! owner approves or rejects it. The owner can freeze the account at any time, and agents can freeze it
//! too (a compromised-key "panic button"), but only the owner can unfreeze or withdraw.

use std::collections::BTreeMap;
use tari_template_lib::prelude::*;

#[template]
mod allowance {
    use super::*;

    /// Upper bound on open requests, so the component state stays small.
    const MAX_PENDING: usize = 16;
    /// Upper bound on agents and payees for the same reason.
    const MAX_ENTRIES: usize = 64;
    const MAX_MEMO: usize = 128;

    pub struct Allowance {
        vault: Vault,
        admin_badge: ResourceAddress,
        agents: BTreeMap<RistrettoPublicKeyBytes, String>,
        payees: BTreeMap<ComponentAddress, String>,
        policy: Policy,
        window_epoch: u64,
        window_spent: Amount,
        window_count: u32,
        reserved: Amount,
        pending: BTreeMap<u64, Request>,
        next_request_id: u64,
        frozen: bool,
        total_spent: Amount,
        payment_count: u64,
    }

    impl Allowance {
        /// Creates an allowance for `resource` and returns the admin badge. Fund it with `fund`.
        pub fn create(
            resource: ResourceAddress,
            per_payment_cap: Amount,
            per_epoch_budget: Amount,
            max_payments_per_epoch: u32,
        ) -> Bucket {
            let policy = Policy::new(per_payment_cap, per_epoch_budget, max_payments_per_epoch);
            let badge = ResourceBuilder::non_fungible()
                .with_token_symbol("ALLOW")
                .metadata("name", "Allowance admin badge")
                .initial_supply(Some(NonFungibleId::from_u64(0)));
            let admin_badge = badge.resource_address();
            let admin = rule!(resource(admin_badge));

            let rules = AccessRules::new()
                .add_method_rule("fund", AccessRule::AllowAll)
                .add_method_rule("pay", AccessRule::AllowAll)
                .add_method_rule("request", AccessRule::AllowAll)
                .add_method_rule("panic_freeze", AccessRule::AllowAll)
                .add_method_rule("status", AccessRule::AllowAll)
                .add_method_rule("balance", AccessRule::AllowAll)
                .add_method_rule("pending_requests", AccessRule::AllowAll)
                .add_method_rule("is_agent", AccessRule::AllowAll)
                .add_method_rule("is_payee", AccessRule::AllowAll)
                .default(admin);

            Component::new(Self {
                vault: Vault::new_empty(resource),
                admin_badge,
                agents: BTreeMap::new(),
                payees: BTreeMap::new(),
                policy,
                window_epoch: Consensus::current_epoch(),
                window_spent: Amount::ZERO,
                window_count: 0,
                reserved: Amount::ZERO,
                pending: BTreeMap::new(),
                next_request_id: 1,
                frozen: false,
                total_spent: Amount::ZERO,
                payment_count: 0,
            })
            .with_access_rules(rules)
            .with_owner_rule(OwnerRule::None)
            .create();

            emit_event("Created", metadata![
                "resource" => resource.to_string(),
                "admin_badge" => admin_badge.to_string(),
                "per_payment_cap" => per_payment_cap.to_string(),
                "per_epoch_budget" => per_epoch_budget.to_string(),
                "max_payments_per_epoch" => max_payments_per_epoch.to_string(),
            ]);
            badge
        }

        // ---------- anyone ----------

        pub fn fund(&mut self, bucket: Bucket) {
            let amount = bucket.amount();
            self.vault.deposit(bucket);
            emit_event("Funded", metadata!["amount" => amount.to_string()]);
        }

        // ---------- agents ----------

        /// Pays an allowlisted payee within the policy. Panics (and changes nothing) otherwise.
        pub fn pay(&mut self, payee: ComponentAddress, amount: Amount, memo: String) {
            let agent = self.require_agent();
            assert!(!self.frozen, "Allowance is frozen");
            assert!(memo.len() <= MAX_MEMO, "Memo too long");
            assert!(amount.is_positive(), "Amount must be positive");
            assert!(self.payees.contains_key(&payee), "Payee is not allowlisted");
            assert!(amount <= self.policy.per_payment_cap, "Amount exceeds the per-payment cap");

            self.roll_window();
            assert!(
                self.window_count < self.policy.max_payments_per_epoch,
                "Payment count limit reached for this epoch"
            );
            let spent = self.window_spent.checked_add(amount).expect("overflow");
            assert!(spent <= self.policy.per_epoch_budget, "Epoch budget exceeded");
            self.assert_available(amount);

            self.window_spent = spent;
            self.window_count += 1;
            self.send(payee, amount);
            emit_event("Paid", metadata![
                "agent" => agent.to_string(),
                "payee" => payee.to_string(),
                "amount" => amount.to_string(),
                "memo" => memo,
                "epoch" => self.window_epoch.to_string(),
            ]);
        }

        /// Files a payment that needs owner approval and reserves the funds. Returns the request id.
        pub fn request(&mut self, payee: ComponentAddress, amount: Amount, memo: String) -> u64 {
            let agent = self.require_agent();
            assert!(!self.frozen, "Allowance is frozen");
            assert!(memo.len() <= MAX_MEMO, "Memo too long");
            assert!(amount.is_positive(), "Amount must be positive");
            assert!(self.pending.len() < MAX_PENDING, "Too many open requests");
            self.assert_available(amount);

            let id = self.next_request_id;
            self.next_request_id += 1;
            self.reserved = self.reserved.checked_add(amount).expect("overflow");
            emit_event("Requested", metadata![
                "id" => id.to_string(),
                "agent" => agent.to_string(),
                "payee" => payee.to_string(),
                "amount" => amount.to_string(),
                "memo" => memo.clone(),
            ]);
            self.pending.insert(id, Request { agent, payee, amount, memo });
            id
        }

        /// Lets an agent stop all spending, e.g. when it suspects its key has leaked.
        pub fn panic_freeze(&mut self, reason: String) {
            let agent = self.require_agent();
            assert!(reason.len() <= MAX_MEMO, "Reason too long");
            self.frozen = true;
            emit_event("Frozen", metadata!["by" => agent.to_string(), "reason" => reason]);
        }

        // ---------- owner (admin badge) ----------

        pub fn approve(&mut self, id: u64) {
            let req = self.pending.remove(&id).expect("No such request");
            self.reserved = self.reserved.checked_sub(req.amount).expect("reserve underflow");
            self.send(req.payee, req.amount);
            emit_event("Approved", metadata![
                "id" => id.to_string(),
                "payee" => req.payee.to_string(),
                "amount" => req.amount.to_string(),
            ]);
        }

        pub fn reject(&mut self, id: u64) {
            let req = self.pending.remove(&id).expect("No such request");
            self.reserved = self.reserved.checked_sub(req.amount).expect("reserve underflow");
            emit_event("Rejected", metadata!["id" => id.to_string()]);
        }

        pub fn add_agent(&mut self, key: RistrettoPublicKeyBytes, label: String) {
            assert!(label.len() <= MAX_MEMO, "Label too long");
            assert!(
                self.agents.contains_key(&key) || self.agents.len() < MAX_ENTRIES,
                "Too many agents"
            );
            self.agents.insert(key, label);
            emit_event("AgentAdded", metadata!["agent" => key.to_string()]);
        }

        pub fn remove_agent(&mut self, key: RistrettoPublicKeyBytes) {
            assert!(self.agents.remove(&key).is_some(), "Not an agent");
            emit_event("AgentRemoved", metadata!["agent" => key.to_string()]);
        }

        pub fn allow_payee(&mut self, payee: ComponentAddress, label: String) {
            assert!(label.len() <= MAX_MEMO, "Label too long");
            assert!(
                self.payees.contains_key(&payee) || self.payees.len() < MAX_ENTRIES,
                "Too many payees"
            );
            self.payees.insert(payee, label);
            emit_event("PayeeAllowed", metadata!["payee" => payee.to_string()]);
        }

        pub fn remove_payee(&mut self, payee: ComponentAddress) {
            assert!(self.payees.remove(&payee).is_some(), "Not a payee");
            emit_event("PayeeRemoved", metadata!["payee" => payee.to_string()]);
        }

        pub fn set_policy(
            &mut self,
            per_payment_cap: Amount,
            per_epoch_budget: Amount,
            max_payments_per_epoch: u32,
        ) {
            self.policy = Policy::new(per_payment_cap, per_epoch_budget, max_payments_per_epoch);
            emit_event("PolicyChanged", metadata![
                "per_payment_cap" => per_payment_cap.to_string(),
                "per_epoch_budget" => per_epoch_budget.to_string(),
                "max_payments_per_epoch" => max_payments_per_epoch.to_string(),
            ]);
        }

        pub fn freeze(&mut self) {
            self.frozen = true;
            emit_event("Frozen", metadata!["by" => "owner"]);
        }

        pub fn unfreeze(&mut self) {
            self.frozen = false;
            emit_event("Unfrozen", metadata![]);
        }

        /// Withdraws unreserved funds to the owner.
        pub fn withdraw(&mut self, amount: Amount) -> Bucket {
            self.assert_available(amount);
            emit_event("Withdrawn", metadata!["amount" => amount.to_string()]);
            self.vault.withdraw(amount)
        }

        // ---------- views ----------

        pub fn status(&self) -> Status {
            let epoch = Consensus::current_epoch();
            let (spent, count) = if epoch == self.window_epoch {
                (self.window_spent, self.window_count)
            } else {
                (Amount::ZERO, 0)
            };
            Status {
                resource: self.vault.resource_address(),
                admin_badge: self.admin_badge,
                balance: self.vault.balance(),
                reserved: self.reserved,
                frozen: self.frozen,
                policy: self.policy.clone(),
                epoch,
                epoch_spent: spent,
                epoch_payments: count,
                total_spent: self.total_spent,
                payment_count: self.payment_count,
                agents: self.agents.len() as u32,
                payees: self.payees.len() as u32,
                open_requests: self.pending.len() as u32,
            }
        }

        pub fn balance(&self) -> Amount {
            self.vault.balance()
        }

        pub fn pending_requests(&self) -> BTreeMap<u64, Request> {
            self.pending.clone()
        }

        pub fn is_agent(&self, key: RistrettoPublicKeyBytes) -> bool {
            self.agents.contains_key(&key)
        }

        pub fn is_payee(&self, payee: ComponentAddress) -> bool {
            self.payees.contains_key(&payee)
        }

        // ---------- internal ----------

        fn require_agent(&self) -> RistrettoPublicKeyBytes {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(self.agents.contains_key(&signer), "Signer is not an agent");
            signer
        }

        fn roll_window(&mut self) {
            let epoch = Consensus::current_epoch();
            if epoch != self.window_epoch {
                self.window_epoch = epoch;
                self.window_spent = Amount::ZERO;
                self.window_count = 0;
            }
        }

        fn assert_available(&self, amount: Amount) {
            let free = self.vault.balance().checked_sub(self.reserved).unwrap_or(Amount::ZERO);
            assert!(amount <= free, "Insufficient unreserved balance");
        }

        fn send(&mut self, payee: ComponentAddress, amount: Amount) {
            let bucket = self.vault.withdraw(amount);
            ComponentManager::get(payee).invoke("deposit", args![bucket]);
            self.total_spent = self.total_spent.checked_add(amount).expect("overflow");
            self.payment_count += 1;
        }
    }

    #[derive(Clone)]
    pub struct Policy {
        pub per_payment_cap: Amount,
        pub per_epoch_budget: Amount,
        pub max_payments_per_epoch: u32,
    }

    impl Policy {
        fn new(per_payment_cap: Amount, per_epoch_budget: Amount, max_payments_per_epoch: u32) -> Self {
            assert!(per_payment_cap <= per_epoch_budget, "Per-payment cap exceeds the epoch budget");
            Self { per_payment_cap, per_epoch_budget, max_payments_per_epoch }
        }
    }

    #[derive(Clone)]
    pub struct Request {
        pub agent: RistrettoPublicKeyBytes,
        pub payee: ComponentAddress,
        pub amount: Amount,
        pub memo: String,
    }

    pub struct Status {
        pub resource: ResourceAddress,
        pub admin_badge: ResourceAddress,
        pub balance: Amount,
        pub reserved: Amount,
        pub frozen: bool,
        pub policy: Policy,
        pub epoch: u64,
        pub epoch_spent: Amount,
        pub epoch_payments: u32,
        pub total_spent: Amount,
        pub payment_count: u64,
        pub agents: u32,
        pub payees: u32,
        pub open_requests: u32,
    }
}
