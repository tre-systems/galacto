#!/usr/bin/env node
import { createReadStream } from 'node:fs';
import { access, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { take, hasFlag } from './cli.mjs';

const TYPES = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.png', 'image/png'],
  ['.svg', 'image/svg+xml'],
  ['.txt', 'text/plain; charset=utf-8'],
  ['.wasm', 'application/wasm'],
  ['.webmanifest', 'application/manifest+json; charset=utf-8'],
  ['.xml', 'application/xml; charset=utf-8'],
]);

export async function startStaticServer({ dir = 'dist', host = '127.0.0.1', port = 8000, cors = false } = {}) {
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    throw new Error(`serve-static: invalid port ${port}`);
  }
  const root = resolve(process.cwd(), dir);

  const server = createServer(async (req, res) => {
    if (cors) res.setHeader('Access-Control-Allow-Origin', '*');
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { Allow: 'GET, HEAD' });
      res.end('Method Not Allowed');
      return;
    }

    try {
      const file = await resolveRequest(root, req.url || '/');
      const type = TYPES.get(extname(file)) || 'application/octet-stream';
      res.setHeader('Content-Type', type);
      res.setHeader('Cache-Control', 'no-cache');
      if (req.method === 'HEAD') {
        res.writeHead(200);
        res.end();
        return;
      }
      createReadStream(file)
        .on('error', () => {
          res.writeHead(500);
          res.end('Internal Server Error');
        })
        .pipe(res);
    } catch (error) {
      const status = error.code === 'ENOENT' ? 404 : 400;
      res.writeHead(status, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end(status === 404 ? 'Not Found' : 'Bad Request');
    }
  });

  await new Promise((resolveListen, rejectListen) => {
    const onError = (error) => rejectListen(describeListenError(error, host, port));
    server.once('error', onError);
    server.listen(port, host, () => {
      server.off('error', onError);
      resolveListen();
    });
  });
  const address = server.address();
  return { server, root, url: `http://${address.address}:${address.port}/` };
}

function describeListenError(error, host, port) {
  if (error.code === 'EADDRINUSE') {
    return Object.assign(
      new Error(
        `serve-static: ${host}:${port} is already in use; stop that process or pass a different --port`,
      ),
      { code: error.code, cause: error },
    );
  }
  return error;
}

async function resolveRequest(root, rawUrl) {
  const url = new URL(rawUrl, 'http://local.invalid');
  let pathname = decodeURIComponent(url.pathname);
  if (pathname.endsWith('/')) pathname += 'index.html';

  const candidate = resolve(root, pathname.replace(/^\/+/, ''));
  if (candidate !== root && !candidate.startsWith(root + sep)) {
    throw Object.assign(new Error('Path traversal rejected'), { code: 'BAD_PATH' });
  }

  let info;
  try {
    info = await stat(candidate);
  } catch (error) {
    if (error.code === 'ENOENT' && !extname(candidate)) {
      const htmlCandidate = `${candidate}.html`;
      if (htmlCandidate.startsWith(root + sep)) {
        const html = await stat(htmlCandidate).catch(() => null);
        if (html?.isFile()) return htmlCandidate;
      }
    }
    throw error;
  }
  if (info.isDirectory()) {
    const index = join(candidate, 'index.html');
    await access(index);
    return index;
  }
  if (!info.isFile()) throw Object.assign(new Error('Not a file'), { code: 'ENOENT' });
  return candidate;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dir = take('--dir', process.argv[2] && !process.argv[2].startsWith('--') ? process.argv[2] : 'dist');
  const port = Number(take('--port', '8000'));
  const host = take('--host', '127.0.0.1');
  const cors = hasFlag('--cors');
  startStaticServer({ dir, port, host, cors })
    .then(({ url, root }) => {
      console.log(`serve-static: ${root}`);
      console.log(`serve-static: ${url}`);
    })
    .catch((error) => {
      console.error(error.message || error);
      process.exit(1);
    });
}
