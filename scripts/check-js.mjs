#!/usr/bin/env node
// Fast syntax check for repo-owned JavaScript. Vendored/minified Sentry bundles
// are intentionally excluded.
import { spawnSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const roots = ['scripts'];
const files = [
  'static/_worker.js',
  'static/app.js',
  'static/sentry-config.js',
  'static/sentry.js',
];

for (const root of roots) {
  for (const file of readdirSync(root).filter((name) => name.endsWith('.mjs')).sort()) {
    files.push(join(root, file));
  }
}

let failed = false;
for (const file of files) {
  if (!statSync(file).isFile()) continue;
  const result = spawnSync(process.execPath, ['--check', file], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.status !== 0) {
    failed = true;
    process.stderr.write(result.stdout || '');
    process.stderr.write(result.stderr || '');
  }
}

if (failed) process.exit(1);
console.log(`check-js: ${files.length} files parsed`);
