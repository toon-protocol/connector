# Signing a write

**Status:** repo-local guidance for calling an authenticated operator write from a script,
without the interactive dashboard (PR #1222). Written for whoever needs to establish a peering,
or make any other write, from a shell or a CI job rather than a browser.

---

## The gap this closes

`docs/protocol/operator-spec.md` and the operator-surface guide's ["Signing a write"](../guide/operator-surface.md#signing-a-write)
section state the rule: a write needs an RFC 9421 HTTP Message Signature, `alg="ed25519"`, over
exactly `@method`, `@path` and `content-digest`, from a key on `[operator] write_keys`, with an RFC
9530 `Content-Digest` binding the body. What neither ever gave you was something that produces one.
`connector send` is the only signer this repository ships, and it signs exactly one write —
`POST /packets`, originating a packet. Every other write, `POST /peers` above all — the flagship of
"peer with a stranger's node" — had no tool at all. Making the call meant reading the verifier's own
source (`crates/connector-operator/src/rfc9421.rs`) and reimplementing it by hand.

[`sign-write.sh`](sign-write.sh) is that implementation, done once, shipped, and held to the
verifier it targets: everything it computes is named after the function in `rfc9421.rs` that checks
it, not restated from memory.

## Worked example: establishing a peering

```bash
# 1. Generate an operator write key. NEVER commit it, and NEVER reuse your
#    node's own [signer] key -- this is a distinct ed25519 keypair whose
#    only job is authorizing writes (ADR 0009, ADR 0012: a key is
#    referenced by path, never by value, and this one is no exception).
openssl rand -hex 32 > operator-write.key
chmod 600 operator-write.key

# 2. Derive the public half -- the value that goes in the TARGET node's
#    config, not yours. `connector send` already does this derivation;
#    the script's is byte-for-byte the same algorithm.
connector send --operator-key operator-write.key --print-keyid
# -> e.g. 3af2...9c10 (64 hex characters)
```

The target node's operator adds that value to their own config and restarts:

```toml
[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys = ["3af2...9c10"]        # or write_keys_file for a committed config
```

Then, from wherever `operator-write.key` lives:

```bash
BODY='{"id":"their-node","url":"https://their-node.example/ilp","fee":100,"max_packet_amount":1000000}'

docs/operators/sign-write.sh -k operator-write.key -X POST -p /peers \
    -b "$BODY" -u https://your-node.example
```

`-u` makes the script `curl` the signed request itself and print the response. Omit it and the
script prints only the three headers, for a caller that wants to build the request another way (a
CI step assembling its own `curl`, a different HTTP client, a proxy that adds its own headers
first):

```
Signature-Input: sig1=("@method" "@path" "content-digest");created=...;expires=...;keyid="...";alg="ed25519"
Signature: sig1=:...:
Content-Digest: sha-256=:...:
```

`fee` and `max_packet_amount` are the calling operator's own policy about this counterparty — see
the [Peering](../guide/peering.md) guide for what each means. A `502` back means the
URL was unreachable, redirected, or described a node with no shared settlement chain — "about them,"
in that guide's own words; a `400` is about the request itself.

## How it works

No dependency beyond `bash` and `openssl` (already required — Foundry's `openssl`-linked tooling and
this container's own test gate assume it). The three steps `rfc9421.rs`'s `sign_request` performs,
done the same way in shell:

1. **`Content-Digest`** (RFC 9530): `sha-256=:<base64 of SHA-256(body)>:`, via
   `openssl dgst -sha256 -binary | openssl base64`.
2. **The signature base**, exactly `build_signature_base`'s four lines — `"@method"`, `"@path"`,
   `"content-digest"`, then `"@signature-params"` naming the covered set and carrying `created`,
   `expires`, `keyid` and `alg` — signed as one string with `openssl pkeyutl -sign -rawin` (PureEdDSA:
   Ed25519 hashes its own input, so there is no separate digest step before signing, unlike
   `Content-Digest` above).
3. **The key itself** never touches a key-generation tool. This repository's key files are a bare
   32-byte seed (or that seed as 64 hex characters) — the same shape `[signer] key_file` and
   `[settlement.*.key] key_file` already use (`docs/operators/key-rotation-runbook.md`). An Ed25519
   PKCS8 private key is that seed wrapped in fourteen fixed, publicly-known DER bytes with no
   per-key content of their own
   (`SEQUENCE{version=0, AlgorithmIdentifier{1.3.101.112}, OCTET STRING{OCTET STRING{seed}}}`), so
   the script builds that wrapper itself and hands the result to `openssl pkey` — no conversion
   tool, and no second key file in a different format to keep in sync with the one this repository
   already generates.

`keyid` — the value that goes in `write_keys` — is the public key, hex, read back off the same
PKCS8 key with `openssl pkey -pubout`: the last 32 bytes of its DER `SubjectPublicKeyInfo`, which
carries the same fixed 12-byte wrapper in front of the raw key. It is exactly what
`connector send --operator-key <file> --print-keyid` prints for the same key file, because both
derive it the same way from the same seed.

## Tested against the real thing

`crates/connector-cli/tests/sign_write_script.rs` shells out to this script, boots a real
config-driven node with a real `[operator] write_keys` allowlist, and sends the signed request at
it. It asserts `502` rather than `200`, deliberately: the request is pointed at an address nothing
answers, so the assertion that matters is that authentication was never the reason it failed — a
`401`/`403` there would mean this script's signature never verified at all, and the test would fail
loudly rather than reporting a false pass.
