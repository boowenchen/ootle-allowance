#!/usr/bin/env python3
"""MCP server that lets an AI agent pay from an Allowance on Tari Ootle.

The agent's key lives in a local tari_ootle_walletd; this server only builds manifests and asks the
daemon to sign them with that key. Every limit is enforced on-chain by the Allowance template, so a
confused or prompt-injected agent can at worst spend what the owner's policy allows.

Environment:
  ALLOWANCE_COMPONENT  component_... address of the Allowance
  AGENT_ACCOUNT        wallet account name holding the agent key (default "agent")
  WALLETD_URL          JSON-RPC endpoint (default http://127.0.0.1:5100/json_rpc)

Speaks MCP over stdio (JSON-RPC 2.0, newline-delimited). Standard library only.
"""
import json, os, re, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "scripts"))
from rpc import call  # noqa: E402

COMPONENT = os.environ.get("ALLOWANCE_COMPONENT", "")
AGENT = os.environ.get("AGENT_ACCOUNT", "agent")
ADDR = re.compile(r"^component_[0-9a-f]{64}$")

TOOLS = [
    {
        "name": "allowance_pay",
        "description": "Pay an allowlisted payee from the Allowance. Fails on-chain if the amount exceeds the "
                       "per-payment cap, the epoch budget or payment count, or if the payee is not allowlisted.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "payee": {"type": "string", "description": "component_... account address"},
                "amount": {"type": "integer", "description": "amount in the token's smallest unit"},
                "memo": {"type": "string", "description": "invoice or call reference, at most 128 characters"},
            },
            "required": ["payee", "amount", "memo"],
        },
    },
    {
        "name": "allowance_request",
        "description": "Ask the owner to approve a payment that does not fit the policy (too large or new payee). "
                       "Funds are reserved until the owner approves or rejects. Returns the request id.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "payee": {"type": "string"}, "amount": {"type": "integer"}, "memo": {"type": "string"},
            },
            "required": ["payee", "amount", "memo"],
        },
    },
    {
        "name": "allowance_panic_freeze",
        "description": "Freeze the Allowance immediately, e.g. if you suspect your instructions or key are "
                       "compromised. Only the owner can unfreeze.",
        "inputSchema": {"type": "object", "properties": {"reason": {"type": "string"}}, "required": ["reason"]},
    },
    {
        "name": "allowance_status",
        "description": "Read the Allowance balance, reserved funds, policy, this epoch's spending and frozen flag.",
        "inputSchema": {"type": "object", "properties": {}},
    },
]


def manifest_call(body, variables, dry_run=False):
    acct = call("accounts.create_or_get", {"account": {"Name": AGENT}})["account"]
    key = acct["owner_key_id"]
    text = ('fn fee_main() { let a = var!["fee_account"]; a.pay_fee(var!["max_fee"]); }\n'
            'fn main() { let al = var!["allowance"]; ' + body + " }")
    variables = {"fee_account": acct["component_address"], "allowance": COMPONENT, **variables}
    req = {"manifest": text, "variables": {**variables, "max_fee": "20000000"}, "seal_signer_key_id": key,
           "signing_key_ids": [key], "max_fee": 20_000_000, "dry_run": True}
    # The dry run surfaces policy violations before anything is submitted.
    dry = call("transactions.submit_manifest", req)
    if dry_run:
        return dry
    fee = int(dry.get("required_fees") or 20_000_000)
    req.update(dry_run=False, max_fee=fee, variables={**variables, "max_fee": str(fee)})
    tx = call("transactions.submit_manifest", req)["transaction_id"]
    res = call("transactions.wait_result", {"transaction_id": tx, "timeout_secs": 240})
    return {"transaction_id": tx, "status": res.get("status"), "fee": res.get("final_fee"), "result": res}


def check_payment(args):
    payee, amount, memo = args.get("payee", ""), args.get("amount"), args.get("memo", "")
    if not ADDR.match(payee):
        raise ValueError("payee must be a component_ address")
    if not isinstance(amount, int) or amount <= 0:
        raise ValueError("amount must be a positive integer")
    if len(memo) > 128 or '"' in memo or "\\" in memo:
        raise ValueError("memo must be at most 128 characters without quotes or backslashes")
    return payee, amount, memo


def run_tool(name, args):
    if not ADDR.match(COMPONENT):
        raise ValueError("ALLOWANCE_COMPONENT is not set")
    if name == "allowance_pay":
        payee, amount, memo = check_payment(args)
        r = manifest_call(f'al.pay(var!["payee"], amount!({amount}), "{memo}");', {"payee": payee})
        return {k: r[k] for k in ("transaction_id", "status", "fee")}
    if name == "allowance_request":
        payee, amount, memo = check_payment(args)
        r = manifest_call(f'al.request(var!["payee"], amount!({amount}), "{memo}");', {"payee": payee})
        ids = re.findall(r'"U64"\s*:\s*(\d+)', json.dumps(r["result"]))
        return {"transaction_id": r["transaction_id"], "status": r["status"], "request_id": ids[-1] if ids else None}
    if name == "allowance_panic_freeze":
        reason = args.get("reason", "")[:128].replace('"', "'").replace("\\", "/")
        r = manifest_call(f'al.panic_freeze("{reason}");', {})
        return {k: r[k] for k in ("transaction_id", "status", "fee")}
    if name == "allowance_status":
        r = manifest_call("al.status();", {}, dry_run=True)
        return {"dry_run_result": r.get("result")}
    raise ValueError("unknown tool " + name)


def respond(msg_id, result=None, error=None):
    out = {"jsonrpc": "2.0", "id": msg_id}
    if error is not None:
        out["error"] = {"code": -32603, "message": error}
    else:
        out["result"] = result
    sys.stdout.write(json.dumps(out) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        method, msg_id = msg.get("method"), msg.get("id")
        if msg_id is None:
            continue  # notification
        if method == "initialize":
            respond(msg_id, {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                             "serverInfo": {"name": "ootle-allowance", "version": "0.1.0"}})
        elif method == "tools/list":
            respond(msg_id, {"tools": TOOLS})
        elif method == "tools/call":
            p = msg.get("params", {})
            try:
                result = run_tool(p.get("name"), p.get("arguments") or {})
                respond(msg_id, {"content": [{"type": "text", "text": json.dumps(result, default=str)[:20000]}]})
            except Exception as e:  # report policy rejections to the agent as tool errors
                respond(msg_id, {"content": [{"type": "text", "text": str(e)[:2000]}], "isError": True})
        elif method == "ping":
            respond(msg_id, {})
        else:
            respond(msg_id, error="method not found: " + str(method))


if __name__ == "__main__":
    main()
