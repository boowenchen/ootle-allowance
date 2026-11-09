# Allowance — spend-limited accounts for AI agents on Tari Ootle

An AI agent that buys API calls, GPU time or data needs to hold money, and a key an agent holds can be
leaked or talked into paying the wrong party. **Allowance** gives the agent its own account with rules
the owner sets and the Ootle engine enforces:

- **Per-payment cap** and **per-epoch budget** with a **payment-count limit** (velocity check).
- **Payee allowlist**: the agent can only pay accounts the owner approved.
- **Requests**: anything outside the policy (too large, new vendor) becomes a request. The funds are
  reserved, and only the owner's admin badge can approve or reject it.
- **Panic freeze**: the agent itself can freeze the account if it suspects its key or instructions are
  compromised. Only the owner can unfreeze.
- **Owner controls**: change policy, add/remove agents and payees, withdraw unreserved funds. All of them
  require the admin badge NFT minted at creation; the component has no other owner (`OwnerRule::None`)
  and its code is not upgradable.

It ships with three ways to use it:

| Piece | What it is |
|---|---|
| `src/lib.rs` | The `Allowance` template (Rust → WASM) |
| `app/index.html` | A status page that reads any Allowance straight from the Esmeralda indexer: balance, reserved funds, this epoch's spending, agents, payees, open requests and every event. **Live: https://boowenchen.github.io/ootle-allowance/** |
| `mcp/allowance_mcp.py` | An MCP server so an AI agent (Claude, any MCP client) can call `allowance_pay`, `allowance_request`, `allowance_panic_freeze` and `allowance_status`. The agent key stays in your local wallet daemon. |

`demo_token/` is a freely mintable test token (dUSD, two decimals) used by the tests and the live demo.

## Live on Esmeralda

Published templates:

- Allowance `template_41e446e15712724265acdba3be30fdd72bed7f217f7259f7e0964359001c826b`
- DemoToken `template_086ae2445e70759e4de5078807d2734a72baa78bc2be14731a25e8f86241aa58`

Demo Allowance `component_a759f048344c22efe92872dd7222d79058eda5d39e500667d2bf4954db0276c1`
(policy: cap 5.00, budget 12.00 and 5 payments per epoch). Every step below is a real transaction; the
full list with ids is in [`evidence/esmeralda-run.json`](evidence/esmeralda-run.json), produced by
[`scripts/live.py`](scripts/live.py).

| Step | Result |
|---|---|
| Owner creates the Allowance, funds 50.00, registers the agent key and allowlists the inference API | accepted |
| Agent pays 2.50 for call #1, 2.50 for call #2 | accepted |
| Agent tries 6.00 (over the cap) | rejected on-chain: `Amount exceeds the per-payment cap` |
| Agent tries to pay an unlisted GPU vendor | rejected: `Payee is not allowlisted` |
| Agent files a 20.00 request to the GPU vendor | accepted, 20.00 reserved |
| Agent tries to approve its own request | rejected: `Access Denied` |
| Owner approves request #1 | accepted, vendor receives 20.00 |
| Agent panic-freezes; then tries to pay | accepted; then rejected: `Allowance is frozen` |
| Agent tries to unfreeze | rejected: `Access Denied` |
| Owner unfreezes; agent pays 1.00 | accepted |
| Agent pays 1.50 through the MCP server; then tries the unlisted vendor | accepted; rejected (`evidence/mcp-run.json`) |

Final balances read back from the network: inference API 7.50 dUSD, GPU vendor 20.00 dUSD.

## Template API

```text
create(resource, per_payment_cap, per_epoch_budget, max_payments_per_epoch) -> Bucket   // admin badge
fund(bucket)                                          anyone
pay(payee, amount, memo)                              registered agent key
request(payee, amount, memo) -> u64                   registered agent key
panic_freeze(reason)                                  registered agent key
approve(id) / reject(id)                              admin badge
add_agent(key, label) / remove_agent(key)             admin badge
allow_payee(account, label) / remove_payee(account)   admin badge
set_policy(cap, budget, max_payments) / freeze() / unfreeze() / withdraw(amount) -> Bucket   admin badge
status() / balance() / pending_requests() / is_agent(key) / is_payee(account)   anyone
```

Agents are identified by the transaction signer (`CallerContext::transaction_signer_public_key()`), never
by an argument. Budgets reset when the network epoch changes. Every state change emits an event
(`Allowance.Paid`, `Requested`, `Approved`, `Frozen`, …) with the agent, payee, amount and memo, which is
what the status page shows.

## Build and test

Tests use `tari_template_test_tooling`, which runs the real engine in-process (Linux or macOS; the
Cranelift backend it uses is not available on Windows, so use WSL there).

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cargo test
```

14 tests cover payments, cap, epoch budget and reset, payment count, allowlist, non-agent and removed
agent, requests (reserve, approve, reject, replay, self-approval), freeze/unfreeze, owner-only methods,
policy changes and input limits. Breaking the cap check, the epoch reset, the reserve accounting or the
freeze check each makes the suite fail ([`evidence/mutation.txt`](evidence/mutation.txt)).

## Run the demo yourself

1. Start a wallet daemon on Esmeralda: `tari_ootle_walletd --network esme --authentication none`
   (add `--override-keyring-password` on a machine without an OS keyring).
2. Create an account named `owner` and give it some tTARI (wallet UI faucet, or burn on L1).
3. Publish the two templates (`python3 scripts/publish.py <wasm>`) or reuse the addresses above.
4. `python3 scripts/live.py <demo_token_template> <allowance_template>`

## Use it from an AI agent (MCP)

```json
{
  "mcpServers": {
    "allowance": {
      "command": "python3",
      "args": ["/path/to/ootle-allowance/mcp/allowance_mcp.py"],
      "env": {
        "ALLOWANCE_COMPONENT": "component_...",
        "AGENT_ACCOUNT": "agent"
      }
    }
  }
}
```

The server dry-runs every call first, so a policy violation comes back to the agent as a tool error
before any transaction is sent. Even if the agent is manipulated, the most it can move is what the
owner's policy allows.

## Limits

- Testnet prototype; demo amounts use a test token with no value.
- Budgets are per network epoch, not per wall-clock hour.
- The agent key is a normal wallet key; protect the machine that holds it. The allowlist, caps and freeze
  limit what a stolen key can do; they do not prevent the theft.
- At most 64 agents, 64 payees and 16 open requests per Allowance, to keep state small.

## License

MIT
