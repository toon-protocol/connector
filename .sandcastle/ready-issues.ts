// Which `ready-for-agent` issues can an agent start on right now?
//
// This is the frontier `to-tickets` describes: tickets whose blockers are all
// closed. `to-tickets` opens every ticket with `ready-for-agent` at once, blockers
// included, so the label alone is not permission to start. An issue is picked up
// only when ALL of these hold:
//
//   - it is open and carries `ready-for-agent`;
//   - it is not a spec. `to-spec` also applies `ready-for-agent`, and building a
//     whole spec in one run is the rough edge upstream documents. A spec is an
//     issue with sub-issues, or one written from the to-spec template (it has a
//     `## User Stories` section);
//   - nothing blocks it: no open issue in GitHub's native "blocked by"
//     relationship, and no open issue listed under the `## Blocked by` heading
//     that to-tickets writes when the native relationship is not used;
//   - no open PR already exists for its branch (`sandcastle/issue-N`).
//
// A blocked issue is skipped quietly and keeps its label. The workflow sweeps
// again whenever an issue closes, which is the moment a blocker can clear.
//
// Usage (host side, `gh` authenticated via GH_TOKEN):
//   npx tsx .sandcastle/ready-issues.ts          # sweep every ready-for-agent issue
//   npx tsx .sandcastle/ready-issues.ts 123      # consider only #123
// Prints a JSON array of issue numbers, and writes it as `issues=` to
// $GITHUB_OUTPUT when that is set.

import { execFileSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';

const READY_LABEL = 'ready-for-agent';

interface IssueFacts {
  number: number;
  state: string;
  body: string;
  labels: { nodes: { name: string }[] };
  subIssues: { totalCount: number };
  blockedBy: { nodes: { number: number; state: string }[] };
}

function gh(args: string[]): string {
  return execFileSync('gh', args, { encoding: 'utf8' });
}

const [owner, repo] = (
  process.env.GITHUB_REPOSITORY ??
  gh(['repo', 'view', '--json', 'nameWithOwner', '--jq', '.nameWithOwner']).trim()
).split('/') as [string, string];

function facts(number: number): IssueFacts {
  const query = `query($owner:String!,$repo:String!,$num:Int!){
    repository(owner:$owner,name:$repo){
      issue(number:$num){
        number state body
        labels(first:50){ nodes{ name } }
        subIssues(first:1){ totalCount }
        blockedBy(first:50){ nodes{ number state } }
      }
    }
  }`;
  const out = gh([
    'api',
    'graphql',
    '-f',
    `query=${query}`,
    '-F',
    `owner=${owner}`,
    '-F',
    `repo=${repo}`,
    '-F',
    `num=${number}`,
    '--jq',
    '.data.repository.issue',
  ]);
  return JSON.parse(out) as IssueFacts;
}

/** Issue numbers referenced under a `## Blocked by` heading, up to the next heading. */
function blockersInBody(body: string): number[] {
  const section = /^##\s+Blocked by\s*$([\s\S]*?)(?=^##\s|(?![\s\S]))/im.exec(body);
  if (!section) return [];
  return [...section[1]!.matchAll(/(?<![\w/])#(\d+)\b/g)].map((m) => Number(m[1]));
}

function isOpen(number: number): boolean {
  return gh(['api', `repos/${owner}/${repo}/issues/${number}`, '--jq', '.state']).trim() === 'open';
}

function hasOpenPr(number: number): boolean {
  const prs = gh([
    'pr',
    'list',
    '--head',
    `sandcastle/issue-${number}`,
    '--state',
    'open',
    '--json',
    'number',
  ]);
  return (JSON.parse(prs) as unknown[]).length > 0;
}

/** Why `issue` cannot start now, or null when it can. */
function reasonToSkip(issue: IssueFacts): string | null {
  if (issue.state !== 'OPEN') return 'not open';
  if (!issue.labels.nodes.some((l) => l.name === READY_LABEL)) return `no ${READY_LABEL} label`;
  if (issue.subIssues.totalCount > 0) return 'has sub-issues (a spec, not a ticket)';
  if (/^##\s+User Stories\s*$/im.test(issue.body)) return 'written from the to-spec template';

  const native = issue.blockedBy.nodes.filter((b) => b.state === 'OPEN').map((b) => b.number);
  if (native.length > 0) return `blocked by #${native.join(', #')}`;

  const listed = blockersInBody(issue.body).filter((n) => n !== issue.number && isOpen(n));
  if (listed.length > 0) return `blocked by #${listed.join(', #')} (## Blocked by)`;

  if (hasOpenPr(issue.number)) return 'already has an open PR';
  return null;
}

function candidates(): number[] {
  const only = process.argv[2]?.trim();
  if (only) {
    if (!/^\d+$/.test(only))
      throw new Error(`Expected an issue number, got ${JSON.stringify(only)}.`);
    return [Number(only)];
  }
  const list = gh([
    'issue',
    'list',
    '--state',
    'open',
    '--label',
    READY_LABEL,
    '--limit',
    '200',
    '--json',
    'number',
    '--jq',
    '[.[].number]',
  ]);
  return JSON.parse(list) as number[];
}

function main() {
  const ready: number[] = [];
  for (const number of candidates()) {
    const skip = reasonToSkip(facts(number));
    if (skip) {
      console.error(`#${number}: skip (${skip})`);
    } else {
      console.error(`#${number}: ready`);
      ready.push(number);
    }
  }

  const json = JSON.stringify(ready);
  console.log(json);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `issues=${json}\n`);
}

main();
