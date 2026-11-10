// Exercises push-lease.ts against a scratch bare repo, never `origin` (#1677).
import assert from 'node:assert/strict';
import { execSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { pushLeased, pushRescue, remoteTip, rescueRef, type Exec } from './push-lease.ts';

const branch = 'sandcastle/issue-1';

function sh(cwd: string, cmd: string): string {
  return execSync(cmd, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function setup() {
  const root = mkdtempSync(join(tmpdir(), 'push-lease-'));
  const remote = join(root, 'remote.git');
  const work = join(root, 'work');
  sh(root, `git init -q --bare -b main ${remote}`);
  sh(root, `git clone -q ${remote} ${work} 2>/dev/null`);
  const id = `-c user.name=t -c user.email=t@t`;
  sh(work, `git ${id} commit -q --allow-empty -m base && git push -q origin HEAD:main`);
  sh(work, `git checkout -q -b ${branch}`);
  const exec: Exec = async (command) => {
    try {
      return { exitCode: 0, stdout: sh(work, command), stderr: '' };
    } catch (e) {
      const err = e as { status: number; stdout: string; stderr: string };
      return { exitCode: err.status ?? 1, stdout: err.stdout ?? '', stderr: err.stderr ?? '' };
    }
  };
  const commit = (msg: string) => {
    writeFileSync(join(work, msg), msg);
    sh(work, `git add -A && git ${id} commit -q -m ${msg}`);
  };
  return { root, remote, work, exec, commit, id };
}

test('a new branch pushes with an empty lease, then the lease advances', async () => {
  const { exec, commit } = setup();
  assert.equal(await remoteTip(exec, 'git', branch), null);
  commit('a');
  const first = await pushLeased(exec, 'git', branch, null);
  assert.ok(first.ok);
  commit('b');
  const second = await pushLeased(exec, 'git', branch, first.ok ? first.sha : null);
  assert.ok(second.ok);
  assert.equal(await remoteTip(exec, 'git', branch), second.ok ? second.sha : 'x');
});

test('a rebase onto main still pushes at every push point', async () => {
  const { exec, commit, work, id } = setup();
  commit('a');
  let r = await pushLeased(exec, 'git', branch, null);
  assert.ok(r.ok);
  // main moves on; the session rebases the branch onto it.
  sh(
    work,
    `git checkout -q main && git ${id} commit -q --allow-empty -m m2 && git push -q origin main`
  );
  sh(work, `git checkout -q ${branch} && git ${id} rebase -q main`);
  commit('b');
  r = await pushLeased(exec, 'git', branch, r.ok ? r.sha : null);
  assert.ok(r.ok);
  sh(work, `git ${id} commit -q --allow-empty -m c`);
  r = await pushLeased(exec, 'git', branch, r.ok ? r.sha : null);
  assert.ok(r.ok);
});

test('a push by someone else mid-run is a lease failure and is not overwritten', async () => {
  const { exec, commit, remote, root, id } = setup();
  commit('a');
  const first = await pushLeased(exec, 'git', branch, null);
  assert.ok(first.ok);
  const other = join(root, 'other');
  sh(
    root,
    `git clone -q ${remote} ${other} 2>/dev/null && cd ${other} && git checkout -q ${branch}`
  );
  sh(other, `git ${id} commit -q --allow-empty -m theirs && git push -q origin ${branch}`);
  const theirs = sh(other, 'git rev-parse HEAD');
  commit('b');
  const second = await pushLeased(exec, 'git', branch, first.ok ? first.sha : null);
  assert.ok(!second.ok && second.leaseRejected, JSON.stringify(second));
  assert.equal(sh(remote, `git rev-parse ${branch}`), theirs);
});

test('a rescue push leaves HEAD on the rescue ref', async () => {
  const { exec, commit, remote, work } = setup();
  commit('a');
  assert.ok(await pushRescue(exec, 'git', branch));
  assert.equal(sh(remote, `git rev-parse ${rescueRef(branch)}`), sh(work, 'git rev-parse HEAD'));
});
