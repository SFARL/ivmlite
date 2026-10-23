import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { minimalLock, options, readJson, validateRuntime, validateUpstream, writeJson } from './prepare.mjs';

const args = options(['upstream', 'runtime', 'output']);
const upstream = validateUpstream(args.upstream);
const scratch = mkdtempSync(path.join(os.tmpdir(), 'kener-ownership-'));
const prepare = fileURLToPath(new URL('./prepare.mjs', import.meta.url));
const results = [];
function run(runtime) {
  return spawnSync(process.execPath, [prepare, '--upstream', upstream, '--runtime', runtime], { encoding: 'utf8' });
}
function snapshot(root) {
  const files = {};
  function walk(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const filename = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(filename);
      else files[path.relative(root, filename)] = readFileSync(filename).toString('base64');
    }
  }
  walk(root);
  return files;
}
function rejectsWithoutChanges(name, files) {
  const runtime = path.join(scratch, name);
  for (const [file, contents] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(runtime, file)), { recursive: true });
    writeFileSync(path.join(runtime, file), contents);
  }
  const before = snapshot(runtime);
  const result = run(runtime);
  assert.notEqual(result.status, 0, `${name} should be rejected`);
  assert.deepEqual(snapshot(runtime), before, `${name} must remain byte-identical`);
  results.push({ name, rejected: true, preserved: true });
}
try {
  rejectsWithoutChanges('unowned-without-manifest', {
    'package-lock.json': '{"unrelated":"lock"}', 'node_modules/sentinel': 'unrelated install',
  });
  rejectsWithoutChanges('unowned-with-manifest', { 'package.json': '{"name":"unrelated"}' });
  const { manifest, lock } = minimalLock(upstream);
  const owned = {
    'package.json': JSON.stringify(manifest), 'package-lock.json': JSON.stringify(lock),
    'kener-runtime.json': JSON.stringify(readJson(path.join(args.runtime, 'kener-runtime.json'))),
    'node_modules/sentinel': 'preserve on rejection',
  };
  const marker = readJson(path.join(args.runtime, 'kener-runtime.json'));
  rejectsWithoutChanges('corrupt-marker', { ...owned, 'kener-runtime.json': JSON.stringify({ ...marker, sourceSHA: 'wrong' }) });
  rejectsWithoutChanges('wrong-marker-count', { ...owned, 'kener-runtime.json': JSON.stringify({ ...marker, packageCount: 0 }) });
  rejectsWithoutChanges('corrupt-manifest', { ...owned, 'package.json': JSON.stringify({ ...manifest, name: 'wrong' }) });
  rejectsWithoutChanges('corrupt-lock', { ...owned, 'package-lock.json': JSON.stringify({ ...lock, packages: {} }) });
  const empty = path.join(scratch, 'empty-runtime');
  mkdirSync(empty);
  const firstInstall = run(empty);
  assert.equal(firstInstall.status, 0, firstInstall.stderr);
  validateRuntime(upstream, empty);
  results.push({ name: 'empty-directory-install', installed: true });
  const rerun = run(args.runtime);
  assert.equal(rerun.status, 0, rerun.stderr);
  validateRuntime(upstream, args.runtime);
  results.push({ name: 'valid-owned-rerun', installed: true });
  validateUpstream(upstream);
  writeJson(args.output, { status: 'passed', results });
  console.log(`PASS: ${results.length} runtime ownership regression checks`);
} catch (error) {
  writeJson(args.output, { status: 'failed', results, error: error.stack });
  throw error;
}
