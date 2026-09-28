// The gate, run DETERMINISTICALLY by the runner — not asked of the agent.
//
// WHY THIS EXISTS
// ---------------
// `implement-prompt.md` ends with "Do not commit until lint, typecheck, build,
// and test all pass." That is advisory prose the agent self-reports on, and it
// listed only the npm gate — not one `cargo` command — even though the #409
// rewrite is Rust. The result is visible in the epic's own history: #441, #444,
// #446, #449 and #454 are all remediation tickets for agent PRs that did not
// compile or conflicted, each discovered only once CI ran.
//
// This is the same failure mode toon-meta#235 fixed for `git push`: the agent
// reported COMPLETE without having pushed, and the cure was to stop asking and
// run it from the runner. Verifying a build is plumbing, so the runner does it.
//
// PATH-AWARE, mirroring ci.yml's own filtering: a TypeScript-only ticket does
// not pay for a Rust build, and vice versa. Commands are kept byte-identical to
// the gate jobs in ci.yml — a gate that runs something *similar* to CI teaches
// the agent the wrong lesson.

import type * as sandcastle from '@ai-hero/sandcastle';

type Sandbox = Awaited<ReturnType<typeof sandcastle.createSandbox>>;

export interface GateStep {
  readonly name: string;
  readonly command: string;
}

export interface GateFailure {
  readonly step: string;
  readonly command: string;
  readonly exitCode: number;
  /** Tail of combined output — enough for an agent to act on, bounded so it cannot blow a prompt. */
  readonly output: string;
}

export interface GateResult {
  readonly passed: boolean;
  readonly ran: readonly string[];
  readonly failure: GateFailure | null;
}

/** Keep fed-back output useful but bounded — a full cargo build log is megabytes. */
const MAX_OUTPUT_CHARS = 12_000;

/**
 * Rust gate — the exact steps of ci.yml's `Rust Workspace Gate`, in the same
 * order, so passing here means the same thing as passing there.
 *
 * No `--exclude` (issue #1386 removed `packages/solana-program`, the
 * `payment-channel` crate it excluded). Foundry (anvil) is in the agent
 * image, so the chain tests genuinely run rather than skipping (#471).
 */
const RUST_STEPS: readonly GateStep[] = [
  { name: 'cargo fmt', command: 'cargo fmt --all -- --check' },
  { name: 'cargo build', command: 'cargo build --workspace' },
  { name: 'cargo test', command: 'cargo test --workspace' },
  {
    name: 'cargo clippy',
    command: 'cargo clippy --workspace --all-targets -- -D warnings',
  },
];

/**
 * npm gate — what implement-prompt.md already described, now enforced. Since
 * ADR 0017 the npm surface is devnet tooling only (faucet, announcer,
 * tools/fund-peers); ADR 0065 removed the last workspace that needed a
 * hand-ordered build, so `npm run build` is now just `--workspaces`.
 */
const NPM_STEPS: readonly GateStep[] = [
  { name: 'npm lint', command: 'npm run lint --workspaces --if-present' },
  { name: 'npm typecheck', command: 'npm run typecheck' },
  { name: 'npm build', command: 'npm run build' },
  { name: 'npm test', command: 'npm run test --workspaces --if-present' },
];

/**
 * Which gates apply, from what the branch actually changed against the base.
 *
 * Deliberately errs toward running MORE: if the diff cannot be read for any
 * reason we run both, because skipping a gate silently is the failure this
 * module exists to prevent.
 */
export async function selectSteps(
  sandbox: Sandbox,
  baseBranch: string
): Promise<readonly GateStep[]> {
  const diff = await sandbox.exec(`git diff --name-only ${baseBranch}...HEAD`);
  if (diff.exitCode !== 0) {
    console.log('  [gate] could not read the changed-file list — running BOTH gates.');
    return [...RUST_STEPS, ...NPM_STEPS];
  }

  const files = diff.stdout
    .split('\n')
    .map((f) => f.trim())
    .filter(Boolean);
  if (files.length === 0) {
    console.log('  [gate] no files changed against the base — running BOTH gates.');
    return [...RUST_STEPS, ...NPM_STEPS];
  }

  const touchesRust = files.some(
    (f) => f.startsWith('crates/') || f.startsWith('Cargo.') || f.endsWith('.rs')
  );
  const touchesNpm = files.some(
    (f) => f.startsWith('packages/') || f === 'package.json' || f === 'package-lock.json'
  );

  const steps = [...(touchesRust ? RUST_STEPS : []), ...(touchesNpm ? NPM_STEPS : [])];

  console.log(
    `  [gate] ${files.length} changed file(s) — rust: ${touchesRust ? 'yes' : 'no'}, ` +
      `npm: ${touchesNpm ? 'yes' : 'no'}`
  );

  // Neither matched (docs, workflows, .sandcastle) — nothing to compile. Say so
  // explicitly rather than reporting a silent pass.
  if (steps.length === 0) {
    console.log('  [gate] no Rust or npm sources changed — no build gate applies.');
  }
  return steps;
}

/**
 * Run `steps` in order, stopping at the first failure.
 *
 * Failure is returned, not thrown, so the caller can decide between a fix
 * iteration and failing the job.
 */
export async function runGate(sandbox: Sandbox, steps: readonly GateStep[]): Promise<GateResult> {
  const ran: string[] = [];

  for (const step of steps) {
    console.log(`  [gate] ${step.name}: ${step.command}`);
    const lines: string[] = [];
    const result = await sandbox.exec(step.command, {
      onLine: (line) => {
        lines.push(line);
        // Stream sparingly: full build output would bury the runner log.
        if (lines.length <= 40) console.log(`    | ${line}`);
      },
    });
    ran.push(step.name);

    if (result.exitCode !== 0) {
      const combined = [result.stdout, result.stderr].filter(Boolean).join('\n');
      const output =
        combined.length > MAX_OUTPUT_CHARS
          ? `...(truncated to the last ${MAX_OUTPUT_CHARS} chars)...\n` +
            combined.slice(-MAX_OUTPUT_CHARS)
          : combined;

      console.log(`  [gate] FAILED at ${step.name} (exit ${result.exitCode}).`);
      return {
        passed: false,
        ran,
        failure: { step: step.name, command: step.command, exitCode: result.exitCode, output },
      };
    }
  }

  console.log(`  [gate] PASSED (${ran.length} step(s): ${ran.join(', ') || 'none applicable'}).`);
  return { passed: true, ran, failure: null };
}

/** The prompt handed to a fix iteration. Concrete failure, no room to reinterpret the task. */
export function fixPrompt(failure: GateFailure, attempt: number, maxAttempts: number): string {
  return [
    `The repository gate is RED. This is fix attempt ${attempt} of ${maxAttempts}.`,
    '',
    `Failing step: ${failure.step}`,
    `Command:      ${failure.command}`,
    `Exit code:    ${failure.exitCode}`,
    '',
    'Output:',
    '```',
    failure.output,
    '```',
    '',
    'Fix the cause and commit. Rules:',
    `- Re-run \`${failure.command}\` yourself and confirm it passes before you finish.`,
    '- Fix the code. Do NOT weaken, skip, delete or #[ignore] a test, and do not',
    '  loosen a lint to make this pass — if the test is genuinely wrong, say so',
    '  explicitly in the commit message and explain why.',
    '- Change only what this failure requires. Do not refactor beyond it.',
    '- If you cannot fix it, commit nothing and explain what is blocking you.',
  ].join('\n');
}
