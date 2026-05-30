// offscreen.js
// This document exists solely to keep the service worker alive.
// It opens a port to the background worker. As long as this port is open,
// the service worker will not be suspended.

let keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });

keepAlivePort.onDisconnect.addListener(() => {
  // If the background worker disconnects for some reason, try to reconnect
  setTimeout(() => {
    keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });
  }, 1000);
});
