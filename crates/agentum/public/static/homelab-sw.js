// The old service worker's address (Odysseus days). Browsers that still
// check it get the new worker, so existing push subscriptions keep working.
importScripts('/sw.js');
