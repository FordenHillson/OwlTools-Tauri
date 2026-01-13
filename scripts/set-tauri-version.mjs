import fs from 'node:fs/promises';

const [,, versionArg] = process.argv;
const version = (versionArg || '').trim();

if (!version) {
  console.error('Missing version argument');
  process.exit(1);
}

// Very lightweight validation: dd.MM.yyyy.xx
if (!/^\d{2}\.\d{2}\.\d{4}\.\d{2}$/.test(version)) {
  console.error(`Invalid version format (expected dd.MM.yyyy.xx): ${version}`);
  process.exit(1);
}

const confPath = new URL('../src-tauri/tauri.conf.json', import.meta.url);
const raw = await fs.readFile(confPath, 'utf8');
const conf = JSON.parse(raw);

conf.version = version;

await fs.writeFile(confPath, JSON.stringify(conf, null, 2) + '\n', 'utf8');
console.log(`Updated src-tauri/tauri.conf.json version -> ${version}`);
