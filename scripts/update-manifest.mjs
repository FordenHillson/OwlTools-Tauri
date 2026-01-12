import fs from 'node:fs/promises';

const [,, versionArg, sha256Arg, fileNameArg] = process.argv;

const version = (versionArg || '').trim();
const sha256 = (sha256Arg || '').trim().toLowerCase();
const fileName = (fileNameArg || '').trim();

if (!version) {
  console.error('Missing version');
  process.exit(1);
}
if (!sha256) {
  console.error('Missing sha256');
  process.exit(1);
}
if (!fileName) {
  console.error('Missing fileName');
  process.exit(1);
}

const repo = process.env.GITHUB_REPOSITORY;
if (!repo) {
  console.error('Missing GITHUB_REPOSITORY');
  process.exit(1);
}

const manifestPath = new URL('../manifest.json', import.meta.url);
let manifest;
try {
  const raw = await fs.readFile(manifestPath, 'utf8');
  manifest = JSON.parse(raw);
} catch {
  manifest = { product: 'OwlTools', platform: 'windows-x64', latest: version, versions: [] };
}

if (!manifest || typeof manifest !== 'object') {
  manifest = { product: 'OwlTools', platform: 'windows-x64', latest: version, versions: [] };
}

manifest.product = manifest.product || 'OwlTools';
manifest.platform = manifest.platform || 'windows-x64';
manifest.latest = version;

const url = `https://github.com/${repo}/releases/download/${version}/${fileName}`;
const entry = {
  version,
  notes: `Release ${version}`,
  msi: {
    url,
    sha256: sha256.toLowerCase()
  }
};

const list = Array.isArray(manifest.versions) ? manifest.versions : [];
const without = list.filter(v => v && typeof v === 'object' && v.version !== version);
manifest.versions = [entry, ...without];

await fs.writeFile(manifestPath, JSON.stringify(manifest, null, 2) + '\n', 'utf8');
console.log(`Updated manifest.json for ${version}`);
