// Lease-guarded pushes of the runner's own `sandcastle/issue-<n>` branch (#1677).
//
// A session may rewrite its branch (a rebase onto main), which a plain fast-forward
// push refuses. The runner owns the branch, so it force-pushes it, but only with a
// lease on the tip it last saw: a push by anyone else mid-run is never overwritten.
// Kept free of sandcastle so a scratch bare repo can exercise it (push-lease.test.ts).

export type Exec = (
  command: string
) => Promise<{ exitCode: number; stdout: string; stderr: string }>;

export type PushResult =
  | { ok: true; sha: string }
  | { ok: false; leaseRejected: boolean; message: string };

/** `--force-with-lease` argument. A null tip means "the ref must not exist". */
export function leaseArg(branch: string, expected: string | null): string {
  return `--force-with-lease=${branch}:${expected ?? ''}`;
}

/** The name of the ref a failed final push is rescued to. */
export function rescueRef(branch: string): string {
  return `${branch}-rescue`;
}

/** The remote tip of `branch`, or null when it is absent. `git` is the command prefix. */
export async function remoteTip(exec: Exec, git: string, branch: string): Promise<string | null> {
  const r = await exec(`${git} ls-remote origin refs/heads/${branch}`);
  if (r.exitCode !== 0) {
    throw new Error(`git ls-remote of '${branch}' failed (exit ${r.exitCode}).\n${r.stderr}`);
  }
  const sha = r.stdout.trim().split(/\s+/)[0];
  return sha ? sha : null;
}

/**
 * Push `branch` with a lease on `expected`. On success the caller's expected tip
 * becomes the returned `sha`.
 */
export async function pushLeased(
  exec: Exec,
  git: string,
  branch: string,
  expected: string | null,
  onLine?: (line: string) => void
): Promise<PushResult> {
  const head = await exec(`git rev-parse refs/heads/${branch}`);
  if (head.exitCode !== 0) {
    return {
      ok: false,
      leaseRejected: false,
      message: `cannot resolve '${branch}': ${head.stderr}`,
    };
  }
  const sha = head.stdout.trim();
  // --no-verify: this repo's husky pre-push hook runs a local test pass meant for
  // people pushing by hand. The runner has just run CI's own gate, and the hook
  // failing in the sandbox is what cost #1429 its final push.
  const push = await exec(
    `${git} push --no-verify -u ${leaseArg(branch, expected)} origin ${branch}`
  );
  for (const line of `${push.stdout}\n${push.stderr}`.split('\n')) if (line && onLine) onLine(line);
  if (push.exitCode === 0) return { ok: true, sha };
  const leaseRejected = /stale info|\(fetch first\)|remote ref .* changed/i.test(push.stderr);
  return {
    ok: false,
    leaseRejected,
    message:
      `git push of '${branch}' failed (exit ${push.exitCode})` +
      `${leaseRejected ? ': lease failure, the remote branch moved during the run' : ''}.\n${push.stderr}`,
  };
}

/** Force-push HEAD to the runner's own rescue ref. Returns true on success. */
export async function pushRescue(exec: Exec, git: string, branch: string): Promise<boolean> {
  const r = await exec(
    `${git} push --no-verify --force origin HEAD:refs/heads/${rescueRef(branch)}`
  );
  return r.exitCode === 0;
}
