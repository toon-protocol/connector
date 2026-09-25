# Provenance: x402's batch-settlement contracts and the ERC-3009 USDC

Issue #1342, ADR 0074. Nothing in this directory is this repository's Solidity.
`x402BatchSettlement` is x402's, pinned at
[`0cb1a1f0`](https://github.com/x402-foundation/x402/tree/0cb1a1f0f4c2163357e255c824d319674e1db43f),
and the token is Circle's FiatToken v2.2. The backend binds to the ABI. The tier-3
tests place the bytecode on `anvil` (`src/test_support/x402.rs`) exactly as
toon-protocol/infra's `sandbox/scripts/seed-x402.sh` does. The files are
byte-identical to that sandbox's `sandbox/artifacts/evm/` copies, which its
`scripts/fetch-artifacts.sh` fetched.

| File                                  | What it is                                                                        | Where it comes from                                                                                                          |
| ------------------------------------- | --------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `x402BatchSettlement.abi.json`        | The contract's ABI                                                                | The `abi` field of `out/x402BatchSettlement.sol/x402BatchSettlement.json`, from `forge build` in `contracts/evm/` at the pin |
| `x402BatchSettlement.runtime.hex`     | RUNTIME bytecode, 11175 bytes                                                     | `eth_getCode` at `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` on Base Sepolia                                                |
| `ERC3009DepositCollector.runtime.hex` | RUNTIME bytecode of the ERC-3009 deposit collector, 1150 bytes                    | `eth_getCode` at `0x4020806089470a89826cB9fB1f4059150b550004` on Base Sepolia                                                |
| `SignatureChecker.runtime.hex`        | RUNTIME bytecode of Circle's `SignatureChecker` library, 1741 bytes               | `eth_getCode` at `0xbA3b60c21e28C41df4bABd90f228e1D368627DA6` on Base Sepolia                                                |
| `FiatTokenV2_2.creation.hex`          | CREATION bytecode of the FiatToken v2.2 implementation                            | the input of Base Sepolia tx `0x6dbb9d759e3388911863490b3bde0e8fa3c22a8e70f48f758302592b7dc52fe1`                            |
| `FiatTokenProxy.creation.hex`         | CREATION bytecode of `FiatTokenProxy`, with its one constructor argument stripped | the input of Base Sepolia tx `0xd835c0abef5b7988ba6230f92da809391716b8dc5e6cd4e430263b52d3bf69f3`, less its last 32 bytes    |

## Checked on 2026-09-25

Each runtime file is byte-identical to `cast code <address> --rpc-url https://sepolia.base.org`.
Each creation file is byte-identical to `cast tx <hash> input` on the same RPC, with the
proxy's trailing constructor argument removed.

**The ABI describes the deployed bytecode.** x402's `foundry.toml` at the pin sets
`cbor_metadata = false` and `bytecode_hash = "none"`, so a build carries no metadata hash
that could vary between build environments. `forge build` at the pin (solc 0.8.28,
optimizer 200 runs, `evm_version = "cancun"`, `via_ir = false`) produces a
`deployedBytecode` of 11175 bytes. It matches the live runtime **exactly** once the 224
bytes that the artifact's `immutableReferences` list are masked. Those bytes are
OpenZeppelin `EIP712`'s immutables. Read back out of the live code, they decode to what
the deployment implies:

| Immutable                                        | Live value                                   |
| ------------------------------------------------ | -------------------------------------------- |
| cached chain id                                  | `0x14a34` = 84532, Base Sepolia              |
| cached `address(this)`                           | `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` |
| name, as a `ShortString`                         | `"x402 Batch Settlement"`                    |
| version, as a `ShortString`                      | `"1"`                                        |
| cached separator, hashed name and hashed version | derived from the four values above           |

`tests/x402_provenance.rs` runs offline in the gate. It checks that the committed ABI
and the committed runtime agree: every function the ABI declares has its selector in
the runtime's dispatcher. It does not rebuild x402, because that would put a network
clone of a third-party repository into the gate. The pin is fixed, so a rebuild in CI
would have nothing to catch that this record does not already say.

## Why the addresses are production's

`@x402/evm` and x402's facilitators hardcode the CREATE2 addresses above. A voucher's
EIP-712 domain names the contract's address, so a channel id or voucher digest computed
here matches a real deployment only if the contract sits at that address. This is also
what puts the audited deployed code under test, not a rebuild of it.
`ERC3009DepositCollector`'s one immutable is `x402BatchSettlement`'s canonical address.
FiatToken v2.2's creation code links `SignatureChecker` at `0xbA3b…7DA6`, and the
library's call guard compares its own address to that one. Both therefore have to sit
at exactly these addresses too.

## Reproducing

```bash
git clone https://github.com/x402-foundation/x402 && cd x402 && git checkout 0cb1a1f0
cd contracts/evm && forge build
jq '.abi' out/x402BatchSettlement.sol/x402BatchSettlement.json   # = x402BatchSettlement.abi.json
cast code 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003 --rpc-url https://sepolia.base.org
```

Compare that output with `x402BatchSettlement.runtime.hex`, and with the build's
`.deployedBytecode.object` after masking `.deployedBytecode.immutableReferences`.
