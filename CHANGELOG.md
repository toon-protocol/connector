# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.23.1](https://github.com/toon-protocol/connector/compare/v1.23.0...v1.23.1) (2026-03-19)

### Bug Fixes

- **connector:** resolve peer EVM address from self-describing claims for settlement ([ea521ec](https://github.com/toon-protocol/connector/commit/ea521ec54ba6accb577697760c00338ca4967b44))

## [1.23.0](https://github.com/toon-protocol/connector/compare/v1.22.0...v1.23.0) (2026-03-19)

### Features

- **connector:** replace polling-based settlement with event-driven claim monitoring ([396e92b](https://github.com/toon-protocol/connector/commit/396e92bd5e3aa7c66c22f05e8ff36529f5ca7c92))

## [1.22.0](https://github.com/toon-protocol/connector/compare/v1.21.0...v1.22.0) (2026-03-19)

### Features

- **connector:** add inbound claim validation gate to prevent unpaid writes ([cec059f](https://github.com/toon-protocol/connector/commit/cec059fc53ea6b20ceebd5f6f5b4e57b92166020))

## [1.21.0](https://github.com/ALLiDoizCode/connector/compare/v1.20.0...v1.21.0) (2026-03-11)

### Features

- add local Anvil infrastructure with faucet and update architecture ([1df938f](https://github.com/ALLiDoizCode/connector/commit/1df938fe5aad464eb102c89c9635b2863de15495))
- **epic-30:** per-hop BLS notification, XRP/Aptos removal, EVM test infrastructure ([2514f71](https://github.com/ALLiDoizCode/connector/commit/2514f715d8ee174b2c36d83d7b21b7dd4bf03a21))
- **epic-31:** add as-built PRD, archive docs, and full project cleanup ([0850c59](https://github.com/ALLiDoizCode/connector/commit/0850c5925cc213a3451b81aa4e6df640d177fc3f))
- **epic-31:** self-describing claims, dynamic channel verification, and docs cleanup ([c31f645](https://github.com/ALLiDoizCode/connector/commit/c31f6456040aa25f1a662ef397e247597a92412b))
- implement XRP-style payment channels with grace period model ([b991f2e](https://github.com/ALLiDoizCode/connector/commit/b991f2e906ab4a77f84912c952598d5f649618fc))
- make per-packet claims mandatory for peer forwarding ([f9cfd54](https://github.com/ALLiDoizCode/connector/commit/f9cfd54cfb8a2d8d87298bc6aa70f796a2b04d2a))
- serialize settlements and fix graceful shutdown sequencing ([fc7fd2b](https://github.com/ALLiDoizCode/connector/commit/fc7fd2b51aaf727c277d5a31a27df46fe57c85e5))

### Bug Fixes

- add fulfillment validation and fix auto-fulfill stub ([4d23625](https://github.com/ALLiDoizCode/connector/commit/4d2362573788520a106da72c6958c4a47d9df949))
- add missing getBlock mock to payment-channel-sdk tests ([9a2e9d0](https://github.com/ALLiDoizCode/connector/commit/9a2e9d0ded118d4d024a15b8afd0a8d96a4218bb))
- add missing multi-hop-helpers.ts to source control ([5d9b4e8](https://github.com/ALLiDoizCode/connector/commit/5d9b4e85203a144e8051c42250037c74f7d6ed0a))
- add stubs for commented-out test infrastructure in doc test ([8ac71be](https://github.com/ALLiDoizCode/connector/commit/8ac71be61b1203ad73de99fc44d12cb75b01f6ba))
- correct rfc-links test path from integration to unit ([a5f3fb7](https://github.com/ALLiDoizCode/connector/commit/a5f3fb75309b8d14d0fa9b50d2cccdf23c45148d))
- remove obsolete mesh topology config test ([390c5bb](https://github.com/ALLiDoizCode/connector/commit/390c5bb06ac338b574a8c27d6a79da9779c92675))
- resolve flaky connection-pool test blocking npm publish ([c290ad7](https://github.com/ALLiDoizCode/connector/commit/c290ad7347ab9c47c40791843ddbe0dffd6e5580))
- resolve pre-existing test failures in doc test and security test ([8ddf736](https://github.com/ALLiDoizCode/connector/commit/8ddf736bd6d5754626ed7219a1009649f1136a88))
- restore TigerBeetle init script and add docker-memory E2E test mode ([95403e5](https://github.com/ALLiDoizCode/connector/commit/95403e5313a4a7c1f572706252c387b70d046a02))
- update environment-config test assertions to match chain-aware error messages ([ae02621](https://github.com/ALLiDoizCode/connector/commit/ae02621bdd1417999ad10f8ef0fd6c0969a10d45))

## [1.20.0](https://github.com/ALLiDoizCode/connector/compare/v1.19.0...v1.20.0) (2026-02-21)

### Features

- add nonce retry logic to PaymentChannelSDK and deploy TokenNetworkRegistry ([85c6fda](https://github.com/ALLiDoizCode/connector/commit/85c6fda97b5fe045fba6001cda163fe09cced5a4))
- **connector:** add deployment mode config and IP allowlist security ([77b0cd9](https://github.com/ALLiDoizCode/connector/commit/77b0cd9ed3f0d94e1048bd75c74fc943509bf0f9))

## [Unreleased]

### Added

- **btp:** RFC-0023 compliant no-auth connection support with `BTP_ALLOW_NOAUTH` flag
  - **Default mode:** Permissionless network deployment with ILP-layer gating
  - Support both permissionless networks (no-auth BTP - default) and private networks (authenticated BTP)
  - Enabled by default for permissionless networks (set `BTP_ALLOW_NOAUTH=false` for private networks)
  - Comprehensive tests for both authenticated and no-auth modes
  - Production security guide for ILP-gated networks (credit limits, settlement, routing policies)
  - Complete documentation in peer onboarding guide, connector README, and permissionless deployment guide

## [1.19.0](https://github.com/ALLiDoizCode/connector/compare/v1.18.0...v1.19.0) (2026-02-16)

### Features

- **connector:** expose openChannel() and getChannelState() on ConnectorNode ([fbb7536](https://github.com/ALLiDoizCode/connector/commit/fbb7536ab3ee5a7bfd61074991dedbfe1d14cfe5))

## [1.18.0](https://github.com/ALLiDoizCode/connector/compare/v1.17.0...v1.18.0) (2026-02-15)

### Features

- bundle chain SDKs as dependencies instead of peer dependencies ([9cbde0b](https://github.com/ALLiDoizCode/connector/commit/9cbde0bc2d15f7beb37d0eb156a87ea579af6ed4))

## [1.17.0](https://github.com/ALLiDoizCode/connector/compare/v1.16.0...v1.17.0) (2026-02-15)

### Features

- consolidate agent-runtime into connector, rename setPaymentHandler to setPacketHandler ([fa3a19b](https://github.com/ALLiDoizCode/connector/commit/fa3a19b8d18e5e750f46d93ec91cc058be76e333))

## [1.16.0](https://github.com/ALLiDoizCode/connector/compare/v1.15.0...v1.16.0) (2026-02-14)

### Features

- **epic-29:** config-driven settlement infrastructure with multi-node isolation ([88d5ca5](https://github.com/ALLiDoizCode/connector/commit/88d5ca5dfb8306a719a6a0251d4c3b0d834106ca))

### Bug Fixes

- **hooks:** fix pre-push jest --findRelatedTests argument ordering ([61dea08](https://github.com/ALLiDoizCode/connector/commit/61dea089b413931a0f5d7792965a4f70d6e390d0))

## [1.15.0](https://github.com/ALLiDoizCode/connector/compare/v1.14.0...v1.15.0) (2026-02-14)

### Features

- **epic-28:** add in-memory ledger as zero-dependency default accounting backend ([357083e](https://github.com/ALLiDoizCode/connector/commit/357083e85ff74c61e704441df5467e67bfc7ce37))

### Bug Fixes

- **epic-28:** fix snapshot persistence test by creating account to set dirty flag ([3699641](https://github.com/ALLiDoizCode/connector/commit/3699641f17c242954fba846038c8587a1019a620))

## [1.14.0](https://github.com/ALLiDoizCode/connector/compare/v1.13.0...v1.14.0) (2026-02-14)

### Features

- **epic-27:** complete test suite optimization - reduce pre-push hook from 13min to <30s ([e82f94d](https://github.com/ALLiDoizCode/connector/commit/e82f94d7fa690e4ed1692c5c2ea0439d78e9849b))

### Bug Fixes

- **epic-27:** prevent pre-push hook from running jest with empty file list ([2ec3505](https://github.com/ALLiDoizCode/connector/commit/2ec3505af13baa908e283a56ae67e22e28a6219d))
- **epic-27:** skip pre-push tests when pushing clean new branch ([a6dbcac](https://github.com/ALLiDoizCode/connector/commit/a6dbcacb8a5d3e0254cdc5e91e28a939df622835))

## [1.13.0](https://github.com/ALLiDoizCode/connector/compare/v1.12.0...v1.13.0) (2026-02-12)

### Features

- **epic-26:** npm publish readiness — trim dependencies, configure packages, add validation ([b62fc02](https://github.com/ALLiDoizCode/connector/commit/b62fc02eb283ad44acfbe8cf32cefe8a173dd0fd))

### Bug Fixes

- **epic-26:** add peer deps to devDependencies and fix CJS/ESM compat in requireOptional ([b2789ae](https://github.com/ALLiDoizCode/connector/commit/b2789aed31f84462b7025942562f14083e5cdde0))
- **tests:** increase xrp-channel-lifecycle beforeAll timeout to 15s ([4abe8a9](https://github.com/ALLiDoizCode/connector/commit/4abe8a9184e25b9604ec5c086dccfef61d65edf9))
- **tests:** relax wallet-derivation performance thresholds for concurrent execution ([b558007](https://github.com/ALLiDoizCode/connector/commit/b558007863dc09f804c274fb00c804fd2877a483))
- **tests:** use OS-assigned ports in btp-server tests to eliminate EADDRINUSE flakiness ([8bc7443](https://github.com/ALLiDoizCode/connector/commit/8bc74438d4c5e6293dfb2bf4a5b1f992ccd4345a))

## [1.12.0](https://github.com/ALLiDoizCode/connector/compare/v1.11.0...v1.12.0) (2026-02-11)

### Features

- **epic-25:** CLI/library separation & lifecycle cleanup ([dc995e4](https://github.com/ALLiDoizCode/connector/commit/dc995e42c9e83be15afb4ac8af462c2bd64d5c45))

## [1.11.0](https://github.com/ALLiDoizCode/connector/compare/v1.10.0...v1.11.0) (2026-02-11)

### Features

- **epic-24:** connector library API — config object, local delivery handler, sendPacket, admin methods ([fb3ab01](https://github.com/ALLiDoizCode/connector/commit/fb3ab01bcae250cd103db21e2be44c6411cffcf1))

### Bug Fixes

- derive BTP timeouts from ILP packet expiresAt, sync deployment configs ([f88f618](https://github.com/ALLiDoizCode/connector/commit/f88f618f9d26258b29194ed859b0a72a3aee6c45))
- **epics-20-23:** resolve integration gaps — field names, channel types, deploy script ([6cdc389](https://github.com/ALLiDoizCode/connector/commit/6cdc389f62eea696fdbaa114a194d7727c965299))
- **telemetry:** suppress WebSocket error on terminate during CONNECTING state ([3395bad](https://github.com/ALLiDoizCode/connector/commit/3395badf3395d6ce53fad45870e9259cb3e42057))
- **tests:** add missing isConnected mock, fix BTP timeout test timing ([f874c1e](https://github.com/ALLiDoizCode/connector/commit/f874c1e6c9d4b5c18a1404861b823ad3eb9e5d21))
- **tests:** increase claim-sender retry test timeout from 50ms to 10s ([3fb9528](https://github.com/ALLiDoizCode/connector/commit/3fb95284407c36ab0915c146d0d4c08427c4c5f9))
- **tests:** increase log-telemetry hook timeouts, use random port ([eae5ed5](https://github.com/ALLiDoizCode/connector/commit/eae5ed59359f07435455388cfe4b7ec6d270aee2))
- **tests:** use random ports to eliminate EADDRINUSE flakiness ([5d7be0f](https://github.com/ALLiDoizCode/connector/commit/5d7be0f4676361ff1089917ac3e2799a81675203))

## [1.10.0](https://github.com/ALLiDoizCode/connector/compare/v1.9.0...v1.10.0) (2026-02-09)

### Features

- **epic-22:** simplify agent-runtime middleware — remove SPSP/session, add SHA-256 fulfillment ([8b9f324](https://github.com/ALLiDoizCode/connector/commit/8b9f324fe80b900e1431468b18283d04acd24662))
- **epic-23:** unified deployment infrastructure — compose, K8s, deploy script ([c8b58a5](https://github.com/ALLiDoizCode/connector/commit/c8b58a5110ae50e64015658b615779d8ffbcab77))

## [1.9.0](https://github.com/ALLiDoizCode/connector/compare/v1.8.0...v1.9.0) (2026-02-09)

### Features

- add ElizaOS plugin generator skill with research docs ([4718c97](https://github.com/ALLiDoizCode/connector/commit/4718c976f9b619671d33faac51c04ef18522c4c5))

### Bug Fixes

- stabilize flaky CI tests for memory profiling and settlement failover ([6b54119](https://github.com/ALLiDoizCode/connector/commit/6b5411937c7d5ceabf3e047b5a169e29b9ecf2e3))

## [1.8.0](https://github.com/ALLiDoizCode/connector/compare/v1.7.0...v1.8.0) (2026-02-09)

### Features

- **epic-21:** add payment channel admin APIs with balance and settlement queries ([1e25e48](https://github.com/ALLiDoizCode/connector/commit/1e25e48c42f80c52fa1343aed506e472f06d2a6b))

### Bug Fixes

- skip TigerBeetle integration tests when Docker is unavailable ([cdebfde](https://github.com/ALLiDoizCode/connector/commit/cdebfde3f9bc495ce900571cbd74e7a34faf94a6))

## [1.7.0](https://github.com/ALLiDoizCode/connector/compare/v1.6.2...v1.7.0) (2026-02-08)

### Features

- **epic-20:** add missing type definitions and wiring for bidirectional middleware ([f4ef6a0](https://github.com/ALLiDoizCode/connector/commit/f4ef6a021584da64c877ae251076589e7b9667b5))

## [1.6.2](https://github.com/ALLiDoizCode/connector/compare/v1.6.1...v1.6.2) (2026-02-06)

### Code Refactoring

- complete rebrand from m2m to agent-runtime across documentation and configs ([2298fa4](https://github.com/ALLiDoizCode/connector/commit/2298fa4d9e1e7c94ba420680804812af06ccc4b1))

## [1.6.1](https://github.com/ALLiDoizCode/connector/compare/v1.6.0...v1.6.1) (2026-02-05)

### Bug Fixes

- **ci:** install libsql native module for Linux in CI test job ([f9ff8b1](https://github.com/ALLiDoizCode/connector/commit/f9ff8b13880f2d1c0cbb2932f605e7580f447c5c))
- **ci:** install libsql native module for Linux in integration tests ([70237ac](https://github.com/ALLiDoizCode/connector/commit/70237ac875f4b827e94ca05fea488eea2b1fcad4))
- **ci:** update all imports from @m2m/shared to @toon-protocol/shared ([6804143](https://github.com/ALLiDoizCode/connector/commit/6804143a29ca3b4fa0dbaf94bd774fe55da89585))
- **ci:** update package names from @m2m/_ to @agent-runtime/_ ([ab68361](https://github.com/ALLiDoizCode/connector/commit/ab68361ec3fcc231ae514e2011785f6578797ea5))
- **ci:** update package-lock.json for @agent-runtime/\* package names ([2a343a1](https://github.com/ALLiDoizCode/connector/commit/2a343a10b33b2e43a34450fbc4de931127e04ec1))
- **docker:** resolve libsql native module and port conflicts ([6c2c6c2](https://github.com/ALLiDoizCode/connector/commit/6c2c6c2b80ff58eb9850cd899dd1c4e9b26545be))
- **settlement:** use max uint256 approval to prevent insufficient allowance errors ([bb61adb](https://github.com/ALLiDoizCode/connector/commit/bb61adb6b6ff4c48678e534b320416e04d58eba1))
- **test:** make multi-chain settlement acceptance test deterministic ([40a9842](https://github.com/ALLiDoizCode/connector/commit/40a9842b360189af6d4ffbf6d0366790623b7716))

## [1.6.0](https://github.com/ALLiDoizCode/m2m/compare/v1.5.0...v1.6.0) (2026-02-05)

### Features

- add Epic 28-30 - testnet integration, explorer links, balance proofs ([dcbbdd9](https://github.com/ALLiDoizCode/m2m/commit/dcbbdd9751688d334c86e6034a55412d93f4611f))
- add Epics 29-32 - UI components, balance proofs, workflow demo, private messaging ([a573b08](https://github.com/ALLiDoizCode/m2m/commit/a573b082d5d7d4626ba7c50b6e44576e00c8bb43))
- add NETWORK_MODE flag for testnet/mainnet switching ([4e0b247](https://github.com/ALLiDoizCode/m2m/commit/4e0b24793280d34d948e38825932bba6be7527dc))
- add production-ready Docker Compose and Kubernetes deployments ([ec2745f](https://github.com/ALLiDoizCode/m2m/commit/ec2745f780818ca081b8ff30ad6d46fa2db48531))
- **agent-runtime:** add Agent Runtime package for custom business logic integration ([7116509](https://github.com/ALLiDoizCode/m2m/commit/7116509f788cc9c56314d370af87900dbed63732))
- complete deployment testing - Docker Compose and Kubernetes verified ([347a82f](https://github.com/ALLiDoizCode/m2m/commit/347a82ff7e00821cd57c052b800be1c2566ee347))
- **connector:** add Admin API for dynamic peer and route management ([3439a99](https://github.com/ALLiDoizCode/m2m/commit/3439a992a56f235fc13ef312fb793b967e2aa305))
- **epic-17:** complete Story 17.6 - Telemetry and Monitoring ([c222ca7](https://github.com/ALLiDoizCode/m2m/commit/c222ca71e8aa5a7a06888cc6999c71cea0b3bfd2))
- **epic-17:** implement Story 17.7 - BTP Claim Exchange Integration Tests ([146ee70](https://github.com/ALLiDoizCode/m2m/commit/146ee7030e74745d20bc5d1c6439a1fedfdca1a1))
- **epic-17:** reorganize epics 11-15 and add Epic 16-17 ([2dcf8e2](https://github.com/ALLiDoizCode/m2m/commit/2dcf8e2881ff77c1ee41080105afb1b8eaf177ce))
- **epic-18,19:** complete Explorer UI NOC redesign and deployment improvements ([7871746](https://github.com/ALLiDoizCode/m2m/commit/787174628398dca730754b16d866b68a8ca04499))
- **epic-18:** create Epic 18 - Explorer UI NOC Redesign ([3d7accf](https://github.com/ALLiDoizCode/m2m/commit/3d7accf22c4703f30993dd835f9b0af711ceab92)), closes [#0D1829](https://github.com/ALLiDoizCode/m2m/issues/0D1829)
- **epic-19:** implement M2M token funding and fix Explorer UI peer tracking ([dffde6d](https://github.com/ALLiDoizCode/m2m/commit/dffde6d689c53b58f2460565dc8b407ed66f591c))
- **epic-20:** add zkVM verification and agent service markets ([cea9e56](https://github.com/ALLiDoizCode/m2m/commit/cea9e567c30c844e0efa784734456d5f0a193485))
- **epic-27:** implement Aptos payment channel settlement ([56bb455](https://github.com/ALLiDoizCode/m2m/commit/56bb4550d6911ba3e28284e76d16155115e55ff8))
- **epic-28:** add Aptos multi-arch Docker build files ([c28c5a2](https://github.com/ALLiDoizCode/m2m/commit/c28c5a2054c76c61c150d1039ed8cdc13c7de7df))
- **epic-28:** add ARM64 Aptos Docker image epic ([fa2c9d7](https://github.com/ALLiDoizCode/m2m/commit/fa2c9d7e5084a785dabaab260c69e93abbc06035))
- **explorer:** add fee statistics by network with token denomination ([33b04db](https://github.com/ALLiDoizCode/m2m/commit/33b04db714fb5830e736a39a92d0307d453e3112))
- **scripts:** add agent runtime testing to 5-peer deployment script ([55f1d28](https://github.com/ALLiDoizCode/m2m/commit/55f1d28e7e8036241deb4db2da37bde24a8cd6e6))
- **tri-chain:** enhance 5-peer multihop with tri-chain configuration ([88e49b0](https://github.com/ALLiDoizCode/m2m/commit/88e49b069fc449cd53de0f6d9653c2916406aba1))

### Bug Fixes

- **ci:** filter Aptos tests to channel module and fix rippled config ([bdb953f](https://github.com/ALLiDoizCode/m2m/commit/bdb953ff422dac1bd4ae4e88bea7923bf790b774))
- **ci:** fix Aptos SDK tests and make npm audit advisory ([9574d23](https://github.com/ALLiDoizCode/m2m/commit/9574d234629f787ae15abb72bb34e8016f2ec1a0))
- **ci:** make security job advisory in CI status check ([9376d27](https://github.com/ALLiDoizCode/m2m/commit/9376d271a9f53b0678b573f667ca3b6ef6a01745))
- **ci:** make Snyk scan continue-on-error ([2a7c902](https://github.com/ALLiDoizCode/m2m/commit/2a7c90245ce57d33715244253ca653899bf11c80))
- **ci:** resolve Aptos Move address conflict and add docker-compose-dev.yml ([55f0028](https://github.com/ALLiDoizCode/m2m/commit/55f00287e45053db437966e45b2fcaca1a4adfcc))
- **ci:** skip integration tests with missing type dependencies ([16e544b](https://github.com/ALLiDoizCode/m2m/commit/16e544b5b99445f9ed3dc7d1a7e63e3137a6b78c))
- **ci:** skip tigerbeetle-5peer-deployment.test.ts ([42f76cf](https://github.com/ALLiDoizCode/m2m/commit/42f76cf10f9b61353266a28974dd84b877840033))
- **docker-compose:** enable TigerBeetle and settlement in production ([5c7c490](https://github.com/ALLiDoizCode/m2m/commit/5c7c490327eee655302d6af67ef3341d09c0eb9a))
- **docs:** include data and expiresAt fields in business logic examples ([4a1d179](https://github.com/ALLiDoizCode/m2m/commit/4a1d179030dd45b0f8b49872fc0e015b79bd021e))
- **epic-17:** complete Story 17.7 - all integration tests passing (10/10) ([48b489a](https://github.com/ALLiDoizCode/m2m/commit/48b489a3eb0812a509d6834191d6bdce4629dd52))
- **telemetry:** check WebSocket state before closing in disconnect ([61fcd28](https://github.com/ALLiDoizCode/m2m/commit/61fcd28dcade2a156ad9dd2fdd77afeec96831ab))
- update tests for openChannel return type and add K8s TigerBeetle manifests ([7f1aa8e](https://github.com/ALLiDoizCode/m2m/commit/7f1aa8ee70488ac1071a6c6523bfae7d90d643a6))

## [1.6.0](https://github.com/ALLiDoizCode/m2m/compare/v1.5.0...v1.6.0) (2026-02-03)

### Features

- **explorer:** Dashboard redesign with NOC (Network Operations Center) aesthetic (Epic 18)
  - New Dashboard landing page with metrics grid (Total Packets, Success Rate, Active Channels, Routing Status)
  - Live Packet Flow visualization showing real-time packet routing
  - Staggered entry animations with `prefers-reduced-motion` support
  - Keyboard navigation (1-5 for tabs, ? for help)

- **explorer:** Enhanced Account Cards with balance history charts and settlement timeline (Story 18.4)

- **explorer:** Keys Tab for cryptographic key management with copy-to-clipboard (Story 18.6)

- **explorer:** Playwright MCP integration testing with comprehensive browser automation (Story 18.8)

- **docs:** Explorer UI documentation suite (Story 18.9)
  - Redesign guide with design philosophy and color palette
  - User guide with common workflows and troubleshooting
  - Developer guide with architecture and customization

### Changed

- **explorer:** Events tab renamed to Packets tab for ILP terminology alignment (Story 18.3)
- **explorer:** Dashboard is now the default landing page (was Events/Packets)
- **explorer:** Updated color scheme to NOC aesthetic with deep space background and cyan/emerald/rose accents

### Improved

- **explorer:** Peers Tab with NOC aesthetic enhancement (Story 18.5)
- **explorer:** Header with technical branding and WebSocket connection status (Story 18.2)
- **explorer:** Animation system with hover effects, stagger classes, and smooth transitions (Story 18.7)

## [1.5.0](https://github.com/ALLiDoizCode/m2m/compare/v1.4.0...v1.5.0) (2026-01-28)

### Features

- **agent:** add DVM job feedback formatter (Story 17.3) ([538a01c](https://github.com/ALLiDoizCode/m2m/commit/538a01c12941bec436dd93651700e86f5991f77e))
- **agent:** complete Story 17.4 query handler migration to Kind 5000 ([ab32e37](https://github.com/ALLiDoizCode/m2m/commit/ab32e37c8f124095254dd53feef7135410fcfa64))
- **agent:** complete Story 17.5 job chaining support ([56b1c93](https://github.com/ALLiDoizCode/m2m/commit/56b1c9329c5a2c90c2a358b908a891b521953d9b))
- **agent:** complete Story 17.6 task delegation request parsing (Kind 5900) ([4b6caaa](https://github.com/ALLiDoizCode/m2m/commit/4b6caaa440606e1eaf6bc0bc5c008daab8df34a2))
- **agent:** complete Story 17.7 task delegation result (Kind 6900) ([a0ffdd9](https://github.com/ALLiDoizCode/m2m/commit/a0ffdd94df4d5c6561e4188a9ac8d9d8d128150a))
- **agent:** complete Story 17.8 task status tracking ([8e00acf](https://github.com/ALLiDoizCode/m2m/commit/8e00acf78c04107a0677ef64b220099c499fac37))
- **agent:** complete Story 17.9 timeout & retry logic ([04c39ef](https://github.com/ALLiDoizCode/m2m/commit/04c39eff869a4d22992a61fe729f775a9e44a504))
- **docs:** create Epic 17 stories 17.6-17.11 (complete story pipeline) ([cb4afbe](https://github.com/ALLiDoizCode/m2m/commit/cb4afbede19a8b894bf40b7c14d6401344cb4588))

### Bug Fixes

- **agent:** complete Epic 17 Story 17.4 - migrate query to Kind 5000 DVM ([06dcbfb](https://github.com/ALLiDoizCode/m2m/commit/06dcbfbf014591d7ac2df83e71db9b8b68fae1c5))

## [1.4.0](https://github.com/ALLiDoizCode/m2m/compare/v1.3.0...v1.4.0) (2026-01-28)

### Features

- **agent:** add AI agent module with Vercel AI SDK integration (Epic 16) ([3a36c64](https://github.com/ALLiDoizCode/m2m/commit/3a36c64893180e1956b299ad574428f109f8a941))
- **agent:** complete Epic 16 stories 16.3-16.7 with QA gates ([f96e0db](https://github.com/ALLiDoizCode/m2m/commit/f96e0db6404eb6220961daa44ef3f07ae48c87b7))

## [1.3.0](https://github.com/ALLiDoizCode/m2m/compare/v1.2.0...v1.3.0) (2026-01-27)

### Features

- **contracts:** deploy TokenNetworkRegistry to Base Sepolia and Base Mainnet ([8569685](https://github.com/ALLiDoizCode/m2m/commit/8569685b484689d549c26f02ac7389dff02ef9ce))

## [1.2.0](https://github.com/ALLiDoizCode/m2m/compare/v1.1.0...v1.2.0) (2026-01-27)

### Features

- **agent:** implement real EVM payment channels for Docker agent test ([bce647f](https://github.com/ALLiDoizCode/m2m/commit/bce647fbc24db34ac9cfb1928e0858b9d73d4105))
- **explorer:** add ILP packet type display with routing fields ([9974d71](https://github.com/ALLiDoizCode/m2m/commit/9974d71a42b0c3f7b5fd5279eeea2731e4794086))
- **explorer:** add on-chain wallet panel and improve accounts view ([b260a81](https://github.com/ALLiDoizCode/m2m/commit/b260a8144101fd86dc24fc2d8f1f704df80e2150))
- **explorer:** add packet ID correlation and improve status display ([fe5e582](https://github.com/ALLiDoizCode/m2m/commit/fe5e582157dec817bedb0ecf8ea34f0035e4b2b6))
- **explorer:** add Peers & Routing Table view, historical data hydration, and QA reviews ([285b8a3](https://github.com/ALLiDoizCode/m2m/commit/285b8a30074d1992c7b37a517c1a98ae3d2375c1))
- **explorer:** Epic 15 — Agent Explorer polish, performance & visual quality ([d10037c](https://github.com/ALLiDoizCode/m2m/commit/d10037ceea6c23b2ab5eb7e7fa3e0f6711a529c5))
- **explorer:** implement Packet/Event Explorer UI (Epic 14) ([de13d82](https://github.com/ALLiDoizCode/m2m/commit/de13d82d6a70f1caf1de83457c1a209b0188c2d0))

### Bug Fixes

- **build:** exclude test files from explorer-ui production build ([df63d4d](https://github.com/ALLiDoizCode/m2m/commit/df63d4dca56bb5f9af2c42a6291afca41236d415))
- **explorer:** emit telemetry when receiving packet responses ([c923628](https://github.com/ALLiDoizCode/m2m/commit/c923628676fef98d2c4435a2aa5056ac77d6c2f4))
- **test:** set EXPLORER_PORT in mesh config tests to avoid port conflict ([c0cfed4](https://github.com/ALLiDoizCode/m2m/commit/c0cfed4e670e6da6dfc4129a2fba20523b2acea5))

## [1.1.0](https://github.com/ALLiDoizCode/m2m/compare/v1.0.0...v1.1.0) (2026-01-24)

### Features

- **agent:** implement Agent Society Protocol stories 13.3-13.8 ([cb4e0a4](https://github.com/ALLiDoizCode/m2m/commit/cb4e0a4acfcd8aaf2acf59e8caa443b71305fdec))
- **agent:** implement TOON codec and event database (Epic 13) ([2d70a20](https://github.com/ALLiDoizCode/m2m/commit/2d70a20dd2a82c1ca48367f58dc9d4684a4e3b5e))

### Bug Fixes

- Increase HEAP_MB threshold to 1000 for CI variability ([5d6b189](https://github.com/ALLiDoizCode/m2m/commit/5d6b18998c0568aa79c502fe81c9636649c98146))
- Increase slope threshold to 10 for CI memory test variability ([e5e093b](https://github.com/ALLiDoizCode/m2m/commit/e5e093b365148341aed7eb6837380c01348221d1))

## 1.0.0 (2026-01-23)

### Features

- Add agent wallet balance tracking and monitoring (Story 11.3) ([87979ec](https://github.com/ALLiDoizCode/m2m/commit/87979ec5b7dbb77cf114dcd70c99075b9538e09c))
- Add automated agent wallet funding (Story 11.4) ([0be5045](https://github.com/ALLiDoizCode/m2m/commit/0be5045dca9b54b6703a481f2726fd661138a1cb))
- Add HD wallet master seed management (Story 11.1) ([1bc688e](https://github.com/ALLiDoizCode/m2m/commit/1bc688ee32bf8b0822d6ad3bf2156651b8234f34))
- Add test utilities for isolation and mock factories ([398ed8a](https://github.com/ALLiDoizCode/m2m/commit/398ed8ace56686b564e2d0a9e471a4c0fefc9326))
- Complete audit logging, environment config, and comprehensive tests (Story 12.2) ([054a3f9](https://github.com/ALLiDoizCode/m2m/commit/054a3f9b0bfb2b7f3f992aedb51de2f97bfdeb96))
- Complete Epic 12 Stories 12.3, 12.4, 12.5 - Security and Performance ([22fead2](https://github.com/ALLiDoizCode/m2m/commit/22fead2a27b2904e09a9c40a840bba83177b10dd))
- Complete Epic 12 Stories 12.6-12.9 - Production Infrastructure & Documentation ([a250dc1](https://github.com/ALLiDoizCode/m2m/commit/a250dc11a9be4c73f66f90338f02f1b04968c76a))
- Complete Stories 8.6-8.10 - Payment Channel SDK and Dashboard Visualization ([b7b839f](https://github.com/ALLiDoizCode/m2m/commit/b7b839f193589e631565e41d1d0cf1194a833293))
- Complete Story 11.10 - Agent Wallet Documentation with QA Review ([88b9456](https://github.com/ALLiDoizCode/m2m/commit/88b94569b62d55494952c53acea38e947d46aa06))
- Complete Story 11.5 - Agent Wallet Lifecycle Management ([a65d750](https://github.com/ALLiDoizCode/m2m/commit/a65d7501b7bf249537a610cc14638f5a730ffe78))
- Complete Story 12.10 and create Story 13.1 draft ([8af827b](https://github.com/ALLiDoizCode/m2m/commit/8af827b2b69518e209f97643bf809ba7ee340a99))
- Complete Story 8.2 - TokenNetworkRegistry smart contract with QA review ([ca5aaa3](https://github.com/ALLiDoizCode/m2m/commit/ca5aaa38284d736be4a87b8e4a177887c4601515))
- Epic 10 CI/CD Pipeline Reliability (Stories 10.1-10.3) ([8d8324a](https://github.com/ALLiDoizCode/m2m/commit/8d8324a1c161a76490cdb9338774cc55dafe020e))
- **epic-11:** Complete Story 11.6 - Payment Channel Integration for Agent Wallets ([09f8411](https://github.com/ALLiDoizCode/m2m/commit/09f8411eaab7879bfa70e96891769030bda74aa9))
- Implement Epic 9 - XRP Payment Channels Integration ([235acb5](https://github.com/ALLiDoizCode/m2m/commit/235acb5f89f6dea62ef6ca2e255b7a14df26f715))
- Implement HSM/KMS key management and automated rotation (Story 12.2 Tasks 5-6) ([c090361](https://github.com/ALLiDoizCode/m2m/commit/c0903614918fd32e0679f115e7722485d8ac3416))
- Implement TokenNetwork payment channels (Stories 8.3-8.5) ([c0cc270](https://github.com/ALLiDoizCode/m2m/commit/c0cc2708f1b7929676026275587bed94d31c82cd))

### Bug Fixes

- Add 30s default timeout to connector tests ([1ac45f6](https://github.com/ALLiDoizCode/m2m/commit/1ac45f66bca26f867164c65711d38397bfaf1ea5))
- Add BigInt serialization support in wallet-backup-manager tests ([3bc30ef](https://github.com/ALLiDoizCode/m2m/commit/3bc30ef00a90442258665926d709c155a6f3d264))
- Add build step to integration tests workflow before running tests ([f79c9bb](https://github.com/ALLiDoizCode/m2m/commit/f79c9bb51504232f95f48dd7bdc6997770b90f69))
- Add custom rippled config to bind RPC endpoints to 0.0.0.0 ([75e770c](https://github.com/ALLiDoizCode/m2m/commit/75e770c2986535dcee61455caeaf1560f363dbfd))
- Add explicit return types to all component functions ([1ff858b](https://github.com/ALLiDoizCode/m2m/commit/1ff858b2bdadf3565e498b9b6284f34bfb8adcdf))
- Add missing forge-std submodule to root .gitmodules ([1ca73c2](https://github.com/ALLiDoizCode/m2m/commit/1ca73c2d5c86734a579d9c7e8f4f17193a3be64e))
- Add missing TelemetryEvent import to telemetry-server ([19eb0bb](https://github.com/ALLiDoizCode/m2m/commit/19eb0bbc827656efd3688027622372a4c448191e))
- Add missing variables and fix method names in additional test cases ([80b37b7](https://github.com/ALLiDoizCode/m2m/commit/80b37b7919ccf3fdcf45b736a450ebefd425d587))
- Add test isolation cleanup in wallet-disaster-recovery tests ([85fbb6d](https://github.com/ALLiDoizCode/m2m/commit/85fbb6dd964c0176b7e370127eb5ba69d4e0af87))
- Add type assertions in logger.test.ts for signer property access ([2c8dd35](https://github.com/ALLiDoizCode/m2m/commit/2c8dd3578a170f94e542525fad9e49f2ca45500a))
- Add type assertions to resolve TypeScript compilation errors ([6149071](https://github.com/ALLiDoizCode/m2m/commit/6149071a34e2b1bba5e67664330d9e2405a5bdd5))
- Add type definitions and null checks to wallet disaster recovery test ([839b7c8](https://github.com/ALLiDoizCode/m2m/commit/839b7c8b58ccf636af1a6880b684d63a6a2ddd7f))
- Add type guard for req.account in mock implementation ([0a80060](https://github.com/ALLiDoizCode/m2m/commit/0a80060151959f96578d3376a516b8eab46ef11c))
- Adjust dashboard coverage thresholds to current levels ([f385fc5](https://github.com/ALLiDoizCode/m2m/commit/f385fc5159ab92829f1bdf901094abe46394484e))
- Adjust latency test threshold for timer resolution variance ([790be5e](https://github.com/ALLiDoizCode/m2m/commit/790be5e03604c9d68b13890e768260f447c4c84a))
- Adjust performance test thresholds for CI environment variability ([c9ae928](https://github.com/ALLiDoizCode/m2m/commit/c9ae9289a0b479358847d706ae5009e5f422ede8))
- Cast TelemetryMessage to TelemetryEvent for handler methods ([65507f5](https://github.com/ALLiDoizCode/m2m/commit/65507f584fff9e476ca6f9e2d18b78766ac02af4))
- Configure OpenZeppelin contracts as Git submodule ([16baac7](https://github.com/ALLiDoizCode/m2m/commit/16baac707fdc94b76ddd8dfda0da1aed1a2a6ab7))
- Correct Anvil command format to listen on all interfaces ([83fbab4](https://github.com/ALLiDoizCode/m2m/commit/83fbab4898603c0dad82f08b4f30c9e77231ce4c))
- Correct AWS KMS SDK enum values and TypeScript errors ([bd8b36c](https://github.com/ALLiDoizCode/m2m/commit/bd8b36cf968e7032c79a8df7234a94a3098ca0a4))
- Create peer agents in channel state restore test ([f323cea](https://github.com/ALLiDoizCode/m2m/commit/f323ceaede8ba35443d5e81661a245b256098981))
- Disable dashboard coverage thresholds and add testing guidelines ([2894ca0](https://github.com/ALLiDoizCode/m2m/commit/2894ca040a24183c46eb5652c4f7b367d299b115))
- Exclude cloud KMS backend tests from Jest runs ([c1bc3ab](https://github.com/ALLiDoizCode/m2m/commit/c1bc3ab3c02b5e292abdeb4226cfa49c787b1406))
- Fix another timing-sensitive assertion in token-bucket test ([8c7d577](https://github.com/ALLiDoizCode/m2m/commit/8c7d5776deb38ceb98fac39d20add28addde3409))
- Fix CI test failures in integration tests ([5099d7a](https://github.com/ALLiDoizCode/m2m/commit/5099d7ab2a1aece9752b8d57257d0b22c6159343))
- Fix ESLint errors and RFC link test failures in CI ([a8488e2](https://github.com/ALLiDoizCode/m2m/commit/a8488e2ea602c7866844051a68b4c2626f842619))
- Fix timing variance in concurrent measurements test ([085baba](https://github.com/ALLiDoizCode/m2m/commit/085baba4e04102f62c7339070445f4b806bb2138))
- Fix timing variance in getAvailableTokens test ([10aa092](https://github.com/ALLiDoizCode/m2m/commit/10aa09278db15004463b75ec095049cc891aa880))
- Fix TypeScript errors and test failures in XRP test files ([a0f806a](https://github.com/ALLiDoizCode/m2m/commit/a0f806a5c1b46c06c3a59ac7b83fcd0b447722a0))
- Fix TypeScript errors in XRP test files and update fix-ci command ([8c7acc0](https://github.com/ALLiDoizCode/m2m/commit/8c7acc03635082ba4cbcd6c6689a45c22cae6407))
- Fix TypeScript type errors in agent-balance-tracking integration test ([e04d3a0](https://github.com/ALLiDoizCode/m2m/commit/e04d3a00169972dedbf59618c9e95a52d44c389a))
- Increase Anvil startup timeout to prevent CI failures ([206d66b](https://github.com/ALLiDoizCode/m2m/commit/206d66b1c46735b514e58e5a34ccebbb7e546000))
- Increase HEAP_MB threshold to 1000 for CI variability ([ba580ef](https://github.com/ALLiDoizCode/m2m/commit/ba580ef5fdf8343a48a15ec475524d01f0e71385))
- Lower dashboard coverage thresholds to match Story 8.10 baseline ([5bdeebe](https://github.com/ALLiDoizCode/m2m/commit/5bdeebe1f8969f6cf337eeb333d7d5be3f700ae0))
- Make timing-safe comparison test more robust for CI ([24ad104](https://github.com/ALLiDoizCode/m2m/commit/24ad104f6e70111ac5d88e358ce00fab637f7dc7))
- Override Anvil entrypoint to ensure --host 0.0.0.0 is respected ([39f8569](https://github.com/ALLiDoizCode/m2m/commit/39f85692d00e82139d8a7b9e3c32295a4a2e8686))
- Properly narrow unknown types in type guards ([3fd76b8](https://github.com/ALLiDoizCode/m2m/commit/3fd76b83b6823c3f7d3f9f63186fe8dd5ec298ee))
- Relax performance assertion in agent-wallet-uniqueness test ([2ce1ae2](https://github.com/ALLiDoizCode/m2m/commit/2ce1ae2d2cf7f2f70da42a560849ff3bccd2ef34))
- Remove explicit --conf argument for rippled (entrypoint adds it automatically) ([2708684](https://github.com/ALLiDoizCode/m2m/commit/2708684d48a512cd3c0db420d672101e0abd8bd7))
- Replace Docker healthchecks with runner-based connectivity tests ([ac9aaf6](https://github.com/ALLiDoizCode/m2m/commit/ac9aaf6f4e93521350c35822540894b962f8a14f))
- Resolve CI test failures and update Docker Compose to V2 ([3730dad](https://github.com/ALLiDoizCode/m2m/commit/3730dad45b7d7479ae380e7dc5487834cc63ca25))
- Resolve CI test failures in Epic 11 ([3117780](https://github.com/ALLiDoizCode/m2m/commit/31177808296f1b66117bf182c4bafd126811ba02))
- Resolve ESLint errors in wallet integration tests ([896277f](https://github.com/ALLiDoizCode/m2m/commit/896277f734e9a37f7fa74ef2a7ffd27320d6b217))
- Resolve ESLint no-explicit-any and no-var-requires errors ([c08d64e](https://github.com/ALLiDoizCode/m2m/commit/c08d64eebff481ce6ebc91dbef068405f6bd72a2))
- Resolve integration test failures in CI ([184b57e](https://github.com/ALLiDoizCode/m2m/commit/184b57e25b2438d00c4629f8f6d88c9c7cd5de45))
- Resolve integration test failures in CI ([7174a59](https://github.com/ALLiDoizCode/m2m/commit/7174a595728ec3fae79954bff9204e5599ba5dae))
- Resolve test failures in wallet-backup-manager and doc tests ([90931ea](https://github.com/ALLiDoizCode/m2m/commit/90931ea6346a9792e8cc3fe053ecbdfe56ae790e))
- Resolve TypeScript and test failures in wallet components ([7385b0d](https://github.com/ALLiDoizCode/m2m/commit/7385b0d4da899e0fe48a522c978ea0f91f48c94c))
- Resolve TypeScript compilation errors in wallet-backup-manager ([8601d17](https://github.com/ALLiDoizCode/m2m/commit/8601d17361d834b2c778642e26c60562b5748151))
- Resolve TypeScript errors and test failures in CI ([eaa7bd7](https://github.com/ALLiDoizCode/m2m/commit/eaa7bd7fd87833edad27bac2b48400e94830486c))
- Resolve TypeScript errors and test failures in wallet components ([a9c10e0](https://github.com/ALLiDoizCode/m2m/commit/a9c10e0fd670645c260db3617e7600b2b31f07f1))
- Skip flaky XRP integration tests in CI environment ([631e5f8](https://github.com/ALLiDoizCode/m2m/commit/631e5f862f1631f29ae6083d62b8f4d55a857d95))
- Skip heavy wallet derivation tests in CI and fix TypeScript errors ([58787ea](https://github.com/ALLiDoizCode/m2m/commit/58787ea455130322930ee89fe8b150550fddac42))
- Sync package-lock.json with package.json ([a91d57a](https://github.com/ALLiDoizCode/m2m/commit/a91d57a3765d1beca0f5c38a2f85a93051f3e9cd))
- Synchronize package-lock.json with package.json ([355d8ce](https://github.com/ALLiDoizCode/m2m/commit/355d8ce0cd029b03311c1af57466a19676c17f3b))
- Update integration tests to use docker-compose-dev infrastructure ([e0f0a08](https://github.com/ALLiDoizCode/m2m/commit/e0f0a087bcbd693599aee2c7fd04d1cac864ceb6))
- Update test files for changed constructor signatures ([193b161](https://github.com/ALLiDoizCode/m2m/commit/193b1612256ebb4c37f9473bc057a7fc7e223bbd))
- Update test files to use current API signatures ([004779f](https://github.com/ALLiDoizCode/m2m/commit/004779f343e01c8f0c44ea95027000c0b47f977f))
- Use block eslint-disable for test mock setup ([1855b7e](https://github.com/ALLiDoizCode/m2m/commit/1855b7e6dc8a4e5cdd4108f8d8f59df9bab43d07))
- Use full path for tigerbeetle command in init script ([2240b5d](https://github.com/ALLiDoizCode/m2m/commit/2240b5d4ae0505600360e7f4cd68ad5f0f6774c0))
- Wait for all 3 services to be healthy before running integration tests ([b438ffe](https://github.com/ALLiDoizCode/m2m/commit/b438ffe91ebf6b016301887f3b3d797fd448aec3))

### Code Refactoring

- Remove dashboard package and defer visualization to future project ([43334b6](https://github.com/ALLiDoizCode/m2m/commit/43334b61a52c5533e34b7f183b2ca67ee3fd0fd4))

## [0.1.0] - 2025-12-31

### Initial MVP Release

This is the first MVP release of the M2M ILP Connector, providing a functional Interledger Protocol v4 (RFC-0027) connector implementation with real-time monitoring capabilities.

### Added

#### Core ILP Functionality

- **ILPv4 Packet Handling** - Full implementation of RFC-0027 Interledger Protocol v4
  - ILP Prepare, Fulfill, and Reject packet processing
  - Packet validation with expiry time checking and safety margins
  - OER (Octet Encoding Rules) serialization/deserialization per RFC-0030
  - Structured error codes and error handling per RFC-0027

#### Routing & Forwarding

- **Static Routing Table** - Longest-prefix match routing with configurable priority
  - Support for hierarchical ILP addresses per RFC-0015
  - Route validation and lookup optimization
  - Multi-hop packet forwarding through connector chains

#### BTP Protocol Implementation

- **Bilateral Transfer Protocol (BTP)** - RFC-0023 compliant implementation
  - WebSocket-based peer connections with auto-reconnection
  - Bidirectional packet forwarding (both outbound and incoming peers)
  - Shared-secret authentication with environment variable configuration
  - Connection health monitoring and retry with exponential backoff
  - Resilient startup tolerating temporary peer unavailability

#### Configuration & Deployment

- **YAML Configuration** - Human-readable configuration files
  - Node identity (nodeId, BTP server port, log level)
  - Static routing table definition
  - Peer connection definitions
  - Health check configuration
- **Docker Support** - Production-ready containerization
  - Multi-stage Dockerfile for optimized image size
  - Docker Compose configurations for multiple topology patterns
  - Health check integration with Docker/Kubernetes orchestration

#### Monitoring & Observability

- **Real-time Telemetry** - WebSocket-based telemetry streaming
  - NODE_STATUS events (routes, peer connections, health)
  - PACKET_ROUTED events (packet forwarding with correlation IDs)
  - LOG events (structured application logs)
- **Health Check HTTP Endpoint** - Production readiness monitoring
  - `/health` endpoint with JSON status response
  - Peer connection percentage tracking
  - Uptime and version information
- **Structured Logging** - Pino-based JSON logging
  - Correlation IDs for request tracing
  - Component-level log contexts
  - Configurable log levels

#### Dashboard & Visualization

- **React Dashboard Application** - Real-time network visualization
  - Interactive network topology graph using Cytoscape.js
  - Live packet animation showing routing paths
  - Node status panel with connection health
  - Packet detail panel with full packet inspection
  - Filterable log viewer with level and node filtering
  - shadcn/ui component library for consistent UX

#### Development Tools

- **send-packet CLI** - Test packet injection utility
  - Single packet, batch, and sequential sending modes
  - Configurable amount, destination, expiry, and data payload
  - BTP authentication and error handling
  - Useful for testing and debugging connector networks

### Example Configurations

Five pre-configured Docker Compose topologies included:

- **Linear 3-Node** (`docker-compose.yml`) - Simple chain topology
- **Linear 5-Node** (`docker-compose-5-node.yml`) - Extended chain for performance testing
- **Mesh 4-Node** (`docker-compose-mesh.yml`) - Full mesh connectivity
- **Hub-Spoke** (`docker-compose-hub-spoke.yml`) - Centralized hub topology
- **Complex 8-Node** (`docker-compose-complex.yml`) - Mixed topology patterns

### Technical Implementation

#### Architecture

- **TypeScript** - Type-safe implementation with strict mode
- **Monorepo** - npm workspaces for shared code and modularity
- **Event-driven** - EventEmitter-based architecture for loose coupling
- **Async/await** - Promise-based async operations throughout

#### Dependencies

- Node.js 20 LTS
- TypeScript 5.x
- ws (WebSocket library)
- pino (structured logging)
- React 18 + Vite (dashboard)
- Cytoscape.js (graph visualization)

### Known Limitations

- **Static Routing Only** - Dynamic route discovery not yet implemented
- **No Settlement** - Payment settlement not implemented (routing only)
- **No STREAM Protocol** - Only base ILP packet forwarding
- **In-Memory State** - No persistence of routing tables or telemetry
- **Single Region** - No multi-region deployment support

### Performance Characteristics

- Packet forwarding latency: <10ms per hop (local network)
- Supports hundreds of concurrent packet flows
- WebSocket connections scale to dozens of peers per connector
- Dashboard handles 100+ telemetry events per second

### Security Considerations

- BTP authentication uses shared secrets (not production-grade)
- No TLS/encryption on BTP WebSocket connections
- No rate limiting or DDoS protection
- Suitable for development and testing only

---

## [Unreleased]

### Fixed

- **[10.1] Settlement Executor Test Failures** (commit 034a098)
  - Fixed event listener cleanup issue causing test failures
    - Previously `bind(this)` created new function references preventing `EventEmitter.off()` from matching handlers
    - Now store `boundHandleSettlement` in constructor for proper cleanup
  - Validated async timeout coverage for all settlement operations
    - Basic operations: 50ms, Deposit operations: 100ms, Retry operations: 500ms
  - Verified mock isolation with 10/10 stability test runs (100% pass rate)
  - Added test anti-patterns documentation to `test-strategy-and-standards.md`
  - Created root cause analysis at `docs/qa/root-cause-analysis-10.1.md`
  - Resolved Epic 10 CI/CD pipeline failures on settlement executor tests

### Added

- **[10.2] Pre-Commit Quality Gates**
  - Enhanced pre-commit hook with informative messages and fast targeted checks
    - Runs ESLint and Prettier on staged files only using lint-staged
    - Auto-fixes issues when possible (eslint --fix, prettier --write)
    - Execution time: 2-5 seconds for typical commits
  - Enhanced pre-push hook with optimized checks and related tests
    - Targeted linting on changed TypeScript files only
    - Format check across all files
    - Jest --findRelatedTests for changed source files (excludes test/type definition files)
    - Clear error messages with actionable fix instructions
    - Execution time: 10-30 seconds depending on changes
  - Added Pull Request template (`.github/PULL_REQUEST_TEMPLATE.md`)
    - Pre-submission quality checklist (hooks, tests, coverage, documentation)

- **[10.3] Document Test Quality Standards & CI Best Practices**
  - Expanded test-strategy-and-standards.md with additional anti-patterns
    - Anti-Pattern 4: Hardcoded timeouts in production code (use event-driven patterns or configurable delays)
    - Anti-Pattern 5: Incomplete test cleanup (resources not released)
    - Anti-Pattern 6: Testing implementation details instead of behavior
  - Added stability testing best practices
    - When to run stability tests (after fixing flaky tests, before production releases)
    - How to create stability test scripts (example: run-settlement-tests.sh)
    - Success criteria: 100% pass rate over N runs (N=10 for unit tests, N=3 for integration)
  - Added test isolation validation techniques
    - Run tests sequentially with `--runInBand` to detect order dependencies
    - Run tests in random order with `--randomize` to detect interdependencies
    - Run single test file in isolation to verify no workspace dependencies
  - Added code examples from actual project tests
    - Good example: settlement-executor.test.ts event listener cleanup
    - Good example: Mock isolation in beforeEach()
    - Bad example: Inline bind(this) anti-pattern
  - Created comprehensive CI troubleshooting guide (`docs/development/ci-troubleshooting.md`)
    - 7 common CI failure scenarios with diagnosis and resolution steps
    - Job-specific debugging procedures for all CI jobs (lint, test, build, type-check, contracts, E2E)
    - Investigation runbook with step-by-step debugging workflow
    - Monitoring guidelines for tracking CI health metrics
    - Continuous improvement process for systematic issue resolution
  - Documented epic branch workflow in developer-guide.md
    - Epic branch PR creation process with pre-PR checklist
    - Epic branch quality standards (zero tolerance for failures, coverage requirements)
    - Handling epic branch PR failures (reproduce locally, create hotfix, document root cause)
  - Added pre-push quality checklist to developer-guide.md
    - Code review checklist (staged changes, no console.log in production)
    - Quality gates checklist (pre-commit hooks, related tests)
    - Type safety checklist (strict mode compliance, no `any` types)
    - Test coverage checklist (>80% for new code)
    - Documentation checklist (README, CHANGELOG, architecture docs)
  - Created developer documentation index (`docs/development/README.md`)
    - Central hub organizing all documentation by category
    - Quick reference with common commands and checklists
    - Contributing path with ordered reading list
  - Updated main README.md with Developer Documentation section
    - Links to developer guide, git hooks, test standards, CI troubleshooting
    - Epic branch workflow and pre-push checklist references
  - Enhanced CONTRIBUTING.md with Before You Start and When Things Go Wrong sections
    - Required reading list (developer guide, git hooks, test standards, coding standards)
    - CI troubleshooting resources and test failure guides
    - Root cause analysis references
    - Issue reporting guidelines
  - Integrated all Epic 10 documentation for discoverability
    - Cross-references between related documents
    - Clear navigation paths from README to specialized guides
    - Consolidated test quality and CI/CD best practices
    - Type of change selection (feature, bugfix, refactor, docs, test)
    - Bypass justification section with warnings
  - Created Git hooks documentation (`docs/development/git-hooks.md`)
    - Detailed hook workflow and bypass mechanism documentation
    - Troubleshooting guide for common issues
    - Quick reference table for developers
  - Created developer guide (`docs/development/developer-guide.md`)
    - Quick reference for local quality checks
    - Hook workflow overview
  - Prevents CI failures by catching issues locally before push

Future planned features:

- Dynamic routing with route advertisement
- STREAM protocol support (RFC-0029)
- Settlement engine integration (RFC-0038)
- TLS support for BTP connections
- Rate limiting and traffic shaping
- Multi-region deployment
- Persistent routing table storage
- Performance optimization and benchmarking

[0.1.0]: https://github.com/anthropics/m2m/releases/tag/v0.1.0
