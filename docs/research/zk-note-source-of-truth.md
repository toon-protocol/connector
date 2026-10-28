# Which crate defines a zk note, and what follows (#1555)

> **Research note, not a decision record.** It decides nothing.
> [ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) (_A client may pay from a zk note_)
> is the record this note serves, and the amendment drafted in §10 is where the decision goes. Two
> questions were settled by the owner on 2026-10-28 in the grilling on #1555, before this note
> was written: toon-protocol/zk_toon's `zk-note` crate is the source of truth for every zk-note
> derivation, pinned by git commit, with its `test-vectors/zk-note.json` as the parity contract
> (option 1); and the decision is recorded as an amendment to ADR 0080. This note answers what
> follows from that. The owner asked for a recommendation on each question. Each one is marked,
> and so is every place where the facts make a recommendation risky or where only the owner can
> decide. Written 2026-10-28. Nothing was built, and no cargo build was run.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-LIVE means
read from GitHub's API on 2026-10-28. CONFIRMED-DOCS means read in a record, issue or design
document. INFERRED means reasoned from the above and not checked by running anything.

**Names.** **A** is the connector's `connector-zk-note`, which exists only on `origin/main`
(`b482f801`, #1526, built for #1521). **B** is zk_toon's `zk-note`.

**Sources pinned.**

- **connector** `origin/main` at `79561fc7` (ADR 0080's 2026-10-28 amendment, #1574). ADR 0080's
  working copy on branch `adr-0077-zk-note` is identical to `origin/main`'s.
- **zk_toon** local checkout `d99eebc` (#25's doc note, the commit the owner named), and
  `origin/main` at **`db978ea`** (#27, _Request proof (unsubsidised)_, merged 2026-10-28 14:18 UTC).
  `db978ea` is newer than the owner's commit. It changes no derivation (§3), and it matters to
  §2, §5 and §10. The repo has no git tags (CONFIRMED-LIVE).
- **o1-labs/proof-systems** `ab84160f` (A's pin) and tag `0.7.0` = `36a8b510` (B's pin).
- **toon-protocol/zk-note-vectors** `658c3d43`, the #1512 spike's fixtures.

---

## Verdict

Option 1 holds up. **B covers everything #1522 and #1523 need from a primitives crate except two
things the connector owns anyway**: the session key's signature over an allowance claim, and
validating the operator's tier list. The proof-systems conflict is not real. Tag `0.7.0` is
`ab84160f` plus a version-number commit, so the connector, its future verifier and B can all use
`0.7.0` with no change to any code that was tested.

What option 1 brings that #1555 did not ask about:

- **B's request proof (zk_toon #27) leaves three checks to the zk edge**, and neither ADR 0080
  nor #1522 lists them: the proof's `vault` key is this zk edge's state key, its `requestTime` is
  close to now, and its subsidy fields are zero. Without the second check, a payer can backdate a
  request to spend a note that has already expired (§5).
- **zk_toon has no licence** (CONFIRMED-LIVE: no `LICENSE` file, no `license` key in
  `zk-note/Cargo.toml`, GitHub reports `license: null`). The connector is MIT
  (`Cargo.toml:37`). Only the owner can fix this, and it should be fixed before the connector
  depends on the crate (§11).
- **#27 dropped the stability note that #25 had just added** to `zk-note/src/lib.rs`
  (`git diff d99eebc db978ea -- zk-note/src/lib.rs`), which looks like a merge from a stale
  branch. #25's to-do list is undone on `main` (§3).

| #   | Question                         | Recommendation (short)                                                                                                                                   |
| --- | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Fate of `connector-zk-note`      | Keep the crate as a thin boundary: re-export B, delete every derivation, keep the session-key verify and tier validation                                 |
| 2   | proof-systems revision           | Use tag `0.7.0` everywhere, including #1514's verifier. It is code-identical to `ab84160f`                                                               |
| 3   | Pin mechanism                    | A git `rev` now, at `db978ea` or later. Don't wait for a tag. The repo is public, so CI and Docker need nothing new                                      |
| 4   | Tiers and lifetime               | Config keeps tiers, refused at boot unless they are a subset of B's `TIERS`. Allowance lifetime and `NOTE_LIFETIME_MS` are unrelated                     |
| 5   | `b` and the binding tag          | `b` is the tier. The difference is in naming, not substance                                                                                              |
| 6   | Period nullifier, genesis anchor | The zk edge checks and records period nullifiers, but only when subsidy is built. Until then it refuses `subsidised`. It never computes a genesis anchor |
| 7   | Successor                        | B covers #1523 item 1 fully. Three phrases in #1523 change                                                                                               |
| 8   | Wire vectors                     | The connector's vectors pin its own wire only, computed through B. A hash-pinned copy of B's file is replayed as a pin-bump tripwire                     |
| 9   | Fixed sizes, `size` in the leaf  | No contradiction. Two optional clarifications                                                                                                            |
| 10  | ADR amendment and issue edits    | Drafted in §10                                                                                                                                           |
| 11  | Undecided                        | Eight items, listed in §11                                                                                                                               |

---

## 1. The fate of `connector-zk-note`

### Facts

- **Nothing in the connector consumes A.** No crate's `Cargo.toml` or source outside
  `crates/connector-zk-note/` names it (CONFIRMED-CODE, `git grep` over `origin/main`). It is a
  workspace member (`Cargo.toml:32`) and is listed in `docs/architecture/source-tree.md:208` and
  `:267`.
- **A's derivations are all incompatible with B's**, as #1555's table says. Each one checked in
  source:
  - **Tags.** A uses 20-byte `toon-zk-note-*` tags through `hashWithPrefix`
    (`src/tags.rs:81-92`, `src/poseidon.rs:30`). B uses `toon.zk.*.v1` tags of up to 31 bytes,
    read as one field element and fed in as Poseidon's first input (`zk-note/src/lib.rs:27-56`
    @`d99eebc`).
  - **Note commitment.** A computes `(secret, anchor, C.x, C.y)` (`src/derive.rs:16`). B computes
    `commit_secret(secret)` (`lib.rs:58`).
  - **Leaf.** A hashes one input (`derive.rs:23`). B hashes five: index, commitment, size, deposit
    time and expiry (`lib.rs:63`).
  - **Binding tag.** A nests a context hash (`derive.rs:36-53`). B uses one flat six-input hash
    (`lib.rs:97`).
  - **Generators.** A's value generator is `hashToGroup` of a seed, and its blinding generator is
    the curve generator (`src/commitment.rs:15-22`). B derives both `G` and `H` by
    try-and-increment over `hash_with_tag("toon.zk.generator.v1", [index, counter])`
    (`zk-note/src/balance.rs:40-65`).
  - **State message.** A signs 4 fields (`src/state.rs:37`). B signs 5:
    `[vault.x, vault.isOdd, C.x, C.y, anchor]` (`zk-note/src/state.rs:42-45`).
- **Both sign the same way:** a chunked nonce under `NetworkId::TESTNET` (A `state.rs:21,80`; B
  `state.rs:64-70`).
- **What A has that B does not:**
  - `group_map::hash_to_group`, a port of mina-rust's conic map. B never uses `hashToGroup`, so
    this is dead under option 1.
  - `tiers::Tiers`, which refuses an empty list, a zero and a duplicate (`src/tiers.rs:41-63`).
  - `state::sign_fields` / `verify_fields`, a Mina Schnorr signature over any list of fields
    (`state.rs:63-75`), and `sign_with`, which exists only for tests.
  - `fixtures/parity.json` and its hash-pinned tests.
- **What #1522 needs from a primitives crate**, from its body:
  - **The binding check** (item 4): B's `auth_tag` and `RequestContext`.
  - **Verifying an allowance claim's signature** (item 5): "wrongly signed claims" must be refused,
    and the session key is a Mina key, given in B's `RequestContext` as
    `(session_key_x, session_key_is_odd)`. **B has no general-purpose Mina signature verify**. Its
    only verify is `verify_state`, which is tied to the state message (`state.rs:73-85`). The bytes
    a session key signs are the connector's wire (#1523 item 6, "the allowance claim's signed
    bytes"). No crate defines them yet.
  - **Validating the tiers** in config (item 3).
- **What #1523 needs** (item 1): `charge`, `next_anchor`, `StateMessage` and `sign_state`. All are
  in B (§7).

### Recommendation

**Keep `connector-zk-note` as a thin boundary crate, and delete everything in it that B defines.**

- **Delete:** `tags.rs`, `derive.rs`, `commitment.rs`, `group_map.rs` (its Apache-2.0
  attribution goes with it), `poseidon.rs`, `state.rs`'s `StateMessage` and `sign_state`,
  `fixtures/parity.json`, `tests/parity.rs`, and the derivation half of `tests/properties.rs`.
- **Keep, connector-owned:**
  - a `verify_fields` (and a test-only `sign_fields`) for the session key's signature over the
    allowance claim's signed bytes. Those bytes are defined in this crate and pinned in
    `vectors/wire-vectors.json`;
  - `Tiers`, which now also refuses a tier that is not in `zk_note::policy::TIERS` (§4).
- **Re-export** `zk_note` as `connector_zk_note::note`, or similar, so that exactly one manifest in
  the workspace pins zk_toon and proof-systems.

Why not delete the crate outright: the session-key verify and tier validation would then go into
`connector-domain` or `connector-config`, and either would take the proof-systems dependency tree.
Keeping one leaf crate keeps that tree off every other crate's path. #1514's verifier is a
separate, heavier crate (spike §1, "the dependency tree"). It should depend on the same pin and not
on this crate.

**Risk:** low. Nothing consumes A, so the deletion breaks nothing.

---

## 2. The proof-systems revision

### Facts

- **A pins `rev = ab84160f`** on five crates (`crates/connector-zk-note/Cargo.toml:17-21`).
  `Cargo.lock` resolves them as `?rev=ab84160f…` (`Cargo.lock:3522`).
- **B pins `tag = "0.7.0"`** on four crates with `default-features = false`
  (`zk-note/Cargo.toml:8-14`). zk_toon's lock resolves this as `?tag=0.7.0#36a8b510`.
- **Why A chose `ab84160f`:** it is the commit the live Mina daemon `4.0.0-mainnet-mesa` pins
  (spike note lines 24 and 148). The spike's Mesa verifier was built by repinning mina-rust to that
  commit (`rust/patches/mina-rust-v0.19.0-mesa-proof-systems-ab84160.patch` at
  `zk-note-vectors@658c3d43`). A verifier build serves one Mina era (spike line 183).
- **`0.7.0` is `ab84160f` plus a release commit.** GitHub's compare of `ab84160f...0.7.0` shows
  two commits ahead and none behind: `release v0.7.0` (`99943680`) and its merge (`36a8b510`). It
  changes seven files: `CHANGELOG.md`, `Cargo.lock`, the workspace `version` (`0.6.0` → `0.7.0`)
  in `Cargo.toml`, and four generated JS comment headers. No Rust source changes
  (CONFIRMED-LIVE). The spike note already calls it "tag `0.7.0` less two commits" (line 24).
- **One workspace, two spellings, two copies.** Cargo treats `?rev=ab84160f` and `?tag=0.7.0` as
  different sources. If the connector kept A's spelling for the verifier and took B's through the
  dependency, the lockfile would hold two copies of `mina-curves`. Their `Fp` would be distinct
  types, so a field from B could not be passed to the verifier without converting it (INFERRED
  from Cargo's source identity; not built).
- **o1js versions agree.** zk_toon's circuits use o1js `3.1.0`
  (`o1js/package.json:11` @`db978ea`), the Mesa release the spike verified with the `ab84160f`
  repin (spike §1).
- **MSRV.** proof-systems' workspace declares `rust-version = "1.92"`, but none of the five member
  crates inherits it (CONFIRMED-LIVE, their `Cargo.toml` at `0.7.0`). Cargo will therefore not
  refuse an older compiler, but the code may still need one. zk_toon builds on `1.92`
  (`rust-toolchain.toml`). The connector's image builds on `rust:1.90-alpine3.22`
  (`deploy/connector-rust/Dockerfile:54`), and no connector build has compiled proof-systems yet:
  the image builds `-p connector`, which does not depend on A. CI uses `stable`.

### Recommendation

**Spell it `tag = "0.7.0"` everywhere in the connector, including #1514's vendored verifier.**
That is one source, B's spelling, and the same code the spike verified. #1514 should still rerun
its committed o1js fixture at `0.7.0`. It will do so anyway, because the fixture is its contract
test.

**If a fork ever splits them** (the verifier needs a commit that B has not moved to), B repins.
B's primitives are Poseidon, Pallas and the signer, none of which a Mesa-style kimchi gate change
touches. B can follow the daemon's pin at no cost, while the verifier cannot avoid following it. So
the rule for the amendment is: _zk_toon pins proof-systems where the live Mina daemon does, and the
connector's verifier pins the same._ A `[patch]` in the connector redirecting B's proof-systems is
a fallback only. Cargo refuses a patch that points at the same source it patches, so it would need
a different URL spelling (INFERRED, not tested here).

**Risk:** low for the revision. **Medium for the image.** When #1522 makes the binary depend on
the crate, the image's Rust `1.90` has to compile proof-systems `0.7.0`, and no one has tried it.
#1522 should raise the Dockerfile's Rust to at least `1.92` if the build needs it, or say that it
does not.

---

## 3. The pin mechanism

### Facts

- **toon-protocol/zk_toon is public** (`gh repo view toon-protocol/zk_toon --json visibility`
  returns `"PUBLIC"`, CONFIRMED-LIVE). So are `zk-note-vectors` and `connector`.
- **Every fetch path already works for a public git dependency, because A has one:**
  - **CI** caches `~/.cargo/git` (`.github/workflows/ci.yml:268-275`) and passes no credential to
    cargo.
  - **The image** builds with `cargo chef cook --release --locked` and `cargo build --release
--locked` (`deploy/connector-rust/Dockerfile:108`, `:118`) and has no SSH or secret mounts. It
    fetches proof-systems over HTTPS for A today.
  - Nothing changes for a second public HTTPS git source.
- **zk_toon has no tags.** #25 proposed tagging `zk-note-v0.1.0` once #1555 chose option 1
  (zk_toon#25, "To do here"). It has not been done.
- **The derivations are unchanged between `d99eebc` and `db978ea`.** #27 adds
  `o1js/src/request*.ts`, `zk-note/tests/request.rs` and a `requestProofs` section in
  `test-vectors/zk-note.json`. In `zk-note/src/` it only deletes the "Stability" doc block that #25
  had added (`git diff d99eebc db978ea -- zk-note test-vectors`, CONFIRMED-CODE).
- **`test-vectors/zk-note.json` is at the repository root, outside the crate**
  (`zk-note/tests/vectors.rs:7` reads it through `CARGO_MANIFEST_DIR/..`). Cargo checks out the
  whole repository for a git dependency, so the file is present beside the crate. Nothing exposes
  it as an API, though.

### Recommendation

**Pin by full `rev` now, at `db978ea` or later. Don't wait for the tag.**

- A `rev` is immutable. A tag can be moved, although `Cargo.lock` would still hold the old commit.
  Cutting `zk-note-v0.1.0` later is harmless, but it adds nothing the lock does not already
  guarantee.
- Pin `db978ea` rather than `d99eebc`. It carries the request proof's public-input layout and its
  sample vectors, which #1514's verifier and #1522's admission both read, and its derivations are
  the same.
- Ask zk_toon to put #25's stability note back. Optionally, ask it to export the vector file as
  `pub const VECTORS: &str = include_str!("../../test-vectors/zk-note.json")`, so a consumer reads
  the vectors of exactly the commit it pins (§8).

**Risk:** low. The licence (§11) is the blocker, not the mechanism.

---

## 4. Tiers, and allowance lifetime

### Facts

- **B's `TIERS` are `[0.01, 0.1, 1]` MINA in nanomina**, marked "provisional" pending zk_toon #2
  (`zk-note/src/policy.rs:4,14-15`). `DEPOSIT_SIZES` are `[10, 100, 1000]` MINA (`:17`).
- **The unsubsidised circuit does not check that `tier` is in `TIERS`.** It range-checks the tier
  to 64 bits and checks `balance ≥ tier` (`o1js/src/request.ts:70-72` @`db978ea`). `request.ts`
  does not import `TIERS`, and `TIERS` appears only in `policy.ts`, `request-sample.ts` and
  `vectors.ts` (CONFIRMED-CODE, `git grep`). zk_toon's own design gives the check to the zk edge
  ("`b` is a published tier", `docs/design/zkapi-payments.md:80`).
- **The subsidised circuit will check** that `b` is the smallest tier (zk_toon #12; ADR 0001; #2
  "When subsidised"). The vault's on-chain state holds the deposit sizes but no tiers (#2, "On-chain
  state").
- **The connector intends the operator to choose tiers.** #1511 user story 28 says "publish only
  the tiers and allowance lifetime I choose". #1522 item 3 names the tiers in config. #1523 item 3
  puts them in the greeting.
- **`NOTE_LIFETIME_MS` is 180 days from deposit** (`policy.rs:7,32-34`), checked in circuit as
  `requestTime < expiry` (`request.ts:65-67`). It is the note's life. The allowance lifetime is the
  longest session a zk edge will grant: a different quantity on a different clock. The allowance's
  own expiry (`requestExpiry`) is private, and the proof binds it only through the tag
  (`request.ts:52,88-91`). Nothing in the circuit bounds it.
- **`CHALLENGE_WINDOW_MS` is 2 days** (`policy.rs:11`).

### Recommendation

- **Config keeps `tiers`.** Boot refuses the table unless the tiers are a non-empty subset of
  `zk_note::policy::TIERS`. Once subsidy is built, a table with a non-zero subsidy budget must also
  include `TIERS[0]`.
  - The subset rule keeps every zk edge on the crowd's published values. A payer cannot be singled
    out by an unusual tier, and the circuit's smallest tier stays one the edge actually offers.
  - It also keeps user story 28.
  - The greeting publishes the configured subset.
- **The allowance lifetime stays connector config**, unrelated to `NOTE_LIFETIME_MS`.
- **Suggestion for #1522:** refuse a lifetime that is not well below `CHALLENGE_WINDOW_MS`. A
  zk edge has to close an allowance, journal Δ and get its evidence to the challenger inside that
  window.

**Owner must decide:** B's tiers are provisional (zk_toon #2, open values), so any value written
now may change. Requiring a subset ties a zk edge's boot to a constant in a crate whose API is
still moving. The alternative is to drop tiers from config and publish B's `TIERS` whole. That is
simpler and gives one crowd, but it loses user story 28.

---

## 5. The binding tag, `b` and the tier

### Facts

- **B's tag binds `(N, edge, key_x, key_is_odd, tier, expiry)`** (`lib.rs:97-109` @`d99eebc`).
  The circuit forces the tag's tier to equal the public `tier`, and the comment calls it "the tier
  `b` the balance must cover" (`request.ts:55,88-91`). zk_toon's policy comment reads "Request tiers
  `b`" (`policy.rs:14`).
- **A's context bound `b` under the name `budget`** (`derive.rs:36`).
- **#1522 item 5 caps an allowance claim at `b`**, and ADR 0080 decision 2 says an allowance
  "must not exceed the budget".
- **B's public inputs**, in order (`request.ts:17-29`; zk_toon#7's triage brief):
  1. `vault` (a `PublicKey`)
  2. `root`
  3. `requestTime`
  4. `tier`
  5. `nullifier`
  6. `tag`
  7. `commitment` (re-randomised, a `Group`)
  8. `subsidised`
  9. `period`
  10. `periodNullifier`

  As o1js flattens a `Struct`, that is 12 field elements: `PublicKey` → `x, isOdd`, and `Group` →
  `x, y` (INFERRED from o1js's types; #1514's fixture is what confirms it). `edge`, the session
  key and the allowance expiry are **private** (`request.ts:48-52`). zk_toon#7's brief records this
  as a triage decision: "The zk edge enforces binding to its own context by recomputing the tag".

- **`vault` is the zk edge's state key, not the zkApp address.** The circuit verifies a signed
  state against `pub.vault` (`request.ts:81`). zk_toon's glossary defines the vault key as "the
  Mina public key the vault accepts state signatures from" (`GLOSSARY.md:29-30`).
- **The circuit checks the note against `requestTime`** (`request.ts:65-67`), but nothing in the
  proof ties `requestTime` to the real clock.

### Recommendation

**`b` is the tier.** The budget an allowance grants equals the tier its proof covers, and the
claim cap in #1522 is that tier. Nothing needs reconciling beyond naming: say "the tier `b`" in
ADR 0080 and the issues.

**Three admission checks follow from B's layout. Add them to ADR 0080 decision 3 and #1522 item 4,
all before the tag:**

1. **The proof's `vault` equals the zk edge's own state public key.** Otherwise the proof can be
   about a state signed by someone else's key.
2. **`requestTime` is within a bounded window of the zk edge's clock.** Otherwise a payer can
   backdate the request and open an allowance on a note that expired after the backdated time. At
   that point its balance belongs to the operator's treasury (zk_toon #11). **This is the one gap
   with money on it.** The window's width is #1522's choice. It should be a few minutes, because
   the payer picks `requestTime` and proves after picking it.
3. **`subsidised` is false and `period` and `periodNullifier` are zero**, until subsidy is built
   (§6).

**What `edge` is, as a field element, is undefined.** B's vectors use `424242`
(`o1js/src/vectors.ts:89`), and A's guess (`derive.rs:36`) was never specified either.

- **Recommendation:** `edge` is the x-coordinate of the zk edge's state public key. That needs no
  new derivation, since it is already a field. It is already in the greeting (#1523 item 3,
  `extra`).
- A hash of the `[signer]` identity key would need a new tag, and by §10 decision 12 a new tag
  belongs in B.
- **Owner must confirm.** This is wire law that every wallet has to compute.

---

## 6. Period nullifiers and the genesis anchor

### Facts

- **zk_toon ADR 0001 gives the connector three jobs:** "which party pays an allowance's charge (the
  note or the subsidy budget), checking the per-period nullifier, and stopping subsidised
  allowances once the budget is spent" (`docs/adr/0001-…md:106-109`).
- **The owner's #1511 comment** ("Additions after design review") says the zk edge refuses a
  subsidised proof unless:
  - the period is current;
  - the per-period nullifier is unseen;
  - the tier is the smallest;
  - the budget is open.

  It also says the zk edge records the per-period nullifier durably before it grants.

- **ADR 0080 says nothing about subsidy** (CONFIRMED-DOCS: no "subsid" in the record). Neither
  does #1522.
- **The subsidised method is not built** (zk_toon #12, open). It will be "a second method on the
  same program" (zk_toon#7 brief). A `ZkProgram` has one verification key, so **once #12 lands, a
  subsidised proof verifies under the key the zk edge pins** (INFERRED from o1js's one key per
  program).
- **The period** is `(now − vault_creation) / SUBSIDY_PERIOD_MS` (`policy.rs:37-39`). The vault
  stores its creation time on chain (#2).
- **The genesis anchor is `genesis_anchor(commit_secret(secret))`** (`lib.rs:71`;
  `request.ts:77`). It needs the note's secret.
  - The connector never has the secret, so it never computes a genesis anchor, a nullifier, a note
    commitment or a leaf.
  - **What the connector computes from B** is `auth_tag`, `next_anchor`, `charge`, `StateMessage`
    and `sign_state`, plus `policy::subsidy_period` and `TIERS` once subsidy is built.

### Recommendation

- **Period nullifiers:** yes, the zk edge must check and record them, exactly as the #1511
  comment says, but as **later work**. Keep subsidy out of #1522 and #1523, and give it its own
  issue blocked by zk_toon #12.
- **Until then, admission refuses `subsidised = true` by name** (§5, check 3). Otherwise #12's
  first merge would let a subsidised proof in through the unsubsidised gate, and the note would be
  charged for an allowance the subsidy was meant to pay. ADR 0080 needs a sentence for this
  (§10).
- **Genesis anchor:** the connector never computes it. Nothing to build.

**Owner must confirm** that subsidy is deferred out of #1522 and #1523 and not folded in. ADR 0080
is silent on it today, so deferring it needs either a sentence in ADR 0080 or a record of its own
when it is built.

---

## 7. The successor (#1523 item 1)

### Facts

- **The payer re-randomises before proving.** The public `commitment` is already
  `rerandomise(state commitment, r')` (`request.ts:93-95`; `balance.rs:72-75`). The zk edge never
  calls `rerandomise`.
- **The zk edge's successor uses only functions B already has:**
  - **Commitment:** `charge(C_pub, Δ) = C_pub − Δ·G` (`balance.rs:77-80`). `G` is B's
    _balance_ generator, and `H` blinds (`balance.rs:1-20,55-70`). The "value generator `G_v`" is
    A's name.
  - **Anchor:** `next_anchor(N) = Poseidon([tag("toon.zk.anchor.v1"), N])` (`lib.rs:80-83`).
  - **Message:** `StateMessage::new(state key, commitment, anchor)`, five fields
    (`state.rs:36-46`).
  - **Signature:** `sign_state`, with a chunked nonce under `TESTNET` (`state.rs:63-70`).
- **B's tests pin every step:** `charged`, `rerandomised` and `stateSignatures` (with
  `mainnetState`, refused) in `test-vectors/zk-note.json`. The request-proof samples show a
  successor's anchor feeding the next proof (`requestProofs[1].private.prevNullifier`).

### Recommendation

**B fully covers #1523 item 1.** Change #1523's text:

- "the re-randomised commitment minus Δ·G_v" → "`charge(C, Δ) = C − Δ·G`, where `C` is the
  proof's public re-randomised commitment and `G` is `zk_note`'s balance generator";
- "`Poseidon(TAG_ANCHOR, N)`" → "`zk_note::next_anchor(N)`";
- "signs `(vault, commitment x, commitment y, anchor)`" (#1511, "State advance") → "signs
  `zk_note::state::StateMessage`: `[vault.x, vault.isOdd, C.x, C.y, anchor]`, where `vault` is the
  zk edge's state public key";
- item 6, "the 20-byte domain tags … as pinned conventions" → delete. B's tags never go through
  `hashWithPrefix` (zk_toon#25, answer 2), so the 20-byte convention does not apply. The
  devnet-domain, chunked-nonce convention stays.

---

## 8. Wire vectors

### Facts

- **ADR 0021:** the Rust implementation defines the wire, and `vectors/wire-vectors.json` is the
  cross-repo contract (`docs/adr/0021-…md:1-30`). It is now at `schema_version` 9 and has no
  zk-note entries (CONFIRMED-CODE).
- **ADR 0080** makes the allowance claim, the allowance request and answer, and the signed
  successor into vectors (`0080…md:3`). Its 2026-10-28 amendment keeps the asset out of them
  (`:173-175`).
- **B's file is the parity contract for derivations** (owner, Q1; zk_toon#25, answer 3). B's CI
  replays it in Rust and o1js and greps `vector cases run: N > 0` (`zk_toon/.github/workflows/ci.yml:23-32`).
- **toon-client's wallet will depend on B for derivations** (#2 user story 32), and on the
  connector's vectors for the wire (ADR 0021).

### Recommendation

**Don't re-pin B's derived values in the connector's vectors as normative entries.** That would
make two files normative for the same bytes, which is the drift option 1 exists to end.

- **`wire-vectors.json` pins the connector's wire only:**
  - the zk-note claim JSON;
  - the allowance request and answer;
  - the allowance claim's signed bytes and its session-key `(r, s)`;
  - the signed successor's five fields and `(r, s)`.

  Every value is generated by `connector-vectors` calling the pinned B.

- **Each zk-note section names the zk_toon commit it was generated at**, so a reader can tell
  which parity contract it sits on.
- **Add one parity test** in `connector-zk-note`. It holds a copy of B's
  `test-vectors/zk-note.json` at the pinned commit, pinned by SHA-256 the way `tests/parity.rs`
  pins `parity.json` today (`parity.rs:17-21`). It replays the entries the connector depends on
  through the connector's call sites: `authTags`, `nullifiers.nextAnchors`, `charged`,
  `stateSignatures`, and the mainnet refusal.
  - It is a tripwire. A pin bump that changes a derivation changes the copy's hash, and a person
    has to read the diff.
  - If zk_toon exports `VECTORS` (§3), read that constant and drop the copy.

---

## 9. Fixed deposit sizes and `size` in the leaf

### Facts

- **B's leaf commits to `size` in its preimage** (`lib.rs:62-68`). The leaf itself is a hash. The
  deposit's size is public anyway: a deposit moves exactly `size` and dispatches an action that
  carries it (zk_toon#6 brief). In the request proof, `size` is a private input
  (`request.ts:34`).
- **ADR 0080 already says deposits come in the vault's fixed sizes**, citing zk_toon#6
  (`0080…md:170-172`).
- **CONTEXT.md:**
  - _Zk note_ is "a deposit in a vault … covers an amount" (`CONTEXT.md:379-383`);
  - _Allowance_ is "an amount from a published set of tiers" (`:389-394`);
  - _Open budget_ (`:402-407`) and _State advance_ (`:409-412`) say nothing about sizes.
- **ADR 0080 puts the anonymity set at "the active notes in one vault"** (`:163-164`). zk_toon's
  design puts it at "the unspent notes of the same size in one vault" (`zkapi-payments.md:147`).

### Recommendation

**Nothing contradicts.** The two anonymity-set sentences are about different observers. The zk
edge never sees `size`, because it is private in the proof, so ADR 0080's sentence is right for
the zk edge. zk_toon's is right for a chain observer matching deposits to withdrawals. Optional
clarifications, not required:

- **ADR 0080, "What still leaks":** add "A deposit's size and a withdrawal are public on chain, so
  against a chain observer the set is the unspent notes of the same size." Once subsidy is built,
  add "whether an allowance is subsidised, which reveals a note at least 7 days old"
  (`zkapi-payments.md:144`).
- **CONTEXT.md, _Zk note_:** "A deposit **of one of the vault's fixed sizes** in a vault…", and
  "It expires 180 days after deposit."
- **ADR 0080, Consequences:** one line saying that a note's remainder goes to the operator's
  treasury at expiry (zk_toon #11). It is a payer-facing fact that no connector record states.

---

## 10. The ADR 0080 amendment, and the issues to edit

Conventions followed: amendments are dated and attributed inline
(`_Amended 2026-10-28 (owner decision, #1513):_`, `0080…md:69`), the Status line records each one
(`:3`), and the index row in `docs/adr/README.md:157` names what changed. Its status stays
**Accepted**: the change is to the record itself, not by a later record.

### Draft

**Status line**, appended after the #1513 sentence:

> **Amended 2026-10-28** (owner decision, #1555): decision 12 makes toon-protocol/zk_toon's
> `zk-note` crate the definition of every zk-note derivation, and decision 3 gains the checks that
> crate's request proof leaves to the zk edge.

**New falsifier**, after the two existing ones:

> **Falsifier:** `crates/**/*.rs` matching `"toon\.zk\.` — a zk-note domain tag written out in
> the connector means a derivation was redefined here instead of taken from zk_toon.

(The pattern needs the dot form. `toon-zk-note-` would also match decision 9's SOCKS username
`toon-zk-note-mina`.)

**Decision 3**, appended:

> _Amended 2026-10-28 (owner decision, #1555):_ the proof's public inputs are those of zk_toon's
> request proof: the state key, root, request time, tier, nullifier, authorization tag,
> re-randomised commitment, subsidised flag, period and per-period nullifier. The tier is `b`.
> The proof binds the zk edge, the session key and the expiry only through the tag, and it checks
> the note's expiry only against the request time it is given. So before the tag, admission also
> refuses by name: a state key other than the zk edge's own; a request time outside a short
> window around the zk edge's clock; and a subsidised flag, period or per-period nullifier that is
> set. The last refusal stands until subsidised allowances are built.

**New decision 12**:

> 12. **The derivations are zk_toon's, and the connector pins them.** _Added 2026-10-28 (owner
>     decision, #1555)._ Every value that a payer, the vault's proof program, the challenger and a
>     zk edge must agree on is defined by the `zk-note` crate in
>     [toon-protocol/zk_toon](https://github.com/toon-protocol/zk_toon), next to the proof program
>     that checks it. That covers the domain tags, the note commitment and leaf, the anchors, the
>     nullifier, the authorization tag, the per-period nullifier, the balance commitment's
>     generators and the state message. The connector depends on that crate pinned by git commit
>     and defines none of these values itself. The crate's `test-vectors/zk-note.json`, at the
>     pinned commit, is the parity contract. A pin bump is a reviewed change in this repository,
>     and a bump that changes a derivation breaks every note in a vault. The connector's own
>     wire stays here, in `vectors/wire-vectors.json` ([0021](0021-vectors-are-normative-prose-is-not.md)),
>     with its values computed through the pinned crate: the allowance claim and its signed bytes,
>     the allowance request and answer, and the signed successor. The crate and the verifier
>     share one `proof-systems` source, the one the live Mina daemon pins. For Mesa that is tag
>     `0.7.0`.

**Considered options**, appended:

> - **The connector's own primitives crate** (#1521, built 2026-10-16). Rejected (2026-10-28,
>   #1555): it was built at the same time as zk_toon's and disagrees with it on every derivation.
>   The vault's proof program is written against zk_toon's, so a zk edge built on the connector's
>   would refuse every real allowance at the tag, and the vault would refuse its successors.
> - **Merging the two into one crate in one place.** Rejected (2026-10-28, #1555): the proof
>   program, the challenger and the wallet already build on zk_toon's crate, and a zk edge only
>   checks against them.

**Consequences**: replace "Built in two issues, unblocked by #1512 (answered yes): the primitives,
gate, journal, …" with:

> - Built in two issues, unblocked by #1512 (answered yes): the gate, journal, config, greeting and
>   vectors over an in-memory vault, on zk_toon's primitives, then the Mina adapter run against
>   committed fixtures. Subsidised allowances (zk_toon's ADR 0001) are a third piece of work. They
>   wait for zk_toon's subsidised request proof, and until it lands admission refuses them.

**Index row** (`docs/adr/README.md:157`): append "; amended 2026-10-28 by #1513 (asset) and #1555
(derivations are zk_toon's)".

### Files that would change with the amendment

Listed here only. This note edits none of them.

- `docs/architecture/source-tree.md:208` and `:267`: the crate entry becomes "the connector's
  zk-note boundary: re-exports zk_toon's `zk-note`, the session-key signature, tier validation".
- CONTEXT.md, _Zk note_: optional (§9).

### Issue bodies to edit

Listed only. None was edited.

- **#1555 (close note):** option 1 taken, and recorded as ADR 0080's decision 12. Pin zk_toon at
  `db978ea` or later by `rev`, and proof-systems at tag `0.7.0`. Link this note.
- **#1511:**
  - "Binding": replace the nested `TAG_AUTH`/`TAG_CTX` formula with B's `auth_tag`.
  - "Opening": list B's ten public inputs.
  - "State advance": B's `G`, `next_anchor` and the 5-field message.
  - "zk-note primitives (pure Rust library)": "lives in toon-protocol/zk_toon; the connector pins
    it".
  - "Implementation decisions": the three new admission checks.
- **#1513 (parent):** a note at the top saying that items 1, 6, 7 and 13's formulas are superseded
  by #1555.
- **#1521 (closed):** a comment saying its crate is reduced to a boundary under #1555.
- **#1522:**
  - "Blocked by: #1521" → "Blocked by: #1555; builds on zk_toon `zk-note` at the pinned rev".
  - Item 3: tiers are a subset of `policy::TIERS`, and the lifetime is well below the challenge
    window.
  - Item 4: the three new checks, and the `edge` encoding.
  - Item 5: the allowance claim's signed bytes and their session-key verify, defined in
    `connector-zk-note`.
  - A line that subsidy is out of scope.
  - A line on the Dockerfile's Rust version (§2).
- **#1523:** item 1's three formulas and item 6's 20-byte line (§7). Item 6 also gets "each
  zk-note vector section names its zk_toon commit" and the parity tripwire (§8).
- **#1514:**
  - Pin `tag = "0.7.0"`, and rerun the fixture there.
  - The public input is B's `RequestPublic`: 12 fields, in declaration order.
  - Take the fixtures from zk_toon's `RequestProgram` (`zk-toon-request`) and its published key
    (zk_toon #16), not the spike's stand-in `toon-1512-spend`.
  - Recapture the fixtures when zk_toon #12 changes the program's key.
- **zk_toon#25:**
  - The tag becomes optional, because the connector pins by `rev`.
  - Put back the stability doc that #27 removed.
  - Add a licence.
  - Optionally export `VECTORS`.
  - Add the rule that proof-systems follows the daemon's pin.
- **New connector issue:** subsidised allowances at the zk edge, blocked by zk_toon #12. Scope as
  in the #1511 comment.

---

## 11. Decisions the owner has not been asked

1. **Licence (blocker).** zk_toon has none. Until it does, the connector depends on code that
   grants no rights. Choose a licence compatible with the connector's MIT (MIT or Apache-2.0)
   before #1522 adds the dependency.
2. **Who may change a derivation.** Recommendation: zk_toon only, through its own reviewed change
   with regenerated vectors. The connector takes it by a reviewed pin bump. A derivation change
   after any note exists on a live vault needs a new vault and verification key, never a silent
   bump. That is a rule for zk_toon's records, not this repository's.
3. **API instability.** #7 has landed (`db978ea`). #8–#12 are open, and #12 changes the program's
   key and adds subsidy fields. zk_toon#25 named `auth_tag` as "most likely to move (#7, #12)".
   #7 kept it. #12 may still move it.
4. **Sequencing.** Recommendation: #1522 starts now on `db978ea` with subsidy refused, rather than
   waiting for #12. #1514 waits for #12 or accepts recapturing its fixture. **Risky** if #12
   changes `auth_tag`: #1522 would then rework its binding check, though only that one call.
5. **The `edge` field** (§5): the x-coordinate of the state key is recommended. This is wire law.
6. **The allowance claim's signing domain.** Session-key signatures are never checked inside a
   proof, so the devnet-domain rule does not force anything here. Recommendation: use the same
   `TESTNET` domain and chunked nonce as the state, so a wallet signs one way with one library.
   This is wire law, so the owner decides.
7. **The `requestTime` window** (§5): its width, and whether it is config or a constant.
8. **The evidence file's format.** zk_toon's challenger reads "the zk edge's evidence" through an
   evidence port (#2, "The challenger"). #2's Further Notes calls the export "a cross-spec request
   to #1511". #1522 item 6 defines the file's durability but not its format. Decide which
   repository owns that format before #1522 fixes the bytes.

---

## Decisions taken

These are the recommendations the owner adopted on 2026-10-28.

| Question                     | Recommendation                                                                                                                                        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. `connector-zk-note`       | Thin boundary crate: re-export B; delete every derivation, `group_map` and `parity.json`; keep session-key `verify_fields` and `Tiers`                |
| 2. proof-systems             | Tag `0.7.0` everywhere, verifier included (code-identical to `ab84160f`). On a future split, B follows the daemon's pin                               |
| 3. Pin                       | Git `rev` at `db978ea` or later, now. No credentials needed (public repo)                                                                             |
| 4. Tiers and lifetime        | Config tiers ⊆ `policy::TIERS`, refused at boot otherwise. Allowance lifetime is separate from `NOTE_LIFETIME_MS` and well below the challenge window |
| 5. `b` and the tag           | `b` is the tier. Add vault-key, `requestTime`-window and subsidy-zero checks to admission                                                             |
| 6. Period nullifier, genesis | Deferred to a subsidy issue blocked by zk_toon #12. Refuse `subsidised` until then. The zk edge never computes a genesis anchor                       |
| 7. Successor                 | B covers it: `charge`, `next_anchor`, 5-field `StateMessage`, `sign_state`. Edit #1523's wording                                                      |
| 8. Vectors                   | Connector vectors pin connector wire only, computed through B and naming the zk_toon commit, plus a hash-pinned replay of B's file as a tripwire      |
| 9. Sizes                     | No contradiction. Optional clarifications to ADR 0080 and CONTEXT.md                                                                                  |
| 10. Amendment                | Decision 3 amended, decision 12 added, a falsifier, two considered options, Consequences line (§10)                                                   |

## Owner confirmed

Every item below was confirmed by the owner on 2026-10-28 in the same grilling, and is recorded
in ADR 0080 (decision 3 as amended, decision 12). The `requestTime` window is 10 minutes behind
and 1 minute ahead of the zk edge's clock, as a constant; the licence is MIT.

- [x] **A licence for zk_toon**, before the connector depends on it (§11.1). **Blocker.**
- [x] **The `requestTime` window check**, and its width (§5). This gap has money on it.
- [x] **Config tiers as a subset of B's provisional `TIERS`**, or B's `TIERS` published whole
      (§4).
- [x] **Subsidy deferred**, with `subsidised = true` refused by name in the meantime (§6).
- [x] **`edge` = the state key's x-coordinate** (§5), and the allowance claim's signing domain
      (§11.6). Both are wire law.
- [x] **Pin at `db978ea`** rather than the `d99eebc` named in the session (§3).
- [x] **#1522 starts before zk_toon #12** (§11.4). Risky only if #12 moves `auth_tag`.
- [x] **Which repository owns the evidence file's format** (§11.8).
- [x] **The rule that derivations change only in zk_toon** (§11.2).
- [x] **The Dockerfile's Rust version**, checked in #1522 (§2).
