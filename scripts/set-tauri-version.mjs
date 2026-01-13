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
// MSI (WiX) requires a numeric-only version: major.minor.patch.build
// with constraints: major<=255, minor<=255, patch<=65535, build<=65535.
// We'll derive a stable Wix version from dd.MM.yyyy.xx:
// major = yyyy - 2000 (0..255)
// minor = MM (1..12)
// patch = dd*100 + xx (0..3199)
// build = 0
const [dd, mm, yyyy, xx] = tag.split('.').map(n => Number.parseInt(n, 10));
const major = yyyy - 2000;
const minor = mm;
const patch = (dd * 100) + xx;
const build = 0;

if (!(major >= 0 && major <= 255)) {
  console.error(`Derived MSI major out of range (0..255): ${major} from ${tag}`);
  process.exit(1);
}
if (!(minor >= 0 && minor <= 255)) {
  console.error(`Derived MSI minor out of range (0..255): ${minor} from ${tag}`);
  process.exit(1);
}
if (!(patch >= 0 && patch <= 65535)) {
  console.error(`Derived MSI patch out of range (0..65535): ${patch} from ${tag}`);
  process.exit(1);
}

const wixVersion = `${major}.${minor}.${patch}.${build}`;

const confPath = new URL('../src-tauri/tauri.conf.json', import.meta.url);
const raw = await fs.readFile(confPath, 'utf8');
const conf = JSON.parse(raw);

conf.version = conf.version || '0.1.0';
conf.bundle = conf.bundle || {};
conf.bundle.windows = conf.bundle.windows || {};
conf.bundle.windows.wix = conf.bundle.windows.wix || {};
conf.bundle.windows.wix.version = wixVersion;

await fs.writeFile(confPath, JSON.stringify(conf, null, 2) + '\n', 'utf8');
console.log(`Updated src-tauri/tauri.conf.json wix.version -> ${wixVersion}`);
