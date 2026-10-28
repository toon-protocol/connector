# A controller's write key carries a scope and two limits

**Status:** Accepted (owner decision, 2026-10-11, #1485) — **built**. Built (#1487): decisions 1, 7 and 10 and the scope, expiry and `403` halves of 9 and 11 — `[operator] controller_keys_file`, the eight named writes, the optional expiry, `403` naming `scope` or `expiry` before anything is signed or journaled, `toon_writes_refused_total{reason}`, and `GET /write-keys` listing each key's id, whether it is restricted, its scope and its expiry. Built (#1488): **the spending limit** — decisions 2, 3 and 5 for packets, decision 8 for the spending count (`DayTotals`, journaled under `state_dir` as `controller-day-totals.journal`), the `spending-limit` reason on the refused-write counter, and the spending half of decision 11 on `GET /write-keys`. Built (#1489): **the deposit limit** — decision 4 and the deposit halves of 2, 5, 8, 9 and 11: `POST /peers` and `POST /channels` count their `deposit` where a new channel is journaled (a found or resumed one counts nothing), `POST /channels/:id/fund` counts its `amount` every time, into the same journal as a second `CountKind`, with the `deposit-limit` reason and the deposit limit on `GET /write-keys`. No entry is refused for stating a limit any more. It **extends** [0008](0008-operator-surface-splits-read-from-write.md): every write stays signed, attributable and individually revocable, and a key on `write_keys` keeps the authority that record gave it. As written it left [0009](0009-one-typed-config-file-no-environment-layer.md) untouched, with a restricted key revoked by an edit and a restart; **[0079](0079-a-controllers-key-is-revoked-by-an-edit-and-may-sign-a-read.md) amends that** — see the Update below.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

**Falsifier:** none — the record no longer claims anything absent: every decision in it is built (#1487, #1488, #1489), so there is no unbuilt half left to check the tree against.

**A write key held by a controller is restricted: it carries a scope, a spending limit and a
deposit limit, and the connector enforces all three.** An operator's write key is unchanged and may
make every write. A controller is automation an operator points at a connector, and the only key it
could be given until now could also withdraw every channel, with nothing on the connector bounding
what it spent.

## Context

[0008](0008-operator-surface-splits-read-from-write.md) gave the write surface one level of trust:
a key on the allowlist may originate a packet, peer, write routes, and open, fund, withdraw and land
channels. Every write handler asks only whether the signature verifies against an allowlisted key,
and never which key it was.

That is right for an operator and wrong for a controller. spaceturtle runs an agent in a zone of
its own, apart from the connector and the wallet, and wants to give it a key that can do a
provider's daily work and nothing more. `toon` checks a spending limit from a file the agent can
rewrite, so that limit does not bind an agent that misbehaves. The bound has to be held by the
party that signs the claims.

## Decision

1. **A scope is a list of named writes**, one name per row of the operator surface's writes:
   `packets`, `peers`, `routes-peers`, `routes-leased`, `channels-open`, `channels-fund`,
   `channels-withdraw` and `channels-land`. A `DELETE` belongs to the name of its `POST`. A name the
   binary does not know is refused at boot.
2. **There are two limits, each with a per-write and a per-UTC-day figure.** The **spending limit**
   counts packets the key originates. The **deposit limit** counts what the key puts into channels
   by peering, opening or funding. They are separate because a deposit stays the node's own money,
   recoverable by a withdrawal, and a packet does not.
3. **A packet counts at its full amount, rejected or not.** Originating a packet sends a claim. An
   honest next hop does not keep the claim for a packet it rejects (#1446), but a hostile one can
   ([0042](0042-a-packet-carries-its-claim.md)), and a limit on a key that is not fully trusted
   bounds what the key exposed, not what the node lost.
4. **A deposit counts when its channel write is journaled, before the transaction is sent.** A
   repeat of `POST /peers` or `POST /channels` that finds or resumes a channel counts nothing. A
   `POST /channels/:id/fund` counts every time, because it is an increment. A deposit whose
   transaction fails is not given back, and a withdrawal gives nothing back.
5. **A limit is one bare figure.** Each write counts the amount it commits, in the base units of
   the channel it is paid over, into one running total per key. There is no per-asset limit and no
   conversion. On a node holding tokens of different decimals the figure adds unlike units.
6. **A scope that commits value must state its limit.** `packets` with no spending limit, or
   `peers`, `channels-open` or `channels-fund` with no deposit limit, is a refusal to start that
   names the entry. A key that needs no limit belongs on `write_keys`.
7. **Restricted keys are written in a file of their own**, named by `[operator]
controller_keys_file`: one table per key holding the key, its scope, its limits and an optional
   expiry. `write_keys` and `write_keys_file` are unchanged. A key in both places is a refusal to
   start.
8. **The day's totals are journaled under `state_dir`, by key.** A count is appended before the
   write it covers proceeds, and a write whose count cannot be appended is refused. Removing an
   entry and adding it back does not reset its day.
9. **A refused write is answered `403` with the reason named** — scope, spending limit, deposit
   limit or expiry — before anything is signed or sent. It is not in `GET /audit-log`, which
   records accepted writes. It is a log line and a metrics counter by reason. `401` stays the
   answer to a missing or bad signature.
10. **An expiry is optional.** After it, every write from the key is refused. An entry already
    expired at boot is not a refusal to start.
11. **`GET /write-keys` is a bearer-token read** listing every write key: whether it is restricted,
    its scope, both limits, what each has counted today, and its expiry.

## Considered options

- **Count what a failed packet actually cost**, as `toon` does and as #1485 first asked. Refused by
  decision 3. It needs a reservation released on reject and a rule for a packet that is never
  answered, and it trusts every next hop to give the claim back.
- **One limit for packets and deposits.** A key that funds a channel with 100 and sends 100 through
  it has used 200. The operator would set the limit at twice the intended spend, which then permits
  twice the spend through a channel already funded.
- **A limit per asset, or one figure converted at declared rates.** Refused by decision 5. The
  second makes a spending bound depend on a rate table.
- **Scopes as raw method and path pairs, or as a few coarse roles.** The first puts the URL layout
  in config. The second hides which writes a role grants.
- **Reread the key file on every write**, so that a revocation is live. It amends 0009 for one
  file. A restart is how every key is revoked today, and the operator controls the box.
- **Scoped bearer tokens.** Not asked for. A bearer token still never moves value.

## Consequences

- A controller that retries rejected packets uses up its day having spent nothing. `GET
/write-keys` is how that is seen.
- The limits are not a balance. A key can be refused with money in the channel, and can be allowed
  with none.
- The setting is added beside the existing ones, so no existing config changes and this is not a
  breaking deploy.
- The spending limit is not a **cap**. A cap is the largest packet a peering carries.

## Update (issue #1485) — a revoke is an edit alone, and a read may be signed

[0079](0079-a-controllers-key-is-revoked-by-an-edit-and-may-sign-a-read.md) reverses one option this record refused and adds to one decision:

- **"Reread the key file on every write" is now what happens.** The contents of
  `controller_keys_file` are read again on every signed request, so removing an entry revokes its
  key at that key's next request. The Status line's "an edit and a restart" described this record
  as first built. Decision 8 is unchanged: the day's totals are kept by key and do not follow the
  file.
- **Decision 11 gains a second way in.** `GET /write-keys`, like every read, also accepts a write
  key's signature in place of the bearer token.
