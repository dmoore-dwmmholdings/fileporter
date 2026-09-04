/* Generates metadata-only notices. License prose remains in each dependency's source. */
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = process.cwd();
const output = join(root, 'THIRD_PARTY_MANIFEST.json');
const manifests = ['crates/fileporter-protocol/Cargo.toml', 'crates/fileporter-identity/Cargo.toml', 'crates/fileporter-network/Cargo.toml', 'crates/fileporter-transfer/Cargo.toml', 'src-tauri/Cargo.toml'];
// Permissive licenses present in the locked dependency graphs; unknown or copyleft-only entries fail closed.
// OFL-1.1 is the SIL Open Font License, which permits bundling and redistributing
// an unmodified font inside an application. It covers the IBM Plex faces the UI
// ships, which are self-hosted because the app's CSP forbids remote stylesheets.
const allowed = new Set(['0BSD', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-1-Clause', 'BSD-2-Clause', 'BSD-3-Clause', 'BlueOak-1.0.0', 'CC-BY-4.0', 'CC0-1.0', 'ISC', 'MIT', 'MIT-0', 'MPL-2.0', 'OFL-1.1', 'Python-2.0', 'Unicode-3.0', 'Unlicense', 'Zlib']);
// Cargo metadata occasionally uses slash-separated alternatives. An OR branch is
// acceptable when every license it requires is allowlisted; a copyleft alternative
// does not invalidate a separately permissive option.
const validLicense = (license) => typeof license === 'string' && license.replace(/\//g, ' OR ').replace(/[()]/g, '').split(/\s+OR\s+/).some((option) => option.split(/\s+AND\s+/).every((atom) => allowed.has(atom.trim())));
const repository = (value) => typeof value === 'string' ? value : value?.url ?? null;
const sourceLicenseFile = (directory) => readdirSync(directory, { withFileTypes: true }).find((entry) => entry.isFile() && /^licen[cs]e|^copying/i.test(entry.name))?.name ?? null;

const rust = new Map();
for (const manifest of manifests) {
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--format-version', '1', '--manifest-path', manifest], { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 }));
  for (const pkg of metadata.packages) rust.set(`${pkg.name}@${pkg.version}`, { name: pkg.name, version: pkg.version, license: pkg.license, repository: pkg.repository ?? null, source: pkg.source ?? 'workspace' });
}

const node = new Map();
const pnpm = join(root, 'node_modules', '.pnpm');
for (const entry of readdirSync(pnpm, { withFileTypes: true })) {
  if (!entry.isDirectory()) continue;
  const modules = join(pnpm, entry.name, 'node_modules');
  if (!existsSync(modules)) continue;
  for (const packageEntry of readdirSync(modules, { withFileTypes: true })) {
    const candidates = packageEntry.name.startsWith('@') && packageEntry.isDirectory()
      ? readdirSync(join(modules, packageEntry.name), { withFileTypes: true }).filter((child) => child.isDirectory()).map((child) => join(modules, packageEntry.name, child.name))
      : packageEntry.isDirectory() ? [join(modules, packageEntry.name)] : [];
    for (const directory of candidates) {
      const packagePath = join(directory, 'package.json');
      if (!existsSync(packagePath)) continue;
      const pkg = JSON.parse(readFileSync(packagePath, 'utf8'));
      node.set(`${pkg.name}@${pkg.version}`, { name: pkg.name, version: pkg.version, license: pkg.license ?? null, repository: repository(pkg.repository), sourceLicenseFile: sourceLicenseFile(directory) });
    }
  }
}
const entries = { rust: [...rust.values()].sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`)), pnpm: [...node.values()].sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`)) };
const unlicensed = [...entries.rust, ...entries.pnpm].filter((pkg) => !validLicense(pkg.license));
if (unlicensed.length) throw new Error(`License policy rejected or could not identify: ${unlicensed.map((pkg) => `${pkg.name}@${pkg.version} (${pkg.license ?? 'missing'})`).join(', ')}`);
const manifest = `${JSON.stringify({ schemaVersion: 1, policy: { allowedLicenseAtoms: [...allowed].sort(), note: 'Metadata-only source manifest; see each dependency sourceLicenseFile or upstream repository for license text.' }, lockfiles: [...manifests.map((file) => `${file.replace('Cargo.toml', 'Cargo.lock')}`), 'pnpm-lock.yaml'], entries }, null, 2)}\n`;
if (process.argv.includes('--write')) writeFileSync(output, manifest);
else if (!existsSync(output) || readFileSync(output, 'utf8') !== manifest) throw new Error(`Third-party manifest is stale. Run: pnpm licenses:generate (${relative(root, output)})`);
