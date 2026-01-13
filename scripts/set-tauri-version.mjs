import fs from 'node:fs/promises';

const [,, tagArg] = process.argv;
const tag = (tagArg || '').trim();

if (!tag) {
  console.error('Missing version tag argument');
  process.exit(1);
}

// Expected updater/release tag format: dd.MM.yyyy.xx
if (!/^\d{2}\.\d{2}\.\d{4}\.\d{2}$/.test(tag)) {
  console.error(`Invalid tag format (expected dd.MM.yyyy.xx): ${tag}`);
  process.exit(1);
}

// Tauri requires a SemVer string for package version.
// Encode our date-stamp tag as SemVer build metadata.
// Example: 0.1.0+13.01.2026.01
const semver = `0.1.0+${tag}`;

const confPath = new URL('../src-tauri/tauri.conf.json', import.meta.url);
const raw = await fs.readFile(confPath, 'utf8');
const conf = JSON.parse(raw);

conf.version = semver;

await fs.writeFile(confPath, JSON.stringify(conf, null, 2) + '\n', 'utf8');
console.log(`Updated src-tauri/tauri.conf.json version -> ${semver}`);
