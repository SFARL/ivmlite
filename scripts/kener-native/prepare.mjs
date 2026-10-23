import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const SOURCE_SHA = '5bc658b771d44f48b61a514cfdb5037468c5aa36';
export const ROOT_DEPENDENCIES = ['knex', 'better-sqlite3', 'dotenv'];
export const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');
export const readJson = (file) => JSON.parse(readFileSync(file, 'utf8'));
export const writeJson = (file, value) => writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
export const inside = (root, file) => file === root || file.startsWith(`${root}${path.sep}`);

export function options(required, optional = []) {
  const result = {};
  for (let index = 2; index < process.argv.length; index += 2) {
    const flag = process.argv[index];
    const value = process.argv[index + 1];
    assert(flag.startsWith('--') && [...required, ...optional].includes(flag.slice(2)), `Unknown option: ${flag}`);
    assert(value && !value.startsWith('--'), `Missing value for ${flag}`);
    assert(!result[flag.slice(2)], `Duplicate option: ${flag}`);
    result[flag.slice(2)] = path.resolve(value);
  }
  for (const name of required) assert(result[name], `Required: --${name}`);
  return result;
}

export function validateUpstream(upstream) {
  const root = realpathSync(upstream);
  assert.equal(execFileSync('git', ['rev-parse', '--show-toplevel'], { cwd: root, encoding: 'utf8' }).trim(), root);
  assert.equal(execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(), SOURCE_SHA, 'Unexpected upstream commit');
  assert.equal(execFileSync('git', ['status', '--porcelain', '--untracked-files=all'], { cwd: root, encoding: 'utf8' }).trim(), '', 'Upstream checkout must be clean');
  assert.equal(readJson(path.join(root, 'package.json')).version, '4.1.8');
  assert(process.versions.node.split('.').map(Number)[0] >= 24, 'Use Node >=24.14');
  if (Number(process.versions.node.split('.')[0]) === 24) assert(Number(process.versions.node.split('.')[1]) >= 14, 'Use Node >=24.14');
  return root;
}

// Resolve a package using npm's nested node_modules lookup, preserving its exact
// upstream record rather than resolving transitive version ranges again.
function lockedDependency(packages, owner, name) {
  let current = owner;
  while (true) {
    const candidate = current ? `${current}/node_modules/${name}` : `node_modules/${name}`;
    if (packages[candidate]) return candidate;
    if (!current) throw new Error(`Missing locked dependency ${name} from ${owner}`);
    const boundary = current.lastIndexOf('/node_modules/');
    current = boundary < 0 ? '' : current.slice(0, boundary);
  }
}

export function minimalLock(upstream) {
  const original = readJson(path.join(upstream, 'package-lock.json'));
  assert.equal(original.lockfileVersion, 3);
  const dependencies = Object.fromEntries(ROOT_DEPENDENCIES.map((name) => [name, original.packages[`node_modules/${name}`].version]));
  assert.equal(dependencies.knex, '3.1.0');
  assert.equal(dependencies['better-sqlite3'], '12.6.2');
  const manifest = { name: 'ivmlite-kener-native-runtime', version: '1.0.0', private: true, type: 'module', dependencies };
  const packages = { '': { name: manifest.name, version: manifest.version, dependencies } };
  const pending = ROOT_DEPENDENCIES.map((name) => `node_modules/${name}`);
  while (pending.length) {
    const key = pending.pop();
    if (packages[key]) continue;
    const record = original.packages[key];
    assert(record && !record.link && record.version && record.resolved && record.integrity, `Incomplete lock record: ${key}`);
    packages[key] = record;
    for (const name of Object.keys(record.dependencies ?? {})) pending.push(lockedDependency(original.packages, key, name));
    for (const name of Object.keys(record.optionalDependencies ?? {})) pending.push(lockedDependency(original.packages, key, name));
  }
  return { manifest, lock: { name: manifest.name, version: manifest.version, lockfileVersion: 3, requires: true, packages } };
}

export function validateRuntime(upstream, runtime) {
  const expected = minimalLock(upstream);
  assert.deepEqual(readJson(path.join(runtime, 'package.json')), expected.manifest, 'Runtime manifest drift');
  assert.deepEqual(readJson(path.join(runtime, 'package-lock.json')), expected.lock, 'Runtime lock drift');
  const identity = readJson(path.join(runtime, 'kener-runtime.json'));
  assert.equal(identity.sourceSHA, SOURCE_SHA);
  assert.equal(identity.sourceLockSHA256, sha256(path.join(upstream, 'package-lock.json')));
  for (const [key, record] of Object.entries(expected.lock.packages)) {
    if (key) assert.equal(readJson(path.join(runtime, key, 'package.json')).version, record.version, `Installed version drift: ${key}`);
  }
  return identity;
}

function prepare() {
  const args = options(['upstream', 'runtime']);
  const upstream = validateUpstream(args.upstream);
  mkdirSync(args.runtime, { recursive: true });
  const runtime = realpathSync(args.runtime);
  const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  assert(!inside(upstream, runtime) && !inside(repository, runtime), 'Runtime must be outside upstream and ivmlite checkouts');
  if (existsSync(path.join(runtime, 'package.json'))) {
    assert(existsSync(path.join(runtime, 'kener-runtime.json')), 'Refusing to overwrite an unowned runtime');
  }
  const { manifest, lock } = minimalLock(upstream);
  writeJson(path.join(runtime, 'package.json'), manifest);
  writeJson(path.join(runtime, 'package-lock.json'), lock);
  writeJson(path.join(runtime, 'kener-runtime.json'), {
    sourceSHA: SOURCE_SHA,
    sourceLockSHA256: sha256(path.join(upstream, 'package-lock.json')),
    packageCount: Object.keys(lock.packages).length - 1,
    node: process.version,
  });
  execFileSync('npm', ['ci', '--no-audit', '--no-fund'], {
    cwd: runtime, stdio: 'inherit', env: { ...process.env, PATH: `${path.dirname(process.execPath)}${path.delimiter}${process.env.PATH ?? ''}` },
  });
  const identity = validateRuntime(upstream, runtime);
  validateUpstream(upstream);
  console.log(JSON.stringify(identity));
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) prepare();
