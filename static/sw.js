// Service worker for galacto's PWA: precaches the app shell so it launches
// offline and loads instantly on repeat visits, while always preferring fresh
// content online. BUILD_HASH is stamped per deploy by scripts/cache-bust.mjs, so
// each release gets its own cache and the old one is wiped on activate.
const BUILD_HASH = '__CACHE_BUST__';
const CACHE_NAME = `galacto-${BUILD_HASH}`;

// The shell needed for a cold, offline launch. The glue JS and CSS carry the same
// per-deploy ?v= as index.html references them with; the wasm filename is stable.
const PRECACHE_URLS = [
  '/',
  `/galacto.js?v=${BUILD_HASH}`,
  '/galacto_bg.wasm',
  `/styles.css?v=${BUILD_HASH}`,
  '/favicon.svg',
  '/site.webmanifest',
  '/manifest.json',
  '/icons/icon-192.png',
  '/icons/icon-512.png',
  '/icons/apple-touch-icon.png',
];

self.addEventListener('install', (event) => {
  // Note: no skipWaiting() here — a new worker stays "waiting" so the page can
  // prompt the user before it takes over (see the SKIP_WAITING message below),
  // rather than swapping the running sim out from under them.
  event.waitUntil(
    caches
      .open(CACHE_NAME)
      // Best-effort: a single missing optional URL must not fail the whole install.
      .then((cache) => Promise.all(PRECACHE_URLS.map((u) => cache.add(u).catch(() => {})))),
  );
});

// The page sends this when the user accepts the "update available" prompt: take
// over now, which fires controllerchange in the page and it reloads.
self.addEventListener('message', (event) => {
  if (event.data && event.data.type === 'SKIP_WAITING') {
    self.skipWaiting();
  }
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);

  // Only handle same-origin GETs. Cross-origin (Sentry, Ko-fi, fonts) and
  // non-GET requests pass straight through to the network untouched.
  if (request.method !== 'GET' || url.origin !== self.location.origin) {
    return;
  }

  // Navigations (the HTML shell): network-first so a new deploy is seen
  // immediately, falling back to the cached shell when offline.
  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request)
        .then((response) => {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(request, clone));
          return response;
        })
        .catch(() => caches.match(request).then((c) => c || caches.match('/'))),
    );
    return;
  }

  // Static assets (wasm, glue, css, icons): stale-while-revalidate — serve the
  // cached copy at once, refresh it in the background for next time.
  event.respondWith(
    caches.match(request).then((cached) => {
      const network = fetch(request)
        .then((response) => {
          if (response.ok) {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(request, clone));
          }
          return response;
        })
        .catch(() => cached);
      return cached || network;
    }),
  );
});
