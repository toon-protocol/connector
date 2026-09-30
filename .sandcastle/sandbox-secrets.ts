// Forward host secrets INTO the sandcastle Docker sandbox.
//
// WHY THIS EXISTS — root cause of the relay#68 first-run failure
// -------------------------------------------------------------
// The `agent:implement` runner reached the sandbox, but claude-code inside it
// died with `Not logged in · Please run /login`, even though the workflow step
// exported CLAUDE_CODE_OAUTH_TOKEN (and GH_TOKEN) into the runner's env.
//
// The reason: @ai-hero/sandcastle@0.12.0's env resolver does NOT blanket-pass
// `process.env` into the container. It only forwards a variable whose KEY also
// appears in `.sandcastle/.env` (see `resolveEnv` in the engine's dist/index.js):
//
//     const sandcastleEnv = parseEnvFile(".sandcastle/.env");   // missing -> {}
//     for (const key of Object.keys(sandcastleEnv))             // keys from the FILE
//       result[key] = sandcastleEnv[key] || process.env[key];
//
// `.sandcastle/.env` is gitignored (only `.env.example` is committed), so in CI
// the file does not exist -> `parseEnvFile` returns {} -> the loop never runs ->
// the resolved env is {} -> NEITHER token is passed to `docker run`. The
// container therefore starts with no credentials and claude-code is unauthed.
//
// THE FIX
// -------
// Pass the secrets through the sandbox PROVIDER's first-class `env` option
// (`docker({ env })`), which the README documents as "Environment variables to
// inject into the sandbox" and merges via `mergeProviderEnv`. `createSandbox`
// bakes `sandboxProviderEnv` into the `docker run -e KEY=VALUE` flags at
// container start (`startContainer` in dist/sandboxes -> chunk-CP3TYXZA.js:
// `Object.entries(env).flatMap(([k, v]) => ["-e", `${k}=${v}`])`). Every
// in-container exec then inherits them: claude-code (CLAUDE_CODE_OAUTH_TOKEN)
// AND the implementer's in-sandbox `git push` / `gh pr create` (GH_TOKEN).
//
// NOTE: a key is included ONLY when it is actually set on the host. This keeps
// local dev working — there the tokens come from `.sandcastle/.env` (resolved
// separately) and we must not clobber those with an `undefined` override, since
// `mergeProviderEnv` layers sandbox-provider env OVER the resolved `.env` values.

// Host env vars that must reach claude-code and `gh`/`git` inside the sandbox.
const PASSTHROUGH_KEYS = [
  'CLAUDE_CODE_OAUTH_TOKEN', // Claude Max-plan credential -> authenticates claude-code
  'GH_TOKEN', // in-sandbox `git push` / `gh pr create` / `gh issue list`
] as const;

// DELIBERATELY ABSENT: E2E_DEV_MNEMONIC.
//
// It was added (label-gated) so a ticket could transact against Base Sepolia,
// then removed once Anvil/Foundry became the tooling for tests and checks — a
// disposable local chain needs no funded key, so no ticket needs a seed in its
// container. It is ONE BIP-39 phrase deriving keys on EVERY chain (NIP-06
// m/44'/1237'/0'/0/i), so it is not testnet-scoped, and this repository is
// public. Do not re-add it without a concrete ticket that cannot be served by a
// local chain, and re-read the redaction step in agent-implement.yml first.
//
// APP_PRIVATE_KEY is absent for the same class of reason: it stays on the host
// so the runner can mint a fresh push credential without the container ever
// holding the key (#462).

/**
 * The subset of {@link PASSTHROUGH_KEYS} that is set on the host, as a
 * `Record<string, string>` suitable for `docker({ env })`. Undefined vars are
 * omitted (never emitted as `KEY=undefined`) so the local `.sandcastle/.env`
 * path is not overridden with empties.
 */
export function sandboxSecrets(): Record<string, string> {
  const env: Record<string, string> = {};
  for (const key of PASSTHROUGH_KEYS) {
    const value = process.env[key];
    if (value) env[key] = value;
  }
  return env;
}
