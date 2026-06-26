#!/usr/bin/env node
// Build artifact assembly: keep `pkg/` as raw wasm-pack output and create a
// clean deploy root (`dist/`) containing only the files Cloudflare Pages serves.
import { cpSync, existsSync, mkdirSync, rmSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const root = process.cwd();
const outDir = resolve(root, process.argv[2] || 'dist');
const staticDir = join(root, 'static');
const pkgDir = join(root, 'pkg');

const wasmFiles = ['galacto.js', 'galacto_bg.wasm'];

function requireFile(path) {
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new Error(`assemble-dist: missing required file ${path}`);
  }
}

if (!existsSync(staticDir) || !statSync(staticDir).isDirectory()) {
  throw new Error(`assemble-dist: missing static directory ${staticDir}`);
}

for (const file of wasmFiles) {
  requireFile(join(pkgDir, file));
}

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });
cpSync(staticDir, outDir, { recursive: true });

for (const file of wasmFiles) {
  cpSync(join(pkgDir, file), join(outDir, file));
}

console.log(`assemble-dist: wrote ${outDir}`);
