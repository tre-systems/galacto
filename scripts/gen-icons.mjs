#!/usr/bin/env node
// Rasterise the PWA icon sources (assets/icons/*.svg) into the PNGs the manifest
// references (static/icons/*.png). Like the diagrams, the output is committed and
// this runs locally on demand; it needs librsvg's `rsvg-convert` on PATH
// (`brew install librsvg`). Re-run after editing the icon SVGs: `npm run icons`.
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

const src = join(process.cwd(), 'assets', 'icons');
const out = join(process.cwd(), 'static', 'icons');

// [source svg, output png, pixel size]
const targets = [
  ['icon.svg', 'icon-192.png', 192],
  ['icon.svg', 'icon-512.png', 512],
  ['icon.svg', 'apple-touch-icon.png', 180],
  ['icon-maskable.svg', 'icon-maskable-512.png', 512],
];

try {
  execFileSync('rsvg-convert', ['--version'], { stdio: 'ignore' });
} catch {
  console.error('gen-icons: rsvg-convert not found. Install librsvg (brew install librsvg).');
  process.exit(1);
}

for (const [svg, png, size] of targets) {
  const input = join(src, svg);
  if (!existsSync(input)) {
    console.error(`gen-icons: missing source ${input}`);
    process.exit(1);
  }
  execFileSync('rsvg-convert', [
    '-w', String(size),
    '-h', String(size),
    input,
    '-o', join(out, png),
  ]);
  console.log(`gen-icons: ${svg} -> icons/${png} (${size}×${size})`);
}
