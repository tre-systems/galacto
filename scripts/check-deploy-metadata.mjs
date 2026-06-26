#!/usr/bin/env node
// Check source deploy metadata that is committed as duplicated/generated files.
// `site.webmanifest` is the canonical PWA manifest path; `manifest.json` is an
// identical compatibility copy for clients that still probe that name.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = process.cwd();
const errors = [];

const siteManifest = join(root, 'static', 'site.webmanifest');
const legacyManifest = join(root, 'static', 'manifest.json');

verifyManifestIdentity(siteManifest, legacyManifest);

if (errors.length) {
  console.error('check-deploy-metadata failed:');
  for (const error of errors) console.error(`- ${error}`);
  process.exit(1);
}

execFileSync(process.execPath, ['scripts/gen-icons.mjs', '--check'], {
  cwd: root,
  stdio: 'inherit',
});

console.log('check-deploy-metadata: manifests are identical');

function verifyManifestIdentity(canonicalPath, compatibilityPath) {
  const canonical = readRequired(canonicalPath);
  const compatibility = readRequired(compatibilityPath);
  if (!canonical || !compatibility) return;
  const canonicalText = canonical.toString('utf8');
  const compatibilityText = compatibility.toString('utf8');

  try {
    JSON.parse(canonicalText);
  } catch (error) {
    errors.push(`static/site.webmanifest is not valid JSON: ${error.message}`);
  }

  try {
    JSON.parse(compatibilityText);
  } catch (error) {
    errors.push(`static/manifest.json is not valid JSON: ${error.message}`);
  }

  if (!canonical.equals(compatibility)) {
    errors.push('static/site.webmanifest and static/manifest.json must be byte-identical');
  }
}

function readRequired(path) {
  if (!existsSync(path)) {
    errors.push(`missing required file: ${path}`);
    return '';
  }
  return readFileSync(path);
}
