#!/usr/bin/env node
// Sanity-check the deploy root after assembly/cache-busting. This catches stale
// placeholders, missing app-shell files, and accidental raw wasm-pack artifacts.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';

const root = process.cwd();
const outDir = resolve(root, process.argv[2] || 'dist');

const requiredFiles = [
  '_headers',
  '_worker.js',
  'app.js',
  'audio.html',
  'engineering.html',
  'favicon.svg',
  'galacto.js',
  'galacto_bg.wasm',
  'icons/apple-touch-icon.png',
  'icons/icon-192.png',
  'icons/icon-512.png',
  'icons/icon-maskable-512.png',
  'index.html',
  'manifest.json',
  'og-card.png',
  'physics.html',
  'robots.txt',
  '10.57.0/feedback-modal.min.js',
  '10.57.0/feedback-screenshot.min.js',
  'sentry-config.js',
  'sentry-sdk.js',
  'sentry.js',
  'site.webmanifest',
  'sitemap.xml',
  'styles.css',
  'sw.js',
];

const forbiddenFiles = [
  'package.json',
  'README.md',
  'LICENSE',
  'galacto.d.ts',
  'galacto_bg.wasm.d.ts',
  'cache-manifest.json',
];

const textExts = new Set([
  '',
  '.css',
  '.html',
  '.js',
  '.json',
  '.svg',
  '.txt',
  '.webmanifest',
  '.xml',
]);

const errors = [];

if (!existsSync(outDir) || !statSync(outDir).isDirectory()) {
  fail(`deploy root does not exist: ${outDir}`);
}

for (const file of requiredFiles) {
  const path = join(outDir, file);
  if (!existsSync(path) || !statSync(path).isFile()) {
    errors.push(`missing required file: ${file}`);
  }
}

for (const file of forbiddenFiles) {
  if (existsSync(join(outDir, file))) {
    errors.push(`raw wasm-pack/non-deploy artifact present: ${file}`);
  }
}

scanTextFiles(outDir, (file, text) => {
  if (text.includes('__CACHE_BUST__')) {
    errors.push(`unreplaced __CACHE_BUST__ placeholder in ${relative(file)}`);
  }
});

const index = readText('index.html');
const app = readText('app.js');
const sw = readText('sw.js');
const headers = readText('_headers');
const manifest = readText('site.webmanifest');
const legacyManifest = readText('manifest.json');

if (index && app && sw) verifyVersioning(index, app, sw);
if (headers) verifyHeaders(headers);
if (manifest) verifyManifest(manifest);
if (manifest && legacyManifest) verifyManifestIdentity(manifest, legacyManifest);

if (errors.length) {
  console.error('verify-build failed:');
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}

console.log(`verify-build: ${requiredFiles.length} required files checked in ${outDir}`);

function verifyVersioning(index, app, sw) {
  const appVersion = match(index, /app\.js\?v=([A-Za-z0-9._-]+)/, 'versioned app.js');
  const cssVersion = match(index, /styles\.css\?v=([A-Za-z0-9._-]+)/, 'versioned styles.css');
  const swBuildHash = match(sw, /const BUILD_HASH = '([^']+)'/, 'service worker BUILD_HASH');

  const versions = [appVersion, cssVersion, swBuildHash].filter(Boolean);
  if (new Set(versions).size > 1) {
    errors.push(`asset versions disagree: ${versions.join(', ')}`);
  }

  if (!app.includes('new URL(import.meta.url).searchParams.get("v")')) {
    errors.push('app.js does not read its cache-bust version from import.meta.url');
  }
  if (!app.includes('./galacto.js${assetVersionSuffix}')) {
    errors.push('app.js does not load versioned galacto.js');
  }
  if (!app.includes('./galacto_bg.wasm${assetVersionSuffix}')) {
    errors.push('app.js does not load versioned galacto_bg.wasm');
  }
  if (!app.includes('./sw.js${assetVersionSuffix}')) {
    errors.push('app.js does not register a versioned service worker');
  }

  if (!sw.includes('/app.js?v=') || !sw.includes('BUILD_HASH')) {
    errors.push('service worker does not precache versioned app.js');
  }
  if (!sw.includes('/galacto.js?v=') || !sw.includes('BUILD_HASH')) {
    errors.push('service worker does not precache versioned galacto.js');
  }
  if (!sw.includes('/galacto_bg.wasm?v=') || !sw.includes('BUILD_HASH')) {
    errors.push('service worker does not precache versioned galacto_bg.wasm');
  }
  if (!sw.includes('/styles.css?v=') || !sw.includes('BUILD_HASH')) {
    errors.push('service worker does not precache versioned styles.css');
  }
}

function verifyHeaders(headers) {
  for (const token of [
    'Cache-Control: no-cache',
    'Content-Security-Policy:',
    "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'",
    'X-Content-Type-Options: nosniff',
    'frame-ancestors',
  ]) {
    if (!headers.includes(token)) errors.push(`_headers missing ${token}`);
  }
}

function verifyManifest(manifestText) {
  try {
    const parsed = JSON.parse(manifestText);
    for (const size of ['192x192', '512x512']) {
      if (!parsed.icons?.some((icon) => icon.sizes === size)) {
        errors.push(`site.webmanifest missing ${size} icon`);
      }
    }
  } catch (error) {
    errors.push(`site.webmanifest is not valid JSON: ${error.message}`);
  }
}

function verifyManifestIdentity(canonical, compatibility) {
  if (canonical !== compatibility) {
    errors.push('site.webmanifest and manifest.json must be byte-identical');
  }
}

function readText(file) {
  const path = join(outDir, file);
  if (!existsSync(path)) return '';
  return readFileSync(path, 'utf8');
}

function match(text, regex, label) {
  const found = text.match(regex);
  if (!found) {
    errors.push(`missing ${label}`);
    return '';
  }
  return found[1];
}

function scanTextFiles(dir, visitor) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      scanTextFiles(path, visitor);
      continue;
    }
    if (!entry.isFile() || !textExts.has(extname(entry.name))) continue;
    visitor(path, readFileSync(path, 'utf8'));
  }
}

function relative(path) {
  return path.slice(outDir.length + 1);
}

function fail(message) {
  console.error(`verify-build failed: ${message}`);
  process.exit(1);
}
