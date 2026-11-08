# A release that changes a journal's encoding is one-way

**Status:** Accepted (owner decision at triage, 2026-11-06, #1667). It states a rule the tree
already obeys and builds nothing. It **notes** [0086](0086-a-node-settles-on-more-than-one-evm-network.md)
decision 10 as the first release it governs, and adds a duty to the release process of
[0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md).

**Scope:** fleet and operations — how a release is marked and how an operator upgrades across one.
Nothing changes in the binary, on the wire, in the vectors or in any journal's decoding. See the
[ADR index](README.md).

**Falsifier:** none — the rule is about what a release note and a release body say, and about what
the binary refuses to build (a down-migration, a dual-write window), which is an absence of code no
pattern can find without naming every shape it might take.

**A release that changes any journal's on-disk encoding cannot be rolled back. The build before it
refuses the migrated journal by name, and that refusal is correct. The remedy for a bad upgrade is
to roll forward, or to restore a snapshot of the whole state directory taken before the upgrade,
and only if the new build has moved no money since. Every such release says so in its
`docs/releases/` note and in its GitHub Release body.**

## Context

[0086](0086-a-node-settles-on-more-than-one-evm-network.md) decision 10 moved the outbound channel
record to version 2. `OutboundChannels::restore` upgrades each version-1 record by proof and appends
the rewrite under the same key before the node serves anything. A build from before that release
cannot read a version-2 record and refuses to boot:

```text
corrupt journal entry: outbound channel '…' has a record this build cannot read
```

So the first boot of `2026.11.02.1` makes `2026.10.23.1` unbootable on that state directory. No
record, release note or runbook said so when it shipped. Reverting a node repository's pin, which is
the rollback [`fleet-release-and-health.md`](../operators/fleet-release-and-health.md) describes,
does not undo it.

## Decision

1. **A release that changes a journal encoding is one-way.** A journal here means any file the
   node keeps under `state_dir`: today the append-only `outbound-channels.log`,
   `client-edge-claims.log` and `controller-day-totals.journal`, the whole-table snapshot
   `runtime-peers.json`, and any file added later. Changing its
   encoding means writing a record an earlier build cannot decode, whether by migrating old records
   or only by writing new ones in the new form.

2. **The older build's refusal is the correct behaviour, and stays.** A build that skipped a record
   it could not read could sign below an outbound watermark it does not know about, or accept a
   claim below an inbound one. Refusing to boot is the only safe answer, and old releases cannot be
   changed anyway.

3. **Nothing is built to make the rollback possible.** In particular:
   - **No deferred migration or opt-in.** It would not make rollback safe: every channel the new
     build opens is written in the new encoding regardless, so the first channel opened breaks the
     older build whether the old records were migrated or not.
   - **No dual-write window.** The older build refuses what it cannot read (decision 2), so writing
     both encodings gains nothing.
   - **No verb that writes a journal back to an older encoding.** For 0086 it would work only on a
     node with one network per chain family, and every later encoding change would need another.

4. **The remedy for a bad upgrade is to roll forward**: fix the fault in a new release built on the
   new encoding.

5. **Or to restore a snapshot, only if the new build moved no money.** The snapshot is the whole
   `state_dir`, taken with the node stopped, before the first boot of the migrating release.
   Restoring it under the older build is safe **only if the new build signed no outbound voucher
   and accepted no inbound claim** since the snapshot. Otherwise the restored journal is missing the
   new build's watermarks:
   - **Outbound** (`outbound_voucher_signed`, `outbound_watermark_set`): the older build restores
     each channel from the larger of its journal and what the chain shows settled, so it resumes
     below any voucher a payee holds but has not yet settled. Those vouchers add nothing, so its
     payments go uncredited until it catches up.
   - **Inbound** (`client-edge-claims.log`, which since 0075 holds every voucher the node accepts,
     a peer's included): the older build accepts a claim at or below what the new build already
     credited as new value, which serves work twice for one payment.

   Opening or funding a channel does not count as moving money here: it moves the node's own
   collateral into a channel and pays no counterparty. Such a channel is not in the snapshot, so it
   is lost **to the older build**, but the chain still holds its collateral, and the new build's
   journal is kept aside to withdraw it later.

6. **Every such release says so where a node repository bumping its pin will see it**: in a
   `docs/releases/` note, which names the journal, the first release that writes the new encoding,
   and the snapshot step and its condition; and in the GitHub Release body, through
   `release-connector.yml`'s `notes` input, linking that note. The release author is the one who
   knows. The rule lives where that author looks:
   [`fleet-release-and-health.md`](../operators/fleet-release-and-health.md#cutting-a-connector-release)
   and the `notes` input's own description.

## Considered options

- **Amend 0086.** Rejected: 0086 is about EVM networks. The rule covers every future journal change,
  and the release author who needs it will not be reading 0086.
- **Build rollback support** (decision 3's three shapes). Rejected at triage for the reasons given
  there.
- **Mark migrating releases by their handle.** Rejected: 0068 decision 8 names a release by date and
  ordinal and carries no stability claim. A marker in the handle would be the semver this project
  refused.

## Consequences

- `2026.11.02.1` is the first release this record governs. Its note,
  [`multi-evm-networks.md`](../releases/multi-evm-networks.md), carries decision 5's step and
  condition. Its GitHub Release body predates this record and was edited on 2026-11-08 to
  say so and link the note.
- [`docs/guide/operations.md`](../guide/operations.md) tells an operator to snapshot `state_dir`
  before a journal-migrating release and gives decision 5's condition. The fleet runbook's
  "revert the pin" rollback is marked as not a rollback across such a release.
- A change that alters a journal's encoding without a release note saying so is a defect in the
  release, not in the change. Nothing checks this mechanically.
