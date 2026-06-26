#!/usr/bin/env node
// Replace the __CACHE_BUST__ placeholders in the deploy root with a per-deploy
// version, so the galacto.js import AND the styles.css link are fresh URLs on
// every deploy. Cloudflare Pages force-caches static assets (.js, .css) with a
// max-age that _headers can't override, so a stable filename serves stale glue or
// styles after a deploy; the ?v= query sidesteps that. index.html itself
// revalidates each load (see _headers), so the new ?v= is always picked up.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { join, resolve } from 'node:path';

const outDir = resolve(process.cwd(), process.argv[2] || 'dist');
const indexPath = join(outDir, 'index.html');
const sentryConfigPath = join(outDir, 'sentry-config.js');
const swPath = join(outDir, 'sw.js');

let version;
try {
  version = execSync('git rev-parse --short HEAD', { encoding: 'utf8' }).trim();
} catch {
  // Not a git checkout (e.g. a release tarball) — fall back to a build timestamp.
  version = String(Date.now());
}

replaceRequired(indexPath, 'index.html', version);
console.log(`cache-bust: pinned HTML asset URLs to ?v=${version}`);

// Stamp the same version into the service worker, so each deploy gets its own
// cache (galacto-<version>) and its precache URLs match index.html's ?v=.
replaceRequired(swPath, 'sw.js', version);
console.log(`cache-bust: pinned service worker cache to galacto-${version}`);

writeFileSync(
  sentryConfigPath,
  `window.GALACTO_SENTRY_CONFIG = ${JSON.stringify(
    {
      app: 'galacto',
      dsn: process.env.SENTRY_DSN || '',
      environment: process.env.SENTRY_ENVIRONMENT || 'production',
      release: process.env.SENTRY_RELEASE || process.env.GITHUB_SHA || version,
      tracesSampleRate: parseTracesSampleRate(process.env.SENTRY_TRACES_SAMPLE_RATE),
    },
    null,
    2,
  )};\n`,
);

function parseTracesSampleRate(value) {
  const rate = Number(value || 0.05);
  return Number.isFinite(rate) && rate >= 0 && rate <= 1 ? rate : 0.05;
}

function replaceRequired(path, label, version) {
  if (!existsSync(path)) {
    throw new Error(`cache-bust: missing ${label} at ${path}`);
  }
  const source = readFileSync(path, 'utf8');
  if (!source.includes('__CACHE_BUST__')) {
    throw new Error(`cache-bust: no __CACHE_BUST__ placeholder found in ${label}`);
  }
  writeFileSync(path, source.replaceAll('__CACHE_BUST__', version));
}
