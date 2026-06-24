#!/usr/bin/env node
// Rasterise the brand SVG sources (assets/) into the PNGs the page references:
// the PWA icons (static/icons/*.png) and the Open Graph social card
// (static/og-card.png). Like the diagrams, the output is committed and this runs
// locally on demand; it needs librsvg's `rsvg-convert` on PATH
// (`brew install librsvg`). Re-run after editing the SVGs: `npm run icons`.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const root = process.cwd();

const iconSvg = join(root, 'assets', 'icons', 'icon.svg');
const iconSvgOut = join(root, 'static', 'icons', 'icon.svg');
if (!existsSync(iconSvg)) {
  console.error(`gen-icons: missing source ${iconSvg}`);
  process.exit(1);
}
copyFileSync(iconSvg, iconSvgOut);
console.log('gen-icons: icons/icon.svg -> icons/icon.svg');

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

for (const [svg, png, w, h] of targets) {
  const input = join(root, 'assets', svg);
  if (!existsSync(input)) {
    console.error(`gen-icons: missing source ${input}`);
    process.exit(1);
  }
  execFileSync('rsvg-convert', [
    '-w', String(w),
    '-h', String(h),
    input,
    '-o', join(root, 'static', png),
  ]);
  console.log(`gen-icons: ${svg} -> ${png} (${w}×${h})`);
}
