#!/usr/bin/env python3
"""Publish a template WASM through the local wallet daemon. Dry-runs first and uses the reported fee."""
import json, sys
from rpc import call

path, account = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "owner")
import base64
raw = open(path, "rb").read()
binary = base64.b64encode(raw).decode()
req = {"binary": binary, "fee_account": {"Name": account}, "max_fee": 50_000_000, "detect_inputs": True, "dry_run": True}
dry = call("transactions.publish_template", req)
fee = int(dry.get("dry_run_fee") or 50_000_000)
print("binary bytes", len(raw), "dry-run fee", fee)
req.update(dry_run=False, max_fee=fee)
res = call("transactions.publish_template", req)
tx = res.get("transaction_id")
print("transaction", tx)
out = call("transactions.wait_result", {"transaction_id": tx, "timeout_secs": 240})
print(json.dumps({k: out.get(k) for k in ("status", "final_fee")}, default=str))
txt = json.dumps(out)
import re
print("template", sorted(set(re.findall(r"template_[0-9a-f]{64}", txt))))
