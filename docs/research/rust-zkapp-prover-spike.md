# Can TOON prove an o1js zkApp method in Rust? (spike #1561)

> **Research note, not a decision record.** It decides nothing. It serves #1551 (how a Mina
> write is proved and submitted) and answers #1554's open question 11 (dummy proofs in tier-3
> tests). Evidence comes from code run for this spike against artefacts o1js 3.1.0 produced, and
> from a local lightnet. Written 2026-10-28. No o1js dependency entered this repository. The
> prototype lives on the throwaway branch
> [`toon-protocol/zk-note-vectors@spike-1561-rust-prover`](https://github.com/toon-protocol/zk-note-vectors/tree/spike-1561-rust-prover/spike-1561),
> as #1512's did.

**Evidence tags.** As in [`zk-note-verifier-spike.md`](zk-note-verifier-spike.md): RAN means the
result comes from code run for this spike, CONFIRMED-CODE means read in source at a pinned commit,
and ESTIMATE is a judgement, not a measurement.

**Sources pinned.** o1js `3.1.0` (Mesa). mina-rust `v0.19.0` (`ed6f4301`) with #1512's Mesa patch
(proof-systems `ab84160f`). Mina `4.0.2` (`aa93c444`), from the docker images
`o1labs/mina-local-network:4.0.2-lightnet` (compiled `proof_level=none`) and `:4.0.2-devnet`.

---

## Verdict

**Feasible, and the cost has been measured, not paid.** The spike was stopped by the owner's
choice (2026-10-28) before it produced an end-to-end Rust proof. What ran:

- o1js exports exactly what mina-rust's prover loads.
- mina-rust's kimchi prover proves an o1js step circuit.
- mina-rust's verifier accepts o1js's own proof of the same method.

Two pieces of code stand between that and a Rust-proved transaction on a proofs-on chain. Neither
exists in any repository, and both have an exact oracle to be written against:

1. **A wrap prover for zkApp programs.** This is a one-time port per Pickles version. It is not
   needed per method.
2. **A step witness generator per method.** This is the recurring cost.

Both are mechanical, but multi-day (ESTIMATE). No step hit a wall: nothing that ran suggests a
Rust-made proof would differ from an o1js-made one under the same verification key.

| #   | Question (from #1561)                               | Answer                                                                                                                                                                     |
| --- | --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Write a tiny "land a voucher" method in o1js        | **Done.** `land(cumulative: UInt64, sig: Signature)`: verify the payer's Schnorr signature over `cumulative`, require it to exceed the stored watermark, and store it. RAN |
| 2   | Export its constraint system and keys from o1js     | **Yes**, with a one-line hook (below). RAN                                                                                                                                 |
| 3   | Witness and Pickles proof in Rust                   | **Step proof: yes**, from o1js's witness, in 0.95 s. **Wrap proof and Rust-made witness: not built.** RAN / ESTIMATE                                                       |
| 4   | Verifies under the o1js VK, accepted by lightnet    | **Not reached** for a Rust proof. #1512's Rust verifier accepts o1js's real `land` proof. RAN                                                                              |
| 5   | Cost: witness lines per method, time, memory, forks | See [Cost](#cost). Lines per method are bounded by a measured witness map, not counted.                                                                                    |
| —   | #1554 Q11: a dummy proof on a proofs-off lightnet?  | **Yes,** and the dummy proof is a constant. **But the same chain accepts an update the circuit forbids.** RAN                                                              |

---

## 1. Exporting the circuit (RAN)

mina-rust's prover does not compile circuits. For each circuit it loads three artefacts that a
patched OCaml build emits (`o1-labs/circuit-blobs`):

- the gates (`*_gates.json`);
- `internal_vars.bin`, linear combinations for the variables snarky creates internally;
- `rows_rev.bin`, which variable sits in each witness cell.

Its `compute_witness` turns the snarky witness vector (public input plus `aux`, one value per
`exists`) into kimchi's 15 columns (CONFIRMED-CODE, `crates/ledger/src/proofs/transaction.rs`).

o1js 3.1.0's js_of_ocaml bundle already contains that dumper, `dump_extra_circuit_data`, in the
same `Plonk_constraint_system` module, but nothing calls it. Calling it from
`finalize_and_get_gates` writes every constraint system o1js finalizes. `Land.compile()` yields five:

| Dump    | What it is               | Rows  | Public | Gates of note                                                                  |
| ------- | ------------------------ | ----- | ------ | ------------------------------------------------------------------------------ |
| `cs001` | `land`, method analysis  | 962   | 0      | not used                                                                       |
| `cs002` | `setup`, method analysis | 613   | 0      | not used                                                                       |
| `cs003` | **`land` step circuit**  | 1,405 | 1      | 902 Poseidon, 103 VarBaseMul, 12 RangeCheck0, 6 RangeCheck1, 3 ForeignFieldAdd |
| `cs004` | `setup` step circuit     | 1,056 | 1      | 869 Poseidon, no range checks                                                  |
| `cs005` | **wrap circuit**         | 6,798 | 40     | 2,848 EndoMul, 663 VarBaseMul                                                  |

This is a compile-time export, which #1561 allows. Run time needs no o1js.

## 2. A step proof in Rust (RAN)

mina-rust built a prover index from `cs003` in 1.9 s. It was then given the witness o1js computed
for one `land` call (17,962 `aux` values, dumped from o1js's own `compute_witness`). kimchi's
constraint check passed on every row, and `create_proof` made the step proof in **0.95 s**. On the
same machine, o1js proves the whole method (step plus wrap) in 4.7 to 5.3 s.

So o1js's step circuit and mina-rust's kimchi agree on gates, wiring, lookup tables and witness
layout. A Rust step prover needs a correct `aux` vector and nothing else.

## 3. Rust verifies o1js's `land` proof (RAN)

#1512's harness (`toon-1512`) was run on the real `land` proof. That proof has range-check gates,
so it carries lookup commitments, which #1512's fixtures did not. Its public input is the zkApp
statement (account update hash, calls hash).

The proof verifies. The VK hash matches o1js (`98709477…69766`), and a tampered input is refused.
A verify takes **92 ms** with a cached index, or 1.1 s cold. One quirk: the proof also verifies
with the trailing `calls = 0` field dropped. A trailing zero does not change the public-input
polynomial, so this is not a forgery.

## 4. What was not built

### 4.1 The wrap prover (one-time)

mina-rust's `wrap()` and `wrap_main` port the Pickles wrap for Mina's own transaction and block
circuits. They hard-code three things:

- `MAX_PROOFS_VERIFIED_N = 2`;
- a single step key in `choose_key`;
- step circuits **without optional gates**: `lookup: None` and empty feature flags throughout the
  in-circuit verifier (CONFIRMED-CODE, `wrap.rs`).

A zkApp program's wrap differs on each count:

- `land` and `setup` are `maxProofsVerified: 0`, so every per-previous-proof `exists` vanishes.
- The wrap chooses between **two** step keys.
- The program's feature flags are the union over its methods. `land` uses range-check gates, so
  the wrap absorbs and checks lookup commitments.

The OCaml source to port is `wrap_main.ml` and the lookup branch of `wrap_verifier.ml` (about 85
sites mention lookups or `Opt`) at Mina `aa93c444`. The out-of-circuit half (deferred values with
feature flags) already works in mina-rust, as §3 shows.

**This cost is not avoidable by choosing a simpler method.** o1js 3.x's `Signature.verify` emits
the range-check gates itself: `scaleShifted` → `fieldToField3` → `multiRangeCheck`. So every
voucher-checking zkApp has lookup features.

The oracle is exact. The spike captured o1js's own step proof for one `land` call
(`stepproof002.b64`, kimchi `ProverProof` in rmp-serde) and o1js's wrap witness for it (99,657
values). A Rust wrap fed that step proof must reproduce the wrap witness value for value.

### 4.2 The step witness (per method)

o1js's `exists` was traced during proving. `land-step-map.txt` on the prototype branch maps each
of the 17,962 `aux` values to the call site that created it:

| Range (aux index) | Values | Source                                                                                                                                                                                                          |
| ----------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1–88              | 88     | Pickles step prelude (the zkApp statement first)                                                                                                                                                                |
| 89–5,008          | ~4,920 | The method and o1js's `SmartContract` wrapper. **598** values from 234 JS-gadget `exists` calls; the rest from OCaml gadgets they call: Poseidon (44, 44, 340, 720) and two scalar multiplications (1,587 each) |
| 5,009–17,962      | 12,954 | OCaml, after the method: account-update hashing (Poseidon) and the Pickles step's own hashing                                                                                                                   |

Of the JS gadgets, the range checks dominate (`rangeCheck0Helper` 168 values, `rangeCheck1Helper`
144). Then come foreign-field `sum`/`negate`, `isOdd`, comparisons, `Group.add`/`scaleShifted`,
`ifField`/`switch`, state reads and writes, and `setKind`.

mina-rust already mirrors OCaml's Poseidon and Pickles step gadgets (`step.rs`, the transaction
circuits). It has **none** of o1js's JS gadgets, because Mina's OCaml circuits never use them. A
Rust witness generator for `land` therefore needs:

- (a) a Rust mirror of each o1js gadget it uses, emitting the same values in the same order;
- (b) the `SmartContract` wrapper: method arguments, account-update witness, preconditions and the
  in-circuit account-update hash;
- (c) the method body itself, written a second time.

Item (a) is shared across methods once written. Items (b) and (c) are per method (ESTIMATE). The
oracle is exact here too: o1js's `aux` vector compared index by index, as mina-rust's own
`ocaml_aux` check does for Mina's circuits.

## 5. Dummy proofs in tier-3 tests (#1554 Q11) (RAN)

On `4.0.2-lightnet` (`proof_level=none`), each proof-authorised update carried o1js's dummy proof
(`dummyBase64Proof()`, a `Pickles.dummyProof(2, 15)`). Deploy, `setup(payer)` and `land(5, sig)`
were each accepted and included in 20 to 50 s, and the watermark read back as 5.

The dummy proof is **a constant**: 35,176 base64 characters, SHA-256 `00e459f1…2161` on every run.
So a Rust backend could embed it as a fixture and run no prover in tier-3 tests.

**The catch.** The same chain then included a forged `land` update that lowered the watermark from
5 to 3. The method forbids that, and o1js refuses to build it. With proofs off, the node checks
preconditions, permissions, nonces and fees, and **never the circuit**. So a proofs-off tier-3 test:

- **can** check the transaction layer: building, signing, preconditions, nonces, fees, confirmation;
- **cannot** check anything the zkApp enforces: watermark monotonicity, the voucher signature,
  payout bounds.

Those need a proofs-on chain (`4.0.2-devnet` with `PROOF_LEVEL=full`, untested here) or o1js's own
tests in zk_toon.

## Cost

| Item                                 | Measured or estimated                                                                                                                                                             |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Export the circuit                   | A one-line hook in o1js's bindings, at compile time. RAN                                                                                                                          |
| Step prover index (`land`)           | 1.9 s to build. RAN                                                                                                                                                               |
| Step proof (`land`), given a witness | 0.95 s. RAN                                                                                                                                                                       |
| Whole method in o1js (step + wrap)   | 4.7–5.3 s. RAN                                                                                                                                                                    |
| Rust verify of an o1js `land` proof  | 92 ms with a cached index, 1.1 s cold. RAN                                                                                                                                        |
| Peak memory                          | Not measured.                                                                                                                                                                     |
| Wrap prover port                     | One-time per Pickles version: N=0, multiple branches, lookup features. Multi-day. ESTIMATE                                                                                        |
| Step witness per method              | For `land`: 17,962 values, 234 JS `exists` sites over ~15 gadget kinds, plus the `SmartContract` wrapper. Multi-day for the first method; gadgets are reused after that. ESTIMATE |

### What breaks at a hard fork, or at an o1js upgrade

Everything #1512 found for the verifier holds, plus more:

- **The circuit blobs** are compile artefacts of one o1js release. They are re-exported on every
  o1js or zkApp change, and the VK hash changes with them.
- **The gadget mirrors** break whenever o1js changes a gadget's implementation. That can happen on
  a minor o1js release with no hard fork at all. It changes the VK too, so a pinned-VK test catches
  it, but the Rust side has to be re-mirrored before the zkApp can be upgraded.
- **The wrap port** breaks when Pickles' `wrap_main` or `wrap_verifier` changes. Mesa changed
  kimchi's EndoMul, and #1512 saw every VK change with it.
- **mina-rust is paused** (since 2026-07-15), so TOON would own the prover as well as the verifier.

ADR 0002's objection was a VK mismatch "undetectable until a transaction is rejected on-chain".
**That objection does not survive this spike:**

- the Rust prover loads o1js's own compiled circuit, so there is no second compilation to drift;
- the witness is checkable against o1js's, value by value, in a test.

What does survive is the maintenance coupling to o1js's gadget implementations. That is the real
cost #1551 has to weigh against an o1js submitter outside the connector (zk_toon#14).
