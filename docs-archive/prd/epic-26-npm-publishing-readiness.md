# Epic 26: npm Publishing Readiness — Brownfield Enhancement

**Epic Number:** 26
**Priority:** Medium — Required before agent-society can depend on published packages
**Type:** Package Configuration & Dependency Management
**Dependencies:** Epic 24 (connector library API), Epic 25 (CLI/library separation)

## Epic Goal

Prepare `@crosstown/shared` and `@crosstown/connector` for npm publication by trimming dependencies to minimize install footprint, configuring package.json for dual library/CLI usage, adding publish automation, and validating that the packages install and import correctly in a clean consumer project.

## Epic Description

### Existing System Context

**Current Functionality:**

- Root `package.json` has `"private": true` — monorepo root correctly excluded from publishing
- `@crosstown/shared` — `"version": "0.1.0"`, zero runtime dependencies, exports ILP types and OER codec. Nearly ready to publish as-is.
- `@crosstown/connector` — `"version": "0.1.0"`, has heavy dependencies including cloud SDKs, AI libraries, image processing, and blockchain clients. All dependencies are direct regardless of whether the consumer needs them.
- `@agent-runtime/core` — `"version": "0.1.0"`, minimal Express middleware package. Evaluate merge into connector or independent publish.
- No `"exports"` field in any package — missing modern Node.js resolution
- No `"files"` field — `npm pack` would include test files, config, Docker assets, etc.
- No publish automation or changeset configuration

**Technology Stack:**

- npm workspaces monorepo, TypeScript, Node.js ≥22.11.0

**Integration Points:**

- `packages/shared/package.json` — Version bump, exports field, files field, publishConfig
- `packages/connector/package.json` — Dependency trimming, exports, files, publishConfig, peer dependencies
- Root `package.json` — Publish scripts or changeset configuration
- `packages/connector/tsconfig.json` — Ensure declaration files generated
- `.github/workflows/` — Optional: automated publish workflow

### Enhancement Details

**What's Being Changed:**

1. **Dependency trimming for `@crosstown/connector`** — Move heavy/optional dependencies (blockchain SDKs, cloud KMS, AI libraries, image processing, observability) to `optionalDependencies` or `peerDependencies` with `peerDependenciesMeta: { optional: true }`. Core library consumers only pull in `ws`, `pino`, `zod`, `tslib`, and `@crosstown/shared`.
2. **Package.json configuration** — Add `"exports"` field for modern resolution, `"files"` field to allowlist only `dist/`, `README.md`, `LICENSE`. Add `"publishConfig": { "access": "public" }` for scoped packages. Version bump to `1.0.0` to reflect stable API.
3. **Publish automation** — Add root-level publish scripts (`publish:shared`, `publish:connector`, `publish:all`) with correct build-then-publish ordering. Optionally integrate changesets for versioning.
4. **Package validation** — `npm pack` both packages, install in a fresh project, verify imports work and types resolve.

**How It Integrates:**

- `@crosstown/core` adds `"@crosstown/connector": "^1.0.0"` and `"@crosstown/shared": "^1.0.0"` to its dependencies
- Consumers who need settlement features install the optional blockchain SDKs alongside the connector
- The connector package remains fully functional for both library and CLI use cases

**Success Criteria:**

1. `npm install @crosstown/connector` pulls only core dependencies (~5 packages, not 30+)
2. `npm install @crosstown/shared` pulls zero runtime dependencies
3. Both packages install and import successfully in a clean TypeScript project
4. All type declarations (`.d.ts`) resolve correctly
5. `npm pack` produces clean tarballs without test files, Docker configs, or source maps
6. Publish scripts work end-to-end (build → publish in correct order)

## Stories

### Story 26.1: Trim Connector Dependencies and Configure Peer Dependencies

**As a** package consumer,
**I want** `@crosstown/connector` to have minimal required dependencies,
**so that** I don't pull in blockchain SDKs, cloud libraries, and AI frameworks when I only need the ILP connector.

**Scope:**

- **Keep as direct dependencies** (required for core functionality):
  - `@crosstown/shared` — ILP types, OER codec
  - `ws` — BTP WebSocket protocol
  - `pino` — Logging
  - `tslib` — TypeScript runtime helpers
  - `zod` — Config validation
  - `js-yaml` — Config file loading (used by ConfigLoader for CLI path)
- **Move to `peerDependencies` with `peerDependenciesMeta: { optional: true }`:**
  - `tigerbeetle-node` — Only if TigerBeetle accounting enabled
  - `ethers` — Only if EVM settlement enabled
  - `xrpl` — Only if XRP settlement enabled
  - `@aptos-labs/ts-sdk` — Only if Aptos settlement enabled
  - `better-sqlite3` — Only if SQLite event store used
  - `express`, `cors` — Only if AdminServer HTTP enabled
- **Move to `optionalDependencies`:**
  - `nostr-tools` — Only if Nostr features used
  - `@aws-sdk/*`, `@azure/*`, `@google-cloud/*` — Only if cloud KMS used
  - `ai`, `@ai-sdk/*` — Only if AI features used
  - `sharp`, `qrcode` — Only if visual features used
  - `prom-client`, `@opentelemetry/*` — Only if observability enabled
- **Add dynamic imports:**
  - Where settlement, cloud KMS, or optional features are initialized, use dynamic `import()` instead of top-level imports
  - Add clear error messages when optional dependency is missing: `"ethers is required for EVM settlement. Install it with: npm install ethers"`
- **Update tests:**
  - Verify connector starts without optional dependencies installed
  - Verify clear error messages for missing optional deps

**Acceptance Criteria:**

1. `npm install @crosstown/connector` installs ≤ 10 direct dependencies
2. Connector starts successfully with only core dependencies (no settlement, no AdminServer)
3. Settlement features produce clear error message if blockchain SDK not installed
4. AdminServer produces clear error message if Express not installed
5. Dynamic imports used for all optional features — no top-level import failures
6. `peerDependenciesMeta` marks all peer dependencies as optional
7. All existing tests still pass (test environment has all deps installed)
8. New test verifies startup without optional dependencies

---

### Story 26.2: Configure Package.json for Both Packages

**As a** package publisher,
**I want** both packages configured with proper npm publishing metadata,
**so that** they publish correctly as scoped public packages with proper resolution.

**Scope:**

- **`@crosstown/shared` package.json:**
  - Version bump: `"version": "1.0.0"`
  - Add `"exports": { ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" } }`
  - Add `"files": ["dist", "README.md", "LICENSE"]`
  - Add `"publishConfig": { "access": "public" }`
  - Add `"repository"` field pointing to monorepo with `"directory": "packages/shared"`
  - Add `"license": "Apache-2.0"` (or project's actual license)
  - Add `"engines": { "node": ">=22.11.0" }`
  - Verify `"types": "dist/index.d.ts"` is set
- **`@crosstown/connector` package.json:**
  - Version bump: `"version": "1.0.0"`
  - Update `"main"` to point to library entry (from Epic 25)
  - Add `"exports"` field matching library entry
  - Add `"files": ["dist", "README.md", "LICENSE"]`
  - Add `"publishConfig": { "access": "public" }`
  - Add `"repository"` with `"directory": "packages/connector"`
  - Update `"@crosstown/shared"` dependency from `"*"` or `"workspace:*"` to `"^1.0.0"`
  - Add `"engines": { "node": ">=22.11.0" }`
- **Verify builds:**
  - `npm run build` in both packages produces all expected files
  - `dist/` includes `.js`, `.d.ts`, and `.d.ts.map` files
  - `tsconfig.json` has `"declaration": true` and `"declarationMap": true`

**Acceptance Criteria:**

1. Both packages have `"version": "1.0.0"` and `"publishConfig": { "access": "public" }`
2. Both packages have `"exports"` field for modern Node.js resolution
3. Both packages have `"files"` field limiting published content to `dist/`, `README.md`, `LICENSE`
4. `@crosstown/connector` depends on `@crosstown/shared` at `"^1.0.0"` (not workspace protocol)
5. `npm pack` in both packages produces tarballs with only `dist/`, `README.md`, `LICENSE`
6. No test files, Docker configs, source maps, or config files in tarballs
7. TypeScript declarations included and resolve correctly

---

### Story 26.3: Publish Automation and Package Validation

**As a** developer,
**I want** publish scripts and a validation workflow,
**so that** packages are published in the correct order with verified import resolution.

**Scope:**

- **Add root-level publish scripts:**
  ```json
  {
    "scripts": {
      "publish:shared": "npm run build --workspace=packages/shared && npm publish --workspace=packages/shared --access public",
      "publish:connector": "npm run build --workspace=packages/connector && npm publish --workspace=packages/connector --access public",
      "publish:all": "npm run publish:shared && npm run publish:connector"
    }
  }
  ```
- **Add package validation script:**
  - Create `scripts/validate-packages.sh` (or `.ts`):
    1. `npm pack` both packages into temp directory
    2. Create a fresh temp project with `npm init -y`
    3. Install both tarballs: `npm install ./agent-runtime-shared-1.0.0.tgz ./agent-runtime-connector-1.0.0.tgz`
    4. Create a TypeScript test file that imports key types and classes:
       ```typescript
       import { ConnectorNode, type ConnectorConfig } from '@crosstown/connector';
       import type { ILPPreparePacket } from '@crosstown/shared';
       // Verify types resolve
       const config: ConnectorConfig = { ... };
       ```
    5. Compile with `tsc --noEmit` — verify types resolve
    6. Run the file with `node` — verify runtime imports work
    7. Clean up temp directory
- **Add pre-publish check:**
  - `"prepublishOnly": "npm run build && npm run test"` in both package.json files
  - Prevents publishing unbuild or failing packages
- **Optional: Changeset integration:**
  - Install `@changesets/cli` as devDependency
  - Add `.changeset/config.json` for independent versioning
  - Add `changeset:version` and `changeset:publish` scripts

**Acceptance Criteria:**

1. `npm run publish:all` builds and publishes both packages in correct order (shared first)
2. `npm run validate-packages` (or equivalent) succeeds — fresh install, types resolve, runtime imports work
3. `prepublishOnly` prevents publishing without building and testing
4. Package validation covers: install, TypeScript compilation, runtime import
5. Both packages installable from tarball in a clean project
6. No circular dependency issues between packages
7. Validation script is documented and runnable by any developer

---

## Compatibility Requirements

- [x] **Workspace development** — `npm install` at monorepo root still links packages locally
- [x] **Existing CI/CD** — build and test scripts unchanged
- [x] **Docker builds** — Dockerfile has all dependencies installed (optional deps available)
- [x] **Standalone CLI** — `npx @crosstown/connector` still works after publishing

## Risk Mitigation

**Primary Risk:** Dynamic imports breaking features when optional dependencies are installed.

**Mitigation:**

- Dynamic import paths verified in tests with all dependencies installed
- Error messages guide users to install missing optional deps
- Settlement/AdminServer features explicitly opt-in via config — no silent failures

**Secondary Risk:** Workspace protocol (`workspace:*`) leaking into published package.json.

**Mitigation:**

- Update to semver range (`^1.0.0`) before publishing
- `validate-packages.sh` installs from tarball — catches workspace protocol issues
- `prepublishOnly` hook runs tests that would catch resolution failures

**Rollback Plan:**

1. `npm unpublish @crosstown/connector@1.0.0` (within 72h npm policy)
2. Revert package.json changes — packages stay unpublished
3. agent-society continues using monorepo-local dependency

## Definition of Done

- [ ] All 3 stories completed with acceptance criteria met
- [ ] `npm install @crosstown/connector` has minimal dependency footprint
- [ ] Both packages configured with proper npm metadata
- [ ] Publish automation works end-to-end
- [ ] Package validation proves install + import + types in clean project
- [ ] All existing tests pass
- [ ] No TypeScript compilation errors across the monorepo

## Related Work

- **Epic 24:** Connector Library API (prerequisite — provides the API surface to publish)
- **Epic 25:** CLI/Library Separation (prerequisite — provides clean library entry point)
- **Epic 22:** Agent-Runtime Middleware Simplification (companion — simplified middleware ships in published package)
- **Epic 23:** Unified Deployment Infrastructure (companion — deployment uses published packages)
- **agent-society integration:** Published packages enable `@crosstown/core` to depend on `@crosstown/connector`
