# A claim proves a peering; the shared secret is deleted

**Status:** Accepted — **built** (#1157). Finished what issue #868 decided on 2026-08-07 and only half landed: `peer-carriage-spec.md` §1.2 had said since then that the bearer credential does not decide role, while `connector-peer-auth` still decided on it and never examined a claim. **Required by [0058](0058-a-peering-is-established-from-a-url.md)** — while a shared secret is mandatory, a public document can never be sufficient to establish a peering. Applies [0008](0008-operator-surface-splits-read-from-write.md)'s rule to the peer surface. Deleted a wire field, so it disturbs [0021](0021-vectors-are-normative-prose-is-not.md) (`schema_version` is now **4**) and [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md). **Amended by [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)** (accepted 2026-09-27): the role proof becomes a voucher on a bound channel, or for a zero-value packet a claim-state challenge from that channel's voucher signer. The principle — a signature, never a shared secret — stands. See the Update at the foot.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**A peering is proven by a signature, not by a shared string.** The `{peerId, secret}` bearer
credential is deleted — from the `Toon-Peer-Auth` header, from the BTP `auth` protocolData entry,
from `[[peers]].credential`, and from the role decision. An interaction has role `peer` if and only
if it carries a claim on a channel one of that peer's `[[peer_channels]]` rows configures, whose
signature verifies against the counterparty key that row configures.

## The spec already says this, and the code does not

`peer-carriage-spec.md` §1.2 carries an amendment dated 2026-08-07:

> **P1, the `{peerId, secret}` bearer credential, no longer decides role.**

and states the rule as **P2 + P3** — a channel binding, and a verified claim on one of that peer's
channels. It argues the case in its own words:

> A bearer secret proves only possession of a string both operators wrote into their own config
> files… A signature over ADR 0024's balance proof proves control of the key the channel was actually
> opened against — **strictly stronger**, and now present on every packet rather than once per
> session.

The implementation is at the pre-#868 rule. `connector-peer-auth/src/decision.rs:186-221`:

```rust
pub fn decide_role(presented: Option<&PresentedCredential>, policy: &PeerAuthPolicy) -> RoleDecision {
    let Some(credential) = presented else { return RoleDecision::client() };
    let Some(entry) = policy.entry(credential.asserted_peer_id()) else { return RoleDecision::client() };
    if !credential.proves(&entry.credential) {
        return RoleDecision::refused(&entry.id, UnmetRequirement::ProvenCredential);   // P1
    }
    if !entry.channel_bound {
        return RoleDecision::refused(&entry.id, UnmetRequirement::ChannelBinding);     // P2
    }
    RoleDecision::peer(&entry.id)
}
```

**No claim is examined.** `UnmetRequirement` has exactly two variants; there is no P3 in the type.
The carriage judges the claim only _afterwards_, with the role already fixed as an input
(`connector-peer-http/src/accept.rs:331` — `judge_claim(&self, role, request)`). Three doc comments
still teach the retired rule: `role.rs:29` says _"A proven peering: P1 and P2 both held"_, and
`decision.rs:20-33` presents the secret as a security requirement.

**The consequence today is the weaker check gating the stronger one.** A peer whose secret is stale
is downgraded to `client` on the strength of a shared string, while the signature that actually
proves who they are is never consulted for the decision. That is not a documentation defect. It is
the security property inverted, and this record's first job is to state that it is live.

## Why the secret earns nothing once P3 decides

**P3 resolves to exactly one relation, with no help from a name.** A `channel_id` may appear in at
most one `[[peer_channels]]` row — a second is `PeerChannelDuplicate` at load
(`peer_channel.rs:285-293`) — and a channel in `[[peer_channels]]` may never also appear in
`[[client_channels]]` (`ChannelInBothNamespaces`). A verified claim therefore names one channel, one
row and one peer id. The credential's only remaining structural job — telling the connector which
peering to evaluate against — is done by the claim itself, at no cost.

**It contradicts a rule this repo already holds.** ADR 0008 splits the operator surface so that a
shared secret gates reads only: _"no shared secret is ever sufficient to move value."_ A peering
moves value. The credential was the last place a symmetric secret sat on that path, and the argument
that removed it from the operator surface applies here unchanged.

**It is the one input that cannot come from a document.** A self-description is public by
construction; a shared secret is bilateral by definition. So long as `credential` is required of every
peering, [ADR 0058](0058-a-peering-is-established-from-a-url.md) cannot be true — a URL will always be
half an answer, and the missing half will always be the one that needs a side channel.

**Two mechanisms for one guarantee is a fault this folder already names.**
[ADR 0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) retired a declared floor for
being _"a restatement, in an advisory field, of a property the claim amounts already carry and
enforce"_, citing ADR 0010's own objection: _"one guarantee two mechanisms that can disagree."_ The
same objection retires the credential. Here the two mechanisms **do** disagree, which is what
`decision.rs` demonstrates.

## What is genuinely lost, and what replaces it

**The `peer_auth_refused` diagnostic.** §1.6 exists to prevent _"peering configured, nothing peers, no
error anywhere"_, and `UnmetRequirement::ProvenCredential` gives an operator a precise, actionable
sentence: your peer's secret is stale. Deleting P1 deletes that sentence, and a misconfigured peering
would otherwise present only as "my packets are being greeted" — the vague symptom §1.6 was written
against.

**It is replaced, not dropped.** `verify_signature`
(`connector-runtime/src/claim.rs:1055-1089`) already distinguishes the two failures precisely:
`UnknownChannel` for a channel this node holds no record of, and `SignatureInvalid` for a signature
that does not recover to the configured key. Those map onto the same operator event, under the same
rate limit, with `UnmetRequirement` becoming `ChannelBinding` and `ClaimSignature`. The diagnostic
gets _better_: "a claim arrived for a channel I do not have" and "a claim arrived signed by the wrong
key" are two different operator fixes, where a secret mismatch was one bucket for both.

**Emitting it must land with the deletion, not after.** Removing P1 first and adding the replacement
event later reproduces exactly the silence §1.6 forbids.

**A cheap kill switch.** Revoking a secret cut a peering dead without touching a key that also signs
claims. With the secret gone, the lever is `DELETE /peers` — an operator write on a durable runtime
table ([0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md),
[0058](0058-a-peering-is-established-from-a-url.md)) — which is immediate, does not require a restart,
and is auditable. That is a better lever than the one being removed; it did not exist when the
credential was designed.

## The decision

1. **Role is P2 + P3**, in code, as `peer-carriage-spec.md` §1.2 already states. `decide_role` takes
   the claim and the peer's channel bindings; `UnmetRequirement::ProvenCredential` is deleted and
   `ClaimSignature` replaces it.
2. **The wire field is deleted** on both carriages together — `Toon-Peer-Auth` and the `auth`
   protocolData entry — because peer behaviour that exists on one carriage and not the other is a
   defect rather than a property of the carriage
   ([0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)).
   A **receiving** connector ignores the header if it arrives, so a stale dialer degrades to `client`
   rather than to a `400`.
3. **`[[peers]].credential` becomes a tombstone**, parsed in order to be rejected by name
   ([0009](0009-one-typed-config-file-no-environment-layer.md)), alongside `addr`, `ceiling`,
   `flush_interval_ms` and `claim_enforcement` in the same struct. A node whose committed TOML still
   sets it stops at boot, by name.
4. **There is no replacement credential.** Not renamed, not demoted to a label, not kept as an
   optional discriminator. A second identifier for a relation the claim already names is the fault
   above, rebuilt smaller.

## Rejected: keep it, renamed, as a non-security discriminator

Keeping the field as a pre-packet hint — "which peering to evaluate against" — was considered, on the
grounds that a BTP session could then take its role at the websocket upgrade rather than at its first
packet.

Rejected on two counts. It is not needed: P3 resolves to one relation without it. And the role it
would grant early is one that nothing can act on, because under
[0042](0042-a-packet-carries-its-claim.md) and
[0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md) a peer PREPARE with no
covering claim is greeted anyway. An unauthenticated websocket that has not yet sent a packet is a
`client` session, which the client edge already serves to anyone — so nothing is admitted that was not
already admitted, and a field is kept for a distinction with no consequence.

**Role attaches to a packet's evidence rather than to a session's greeting.** `CONTEXT.md`'s
**Interaction** entry — "the unit a role attaches to: one BTP session… or one HTTP request" — narrows
accordingly: a session's role is now whatever its current frame proves, which is the shape #868 chose
when it made role and payment read from the same bytes.

## The sweep

**Does not survive:**

- **`peer-carriage-spec.md` §1.4** — the presentation table's credential row, both encodings, and
  the base64 layer that existed only for it. §1.2's superseded-P1 argument is **kept, dated and
  marked**, exactly as that section already keeps it: issue #863 was filed because the argument was
  absent, and deleting it now recreates that gap.
- **§1.6** — retained as a requirement, rewritten as to which failures it reports.
- **§1.9's case 3** — _"a correct `peerId` with a wrong `secret`"_ — stops being expressible. The
  other four regression cases still classify `client` and their reasons are unchanged.
- **`PEER_AUTH_PROTOCOL_ENTRY`, `PEER_AUTH_HEADER`, `CarriageNames`, `PresentedCredential`,
  `PeerCredential`, `RawPeerCredential`** and `ConfigError::PeerCredentialAmbiguous`.
- **`vectors/wire-vectors.json`**'s `peer_carriage` entries that carry a credential. **Cross-repo**
  ([0021](0021-vectors-are-normative-prose-is-not.md)) — regenerate with
  `cargo run -p connector-vectors --bin generate-vectors`.
- **`local/keys.sh`**'s per-peering secret, and the `credential` blocks in every `local/` topology
  and both `infra/linode-*` configs.

**Survives unchanged:**

- **[0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)**'s finding
  that peer semantics survive the transport — which is why the deletion lands on both carriages at
  once.
- **[0042](0042-a-packet-carries-its-claim.md)** and
  **[0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md)** — a packet
  carries its claim, and an uncovered peer PREPARE is greeted. This record is what makes role read
  from those same bytes rather than from a session's first frame.
- **[0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)** — what a claim signs, and that
  its signature is checked against this connector's own record of the channel and _"never the claim's
  own self-declared field"_ (`claim.rs:439-443`). That sentence is now the whole of peer
  authentication.
- **§1.5's anti-escalation rules and §1.7's capability enumeration** — what each role grants is
  untouched; only how `peer` is reached changes.
- **`[[client_identities]]`** and the client edge's own bearer path. This record is about the peer
  surface; a client identity is a different object with a different purpose.

## Consequences

**A peering has one secret fewer, and one key doing more.** The counterparty key already had to be
right for any value to move; now it is also the whole of identity. That is a concentration, and it is
deliberate: it means there is exactly one thing to get right, and it is the thing an operator was
already required to get right.

**`local/keys.sh` and every topology config lose a file and a field.** Onboarding a peer in a local
topology stops requiring the two nodes to agree a string out of band.

**This is a breaking wire and config change, landed together.** The config key becomes a tombstone in
the same release that stops the dialer sending the field, and a receiver ignores an arriving header
throughout — so the two ends may be upgraded in either order without a peering going dark mid-flight.

**It unblocks [0058](0058-a-peering-is-established-from-a-url.md).** With the secret gone, everything
a peering needs about the counterparty is either in the public self-description or derivable from it
([0059](0059-a-channel-is-derived-from-its-participants.md)) — which is what makes a URL a complete
answer rather than half of one.

## Update (issue #1157) — this record's falsifier was mis-specified, and the build is better than it predicted

The falsifier this record carried while unbuilt was:

> `crates/connector-peer-auth/src/**/*.rs` matching `\bcounterparty_key\b` — role decided by a
> verified claim requires the deciding crate to hold the key that claim is verified against.

**It never fired, and it should not have.** `counterparty_key` appears nowhere in
`connector-peer-auth`, and the record is still built. The premise was wrong: deciding role from a
verified claim does **not** require the deciding crate to hold the key.

What shipped instead is a two-line join, `connector_peer_btp::role_gate::decide`. It asks the
connector for a verdict — `Verified`, `UnknownChannel` or `SignatureInvalid` — and hands
`decide_role` a channel id and that verdict, nothing more. The counterparty key stays in
`ClaimBook`, where `[[peer_channels]]` already put it.

That is not merely an equivalent arrangement; it is the one `peer-carriage-spec.md` §1.3 requires.
That section enumerates what MUST NOT enter the role decision, and a decision function holding
verification keys is a decision function that can be tempted to verify. Keeping `decide_role` over a
channel id and a verdict is what makes §1.3 checkable by reading one signature.

**The lesson for the next record: a falsifier must name a fact the decision forces, not a shape the
author imagined the code would take.** A good one for this record would have named the retired
symbol — `UnmetRequirement::ProvenCredential`, which the decision genuinely deletes — rather than a
symbol the author guessed would appear. The convention's own instruction says to pick a pattern the
implementation cannot avoid; this one picked a pattern the implementation was right to avoid.

## Update (ADR 0075, issue #1371) — accepted 2026-09-27, implementation pending

[0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 5 amends the role
rule. An interaction has role `peer` if and only if it carries **a voucher on a channel bound to
that peer**, or — for a packet that moves no value — **the voucher claim-state challenge** (#1364)
signed by such a channel's voucher signer and unexpired. A channel is bound to a peer when its
voucher signer is the key the peer's self-description publishes or a config row names. This replaces
the `toon-channel` claim, and the client BTP `auth_channel_proof` declaration, as the proof. The
decision this record made — a signature proves a peering, and no shared secret does — stands
unchanged.

Until #1371's implementing steps land, the binary behaves as this record describes above; this
Update states what changes when they do.

**#1377 builds the rule, beside the one above rather than in place of it.** A voucher on a bound
channel, or a challenge (expiring within 300 seconds) for a zero-value packet, now decides `peer` on
both carriages (`connector_peer_btp::role_gate::decide_frame`, `connector_peer_auth::decide_voucher_role`),
and binding a voucher signer to a peering is a runtime operation (`Connector::bind_voucher_signer`).
The `toon-channel` proof stays until #1380 moves the last peering off it, and no node binds a signer
until #1378 (the self-description's key) or #1380 (a config row) supplies one.
`peer-carriage-spec.md` §1.2 states both proofs.
