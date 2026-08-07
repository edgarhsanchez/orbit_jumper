// Orbit Jumper service worker: network-first with cache fallback.
// Online players always get the freshest build; offline (or flaky
// signal) falls back to the last one that loaded. Installability is
// the other half: Chrome requires a fetch-handling worker before it
// offers "Add to Home Screen".
const CACHE = 'orbit-jumper-v1';

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));

self.addEventListener('fetch', (e) => {
  if (e.request.method !== 'GET') return;
  e.respondWith(
    fetch(e.request)
      .then((resp) => {
        if (resp.ok) {
          const copy = resp.clone();
          caches.open(CACHE).then((c) => c.put(e.request, copy)).catch(() => {});
        }
        return resp;
      })
      .catch(() => caches.match(e.request).then((hit) => hit || Response.error()))
  );
});
