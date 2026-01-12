import { execFileSync } from 'node:child_process';

function runGit(args) {
  return execFileSync('git', args, { stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' }).trim();
}

function pad2(n) {
  return String(n).padStart(2, '0');
}

function todayPrefix(d = new Date()) {
  const dd = pad2(d.getDate());
  const mm = pad2(d.getMonth() + 1);
  const yyyy = d.getFullYear();
  return `${dd}.${mm}.${yyyy}.`;
}

function ensureCleanWorkingTree() {
  const status = runGit(['status', '--porcelain']);
  if (status) {
    console.error('Working tree is not clean. Commit/stash changes before tagging.');
    process.exit(1);
  }
}

function ensureTagDoesNotExist(tag) {
  const out = runGit(['tag', '--list', tag]);
  if (out) {
    console.error(`Tag already exists: ${tag}`);
    process.exit(1);
  }
}

function computeNextTag() {
  const prefix = todayPrefix();
  const list = runGit(['tag', '--list', `${prefix}*`]);
  const tags = list ? list.split(/\r?\n/).map(s => s.trim()).filter(Boolean) : [];
  let max = -1;
  for (const t of tags) {
    if (!t.startsWith(prefix)) continue;
    const suffix = t.slice(prefix.length);
    const n = Number.parseInt(suffix, 10);
    if (Number.isFinite(n)) max = Math.max(max, n);
  }
  const next = max + 1;
  return `${prefix}${pad2(next)}`;
}

const userTag = (process.argv[2] || '').trim();

try {
  ensureCleanWorkingTree();

  const tag = userTag || computeNextTag();
  ensureTagDoesNotExist(tag);

  runGit(['tag', tag]);
  execFileSync('git', ['push', 'origin', tag], { stdio: 'inherit' });

  console.log(`\nCreated and pushed tag: ${tag}`);
} catch (err) {
  console.error(String(err?.message || err));
  process.exit(1);
}
