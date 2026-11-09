#!/usr/bin/env python3
"""Runs the Allowance demo end to end on Esmeralda through a local wallet daemon and records every
transaction in evidence/esmeralda-run.json.

Usage: live.py <demo_token_template> <allowance_template>
"""
import json, re, sys, time
from pathlib import Path
from rpc import call

DEMO_TEMPLATE, ALLOWANCE_TEMPLATE = sys.argv[1], sys.argv[2]
OUT = Path(__file__).resolve().parent.parent / "evidence" / "esmeralda-run.json"
log = {"network": "esmeralda", "templates": {"DemoToken": DEMO_TEMPLATE, "Allowance": ALLOWANCE_TEMPLATE}, "steps": []}


def save():
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(json.dumps(log, indent=1))


def account(name):
    r = call("accounts.create_or_get", {"account": {"Name": name}})
    return r["account"]


def ensure_funded(name):
    """Tops up an account's fee balance from the owner (the public faucet can run dry)."""
    if name == "owner":
        return
    bal = call("accounts.get_balances", {"account": {"Name": name}, "refresh": True})
    tari = [b for b in bal["balances"] if b.get("token_symbol") == "tTARI"]
    if tari and int(tari[0]["balance"]) >= 20_000_000:
        return
    pk = account(name)["owner_public_key"]
    req = {"account": {"Name": "owner"}, "amount": 30_000_000, "resource_address": "resource_" + "01" * 32,
           "destination_public_key": pk, "max_fee": 2_000_000, "dry_run": False}
    tx = call("accounts.transfer", req)["transaction_id"]
    call("transactions.wait_result", {"transaction_id": tx, "timeout_secs": 240})
    log.setdefault("fee_topups", []).append({"to": name, "transaction_id": tx})


def submit(step, manifest, variables, signer, expect_ok=True):
    """Dry-runs a manifest for the fee, submits it, waits, and records the outcome."""
    acct = account(signer)
    key = acct["owner_key_id"]
    variables = {"fee_account": acct["component_address"], **variables}
    body = "fn fee_main() { let a = var![\"fee_account\"]; a.pay_fee(var![\"max_fee\"]); }\n" + manifest
    req = {"manifest": body, "variables": {**variables, "max_fee": "20000000"}, "seal_signer_key_id": key,
           "signing_key_ids": [key], "max_fee": 20_000_000, "dry_run": True}
    fee = 20_000_000
    try:
        dry = call("transactions.submit_manifest", req)
        fee = int(dry.get("required_fees") or fee)
    except RuntimeError as e:
        # A rejected dry run still tells us the call would fail; submit it anyway so the rejection is on-chain.
        print("  dry run error:", str(e)[:200])
    req.update(dry_run=False, max_fee=fee, variables={**variables, "max_fee": str(fee)})
    tx = call("transactions.submit_manifest", req)["transaction_id"]
    res = call("transactions.wait_result", {"transaction_id": tx, "timeout_secs": 240})
    text = json.dumps(res)
    accepted = '"Accept"' in text or "AcceptFeeRejectRest" not in text and "Reject" not in text
    reason = None
    m = re.search(r'"(?:ExecutionFailure|reason|Reject)"\s*:\s*("(?:[^"\\]|\\.)*"|\{[^{}]*\})', text)
    if m:
        reason = m.group(1)[:300]
    entry = {"step": step, "signer": signer, "transaction_id": tx, "status": res.get("status"),
             "final_fee": res.get("final_fee"), "accepted": accepted and res.get("status") == "Accepted",
             "expected_ok": expect_ok, "reason": reason, "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    log["steps"].append(entry)
    save()
    flag = "OK " if entry["accepted"] == expect_ok else "!! "
    print(flag, step, tx, res.get("status"), (reason or "")[:120])
    return res, text


def new_components(text):
    return sorted(set(re.findall(r"component_[0-9a-f]{64}", text)))


owner, agent, api, vendor = (account(n) for n in ("owner", "agent", "inference-api", "gpu-vendor"))
for n in ("owner", "agent", "inference-api", "gpu-vendor"):
    ensure_funded(n)
log["accounts"] = {n: a["component_address"] for n, a in
                   (("owner", owner), ("agent", agent), ("inference-api", api), ("gpu-vendor", vendor))}
log["agent_public_key"] = agent["owner_public_key"]
known = set(log["accounts"].values())

use = f"use {DEMO_TEMPLATE} as DemoToken;\nuse {ALLOWANCE_TEMPLATE} as Allowance;\n"

res, text = submit("deploy demo token", use + 'fn main() { let t = DemoToken::new("dUSD"); }', {}, "owner")
faucet = [c for c in new_components(text) if c not in known][0]
known.add(faucet)
token = sorted(set(re.findall(r"resource_[0-9a-f]{64}", text)) - {"resource_" + "01" * 32})[0]
log["demo_token"] = {"component": faucet, "resource": token}

# cap 5.00, budget 12.00, five payments per epoch (dUSD has two decimals)
res, text = submit(
    "create allowance",
    use + 'fn main() { let mut acc = var!["owner"]; let badge = Allowance::create(var!["token"], amount!(500), '
          'amount!(1200), 5u32); acc.deposit(badge); }',
    {"owner": owner["component_address"], "token": token}, "owner")
allowance = [c for c in new_components(text) if c not in known][0]
badge = sorted(set(re.findall(r"resource_[0-9a-f]{64}", text)) - {"resource_" + "01" * 32, token})[0]
log["allowance"] = {"component": allowance, "admin_badge": badge}
save()

admin_prefix = ('let mut acc = var!["owner"]; let p = acc.create_proof_for_resource(var!["badge"]); authorize!(p); '
                'let al = var!["allowance"];')
admin_vars = {"owner": owner["component_address"], "badge": badge, "allowance": allowance}


def admin(step, body, extra=None, expect_ok=True):
    return submit(step, "fn main() { " + admin_prefix + body + " drop_all_proofs!(); }",
                  {**admin_vars, **(extra or {})}, "owner", expect_ok)


def agent_call(step, body, extra=None, expect_ok=True):
    return submit(step, 'fn main() { let al = var!["allowance"]; ' + body + " }",
                  {"allowance": allowance, **(extra or {})}, "agent", expect_ok)


submit("fund 50.00 dUSD", 'fn main() { let f = var!["faucet"]; let b = f.mint(amount!(5000)); '
       'let al = var!["allowance"]; al.fund(b); }', {"faucet": faucet, "allowance": allowance}, "owner")
admin("register agent key", f'al.add_agent(public_key!("{agent["owner_public_key"]}"), "research-bot");')
admin("allowlist inference API", 'al.allow_payee(var!["api"], "inference-api");', {"api": api["component_address"]})

pay_vars = {"api": api["component_address"], "vendor": vendor["component_address"]}
agent_call("agent pays 2.50 for call #1", 'al.pay(var!["api"], amount!(250), "inference call #1");', pay_vars)
agent_call("agent pays 2.50 for call #2", 'al.pay(var!["api"], amount!(250), "inference call #2");', pay_vars)
agent_call("over the per-payment cap (6.00) is refused", 'al.pay(var!["api"], amount!(600), "big batch");',
           pay_vars, expect_ok=False)
agent_call("unlisted payee is refused", 'al.pay(var!["vendor"], amount!(100), "gpu");', pay_vars, expect_ok=False)
agent_call("agent files a 20.00 request to the GPU vendor",
           'al.request(var!["vendor"], amount!(2000), "GPU hours block");', pay_vars)
agent_call("agent cannot approve its own request", "al.approve(1u64);", expect_ok=False)
admin("owner approves request #1", "al.approve(1u64);")
agent_call("panic freeze by the agent", 'al.panic_freeze("key may have leaked");')
agent_call("payment while frozen is refused", 'al.pay(var!["api"], amount!(100), "call #3");', pay_vars,
           expect_ok=False)
agent_call("agent cannot unfreeze", "al.unfreeze();", expect_ok=False)
admin("owner unfreezes", "al.unfreeze();")
agent_call("agent pays 1.00 after unfreeze", 'al.pay(var!["api"], amount!(100), "inference call #3");', pay_vars)

for n in ("inference-api", "gpu-vendor"):
    bal = call("accounts.get_balances", {"account": {"Name": n}, "refresh": True})
    log.setdefault("final_balances", {})[n] = [
        {"resource": b["resource_address"], "balance": b["balance"]} for b in bal["balances"]
        if b["resource_address"] == token]
save()
print(json.dumps(log["final_balances"], indent=1))
