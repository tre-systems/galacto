#!/usr/bin/env node
// Rasterise the brand SVG sources (assets/) into the PNGs the page references:
// the PWA icons (static/icons/*.png) and the Open Graph social card
// (static/og-card.png). Like the diagrams, the output is committed and this runs
// locally on demand; it needs librsvg's `rsvg-convert` on PATH
// (`brew install librsvg`). Re-run after editing the SVGs: `npm run icons`.
//
// In `--check` mode it renders the font-independent icon outputs into a
// temporary directory and compares them with the committed files, so CI can catch
// stale launcher assets. The OG card contains text, so its exact pixels depend on
// the OS font rasterizer; check mode verifies its committed PNG dimensions rather
// than treating those platform-specific pixels as reproducible.
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
const iconTargets = [
  ['icons/icon.svg', 'icons/icon-192.png', 192, 192],
  ['icons/icon.svg', 'icons/icon-512.png', 512, 512],
  ['icons/icon.svg', 'icons/apple-touch-icon.png', 180, 180],
  ['icons/icon-maskable.svg', 'icons/icon-maskable-512.png', 512, 512],
];
const ogTarget = ['og-card.svg', 'og-card.png', 1200, 630];
const targets = [
  ...iconTargets,
  ogTarget,
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
    renderPngTargets(tempStatic, { quiet: true, targetList: iconTargets });

    const files = ['icons/icon.svg', ...iconTargets.map(([, output]) => output)];
    const drifted = files.filter((file) => !sameFile(join(tempStatic, file), join(root, 'static', file)));
    if (drifted.length) {
      console.error('gen-icons: committed generated assets are stale:');
      for (const file of drifted) console.error(`- static/${file}`);
      console.error('gen-icons: run `npm run icons` and commit the updated outputs.');
      process.exit(1);
    }
    verifyPngDimensions(join(root, 'static', ogTarget[1]), ogTarget[2], ogTarget[3]);
    console.log(`gen-icons: ${files.length} generated icon assets match committed outputs`);
    console.log(`gen-icons: static/${ogTarget[1]} has expected ${ogTarget[2]}×${ogTarget[3]} dimensions`);
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

function renderPngTargets(staticRoot, { quiet = false, targetList = targets } = {}) {
  for (const [svg, png, w, h] of targetList) {
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

function verifyPngDimensions(path, expectedWidth, expectedHeight) {
  if (!existsSync(path)) {
    console.error(`gen-icons: missing output ${path}`);
    process.exit(1);
  }
  const bytes = readFileSync(path);
  const pngSignature = '89504e470d0a1a0a';
  if (bytes.subarray(0, 8).toString('hex') !== pngSignature) {
    console.error(`gen-icons: ${path} is not a PNG`);
    process.exit(1);
  }
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (width !== expectedWidth || height !== expectedHeight) {
    console.error(
      `gen-icons: ${path} is ${width}×${height}, expected ${expectedWidth}×${expectedHeight}`,
    );
    process.exit(1);
  }
}
