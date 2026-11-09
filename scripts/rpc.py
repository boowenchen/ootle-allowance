#!/usr/bin/env python3
"""Minimal JSON-RPC client for a local tari_ootle_walletd (auth method 'none')."""
import json, os, sys, urllib.request

URL = os.environ.get("WALLETD_URL", "http://127.0.0.1:5100/json_rpc")
_token = None


def _post(method, params):
    headers = {"Content-Type": "application/json"}
    if _token:
        headers["Authorization"] = "Bearer " + _token
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    with urllib.request.urlopen(urllib.request.Request(URL, body, headers), timeout=300) as r:
        return json.load(r)


def call(method, params=None):
    global _token
    if _token is None and not method.startswith("auth."):
        res = _post("auth.request", {"permissions": ["Admin"], "credentials": "None"})
        if "error" in res:
            raise RuntimeError(res["error"])
        _token = res["result"]["token"]
    res = _post(method, params or {})
    if "error" in res:
        raise RuntimeError(f"{method}: {res['error']}")
    return res["result"]


if __name__ == "__main__":
    params = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    print(json.dumps(call(sys.argv[1], params), indent=1)[:6000])
