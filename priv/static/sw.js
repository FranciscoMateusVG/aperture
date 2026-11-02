// Aperture service worker: installability, and a page that says the hub is
// down (with a retry) when a navigation gets no answer. Everything else goes
// straight to the network, uncached: the page is live data.
const CACHE = "aperture-v1";
const OFFLINE = "/offline.html";
const ASSETS = [OFFLINE, "/icons/aperture.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((c) => c.addAll(ASSETS)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.mode === "navigate") {
    event.respondWith(fetch(req).catch(() => caches.match(OFFLINE)));
  } else if (ASSETS.includes(new URL(req.url).pathname)) {
    event.respondWith(fetch(req).catch(() => caches.match(req)));
  }
});

// a notification click brings the app forward on that agent's chat
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const agent = event.notification.data && event.notification.data.agent;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((wins) => {
      const win = wins[0];
      if (win) {
        win.postMessage({ type: "open-chat", agent });
        return win.focus();
      }
      return self.clients.openWindow(agent ? `/?chat=${encodeURIComponent(agent)}` : "/");
    })
  );
});
