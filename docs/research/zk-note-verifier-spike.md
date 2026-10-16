# Can the connector verify an o1js proof in Rust? (spike #1512)

> **Research note, not a decision record.** It decides nothing.
> [ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) (_A client may pay from a zk note_,
> numbered 0077 when #1511 and #1512 were filed) is the record this note serves. It was accepted
> on condition of the three answers below, and its status now points here. Evidence comes from running code against artefacts
> o1js produced, from source at pinned commits, and from live read-only queries. Written
> 2026-10-16. Nothing here was built inside the connector, and no o1js dependency entered this
> repository.

**Evidence tags.** RAN means the result comes from code run for this spike, with the output
quoted. CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-LIVE means read from a
live endpoint on 2026-10-16. CONFIRMED-DOCS means read in the maintainer's own documents.

**Sources pinned.**

- **o1js** `3.1.0` (npm, 2026-09-23; targets Mesa) and `2.15.0` (npm, 2026-05-05; the last
  pre-Mesa line). **mina-signer** `4.1.0` (npm) for both.
- **mina-rust** (the Mina Rust node, `o1-labs/mina-rust`)
  [`v0.19.0` = `ed6f4301`](https://github.com/o1-labs/mina-rust/tree/ed6f4301d594d7db066ae71cbcd7f9e34f6e02a1),
  its latest release (2026-01-15). Its lockfile resolves `o1-labs/proof-systems` tag `0.2.0`
  (`e8e02799`).
- **Mina** (OCaml daemon) `3.4.0` (last pre-Mesa; pins proof-systems `0.4.1` = `5f4fa349`) and
  `4.0.0-mainnet-mesa` (pins proof-systems `ab84160f`, which is tag `0.7.0` less two commits).
- **ChainAgnostic namespaces** [`463bae56`](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/mina/caip2.md).
- **The vectors** and everything that produced them: [`toon-protocol/zk-note-vectors@658c3d43`](https://github.com/toon-protocol/zk-note-vectors/tree/658c3d43342e65730fc47c1f6ecb5b65aebcec80) (outside this repository, as #1512
  requires).

---

## Verdict

**Yes to all three.** The connector can verify an o1js `ZkProgram` proof with custom public
inputs in its own Rust process. It needs o1-labs code that already exists, a small patch, and no
o1js, Node or sidecar. A proof from o1js `3.1.0` (Mesa, the protocol live on Mina mainnet since
2026-09-03) and one from o1js `2.15.0` (pre-Mesa) each verified in about **0.1 s**. Each was
refused under a tampered public input, a short one or the wrong key.

Two findings shape the build issues more than the yes does:

- **The verifier is pinned to a hard fork.** The build that verifies pre-Mesa proofs refuses Mesa
  proofs, and the Mesa build refuses pre-Mesa ones. Mesa changed one kimchi gate, so every
  verification key changed with it. A Mina hard fork therefore breaks a zk edge until its verifier
  is repinned **and** the vault's proof program is recompiled. The committed fixture is what
  catches it, exactly as ADR 0080 argues.
- **No maintained upstream verifier exists.** mina-rust, the only Rust Pickles verifier, has been
  **paused** by o1Labs since 2026-07-15. Its last release predates Mesa, and the Mesa port here is
  this spike's own work. The connector would own the verifier it vendors.

| #   | Question                                                                   | Answer                                                                                                                                                                 |
| --- | -------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Can Pickles verification of an o1js `ZkProgram` proof be called from Rust? | **Yes.** mina-rust's verifier, a ~30-line public entry point, a ~40-line pad and, for Mesa, a repin of proof-systems with mechanical API fixes. About 0.1 s per proof. |
| 2   | Do Rust Poseidon, hash-to-curve and Schnorr match o1js byte for byte?      | **Yes**, with two conventions a Rust caller has to follow (below).                                                                                                     |
| 3   | Which CAIP-2 names Mina?                                                   | **`mina:mainnet`** and **`mina:devnet`**, from the `mina` namespace (status Draft). No CAIP-19 asset entry exists.                                                     |

---

## 1. The verifier

### What was run

An o1js script compiled two `ZkProgram`s with **custom public inputs** and proved one statement
with each:

- `toon-1512-spend`: `maxProofsVerified: 0`. Its public input is a five-field struct shaped like the
  allowance statement (`root`, `nullifier`, `binding`, `tier`, `expiry`). Its constraints are a
  stand-in for the vault's proof program: two Poseidon openings and a range comparison.
- `toon-1512-wrap`: `maxProofsVerified: 1`. It verifies a `spend` proof recursively and has a
  public input and a **public output**.

A Rust binary verified each proof against its verification key and its public fields, using
mina-rust's ledger crate (`mina-tree`). It then checked that the same proof is **refused**:

- with one public field changed by one;
- with the last public field dropped;
- under the other program's verification key.

### What o1-labs code does it, and what it took

mina-rust's `crates/ledger/src/proofs/verification.rs` already contains a complete Rust Pickles
verifier for a side-loaded proof. It is the code the node runs on every zkApp proof:

- `compute_deferred_values` and `run_checks` handle the deferred values;
- `accumulator_check` does the IPA accumulator check;
- `verify_with` runs kimchi.

Its entry point, `verify_zkapp`, takes a `ZkappStatement`, but the function underneath,
`verify_impl`, is generic over any `ToFieldElements<Fp>`. A `Vec<Fp>` of public fields already
implements that trait (CONFIRMED-CODE). Exactly two things were missing:

1. **A public entry point for arbitrary public fields.** This is about 30 lines in two functions.
   `verify_sideloaded(vk, &[Fp], proof, srs)` is `verify_zkapp` without the statement type,
   without the logging and without `verify_zkapp`'s **write of a failure dump to the working
   directory**. That dump is unacceptable on a connector, where a refused proof is routine.
   `verify_sideloaded_with_index` does the same with a verifier index the caller built once.
   The patch is `rust/patches/mina-rust-v0.19.0.patch` in the vectors repository. It also carries
   spike-only stderr diagnostics behind `TOON_1512_DEBUG`, which a vendored crate drops.
2. **Padding.** o1js writes a proof at its own `maxProofsVerified` (`Proof.toJSON()` calls
   `Pickles.proofToBase64([maxProofsVerified, proof])`). A side-loaded verifier reads proofs at the
   maximum, 2. Pickles' `Wrap_hack.pad_proof` front-extends exactly one list,
   `statement.proof_state.messages_for_next_wrap_proof.old_bulletproof_challenges`, with
   `Dummy.Ipa.Wrap.challenges`. The step-side lists stay as they are. An N0 or N1 o1js proof
   therefore fails to deserialize in mina-rust (`ListLengthMismatch … expected_len: 2,
list_len: 0`) until a caller pads it. The pad takes about 40 lines over the proof's
   S-expression. One trap: the crate's `SexpOf` writes a hex64 limb as `0x…`, which its own
   `OfSexp` refuses, so the dummy limbs have to be written by hand as 16 bare hex digits.

The decoding is otherwise mina-rust's own code:

- A proof is URL-safe base64 of an S-expression, parsed into `PicklesProofProofsVerifiedMaxStableV2`.
- A verification key is standard base64 of bin_prot, parsed into
  `MinaBaseVerificationKeyWireStableV1` and converted to `VerificationKey`.
- The SRS is `SRS::create(depth)`. It is deterministic, so no file has to be shipped.

### Result before Mesa: yes (RAN)

These are o1js `2.15.0` proofs verified by mina-rust `v0.19.0` with the patch, on an x86-64 laptop
(16 threads), in a release build:

```
toon-1512-spend: maxProofsVerified=0 public_fields=5 verifies=true vk_hash_matches_o1js=true
  rejects_tampered_input=true rejects_short_input=true
  index_build=1.17s verify_with_cached_index_mean_of_20=110ms
toon-1512-wrap: maxProofsVerified=1 public_fields=2 verifies=true vk_hash_matches_o1js=true
  rejects_tampered_input=true rejects_short_input=true
  index_build=1.85s verify_with_cached_index_mean_of_20=122ms
cross: spend proof under wrap vk refused=true
```

- **Verification time per proof: ~110–120 ms** once the verifier index is built. Building the index
  from the verification key takes 1.2–1.8 s, once per key. A zk edge has one vault key, so it
  builds the index at boot and keeps it. Without that cache every verification costs 1.2–1.8 s.
  For comparison, o1js's own `verify` took 0.5–0.8 s.
- **Verification-key hash.** Rust's `VerificationKey::hash()` equals o1js's `vk.hash` for both
  programs, so the hash a zk edge publishes in its greeting can be computed in Rust from the key
  it verifies with.
- A wrong public field, a missing public field and a wrong key are each refused, and kimchi reports
  `OpenProof`.

### Result after Mesa: yes, after a repin (RAN)

mina-rust `v0.19.0` as released **refuses** o1js `3.1.0` proofs. The key decodes, its hash
matches o1js, `run_checks` and the accumulator check pass, and then kimchi's opening check fails
(`OpenProof`). The cause is the proof-systems pin:

- Mina `3.4.0` pins proof-systems `0.4.1`. mina-rust's `0.2.0` lock is wire-compatible with it,
  which is why o1js 2.15 proofs verify.
- Mina `4.0.0-mainnet-mesa` pins `ab84160f`, and kimchi changed between the two. The verifier-side
  change is commit `64129ce4` (_kimchi: update endosclmul gate_). It adds a twelfth `EndoMul`
  constraint, `(xp − xr)·(xr − xs)·inv = 1`, which rules out a degenerate point addition. It also
  drops the Cairo gates. Pickles' wrap step uses `EndoMul`, so the polynomial the verifier evaluates
  changed, and o1js 3.0's changelog says _"All verification keys will change"_ (CONFIRMED-DOCS).
- Mina's own Pickles (OCaml, `src/lib/pickles`) is **unchanged** between `3.4.0` and
  `4.0.0-mainnet-mesa` (CONFIRMED-CODE, `git diff --stat`).

The repair is to repin mina-rust's proof-systems crates to `ab84160f` and fix the API drift. That
drift is 44 compile errors, all mechanical:

- `VerifierIndex`, `ProverIndex`, `OpeningProof` and the sponges take a `FULL_ROUNDS` parameter;
- `KimchiCurve<FULL_ROUNDS>`;
- `ScalarChallenge.0` becomes `.inner()`;
- the SRS's `lagrange_bases` is reached through an accessor;
- `Signer::sign`'s `packed: bool` becomes `NonceMode::{Chunked, Legacy}`.

Excluding `verification.rs`, which also carries the entry point and diagnostics above, the
repin is 10 files, +131/−103, and most of the insertions are `Cargo.lock`. mina-rust derives the
linearization and the powers of alpha from kimchi at run time, so they follow the repin with no
regeneration. Its hand-generated `scalars.rs` has no callers. The Pickles constants (`public = 40`,
`zk_rows = 3`, two previous challenges, `max_poly_size = 2^15`) did not move, and the verifies
below confirm it. With the repin:

```
toon-1512-spend: maxProofsVerified=0 public_fields=5 verifies=true vk_hash_matches_o1js=true
  rejects_tampered_input=true rejects_short_input=true
  index_build=1.23s verify_with_cached_index_mean_of_20=102ms
toon-1512-wrap: maxProofsVerified=1 public_fields=2 verifies=true vk_hash_matches_o1js=true
  rejects_tampered_input=true rejects_short_input=true
  index_build=1.75s verify_with_cached_index_mean_of_20=108ms
cross: spend proof under wrap vk refused=true
```

The same Mesa build **refuses both o1js 2.15 proofs** (`verifies=false`, while
`vk_hash_matches_o1js=true`). A verifier build serves one Mina era. The patch is
`rust/patches/mina-rust-v0.19.0-mesa-proof-systems-ab84160.patch` in the vectors repository.

### The dependency tree

Using `mina-tree` as a library brings 285 crates. Measured against this workspace's 516, it adds
**91 crate names**: proof-systems (`kimchi`, `poly-commitment`, `mina-poseidon`, `mina-curves`,
`mina-hasher`, `mina-signer`, `o1-utils`, `groupmap`, `turshi`), mina-rust's own `mina-core`,
`mina-p2p-messages`, `redux`, `poseidon` and `binprot`, and a long tail of node-only weight. That
tail includes `juniper` (GraphQL), `reqwest` with `native-tls` and **OpenSSL**, `libp2p-identity`
and `rayon`. The verifier path is about 9,000 of `mina-tree`'s 79,000 lines, but it is tangled
with the prover modules (`wrap.rs`, `step.rs`, `transaction.rs`) through shared types and dummy
values. mina-rust also patches `num-bigint` workspace-wide (`[patch.crates-io]` to an `openmina`
fork). The verifier did not appear to need the fork, but this was not tested without it.

**Upstream is paused.** On 2026-07-15 o1Labs put a notice on mina-rust's README: _"Development
of the Mina Rust Node is currently paused … not actively developing the project, reviewing pull
requests, or accepting external contributions"_ (CONFIRMED-DOCS). Its last release is `v0.19.0`
(2026-01-15), and it predates Mesa. Whatever the connector takes from it, the connector maintains.

**What that implies for #1514 (a recommendation, not a decision).** Vendor a verifier crate. Take
the verifier subset of `mina-tree` and the decoding types it needs from `mina-p2p-messages`, under
their Apache-2.0 licence, into one crate. Pin it to the proof-systems commit the live Mina daemon
pins, and drop everything node-only. Two alternatives are worse:

- Depending on `mina-tree` from git would bring OpenSSL and a GraphQL server into the connector to
  verify one proof per allowance.
- Writing a Pickles verifier from scratch would discard the one implementation that has verified
  mainnet proofs.

ADR 0080's falsifiers are untouched by any of this: no route needs o1js in a manifest.

---

## 2. o1js parity

Each check below was RAN with the same Rust binary against `parity.json`, which o1js generated.
Results are identical for o1js `3.1.0` and `2.15.0`, and on both Rust builds: proof-systems at the
commit mina-rust `v0.19.0` locks, and at Mesa's `ab84160f`.

| Primitive                                                                                                            | Vectors | Byte-identical | Notes                                                                         |
| -------------------------------------------------------------------------------------------------------------------- | ------- | -------------- | ----------------------------------------------------------------------------- |
| `Poseidon.hash` (kimchi parameters, Pallas base field), 0 to 5 inputs, edge values                                   | 7       | **7**          | `mina-hasher` `create_kimchi`, no domain                                      |
| `Poseidon.hashWithPrefix`, 20-byte prefixes (`MinaSignatureMainnet`, `CodaSignature*******`, `toon-zk-note********`) | 12      | **12**         |                                                                               |
| `Poseidon.hashWithPrefix`, a short prefix (`toon-zk-note`)                                                           | 4       | 0              | **Convention 1** below                                                        |
| `Poseidon.hashToGroup`                                                                                               | 6       | **6**          | Through mina-rust's port of Snarky's conic group map, with y negated when odd |
| `Poseidon.hashToGroup` through proof-systems' `groupmap` (BW19)                                                      | 6       | 0              | A different map: the one the IPA uses. Not a parity failure.                  |
| Schnorr, o1js `Signature.create`: public-key address                                                                 | 20      | **20**         | 4 keys × 5 messages                                                           |
| Schnorr, o1js `Signature.create`: signature `(r, s)`, Rust `sign` with the o1js nonce                                | 20      | **20**         | **Convention 2** below                                                        |
| Schnorr, o1js signature verified by Rust `verify`                                                                    | 20      | **20**         |                                                                               |
| Schnorr, mina-signer `signFields`: signature `(r, s)`, packed                                                        | 20      | **20**         |                                                                               |

The two conventions a Rust caller has to follow:

1. **A domain prefix shorter than 20 bytes means different things.** o1js zero-pads it
   (`prefixToField`). Rust's `mina-hasher` pads it with `*` (`domain_prefix_to_field`). Mina's own
   prefixes are all written out to 20 bytes, so they agree. Any TOON prefix (a binding tag, a state
   key) has to be written out to 20 bytes as well, for example `toon-zk-note********`. Either side
   then agrees, and the vectors pin it.
2. **Schnorr nonces must use the o1js derivation.** proof-systems' `Signer::sign` takes
   `packed: bool` at the commit mina-rust `v0.19.0` locks, and `NonceMode::{Chunked, Legacy}` at
   Mesa's pin. `true`/`Chunked` reproduces o1js and OCaml byte for byte, and `false`/`Legacy`
   matched **0/20**. The
   unpacked derivation still produces valid signatures, so a test that only verifies would not
   notice. The state advance's signature has to be signed in that mode, and the connector's
   vectors must pin `(r, s)`, not just validity. mina-rust `v0.9.0` (proof-systems as of 2024) has
   only the unpacked derivation.

And one fact about the signing domain. o1js's provable `Signature.create` and `verify`, and
mina-signer's `signFields`, **always use the devnet prefix and network id** (the prefix is
`CodaSignature*******`), whatever network the client is configured for (CONFIRMED-CODE). A
`Signature.verify` inside the vault's proof program therefore accepts only a devnet-domain
signature. The zk edge signs the state advance under `NetworkId::TESTNET`, even on mainnet. That is
the library's choice, not TOON's, and the vectors pin it.

The vectors, the script that produced them, both lockfiles and the Rust harness are in
[`toon-protocol/zk-note-vectors@658c3d43`](https://github.com/toon-protocol/zk-note-vectors/tree/658c3d43342e65730fc47c1f6ecb5b65aebcec80).

---

## 3. CAIP-2

**`mina:mainnet`** and **`mina:devnet`**.

- The registry entry is the `mina` namespace in ChainAgnostic/namespaces, `mina/caip2.md`, merged
  in PR #91 on 2024-03-16 and authored by the Mina Foundation. It is **status Draft**. Its
  references are human-readable network names (`mainnet`, `devnet`, `testnet`, codenames such as
  `berkeley`), validated by `^mina:[a-z]{1,12}?(-[a-z]{1,12})?$`. It does not use a genesis hash.
- It names a resolution method, and that method is live. A Mina daemon's GraphQL `networkID` field
  returns the CAIP-2 string itself. On 2026-10-16 `https://api.minascan.io/node/mainnet/v1/graphql`
  answered `{"data":{"networkID":"mina:mainnet"}}`, and the devnet endpoint answered `mina:devnet`
  (CONFIRMED-LIVE). A zk edge can therefore check the network it reads against the one it
  advertises, using the node's own answer.
- **No asset entry exists.** The `mina` namespace has `caip2.md` and a README, and no CAIP-19 (or
  CAIP-10) profile. The zk-note table's asset has no registry entry for its asset part. #1513 has
  to define that string itself and say so in the vectors. The chain part is `mina:mainnet` or
  `mina:devnet` either way.

---

## What changes in ADR 0080

#1512 answers **yes**, so ADR 0080 stands and is not withdrawn. Its status drops the
"conditional" and points here. Nothing in its decisions has to change. Three findings are for
#1513 and #1514 to weigh. They are recommendations, not decisions:

- **Vendor the verifier rather than depend on it.** That means a verifier crate taken from
  mina-rust's Pickles verifier, pinned to the proof-systems commit the live Mina daemon pins.
  Upstream is paused, so whichever route is taken, the connector maintains it.
- **A Mina hard fork is a breaking change at a zk edge.** The committed o1js fixture, pinned by
  hash, is what detects it. ADR 0080 already makes that fixture the port's contract test. The
  fixture has to be regenerated with the o1js release for the new era, and the repin cannot pass
  without it.
- **The vectors pin conventions as well as values:** 20-byte domain prefixes, the chunked (o1js)
  nonce, and the devnet signing domain for anything a proof program verifies with
  `Signature.verify`.

The verifier index costs 1.2–1.8 s to build and should be built once per verification key, at
boot. After that, admission's proof check costs about 0.1 s, which sits at the expensive end of
ADR 0080 decision 3's "cheapest refusal first" ordering, as intended.
