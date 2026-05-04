/// <reference types="@sveltejs/kit" />
/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />

// SvelteKit's $service-worker module gives us:
// - build:   names of files in the build chunk graph (content-hashed)
// - files:   names of files in /static
// - version: a unique build identifier (we use it as the cache key)
import { build, files, version } from '$service-worker';

declare const self: ServiceWorkerGlobalScope;

const CACHE = `agentum-${version}`;
const PRECACHE: string[] = [...build, ...files];

self.addEventListener('install', (e: ExtendableEvent) => {
  e.waitUntil(
    caches.open(CACHE).then((cache) => cache.addAll(PRECACHE))
  );
  self.skipWaiting();
});

self.addEventListener('activate', (e: ExtendableEvent) => {
  e.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)));
      await self.clients.claim();
    })()
  );
});

self.addEventListener('fetch', (e: FetchEvent) => {
  const req = e.request;
  if (req.method !== 'GET') return;

  const url = new URL(req.url);

  // Same-origin only.
  if (url.origin !== self.location.origin) return;

  // Never cache API or WebSocket. Let those go straight to network.
  if (url.pathname.startsWith('/api/')) return;

  e.respondWith(
    (async () => {
      const cache = await caches.open(CACHE);

      // Pre-cached build artifact: cache-first.
      if (PRECACHE.includes(url.pathname) || PRECACHE.includes(url.pathname.slice(1))) {
        const hit = await cache.match(req);
        if (hit) return hit;
      }

      // Network-first for everything else (e.g. /sessions/<id> SPA fallback),
      // falling back to the cached SPA shell ('/') when offline.
      try {
        const fresh = await fetch(req);
        if (fresh.ok && fresh.type === 'basic') {
          cache.put(req, fresh.clone());
        }
        return fresh;
      } catch {
        const hit = await cache.match(req);
        if (hit) return hit;
        const shell = await cache.match('/');
        if (shell) return shell;
        return new Response('offline and uncached', {
          status: 503,
          headers: { 'content-type': 'text/plain' }
        });
      }
    })()
  );
});
