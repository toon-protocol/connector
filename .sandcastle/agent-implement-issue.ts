// The AFK runner for one ready-for-agent issue: implement → review → gate →
// push → PR. It is what you would do locally with Matt Pocock's skills, run from
// GitHub instead:
//
//   /implement <issue>        in a fresh session, which commits to the branch
//   /code-review <base>       in a second fresh session, which fixes what it finds
//   the CI gate               run by this runner, not self-reported by an agent
//   git push + gh pr create   run by this runner, never asked of an agent
//
// .github/workflows/agent-implement.yml picks which issues to run
// (ready-issues.ts) and calls this once per issue.
//
// Labels are only the canonical triage set (docs/agents/triage-labels.md):
//   success → the PR gets `ready-for-human` and the issue loses `ready-for-agent`
//   failure → the workflow moves the issue to `needs-triage` with a link to the run
// Nothing here merges a PR or closes an issue. The issue closes when a human
// merges the PR, because the PR body says `Closes #N`.
//
// Required env:
//   SANDCASTLE_ISSUE_NUMBER   the issue to work
//   CLAUDE_CODE_OAUTH_TOKEN   authenticates Claude Code in the sandbox
//   GH_TOKEN                  App installation token (contents, PRs and issues: write)
//   APP_ID, APP_PRIVATE_KEY   optional. Used to mint a fresh token before each push,
//                             because installation tokens expire after an hour (#462).
//                             Host only: they are never forwarded into the sandbox.
//
// Usage:
//   SANDCASTLE_ISSUE_NUMBER=123 npm run sandcastle:implement

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import * as sandcastle from '@ai-hero/sandcastle';
import { docker } from '@ai-hero/sandcastle/sandboxes/docker';
import { sandboxSecrets } from './sandbox-secrets.ts';
import { mintAppToken } from './mint-app-token.ts';
import { fixPrompt, runGate, selectSteps } from './run-gate.ts';
import { pushLeased, pushRescue, remoteTip, rescueRef, type PushResult } from './push-lease.ts';

const issueNumber: string = (() => {
  const value = process.env.SANDCASTLE_ISSUE_NUMBER?.trim();
  if (!value || !/^\d+$/.test(value)) {
    throw new Error(
      'SANDCASTLE_ISSUE_NUMBER must be set to a numeric issue number ' +
        `(got: ${JSON.stringify(process.env.SANDCASTLE_ISSUE_NUMBER)}).`
    );
  }
  return value;
})();

const BASE = 'main';

// Deterministic, so a re-run of the same issue reuses the branch and its commits.
const branch = `sandcastle/issue-${issueNumber}`;

// How many times a red gate is handed back for a fix before the run fails. The
// failures it catches are almost always small compile or lint errors.
const MAX_GATE_FIX_ATTEMPTS = 2;

// How many extra implement sessions a run gets when one ends with a dirty tree or
// nothing committed. Bounded so a session that cannot commit does not loop forever.
const MAX_IMPLEMENT_FOLLOW_UPS = 2;

const COMPLETE = '<promise>COMPLETE</promise>';
const BLOCKED = '<promise>BLOCKED</promise>';

const issue = JSON.parse(
  execFileSync('gh', ['issue', 'view', issueNumber, '--json', 'title,url,labels'], {
    encoding: 'utf8',
  })
) as { title: string; url: string; labels: { name: string }[] };

const hooks = {
  sandbox: {
    onSandboxReady: [
      // The engine sets git identity but no credential helper, so a bare `git push`
      // would be unauthenticated. `gh auth setup-git` makes git read GH_TOKEN at
      // push time and stores no token on disk. Guarded so local runs without a
      // token still start.
      {
        command:
          'if [ -n "$GH_TOKEN" ]; then gh auth setup-git; ' +
          "git config --unset-all 'http.https://github.com/.extraheader' 2>/dev/null || true; fi",
      },
      // Installs the devnet tooling's npm workspaces, which the npm gate needs.
      { command: 'npm ci' },
    ],
  },
};

type Sandbox = Awaited<ReturnType<typeof sandcastle.createSandbox>>;

/** Where the fresh token is staged inside the container. Mode 600, deleted after use. */
const TOKEN_PATH = '/tmp/.sandcastle-push-token';

// Git credential helper that reads the token from TOKEN_PATH. The empty
// `credential.helper=` before it on the command line resets git's helper list, so
// the helper `gh auth setup-git` installed (holding the token from container start)
// is not consulted first. The token reaches the file over stdin, so it never
// appears in argv or in a log line.
const FRESH_CREDENTIAL_HELPER =
  `!f() { test "$1" = get && ` +
  `{ echo username=x-access-token; echo "password=$(cat ${TOKEN_PATH})"; }; }; f`;

// The remote tip this run last saw for `branch` (null: absent), set at sandbox creation
// and advanced on every successful push. Every push leases against it (#1677).
let expectedTip: string | null = null;

/** Mint a token, stage it, run `fn` with the credential-helper git prefix, then unstage. */
async function withCredential<T>(
  sandbox: Sandbox,
  label: string,
  fn: (git: string) => Promise<T>
): Promise<T> {
  const minted = await mintAppToken();
  const token = minted.token;
  process.env.GH_TOKEN = token;
  console.log(`  [${label}] credential: freshly minted (source=${minted.source})`);

  const stage = await sandbox.exec(`umask 077 && cat > ${TOKEN_PATH}`, { stdin: token });
  if (stage.exitCode !== 0) {
    throw new Error(`failed to stage the push credential (exit ${stage.exitCode}).`);
  }
  try {
    return await fn(`git -c credential.helper= -c credential.helper='${FRESH_CREDENTIAL_HELPER}'`);
  } finally {
    // Leave no usable credential in the container for the agent sessions that follow.
    await sandbox.exec(`rm -f ${TOKEN_PATH}`);
  }
}

/**
 * Push the branch from inside the sandbox with a newly minted App token, leased on
 * the last tip this run saw, and refresh the host's GH_TOKEN from the same mint for
 * the `gh` calls that follow. `bestEffort` pushes only warn on failure; the final
 * push throws, after trying to leave HEAD on a rescue ref.
 */
async function pushBranch(
  sandbox: Sandbox,
  label: string,
  { bestEffort = false }: { bestEffort?: boolean } = {}
): Promise<boolean> {
  const fail = (msg: string): false => {
    if (!bestEffort) throw new Error(msg);
    console.warn(`  WARNING: ${msg}`);
    return false;
  };

  let result: PushResult;
  try {
    result = await withCredential(sandbox, label, (git) =>
      pushLeased(sandbox.exec.bind(sandbox), git, branch, expectedTip, (line) =>
        console.log(`  [${label}] ${line}`)
      )
    );
  } catch (err) {
    return fail(`[${label}] could not push: ${(err as Error).message}`);
  }
  if (result.ok) {
    expectedTip = result.sha;
    return true;
  }
  if (bestEffort) return fail(`[${label}] ${result.message}`);

  // The final push: do not let the commits die with the sandbox.
  const rescue = rescueRef(branch);
  let rescued = false;
  try {
    rescued = await withCredential(sandbox, `${label}:rescue`, (git) =>
      pushRescue(sandbox.exec.bind(sandbox), git, branch)
    );
  } catch (err) {
    console.warn(`  WARNING: rescue push failed: ${(err as Error).message}`);
  }
  return fail(
    `[${label}] ${result.message}\n` +
      (rescued
        ? `The run's HEAD was pushed to '${rescue}' instead; recover the work from there.`
        : `The rescue push to '${rescue}' failed too, so the commits are lost with the sandbox.`)
  );
}

/** The implement prompt, plus a paragraph telling a follow-up session what it found. */
function followUpPrompt(args: Record<string, string>, dirty: boolean): string {
  // An inline prompt takes no promptArgs, so substitute them as sandcastle's promptFile
  // does: same placeholder pattern, and a placeholder with no value is an error.
  const base = readFileSync('./.sandcastle/implement-prompt.md', 'utf8').replace(
    /\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}/g,
    (_match, key: string) => {
      const value = args[key];
      if (value === undefined) throw new Error(`Prompt argument {{${key}}} has no value.`);
      return value;
    }
  );
  const found = dirty
    ? 'A previous session left uncommitted work in the working tree. Run `git status` and ' +
      '`git diff` to see it, check it is correct and finish it, run the gate, and commit it.'
    : `A previous session ended with no commits on '${args.BRANCH}' ahead of ${BASE}. ` +
      'Check what it left, finish the ticket and commit it.';
  return `${base}\n\n## Follow-up session\n\n${found} Only a session commits: the runner will not.\n`;
}

/** Last `<review-summary>` block in a session's output, or null if it wrote none. */
function reviewSummary(stdout: string): string | null {
  const blocks = [...stdout.matchAll(/<review-summary>([\s\S]*?)<\/review-summary>/g)];
  return blocks.length > 0 ? blocks[blocks.length - 1]![1]!.trim() : null;
}

function openPrFor(head: string): { number: number; url: string } | undefined {
  const prs = JSON.parse(
    execFileSync('gh', ['pr', 'list', '--head', head, '--state', 'open', '--json', 'number,url'], {
      encoding: 'utf8',
    })
  ) as Array<{ number: number; url: string }>;
  return prs[0];
}

async function main() {
  console.log(`\n=== implement #${issueNumber} "${issue.title}" on ${branch} ===\n`);

  // A sweep can queue this issue behind a run that has since opened its PR, or a
  // human can take it out of the queue while it waits. Check again before spending
  // a sandbox on it.
  if (!issue.labels.some((l) => l.name === 'ready-for-agent') || openPrFor(branch)) {
    console.log(
      'No longer queued (no ready-for-agent label, or a PR is already open). Nothing to do.'
    );
    return;
  }

  const sandbox = await sandcastle.createSandbox({
    branch,
    // Forwards CLAUDE_CODE_OAUTH_TOKEN and GH_TOKEN into the container (see
    // ./sandbox-secrets.ts for why the engine would not otherwise).
    sandbox: docker({ env: sandboxSecrets() }),
    hooks,
  });

  let pr: { number: number; url: string } | undefined;
  try {
    // Record where the remote branch stands, so every push can lease against it.
    expectedTip = await withCredential(sandbox, 'lease:init', (git) =>
      remoteTip(sandbox.exec.bind(sandbox), git, branch)
    );
    console.log(`  remote tip of '${branch}': ${expectedTip ?? '(absent)'}`);

    // 1. Implement: /implement in a fresh session. It may take several sessions
    //    on a large ticket, each continuing from the previous one's commits.
    const implementArgs = { ISSUE_URL: issue.url, ISSUE_NUMBER: issueNumber, BRANCH: branch };
    let implement = await sandbox.run({
      name: 'implement',
      completionSignal: [COMPLETE, BLOCKED],
      maxIterations: 10,
      agent: sandcastle.claudeCode('claude-sonnet-5-5'),
      promptFile: './.sandcastle/implement-prompt.md',
      promptArgs: implementArgs,
    });

    // A session that stops blocked has explained why on the issue. Ending here keeps
    // the runner from starting nine more sessions that hit the same blocker and post
    // the same comment (slop_machine#37 did exactly that). It also skips the
    // follow-ups below, whatever state the tree is in.
    const throwIfBlocked = () => {
      if (implement.completionSignal === BLOCKED) {
        throw new Error(
          'The implement session stopped blocked. It explained why in a comment on the issue.'
        );
      }
    };
    throwIfBlocked();

    // What matters is whether the branch has work on it, not whether this session
    // added any. A rerun of an issue whose earlier run committed the implementation
    // and then failed later (#1429: the push) finds nothing left to do, and should go
    // on to review, gate and PR rather than fail.
    //
    // sandcastle treats a signal as received once the string appears anywhere in the
    // output, so a session that merely quotes it, or that ends its turn with work
    // uncommitted, looks finished. Check the tree too, and hand it back to a session
    // (never committed by the runner: only a session that has run the tests may
    // commit under the agent's name).
    const inspectBranch = async () => {
      const ahead = await sandbox.exec(`git rev-list --count ${BASE}..HEAD`);
      const status = await sandbox.exec('git status --porcelain');
      return {
        commitsOnBranch: ahead.exitCode === 0 ? Number(ahead.stdout.trim()) : NaN,
        dirty: status.exitCode !== 0 || status.stdout.trim() !== '',
      };
    };
    let { commitsOnBranch, dirty } = await inspectBranch();
    for (
      let attempt = 1;
      (dirty || !(commitsOnBranch > 0)) && attempt <= MAX_IMPLEMENT_FOLLOW_UPS;
      attempt++
    ) {
      console.log(
        `\nImplement left ${dirty ? 'uncommitted changes' : `no commits ahead of ${BASE}`} — ` +
          `follow-up session ${attempt}/${MAX_IMPLEMENT_FOLLOW_UPS}.`
      );
      implement = await sandbox.run({
        name: `implement-follow-up-${attempt}`,
        completionSignal: [COMPLETE, BLOCKED],
        // One session per follow-up, so the cap above bounds sessions, not batches of them.
        maxIterations: 1,
        agent: sandcastle.claudeCode('claude-sonnet-5-5'),
        prompt: followUpPrompt(implementArgs, dirty),
      });
      throwIfBlocked();
      ({ commitsOnBranch, dirty } = await inspectBranch());
    }
    if (dirty) {
      throw new Error(
        `The working tree is still dirty after ${MAX_IMPLEMENT_FOLLOW_UPS} follow-up ` +
          `session(s): uncommitted work was left in the sandbox, so '${branch}' was not pushed.`
      );
    }
    if (!(commitsOnBranch > 0)) {
      throw new Error(
        `'${branch}' has no commits ahead of ${BASE}. If the implement session hit a ` +
          'blocker, it explained why in a comment on the issue.'
      );
    }
    if (implement.commits.length === 0) {
      console.log(
        `\nThe last implement session added nothing; '${branch}' already has ${commitsOnBranch} ` +
          `commit(s) ahead of ${BASE}. Continuing to review.`
      );
    }

    // Publish right away, so a run that dies later still leaves its work on the remote.
    await pushBranch(sandbox, 'push:implement', { bestEffort: true });

    // 2. Review: /code-review in a second fresh session, so the reviewer is not
    //    the session that wrote the code. It commits its in-scope fixes.
    const review = await sandbox.run({
      name: 'review',
      maxIterations: 1,
      agent: sandcastle.claudeCode('claude-opus-5-5'),
      promptFile: './.sandcastle/review-prompt.md',
      // Not {{TARGET_BRANCH}}: inside createSandbox() sandcastle sets that built-in to the
      // sandbox's own branch, so the review would diff the branch against itself.
      promptArgs: {
        ISSUE_URL: issue.url,
        ISSUE_NUMBER: issueNumber,
        BRANCH: branch,
        BASE_BRANCH: BASE,
      },
    });
    const summary = reviewSummary(review.stdout);

    // 3. Gate: CI's own commands, run by this runner. A red gate gets a bounded
    //    number of fix sessions with the exact failure; after that the run fails
    //    rather than open a red PR.
    const gateSteps = await selectSteps(sandbox, BASE);
    let gate = await runGate(sandbox, gateSteps);
    for (let attempt = 1; !gate.passed && attempt <= MAX_GATE_FIX_ATTEMPTS; attempt++) {
      console.log(`\nGate is red — fix attempt ${attempt}/${MAX_GATE_FIX_ATTEMPTS}.`);
      await sandbox.run({
        name: `gate-fix-${attempt}`,
        maxIterations: 20,
        agent: sandcastle.claudeCode('claude-sonnet-5-5'),
        prompt: fixPrompt(gate.failure!, attempt, MAX_GATE_FIX_ATTEMPTS),
      });
      await pushBranch(sandbox, `push:gate-fix-${attempt}`, { bestEffort: true });
      gate = await runGate(sandbox, gateSteps);
    }
    if (!gate.passed) {
      await pushBranch(sandbox, 'push:red', { bestEffort: true });
      throw new Error(
        `Gate still red after ${MAX_GATE_FIX_ATTEMPTS} fix attempt(s), so no PR was opened. ` +
          `The work is on '${branch}'.\n` +
          `  ${gate.failure!.step}: ${gate.failure!.command} (exit ${gate.failure!.exitCode})\n\n` +
          gate.failure!.output
      );
    }

    // 4. Push and open the PR. This is plumbing, so no agent does it.
    await pushBranch(sandbox, 'push:final');

    pr = openPrFor(branch);
    if (!pr) {
      const body = [
        `Closes #${issueNumber}`,
        '',
        '## Review',
        '',
        summary ?? '_The review session wrote no summary. Read its log in the run artifacts._',
        '',
        `Gate: ${gate.ran.length > 0 ? gate.ran.join(', ') : 'no Rust or npm sources changed'} — passed.`,
        '',
        '🤖 Generated with [Claude Code](https://claude.com/claude-code)',
      ].join('\n');
      execFileSync(
        'gh',
        ['pr', 'create', '--base', BASE, '--head', branch, '--title', issue.title, '--body', body],
        { stdio: 'inherit' }
      );
      pr = openPrFor(branch);
    }
  } finally {
    await sandbox.close();
  }

  // `gh pr create` can exit 0 without a PR landing, so check from the host.
  if (!pr) throw new Error(`No open PR exists for '${branch}' after pushing it.`);

  // Hand over: the PR is ready for a human to merge, and the issue has left the queue.
  execFileSync('gh', ['pr', 'edit', String(pr.number), '--add-label', 'ready-for-human']);
  execFileSync('gh', ['issue', 'edit', issueNumber, '--remove-label', 'ready-for-agent']);
  console.log(`\nPR #${pr.number} is open and ready for a human: ${pr.url}`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
