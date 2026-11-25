// CERQ service worker: installable PWA, offline fallback for the app shell
// only. API calls and pages' data are never cached.
const CACHE = 'cerq-shell-v1';
self.addEventListener('install', (e) => { self.skipWaiting(); e.waitUntil(caches.open(CACHE).then((c) => c.addAll(['/']))); });
self.addEventListener('activate', (e) => { e.waitUntil(caches.keys().then((ks) => Promise.all(ks.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim())); });
self.addEventListener('fetch', (e) => {
  const u = new URL(e.request.url);
  if (e.request.method !== 'GET' || u.origin !== location.origin || u.pathname.startsWith('/api/')) return;
  if (u.pathname.startsWith('/assets/')) {
    e.respondWith(caches.open(CACHE).then(async (c) => (await c.match(e.request)) || fetch(e.request).then((r) => { if (r.ok) c.put(e.request, r.clone()); return r; })));
    return;
  }
  if (e.request.mode === 'navigate') e.respondWith(fetch(e.request).catch(() => caches.match('/')));
});

// Web Push (mail, brief, plan, leave-by). Every push shows a notification:
// Safari revokes subscriptions that push silently.
self.addEventListener('push', (e) => {
  let m = {};
  try { m = e.data ? e.data.json() : {}; } catch { m = { body: e.data && e.data.text() }; }
  e.waitUntil(self.registration.showNotification(m.title || 'CERQ', {
    body: m.body || '', tag: m.tag || 'cerq', icon: '/icon-192.png', badge: '/favicon-32.png', data: { url: m.url || '/' },
  }));
});

// A tap opens its page: in an open CERQ window if there is one.
self.addEventListener('notificationclick', (e) => {
  e.notification.close();
  const url = new URL((e.notification.data && e.notification.data.url) || '/', self.location.origin);
  if (url.origin !== self.location.origin) return;
  e.waitUntil((async () => {
    const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const win = wins.find((w) => new URL(w.url).origin === url.origin);
    if (win) { win.postMessage({ type: 'cerq:open', url: url.href }); return win.focus(); }
    return self.clients.openWindow(url.href);
  })());
});
