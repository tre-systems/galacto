#!/usr/bin/env node
// Rasterise the brand SVG sources (assets/) into the PNGs the page references:
// the PWA icons (static/icons/*.png) and the Open Graph social card
// (static/og-card.png). Like the diagrams, the output is committed and this runs
// locally on demand; it needs librsvg's `rsvg-convert` on PATH
// (`brew install librsvg`). Re-run after editing the SVGs: `npm run icons`.
//
// In `--check` mode it renders into a temporary directory and compares the
// result with the committed files, so CI can catch stale generated assets.
import { execFileSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';

const root = process.cwd();
const checkMode = process.argv.includes('--check');

const iconSvg = join(root, 'assets', 'icons', 'icon.svg');
const iconSvgOut = join(root, 'static', 'icons', 'icon.svg');
if (!existsSync(iconSvg)) {
  console.error(`gen-icons: missing source ${iconSvg}`);
  process.exit(1);
}

// [source svg, output png, width, height]
const targets = [
  ['icons/icon.svg', 'icons/icon-192.png', 192, 192],
  ['icons/icon.svg', 'icons/icon-512.png', 512, 512],
  ['icons/icon.svg', 'icons/apple-touch-icon.png', 180, 180],
  ['icons/icon-maskable.svg', 'icons/icon-maskable-512.png', 512, 512],
  ['og-card.svg', 'og-card.png', 1200, 630],
];

try {
  execFileSync('rsvg-convert', ['--version'], { stdio: 'ignore' });
} catch {
  console.error('gen-icons: rsvg-convert not found. Install librsvg (brew install librsvg).');
  process.exit(1);
}

if (checkMode) {
  const tempRoot = mkdtempSync(join(tmpdir(), 'galacto-icons-'));
  try {
    const tempStatic = join(tempRoot, 'static');
    const tempIconSvgOut = join(tempStatic, 'icons', 'icon.svg');
    copyIconSvg(tempIconSvgOut);
    renderPngTargets(tempStatic, { quiet: true });

    const files = ['icons/icon.svg', ...targets.map(([, output]) => output)];
    const drifted = files.filter((file) => !sameFile(join(tempStatic, file), join(root, 'static', file)));
    if (drifted.length) {
      console.error('gen-icons: committed generated assets are stale:');
      for (const file of drifted) console.error(`- static/${file}`);
      console.error('gen-icons: run `npm run icons` and commit the updated outputs.');
      process.exit(1);
    }
    console.log(`gen-icons: ${files.length} generated assets match committed outputs`);
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
} else {
  copyIconSvg(iconSvgOut);
  console.log('gen-icons: icons/icon.svg -> icons/icon.svg');
  renderPngTargets(join(root, 'static'));
}

function copyIconSvg(output) {
  mkdirSync(dirname(output), { recursive: true });
  copyFileSync(iconSvg, output);
}

function renderPngTargets(staticRoot, { quiet = false } = {}) {
  for (const [svg, png, w, h] of targets) {
    const input = join(root, 'assets', svg);
    if (!existsSync(input)) {
      console.error(`gen-icons: missing source ${input}`);
      process.exit(1);
    }

    const output = join(staticRoot, png);
    mkdirSync(dirname(output), { recursive: true });
    execFileSync('rsvg-convert', [
      '-w', String(w),
      '-h', String(h),
      input,
      '-o', output,
    ]);
    if (!quiet) console.log(`gen-icons: ${svg} -> ${png} (${w}×${h})`);
  }
}

function sameFile(expected, actual) {
  if (!existsSync(actual)) return false;
  const expectedBytes = readFileSync(expected);
  const actualBytes = readFileSync(actual);
  return expectedBytes.equals(actualBytes);
}
