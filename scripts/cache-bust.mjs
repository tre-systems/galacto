#!/usr/bin/env node
// Replace the __CACHE_BUST__ placeholders in pkg/index.html with a per-deploy
// version, so the galacto.js import AND the styles.css link are fresh URLs on
// every deploy. Cloudflare Pages force-caches static assets (.js, .css) with a
// max-age that _headers can't override, so a stable filename serves stale glue or
// styles after a deploy; the ?v= query sidesteps that. index.html itself
// revalidates each load (see _headers), so the new ?v= is always picked up.
import { readFileSync, writeFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { join } from 'node:path';

const indexPath = join(process.cwd(), 'pkg', 'index.html');
const sentryConfigPath = join(process.cwd(), 'pkg', 'sentry-config.js');

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
  console.log(`cache-bust: pinned galacto.js + styles.css to ?v=${version}`);
}

writeFileSync(
  sentryConfigPath,
  `window.TRE_STATIC_SENTRY_CONFIG = ${JSON.stringify(
    {
      app: 'galacto',
      dsn: process.env.SENTRY_DSN || '',
      environment: process.env.SENTRY_ENVIRONMENT || 'production',
      release: process.env.SENTRY_RELEASE || process.env.GITHUB_SHA || version,
    },
    null,
    2,
  )};\n`,
);
