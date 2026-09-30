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
import * as sandcastle from '@ai-hero/sandcastle';
import { docker } from '@ai-hero/sandcastle/sandboxes/docker';
import { sandboxSecrets } from './sandbox-secrets.ts';
import { mintAppToken } from './mint-app-token.ts';
import { fixPrompt, runGate, selectSteps } from './run-gate.ts';

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

/**
 * Push the branch from inside the sandbox with a newly minted App token, and
 * refresh the host's GH_TOKEN from the same mint for the `gh` calls that follow.
 * `bestEffort` pushes only warn on failure; the final push throws.
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

  let token: string;
  try {
    const minted = await mintAppToken();
    token = minted.token;
    process.env.GH_TOKEN = token;
    console.log(`  [${label}] credential: freshly minted (source=${minted.source})`);
  } catch (err) {
    return fail(`[${label}] could not obtain a push credential: ${(err as Error).message}`);
  }

  const stage = await sandbox.exec(`umask 077 && cat > ${TOKEN_PATH}`, { stdin: token });
  if (stage.exitCode !== 0) {
    return fail(`[${label}] failed to stage the push credential (exit ${stage.exitCode}).`);
  }

  try {
    const push = await sandbox.exec(
      `git -c credential.helper= -c credential.helper='${FRESH_CREDENTIAL_HELPER}' ` +
        `push -u origin ${branch}`,
      { onLine: (line) => console.log(`  [${label}] ${line}`) }
    );
    if (push.exitCode !== 0) {
      return fail(
        `[${label}] git push of '${branch}' failed (exit ${push.exitCode}).\n${push.stderr}`
      );
    }
    return true;
  } finally {
    // Leave no usable credential in the container for the agent sessions that follow.
    await sandbox.exec(`rm -f ${TOKEN_PATH}`);
  }
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
    // 1. Implement: /implement in a fresh session. It may take several sessions
    //    on a large ticket, each continuing from the previous one's commits.
    const implement = await sandbox.run({
      name: 'implement',
      maxIterations: 10,
      agent: sandcastle.claudeCode('claude-sonnet-5-5'),
      promptFile: './.sandcastle/implement-prompt.md',
      promptArgs: { ISSUE_URL: issue.url, ISSUE_NUMBER: issueNumber, BRANCH: branch },
    });

    if (implement.commits.length === 0) {
      throw new Error(
        'The implement session made no commits. If it hit a blocker, it explained why ' +
          'in a comment on the issue.'
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
