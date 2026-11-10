# A node can be a zk edge

[ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) ("A client may pay from a zk note")
has had a `[zk_note]` table since the allowance endpoints landed, and every build refused to boot
with it, because nothing could read a Mina vault or check a proof. This build can
([#1514](https://github.com/toon-protocol/connector/issues/1514)). **Nothing already running is
broken by it**: a config with no `[zk_note]` table behaves as before. A config that has one must add a
key, and the table now boots.

The operator's walkthrough is [Get paid → A zk note](../guide/get-paid.md#a-zk-note-mina).

## Until clearances exist, payers can leave only by escape

A zk edge can grant allowances in this build, but it cannot yet sign a **clearance**, the signature
that lets a payer close their note by agreement
([#1556](https://github.com/toon-protocol/connector/issues/1556)). Until that lands, **a payer
can leave only by escape**: the exit starts on chain and waits out zk_toon's two-day challenge window
before it pays. Say so to the people who will pay you from a note. A zk edge that is fine with that can
ship now; one that is not should wait for clearances.

## Before and after: `connector.toml`

**Before** (a build that parsed the table and refused to boot):

```toml
[zk_note]
vault                   = "B62q…"
network                 = "mina:devnet"
graphql_url             = "https://…/graphql"
state_key_file          = "/app/data/zk-state.key"
verification_key_hash   = "17783576495452972973648719307576910001175195573379209737447262132918659765208"
tiers                   = [100000000]
allowance_lifetime_secs = 3600
open_budget_cap         = 1000000000
```

**After** (one new required key):

```toml
[zk_note]
vault                   = "B62q…"
network                 = "mina:devnet"
graphql_url             = "https://…/graphql"       # a Mina daemon
archive_url             = "https://…/graphql"       # NEW, required: an archive node
state_key_file          = "/app/data/zk-state.key"
verification_key_hash   = "17783576495452972973648719307576910001175195573379209737447262132918659765208"
tiers                   = [100000000]
allowance_lifetime_secs = 3600
open_budget_cap         = 1000000000
```

## What changed

- **`archive_url` is required.** The vault keeps only the root of its nullifier set, so whether a
  note's state is spent comes from the vault's actions, which only an archive node lists. A table
  without it is refused at load. With `rpc_via_socks_proxy = true`, `archive_url` follows the same
  rules as `graphql_url`: an `http` URL is refused on the circuit unless its host is an onion one, and
  both endpoints ride the `toon-zk-note-mina` circuit.
- **`verification_key_hash` must be the carried key's.** The binary carries zk_toon's published
  request key, so the hash above is the only one this build accepts; any other refuses boot, naming it.
  A different Mina era needs a different build.
- **The node reads before it serves.** It builds the verifier (a second or two of CPU), reads the slot
  anchor and, when `subsidy_budget` is set, the vault's creation slot. A zk edge that cannot make these
  reads does not boot. A read that fails later refuses that one allowance (`vault_unavailable`) and the
  node keeps serving; an archive that lags the daemon counts as a failed read, never as an unused
  nullifier.
- **The wire vectors carry a real request proof** in place of the placeholder. `schema_version` stays 13:
  no field or frame moved, and the two request entries gain a `verification_key_hash`. The proof's tag binds
  zk_toon's sample context, which the entry states as `proof_binds`.
