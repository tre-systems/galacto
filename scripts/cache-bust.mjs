#!/usr/bin/env node
// Replace the __CACHE_BUST__ placeholder in pkg/index.html with a per-deploy
// version, so the galacto.js import is a fresh URL on every deploy. Cloudflare
// Pages force-caches .js (a max-age that _headers can't override), so a stable
// filename would let a cached old glue pair with a new wasm; the ?v= query
// sidesteps that. index.html itself always revalidates (see _headers).
import { readFileSync, writeFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { join } from 'node:path';

const indexPath = join(process.cwd(), 'pkg', 'index.html');

let version;
try {
  version = execSync('git rev-parse --short HEAD', { encoding: 'utf8' }).trim();
} catch {
  // Not a git checkout (e.g. a release tarball) — fall back to a build timestamp.
  version = String(Date.now());
}

const html = readFileSync(indexPath, 'utf8');
const busted = html.replaceAll('__CACHE_BUST__', version);

if (busted === html) {
  console.warn('cache-bust: no __CACHE_BUST__ placeholder found in pkg/index.html');
} else {
  writeFileSync(indexPath, busted);
  console.log(`cache-bust: pinned galacto.js import to ?v=${version}`);
}
