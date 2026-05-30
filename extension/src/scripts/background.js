// scripts/background.js
if (typeof importScripts === "function") {
  importScripts("classifier.js");
}

const MEDIA_MENU_ROOT = "tur-detected-media-root";
const MEDIA_MENU_EMPTY = "tur-detected-media-empty";
const MEDIA_MENU_PREFIX = "tur-detected-media-";
const MAX_MEDIA_PER_TAB = 48;
const MAX_CONTEXT_ITEMS = 16;

/**
 * Canonical storage key for drag-offset persistence.
 * Strips volatile query params (timestamp, playlist, tracking) so the same
 * video always maps to the same key regardless of how the URL was reached.
 * Prevents HUD position resets when YouTube/Imgur append ?t=, &list=, etc.
 */
function canonicalMediaKey(rawUrl) {
  try {
    const u = new URL(rawUrl);
    // YouTube: only the video ID is stable
    const ytId = u.searchParams.get("v");
    if ((u.hostname === "www.youtube.com" || u.hostname === "youtube.com") && ytId) {
      return `yt:${ytId}`;
    }
    // Vimeo: path uniquely identifies the video
    if (u.hostname === "vimeo.com" || u.hostname === "player.vimeo.com") {
      return `vimeo:${u.pathname}`;
    }
    // Generic: strip well-known volatile/tracking params
    const STRIP = ["t","list","feature","pp","start","index","si","cb",
                   "utm_source","utm_medium","utm_campaign","ref","_","ts","cache"];
    STRIP.forEach(k => u.searchParams.delete(k));
    const qs = u.searchParams.toString();
    return `${u.origin}${u.pathname}${qs ? "?" + qs : ""}`;
  } catch (_) {
    return rawUrl;
  }
}

let nativePort = null;
const tabMedia = new Map();
const tabTargets = new Map();
const dynamicMenuItems = new Map();
let menuGeneration = 0;
let focusedBrowserWindowId = chrome.windows.WINDOW_ID_NONE;
const resolvedManifestsCache = new Map();
const resolvingManifests = new Set();

const extensionSessionStorage = chrome.storage?.session ?? chrome.storage?.local;
const actionApi = chrome.action ?? chrome.browserAction;

function getMediaType(url) {
  if (!url) return null;
  const path = url.toLowerCase().split('?')[0];
  if (path.endsWith('.mpd')) return 'dash';
  if (path.endsWith('.m3u8')) return 'hls';
  return null;
}

function getCachedManifest(url) {
  const cached = resolvedManifestsCache.get(url);
  if (cached) {
    if (Date.now() - cached.timestamp < 300000) {
      return cached.formats;
    } else {
      resolvedManifestsCache.delete(url);
    }
  }
  return null;
}

const resolvedDirectFiles = new Map();
const resolvingDirectFiles = new Set();

function getCachedDirectFile(url) {
  const cached = resolvedDirectFiles.get(url);
  if (cached) {
    if (Date.now() - cached.timestamp < 300000) {
      return cached.formats;
    } else {
      resolvedDirectFiles.delete(url);
    }
  }
  return null;
}

function resolveDirectFile(tabId, url, width, height, duration) {
  if (resolvingDirectFiles.has(url)) return;
  resolvingDirectFiles.add(url);

  console.log(`[Background] Starting HEAD request to resolve size for direct file: url=${url}`);
  fetch(url, { method: "HEAD", credentials: "omit" })
    .then(response => {
      const contentLength = response.headers.get("content-length");
      const sizeBytes = contentLength ? parseInt(contentLength, 10) : 0;
      console.log(`[Background] HEAD request success: url=${url}, sizeBytes=${sizeBytes}`);
      handleDirectFileResolved(url, width, height, duration, sizeBytes, true);
    })
    .catch(error => {
      console.warn(`[Background] HEAD request failed: url=${url}. Error:`, error);
      handleDirectFileResolved(url, width, height, duration, 0, false);
    });
}

function handleDirectFileResolved(url, width, height, duration, sizeBytes, success) {
  resolvingDirectFiles.delete(url);
  
  // Format the label
  const cleanUrl = url.split(/[?#]/, 1)[0].toLowerCase();
  const ext = isDirectMediaFile(url) ? (cleanUrl.split(".").pop().toUpperCase()) : "VIDEO";
  const label = makeJSLabel(width, height, duration, 0, ext, sizeBytes, 0);
  const formats = [{
    label,
    videoUrl: url,
    audioUrl: "",
    resolution: width > 0 && height > 0 ? `${width}x${height}` : ""
  }];

  resolvedDirectFiles.set(url, {
    formats,
    timestamp: Date.now()
  });

  // Find target in tabTargets and update
  for (const [tId, outgoing] of tabTargets.entries()) {
    let changed = false;
    if (outgoing && outgoing.targets) {
      for (const target of outgoing.targets) {
        if (target.mediaUrl === url) {
          target.formats = formats;
          target.status = "ready";
          changed = true;
          console.log(`[Background] Hydrated target ${target.elementId} with direct file metadata: ${label}`);
        }
      }
    }
    if (changed) {
      sendToHost(outgoing);
    }
  }
}

function resolveManifest(tabId, url, mediaType, duration) {
  if (resolvingManifests.has(url)) return;
  resolvingManifests.add(url);

  if (typeof chrome.offscreen === "undefined") {
    // Firefox fallback: fetch and parse directly in background script
    console.log(`[Background] Firefox fallback manifest fetch: url=${url}`);
    fetch(url, { credentials: "include" })
      .then(response => response.text())
      .then(text => {
        let formats = [];
        if (mediaType === "dash") {
          formats = parseDASH(text, url, duration);
        } else if (mediaType === "hls") {
          formats = parseHLS(text, url);
        }
        console.log(`[Background] Firefox fallback parse success: url=${url}, formatsFound=${formats.length}`);
        handleManifestParsed(url, formats, true);
      })
      .catch(error => {
        console.error("[Background] Fetch/Parse manifest failed:", error);
        handleManifestParsed(url, [], false);
      });
  } else {
    setupOffscreenDocument().then(ready => {
      if (ready) {
        console.log(`[Background] Sending PARSE_MANIFEST to offscreen document: url=${url}`);
        chrome.runtime.sendMessage({
          type: "PARSE_MANIFEST",
          url,
          mediaType,
          duration
        });
      } else {
        console.warn(`[Background] Offscreen document not ready, failing manifest resolution: url=${url}`);
        handleManifestParsed(url, [], false);
      }
    });
  }
}

function handleManifestParsed(url, formats, success) {
  resolvingManifests.delete(url);
  if (success && formats && formats.length > 0) {
    resolvedManifestsCache.set(url, {
      formats,
      timestamp: Date.now()
    });
  }
  
  for (const [tId, outgoing] of tabTargets.entries()) {
    let changed = false;
    if (outgoing && outgoing.targets) {
      for (const target of outgoing.targets) {
        if (target.mediaUrl === url) {
          if (success && formats && formats.length > 0) {
            // Enrich HLS/DASH formats lacking resolution metadata using target element DOM width/height
            target.formats = formats.map(f => {
              if ((!f.resolution || f.resolution === "0x0" || f.label.includes("Direct")) && target.videoWidth > 0 && target.videoHeight > 0) {
                const enrichedLabel = makeJSLabel(target.videoWidth, target.videoHeight, target.duration || 0, 0, "HLS", 0, 0);
                console.log(`[Background] Enriched naked manifest label with DOM resolution: ${target.videoWidth}x${target.videoHeight}`);
                return {
                  ...f,
                  label: enrichedLabel,
                  resolution: `${target.videoWidth}x${target.videoHeight}`
                };
              }
              return f;
            });
            target.status = "ready";
            console.log(`[Background] Manifest parsed successfully for url=${url}, setting status=ready`);
          } else {
            target.formats = [];
            target.status = "pending"; // Fall back to host yt-dlp!
            console.log(`[Background] Manifest parsing failed for url=${url}, setting status=pending for host fallback`);
          }
          changed = true;
        }
      }
    }
    if (changed) {
      sendToHost(outgoing);
    }
  }
}

function isWalledGarden(url) {
  if (!url) return false;
  const u = url.toLowerCase();
  return u.includes("youtube.com") || u.includes("youtu.be") || u.includes("vimeo.com") || u.includes("twitch.tv");
}

// Direct media files that need yt-dlp to extract real resolution/codec/size metadata.
// Without yt-dlp these would only get a generic "Download Stream (Default)" menu entry.
const DIRECT_MEDIA_EXTS = new Set([
  "mp4", "m4v", "mkv", "webm", "avi", "mov", "flv", "wmv", "mpeg", "mpg",
  "ts", "m2ts", "vob", "ogv", "3gp", "3g2", "divx", "f4v", "rm", "rmvb",
  "mp3", "m4a", "aac", "ogg", "opus", "flac", "wav", "wma", "oga",
]);

function isDirectMediaFile(url) {
  if (!url) return false;
  try {
    const path = new URL(url).pathname.toLowerCase();
    const ext = path.split(".").pop();
    return DIRECT_MEDIA_EXTS.has(ext);
  } catch (_) {
    const path = url.split("?")[0].toLowerCase();
    const ext = path.split(".").pop();
    return DIRECT_MEDIA_EXTS.has(ext);
  }
}

let offscreenPromise = null;
async function setupOffscreenDocument() {
  if (offscreenPromise) return offscreenPromise;
  offscreenPromise = (async () => {
    try {
      if (typeof chrome.offscreen === "undefined") {
        console.log("[Background] Offscreen API not supported on this browser.");
        return false;
      }
      if (await chrome.offscreen.hasDocument()) return true;
      console.log("[Background] Creating offscreen document...");
      await chrome.offscreen.createDocument({
        url: "offscreen.html",
        reasons: ["DOM_PARSER"],
        justification: "Keep Native Messaging available for tur",
      });
      console.log("[Background] Offscreen document created.");
      return true;
    } catch (err) {
      console.warn("[Background] Failed to set up offscreen document:", err);
      offscreenPromise = null;
      return false;
    }
  })();
  return offscreenPromise;
}

chrome.runtime.onStartup.addListener(setupOffscreenDocument);
chrome.runtime.onInstalled.addListener(setupOffscreenDocument);
setupOffscreenDocument();
initializeOverlayVisibilityState();

// When the MV3 worker wakes up after suspension, tabTargets is an empty Map
// and the native host has exited (stdin EOF). Ping all active tabs so they
// immediately resend their geometry without waiting for the next scroll/rAF.
(async () => {
  try {
    const activeTabs = await chrome.tabs.query({ active: true });
    for (const tab of activeTabs) {
      chrome.tabs.sendMessage(tab.id, { type: "TUR_REQUEST_TARGETS" }).catch(() => {});
    }
  } catch (_) {}
})();


// ── Programmatic injection: re-inject content script into existing tabs ──
// Chromium only injects content_scripts on navigation. When the extension
// is reloaded/updated, open tabs are left blind. This bridges the gap.
chrome.runtime.onInstalled.addListener(async () => {
  console.log("[Background] onInstalled — injecting content scripts into open tabs");
  try {
    const tabs = await chrome.tabs.query({ url: ["http://*/*", "https://*/*"] });
    
    // Ping each tab first; skip tabs that already have the content script running
    const pingResults = await Promise.allSettled(
      tabs.map((t) =>
        chrome.tabs.sendMessage(t.id, { type: "TUR_PING" }).catch(() => null)
      )
    );

    const toInject = tabs.filter((_, i) => {
      const r = pingResults[i];
      return !(r.status === "fulfilled" && r.value?.ok);
    });

    if (toInject.length === 0) {
      console.log("[Background] all", tabs.length, "tabs already have content script");
      return;
    }

    console.log("[Background] injecting into", toInject.length, "of", tabs.length, "tabs");

    await Promise.allSettled(
      toInject.map(async (tab) => {
        try {
          await chrome.scripting.executeScript({
            target: { tabId: tab.id, allFrames: true },
            files: ["scripts/classifier.js", "scripts/content.js"],
            injectImmediately: false,
            world: "ISOLATED",
          });
          await chrome.scripting.executeScript({
            target: { tabId: tab.id, allFrames: true },
            files: ["scripts/inject.js"],
            injectImmediately: true,
            world: "MAIN",
          });
        } catch (tabErr) {
          console.debug("[Background] inject skip tab", tab.id, tabErr);
        }
      })
    );

    console.log("[Background] injection complete");
  } catch (err) {
    console.warn("[Background] programmatic injection failed", err);
  }
});

chrome.runtime.onConnect.addListener((port) => {
  if (port.name === "keepAlive") {
    console.log("[Background] Keep-alive port connected from offscreen document.");
  }
});

chrome.tabs.onActivated.addListener(() => {
  refreshOverlayVisibility().catch((error) => {
    console.warn("[Background] Failed to refresh overlay on tab activation.", error);
  });
});

chrome.tabs.onRemoved.addListener((tabId) => {
  tabTargets.delete(tabId);
  tabMedia.delete(tabId);
  hideOverlayForTab(tabId);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status === "complete" && tab.active) {
    console.log(`[Background] Tab updated (complete): tabId=${tabId}, url=${tab.url}`);
    chrome.tabs.sendMessage(tabId, { type: "TUR_REQUEST_TARGETS" }).catch(() => {});
  }
});

let focusTimeout = null;

chrome.windows.onFocusChanged.addListener((windowId) => {
  if (focusTimeout) {
    clearTimeout(focusTimeout);
    focusTimeout = null;
  }

  if (windowId === chrome.windows.WINDOW_ID_NONE) {
    // Delay handling focus loss to WINDOW_ID_NONE in case it's a temporary
    // shift to a native popup/context menu (prevents overlay from disappearing).
    focusTimeout = setTimeout(() => {
      focusedBrowserWindowId = windowId;
      refreshOverlayVisibility().catch((error) => {
        console.warn("[Background] Failed to refresh overlay on window focus change.", error);
      });
      focusTimeout = null;
    }, 300);
  } else {
    focusedBrowserWindowId = windowId;
    refreshOverlayVisibility().catch((error) => {
      console.warn("[Background] Failed to refresh overlay on window focus change.", error);
    });
  }
});

// ── Native Messaging reconnect manager ───────────────────────────────────────
// MV3 service workers suspend after ~30 s of idleness, tearing down nativePort.
// Strategy: treat the port as ephemeral. Buffer any payload sent while the port
// is null and flush immediately once the port reconnects (which is synchronous).
// Exponential backoff caps at 3 retries to avoid hammering a missing host binary.

const _nativePending = [];        // payloads queued while port is null
let   _reconnectAttempts = 0;
let   _reconnectTimer    = null;

function connectNative() {
  if (nativePort) return;
  if (_reconnectTimer) return;   // backoff already scheduled

  try {
    nativePort = chrome.runtime.connectNative("com.tur.native_host");
    _reconnectAttempts = 0;      // reset on success

    nativePort.onMessage.addListener(async (msg) => {
      if (msg && msg.type === "OVERLAY_DOWNLOAD_TRIGGER") {
        console.log("[Background] Overlay download trigger received:", msg);
        const tabId = msg.tabId;
        try {
          const tab = await chrome.tabs.get(tabId);
          sendToHost({
            action: "QUEUE_DOWNLOAD",
            payload: {
              url: msg.videoUrl,
              audio_url: msg.audioUrl,
              page_url: tab.url,
              page_title: tab.title || "",
              referer: msg.headers?.Referer || tab.url,
              user_agent: msg.headers?.["User-Agent"] || navigator.userAgent,
              cookie: msg.headers?.Cookie || "",
              mediaType: msg.audioUrl ? "dash" : "direct",
              category: "video",
              label: `Downloaded via overlay`,
            }
          });
        } catch (e) {
          console.warn("[Background] Failed to process overlay download trigger:", e);
        }
      } else if (msg && msg.type === "OVERLAY_MENU_SELECTED") {
        console.log("[Background] Quality menu selection:", msg);
        const tabId = msg.tabId;
        try {
          const tab = await chrome.tabs.get(tabId);
          const mediaList = tabMedia.get(tabId) || [];
          let selectedItem = null;
          if (msg.mediaUrl && !msg.mediaUrl.startsWith("blob:") && !msg.mediaUrl.startsWith("data:")) {
            selectedItem = mediaList.find(item => item.url === msg.mediaUrl);
          }
          if (!selectedItem) {
            selectedItem = mediaList.find(item => item.playable) || mediaList[0];
          }
          if (selectedItem) {
            queueMediaItem({
              ...selectedItem,
              label: msg.quality ? `${selectedItem.label || shortName(selectedItem.url)} (${msg.quality})` : selectedItem.label
            }, tab, { pageUrl: tab.url });
          } else {
            queuePage(tab, { pageUrl: tab.url });
          }
        } catch (e) {
          console.warn("[Background] Failed to handle overlay menu selection:", e);
        }
      } else if (msg && msg.type === "OVERLAY_COPY_URL") {
        console.log("[Background] Copy URL selection:", msg);
        const tabId = msg.tabId;
        try {
          const mediaList = tabMedia.get(tabId) || [];
          let selectedItem = null;
          if (msg.mediaUrl && !msg.mediaUrl.startsWith("blob:") && !msg.mediaUrl.startsWith("data:")) {
            selectedItem = mediaList.find(item => item.url === msg.mediaUrl);
          }
          if (!selectedItem) {
            selectedItem = mediaList.find(item => item.playable) || mediaList[0];
          }
          const textToCopy = selectedItem ? selectedItem.url : (msg.mediaUrl || "");
          if (textToCopy) {
            chrome.tabs.sendMessage(tabId, { type: "TUR_COPY_TO_CLIPBOARD", text: textToCopy }).catch(() => {});
          }
        } catch (e) {
          console.warn("[Background] Failed to handle overlay copy URL:", e);
        }
      } else if (msg && msg.type === "OVERLAY_DRAG_MOVED") {
        // Persist the new HUD drag offset so it survives page reloads.
        // Key is canonicalized to survive query-param churn on YouTube/Imgur/Reddit.
        if (msg.mediaUrl) {
          let keyUrl = msg.mediaUrl;
          if (!keyUrl || keyUrl.startsWith("blob:") || keyUrl.startsWith("data:")) {
            try {
              const tab = await chrome.tabs.get(msg.tabId);
              if (tab && tab.url) {
                keyUrl = tab.url;
              }
            } catch (_) {}
          }
          const key = `drag_${canonicalMediaKey(keyUrl)}`;
          try {
            await chrome.storage.local.set({ [key]: { dx: msg.dx || 0, dy: msg.dy || 0 } });
            console.log("[Background] Drag offset persisted", key, msg.dx, msg.dy);
          } catch (e) {
            console.warn("[Background] Failed to persist drag offset", e);
          }
        }
      } else if (msg && msg.type === "OVERLAY_DISMISS") {
        console.log("[Background] Overlay dismissed for element", msg.elementId);
      } else {
        console.log("[Background] Received from TUR native host:", msg);
      }
    });

    nativePort.onDisconnect.addListener(() => {
      const err = chrome.runtime.lastError;
      console.log("[Background] Disconnected from TUR native host.", err);
      nativePort = null;

      // If we still have pending messages, schedule a reconnect with backoff.
      if (_nativePending.length > 0) {
        const delay = Math.min(500 * Math.pow(2, _reconnectAttempts), 4000);
        _reconnectAttempts = Math.min(_reconnectAttempts + 1, 3);
        console.log(`[Background] Reconnect in ${delay} ms (attempt ${_reconnectAttempts})`);
        _reconnectTimer = setTimeout(() => {
          _reconnectTimer = null;
          connectNative();
          _flushPending();
        }, delay);
      }
    });

    console.log("[Background] Connected to TUR native host.");

    // Flush any payloads that arrived while the port was null.
    _flushPending();
  } catch (err) {
    console.error("[Background] Failed to connect native messaging.", err);
    nativePort = null;
  }
}

function _flushPending() {
  while (_nativePending.length > 0 && nativePort) {
    const payload = _nativePending.shift();
    try {
      nativePort.postMessage(payload);
    } catch (e) {
      console.warn("[Background] Flush failed, requeueing.", e);
      _nativePending.unshift(payload);
      break;
    }
  }
}

connectNative();

function sendToHost(payload) {
  if (!nativePort) {
    // Buffer and reconnect — the flush will drain this once connected.
    _nativePending.push(payload);
    connectNative();
    console.log("[Background] Port was null — buffered payload, reconnecting.", payload);
    return false;
  }

  try {
    nativePort.postMessage(payload);
    console.log("[Background] Sent payload to TUR native host.", payload);
    return true;
  } catch (e) {
    console.warn("[Background] postMessage failed, buffering.", e);
    nativePort = null;
    _nativePending.push(payload);
    connectNative();
    return false;
  }
}

chrome.runtime.onInstalled.addListener(createContextMenus);
chrome.runtime.onStartup.addListener(createContextMenus);
createContextMenus();

const beforeSendHeadersHandler = function(details) {
  if (details.tabId < 0) return;
  if (!["media", "xmlhttprequest", "object", "other"].includes(details.type)) return;

  const classified = TurDownloadClassifier.classifyDownload(details.url);
  if (!classified.downloadable && !classified.playable) return;

  console.log(`[Background] Intercepted beforeSendHeaders: url=${details.url}, type=${details.type}, tabId=${details.tabId}`);
  const item = mediaItemFromClassification(details.url, classified, details.initiator || details.documentUrl || "");
  rememberTabMedia(details.tabId, item);
  notifyTab(details.tabId, item);

  return { requestHeaders: details.requestHeaders };
};

try {
  chrome.webRequest.onBeforeSendHeaders.addListener(
    beforeSendHeadersHandler,
    { urls: ["<all_urls>"] },
    ["requestHeaders", "extraHeaders"]
  );
} catch (_) {
  chrome.webRequest.onBeforeSendHeaders.addListener(
    beforeSendHeadersHandler,
    { urls: ["<all_urls>"] },
    ["requestHeaders"]
  );
}

const headersReceivedHandler = function(details) {
  if (details.tabId < 0) return;
  const headers = details.responseHeaders || [];
  const contentType = findHeader(headers, "content-type");
  const disposition = findHeader(headers, "content-disposition");
  const classified = TurDownloadClassifier.classifyDownload(details.url, contentType, disposition);
  if (!classified.downloadable && !classified.playable && !classified.attachment) return;

  console.log(`[Background] Intercepted headersReceived: url=${details.url}, contentType=${contentType}, tabId=${details.tabId}`);
  const item = mediaItemFromClassification(details.url, classified, details.initiator || details.documentUrl || "");
  rememberTabMedia(details.tabId, item);
  notifyTab(details.tabId, item);
};

try {
  chrome.webRequest.onHeadersReceived.addListener(
    headersReceivedHandler,
    { urls: ["<all_urls>"] },
    ["responseHeaders", "extraHeaders"]
  );
} catch (_) {
  chrome.webRequest.onHeadersReceived.addListener(
    headersReceivedHandler,
    { urls: ["<all_urls>"] },
    ["responseHeaders"]
  );
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const tabId = sender.tab?.id;

  if (message.type === "MANIFEST_PARSED") {
    const { url, formats, success } = message;
    handleManifestParsed(url, formats, success);
    return true;
  }

  if (message.type === "MEDIA_CANDIDATES" && Number.isInteger(tabId)) {
    rememberTabMediaBatch(tabId, message.payload?.media || []);
    sendResponse?.({ ok: true });
    return true;
  }

  if (message.type === "MEDIA_TARGETS_UPDATE" && Number.isInteger(tabId)) {
    handleTargetsUpdate(tabId, sender.tab, message.payload || {})
      .then((result) => sendResponse?.({ ok: true, result }))
      .catch((error) => {
        console.warn("[Background] Failed to handle targets update.", error);
        sendResponse?.({ ok: false, error: String(error) });
      });
    return true;
  }

    if (message.type === "MEDIA_TARGET_UPDATE" && Number.isInteger(tabId)) {
    handleTargetUpdate(tabId, sender.tab, message.payload || {})
      .then((mappedPayload) => sendResponse?.({ ok: true, mappedPayload }))
      .catch((error) => {
        console.warn("[Background] Failed to map target geometry.", error);
        sendResponse?.({ ok: false, error: String(error) });
      });
    return true;
  }

  if (message.type === "SEND_TO_NATIVE") {
    const payload = message.payload || {};
    const finalMessage = {
      type: payload.type || "QUEUE_DOWNLOAD",
      ...payload
    };
    sendToHost(finalMessage);
    sendResponse?.({ ok: true });
    return true;
  }
});

async function handleTargetUpdate(tabId, tab, targetPayload) {
  if (targetPayload && targetPayload.isTopFrame === false) {
    rememberTabMediaBatch(tabId, targetPayload.media || []);
    return { ok: true, isSubFrame: true };
  }

  const viewportWidth = Number(targetPayload.viewportWidth || 0);
  const viewportHeight = Number(targetPayload.viewportHeight || 0);
  const clientX = Number(targetPayload.clientX || 0);
  const clientY = Number(targetPayload.clientY || 0);
  const width = Number(targetPayload.width || 0);
  const height = Number(targetPayload.height || 0);
  const rawWindowScreenX = Number(targetPayload.rawWindowScreenX || 0);
  const rawWindowScreenY = Number(targetPayload.rawWindowScreenY || 0);
  const outerWidth = Number(targetPayload.outerWidth || 0);
  const outerHeight = Number(targetPayload.outerHeight || 0);
  const reportedViewportScreenX = Number(targetPayload.viewportScreenX || 0);
  const reportedViewportScreenY = Number(targetPayload.viewportScreenY || 0);
  let viewportScreenX = reportedViewportScreenX;
  let viewportScreenY = reportedViewportScreenY;
  let fallbackViewportScreenX = reportedViewportScreenX;
  let fallbackViewportScreenY = reportedViewportScreenY;
  let browserWindowLeft = 0;
  let browserWindowTop = 0;
  let browserWindowWidth = 0;
  let browserWindowHeight = 0;

  if (tab?.windowId) {
    try {
      const win = await chrome.windows.get(tab.windowId);
      browserWindowLeft = Number(win.left || 0);
      browserWindowTop = Number(win.top || 0);
      browserWindowWidth = Number(win.width || 0);
      browserWindowHeight = Number(win.height || 0);
    } catch (_) {}
  }

  // Get zoom factor
  let zoomFactor = 1.0;
  try {
    zoomFactor = await chrome.tabs.getZoom(tabId);
  } catch (_) {}

  // Calculate chrome delta in DIPs (OS units) if win is available, fallback to CSS units
  let deltaX = 0;
  let deltaY = 0;
  if (browserWindowWidth > 0) {
    deltaX = Math.max(0, browserWindowWidth - (viewportWidth * zoomFactor));
    deltaY = Math.max(0, browserWindowHeight - (viewportHeight * zoomFactor));
  } else {
    deltaX = Math.max(0, outerWidth - viewportWidth);
    deltaY = Math.max(0, outerHeight - viewportHeight);
  }

  const ratio = deltaX > 0 ? (deltaY / deltaX) : 999.0;

  let classifiedMode = "unknown";
  if (viewportWidth > 0 && viewportHeight > 0) {
    if (deltaX <= 40) {
      classifiedMode = "likely-top-tabs";
    } else {
      classifiedMode = "likely-vertical-tabs";
    }
  }

  if (browserWindowWidth > 0) {
    if (classifiedMode === "likely-vertical-tabs") {
      const L_chrome = Math.max(0, deltaX - 8);
      const T_chrome = Math.max(0, deltaY - 8);

      viewportScreenX = Math.round(browserWindowLeft + L_chrome);
      viewportScreenY = Math.round(browserWindowTop + T_chrome);
    } else {
      const fallbackBorderX = Math.max(0, Math.round(deltaX / 2));
      const fallbackChromeTop = Math.max(
        0,
        Math.round(deltaY - fallbackBorderX)
      );
      viewportScreenX = Math.round(browserWindowLeft + fallbackBorderX);
      viewportScreenY = Math.round(browserWindowTop + fallbackChromeTop);
    }
  } else {
    if (classifiedMode === "likely-vertical-tabs") {
      viewportScreenX = Math.round(reportedViewportScreenX + (deltaX / 2 - 8));
      viewportScreenY = Math.round(reportedViewportScreenY + (deltaY / 2 - 8));
    } else {
      viewportScreenX = reportedViewportScreenX;
      viewportScreenY = reportedViewportScreenY;
    }
  }

  const payload = {
    type: targetPayload.type || "MEDIA_TARGET_UPDATE",
    pageUrl: targetPayload.pageUrl || tab?.url || "",
    pageTitle: targetPayload.pageTitle || tab?.title || "",
    referer: targetPayload.referer || targetPayload.pageUrl || tab?.url || "",
    userAgent: targetPayload.userAgent || navigator.userAgent,
    media: targetPayload.media || [],
    viewportScreenX,
    viewportScreenY,
    viewportWidth,
    viewportHeight,
    clientX,
    clientY,
    width,
    height,
    screenX: width > 0 ? viewportScreenX + clientX - width : 0,
    screenY: height > 0 ? viewportScreenY + clientY : 0,
    videoWidth: Number(targetPayload.videoWidth || 0),
    videoHeight: Number(targetPayload.videoHeight || 0),
    duration: Number(targetPayload.duration || 0),
    frameUrl: targetPayload.frameUrl || targetPayload.pageUrl || tab?.url || "",
    tabId,
    devicePixelRatio: targetPayload.devicePixelRatio || 1.0,
    rawWindowScreenX,
    rawWindowScreenY,
    outerWidth,
    outerHeight,
    browserWindowLeft,
    browserWindowTop,
    browserWindowWidth,
    browserWindowHeight,
    debugLayoutMode: classifiedMode,
    debugViewportDeltaX: deltaX,
    debugViewportDeltaY: deltaY,
    debugChromeRatio: Number(ratio.toFixed(4)),
  };

  console.log("[Background] Geometry debug", {
    tabId,
    pageUrl: payload.pageUrl,
    debugLayoutMode: payload.debugLayoutMode,
    rawWindowScreenX,
    rawWindowScreenY,
    reportedViewportScreenX,
    reportedViewportScreenY,
    fallbackViewportScreenX,
    fallbackViewportScreenY,
    finalViewportScreenX: viewportScreenX,
    finalViewportScreenY: viewportScreenY,
    viewportWidth,
    viewportHeight,
    outerWidth,
    outerHeight,
    browserWindowLeft,
    browserWindowTop,
    browserWindowWidth,
    browserWindowHeight,
    targetScreenX: width > 0 ? viewportScreenX + clientX - width : 0,
    targetScreenY: height > 0 ? viewportScreenY + clientY : 0,
    targetWidth: width,
    targetHeight: height,
    debugViewportDeltaX: deltaX,
    debugViewportDeltaY: deltaY,
    debugChromeRatio: Number(ratio.toFixed(4)),
  });

  console.log("[tur] handleTargetsUpdate tab=" + tabId + " targets=" + (payload.targets || []).length + " vp=(" + payload.viewport_screen_x + "," + payload.viewport_screen_y + " " + payload.viewport_width + "x" + payload.viewport_height + ")");
  (payload.targets || []).forEach(function(t, i) {
    console.log("[tur]   [" + i + "] id=" + (t.element_id || "?") + " sx=" + t.screen_x + " sy=" + t.screen_y + " w=" + t.width + " h=" + t.height);
  });
  tabTargets.set(tabId, payload);
  rememberTabMediaBatch(tabId, payload.media || []);
  persistTabTarget(tabId, payload);

  if (shouldDisplayOverlayForTab(tab)) {
    sendToHost(payload);
  } else {
    hideOverlayForTab(tabId, payload);
  }
  return payload;
}

async function handleTargetsUpdate(tabId, tab, payload) {
  if (payload && payload.isTopFrame === false) {
    rememberTabMediaBatch(tabId, payload.media || []);
    return { isSubFrame: true };
  }

  // ── Correct viewport screen position using chrome.windows.get() ──
  const viewportWidth = Number(payload.viewportWidth || 0);
  const viewportHeight = Number(payload.viewportHeight || 0);
  let viewportScreenX = Number(payload.viewportScreenX || 0);
  let viewportScreenY = Number(payload.viewportScreenY || 0);

  let browserWindowLeft = 0;
  let browserWindowTop = 0;
  let browserWindowWidth = 0;
  let browserWindowHeight = 0;
  if (tab && tab.windowId) {
    try {
      const win = await chrome.windows.get(tab.windowId);
      browserWindowLeft = Number(win.left || 0);
      browserWindowTop = Number(win.top || 0);
      browserWindowWidth = Number(win.width || 0);
      browserWindowHeight = Number(win.height || 0);
    } catch (_) {}
  }

  let zoomFactor = 1.0;
  try {
    zoomFactor = await chrome.tabs.getZoom(tabId);
  } catch (_) {}

  // Calculate chrome chrome delta in DIPs (OS units)
  let deltaX = 0;
  let deltaY = 0;
  if (browserWindowWidth > 0) {
    deltaX = Math.max(0, browserWindowWidth - (viewportWidth * zoomFactor));
    deltaY = Math.max(0, browserWindowHeight - (viewportHeight * zoomFactor));
  } else {
    const outerWidth = Number(payload.outerWidth || 0);
    const outerHeight = Number(payload.outerHeight || 0);
    deltaX = Math.max(0, outerWidth - viewportWidth);
    deltaY = Math.max(0, outerHeight - viewportHeight);
  }

  let classifiedMode = "unknown";
  if (viewportWidth > 0 && viewportHeight > 0) {
    classifiedMode = deltaX <= 40 ? "likely-top-tabs" : "likely-vertical-tabs";
  }

  if (browserWindowWidth > 0) {
    if (classifiedMode === "likely-vertical-tabs") {
      const L_chrome = Math.max(0, deltaX - 8);
      const T_chrome = Math.max(0, deltaY - 8);
      viewportScreenX = Math.round(browserWindowLeft + L_chrome);
      viewportScreenY = Math.round(browserWindowTop + T_chrome);
    } else {
      const fallbackBorderX = Math.max(0, Math.round(deltaX / 2));
      const fallbackChromeTop = Math.max(0, Math.round(deltaY - fallbackBorderX));
      viewportScreenX = Math.round(browserWindowLeft + fallbackBorderX);
      viewportScreenY = Math.round(browserWindowTop + fallbackChromeTop);
    }
  }

  // ── Build targets with corrected screen coordinates ──
  // Also inject persisted drag offsets (keyed by canonical mediaUrl).
  var rawTargets = (payload.targets || []).map(function(t) {
    var sx = Math.round(viewportScreenX + Number(t.clientX || 0) - Number(t.width || 0));
    var sy = Math.round(viewportScreenY + Number(t.clientY || 0));
    var w = Math.round(Number(t.width || 0));
    var h = Math.round(Number(t.height || 0));
    return {
      elementId: t.elementId || "_unknown_",
      clientX: Math.round(Number(t.clientX || 0)),
      clientY: Math.round(Number(t.clientY || 0)),
      width: w,
      height: h,
      screenX: sx,
      screenY: sy,
      mediaUrl: t.mediaUrl || "",
      duration: t.duration || 0,
      cookie: t.cookie || "",
      videoWidth: t.videoWidth || 0,
      videoHeight: t.videoHeight || 0,
    };
  });

  // Fetch all stored drag offsets for this batch in one round-trip.
  const storageKeys = rawTargets.map(t => `drag_${canonicalMediaKey(t.mediaUrl)}`);
  let storedOffsets = {};
  try {
    storedOffsets = await chrome.storage.local.get(storageKeys);
  } catch (_) {}

  // Get tab's detected media list to resolve blob/empty targets
  const mediaList = tabMedia.get(tabId) || [];
  
  // Sort HLS/DASH streams by URL length ascending to prefer master playlist over variant sub-playlists
  const hlsStreams = mediaList.filter(m => m.mediaType === "hls" || m.mediaType === "dash");
  hlsStreams.sort((a, b) => a.url.length - b.url.length);
  
  const resolvedStream = hlsStreams[0] || mediaList.find(m => m.playable);

  var targets = rawTargets.map(function(t, i) {
    const stored = storedOffsets[storageKeys[i]] || {};
    let status = "pending";
    let formats = [];
    let targetUrl = t.mediaUrl;

    if (!targetUrl || targetUrl.startsWith("blob:") || targetUrl.startsWith("data:")) {
      if (resolvedStream) {
        targetUrl = resolvedStream.url;
      }
    }

    const mediaType = getMediaType(targetUrl);
    if (mediaType) {
      // HLS/DASH: try local manifest parser first, fall back to host yt-dlp via pending
      const cached = getCachedManifest(targetUrl);
      if (cached) {
        status = "ready";
        formats = cached.map(f => {
          if ((!f.resolution || f.resolution === "0x0" || f.label.includes("Direct")) && t.videoWidth > 0 && t.videoHeight > 0) {
            const enrichedLabel = makeJSLabel(t.videoWidth, t.videoHeight, t.duration || 0, 0, "HLS", 0, 0);
            return {
              ...f,
              label: enrichedLabel,
              resolution: `${t.videoWidth}x${t.videoHeight}`
            };
          }
          return f;
        });
        console.log(`[Background] Target ${t.elementId} HLS/DASH cache HIT: url=${targetUrl}`);
      } else {
        status = "resolving";
        console.log(`[Background] Target ${t.elementId} HLS/DASH cache MISS, resolving manifest: url=${targetUrl}`);
        resolveManifest(tabId, targetUrl, mediaType, t.duration || 0);
      }
    } else if (isDirectMediaFile(targetUrl)) {
      const cached = getCachedDirectFile(targetUrl);
      if (cached) {
        status = "ready";
        formats = cached;
        console.log(`[Background] Target ${t.elementId} direct file cache HIT: url=${targetUrl}`);
      } else {
        status = "resolving";
        console.log(`[Background] Target ${t.elementId} direct file cache MISS, resolving HEAD: url=${targetUrl}`);
        resolveDirectFile(tabId, targetUrl, t.videoWidth || 0, t.videoHeight || 0, t.duration || 0);
      }
    } else if (
      !targetUrl ||
      targetUrl.startsWith("blob:") ||
      targetUrl.startsWith("data:") ||
      isWalledGarden(targetUrl) ||
      isWalledGarden(payload.pageUrl || (tab && tab.url))
    ) {
      // All of these need host-side yt-dlp extraction: blob/data, and walled gardens (YouTube/Vimeo/Twitch)
      status = "pending";
      console.log(`[Background] Target ${t.elementId} mapped to pending (walled garden/blob): url=${targetUrl}`);
    } else {
      // Unknown URL with no parseable extension — offer default download
      status = "ready";
      console.log(`[Background] Target ${t.elementId} mapped to ready (fallback direct): url=${targetUrl}`);
      const ext = targetUrl.split(/[?#]/, 1)[0].split(".").pop().toUpperCase() || "DOWNLOAD";
      formats = [{
        label: `Download Stream (${ext})`,
        videoUrl: targetUrl,
        audioUrl: "",
        resolution: ""
      }];
    }

    return {
      ...t,
      mediaUrl: targetUrl,
      dragOffsetX: stored.dx || 0,
      dragOffsetY: stored.dy || 0,
      status,
      formats,
    };
  });

  console.log("[Background] handleTargetsUpdate — corrected viewport", {
    tabId,
    pageUrl: payload.pageUrl || (tab && tab.url),
    debugLayoutMode: classifiedMode,
    viewportBefore: Number(payload.viewportScreenX || 0) + "," + Number(payload.viewportScreenY || 0),
    viewportAfter: viewportScreenX + "," + viewportScreenY,
    browserWindowLeft,
    browserWindowTop,
    browserWindowWidth,
    browserWindowHeight,
    viewportWidth,
    viewportHeight,
    deltaX,
    deltaY,
    targetCount: targets.length,
  });

  // ── Build outgoing message ──
  var outgoing = {
    type: "MEDIA_TARGETS_UPDATE",
    pageUrl: payload.pageUrl || (tab && tab.url) || "",
    pageTitle: payload.pageTitle || (tab && tab.title) || "",
    referer: payload.referer || payload.pageUrl || (tab && tab.url) || "",
    media: payload.media || [],
    viewportScreenX: viewportScreenX,
    viewportScreenY: viewportScreenY,
    viewportWidth: viewportWidth,
    viewportHeight: viewportHeight,
    devicePixelRatio: payload.devicePixelRatio || 1.0,
    targets: targets,
    tabId: tabId,
  };

  tabTargets.set(tabId, outgoing);
  rememberTabMediaBatch(tabId, payload.media || []);

  if (shouldDisplayOverlayForTab(tab)) {
    sendToHost(outgoing);
  } else {
    hideOverlayForTab(tabId, outgoing);
  }
  return outgoing;
}

async function initializeOverlayVisibilityState() {
  try {
    const win = await chrome.windows.getLastFocused();
    focusedBrowserWindowId = Number.isInteger(win?.id) ? win.id : chrome.windows.WINDOW_ID_NONE;
  } catch (_) {
    focusedBrowserWindowId = chrome.windows.WINDOW_ID_NONE;
  }
}

function shouldDisplayOverlayForTab(tab) {
  if (!Number.isInteger(tab?.id) || tab.active !== true) return false;
  // If we don't know the focused window yet (worker just restarted and the
  // async initializeOverlayVisibilityState hasn't resolved), be optimistic:
  // show on any active tab rather than hiding everything.
  if (focusedBrowserWindowId === chrome.windows.WINDOW_ID_NONE) return true;
  return Number.isInteger(tab.windowId) && tab.windowId === focusedBrowserWindowId;
}

function buildHiddenPayload(tabId, prior = {}) {
  return {
    type: "MEDIA_TARGETS_UPDATE",
    targets: prior.targets || [],
    
    pageUrl: prior.pageUrl || "",
    pageTitle: prior.pageTitle || "",
    referer: prior.referer || "",
    userAgent: prior.userAgent || navigator.userAgent,
    media: prior.media || [],
    viewportScreenX: 0,
    viewportScreenY: 0,
    viewportWidth: 0,
    viewportHeight: 0,
    clientX: 0,
    clientY: 0,
    width: 0,
    height: 0,
    screenX: 0,
    screenY: 0,
    videoWidth: 0,
    videoHeight: 0,
    duration: 0,
    frameUrl: prior.frameUrl || prior.pageUrl || "",
    tabId,
  };
}

function hideOverlayForTab(tabId, prior = null) {
  const base = prior || tabTargets.get(tabId) || {};
  sendToHost(buildHiddenPayload(tabId, base));
}

async function refreshOverlayVisibility() {
  // If the focused window isn't known yet (worker just woke up), resolve it
  // first so we don't incorrectly hide every tab.
  if (focusedBrowserWindowId === chrome.windows.WINDOW_ID_NONE) {
    await initializeOverlayVisibilityState();
  }

  if (focusedBrowserWindowId === chrome.windows.WINDOW_ID_NONE) {
    // Still unknown (e.g., no windows open) — nothing to do.
    return;
  }

  const activeTabs = await chrome.tabs.query({
    active: true,
    windowId: focusedBrowserWindowId,
  });
  const activeTab = activeTabs[0];
  const activeTabId = activeTab?.id;

  for (const [tabId, payload] of tabTargets.entries()) {
    if (tabId === activeTabId) {
      sendToHost(payload);
    } else {
      hideOverlayForTab(tabId, payload);
    }
  }
}

chrome.contextMenus.onShown?.addListener((info, tab) => {
  const tabId = tab?.id;
  if (!Number.isInteger(tabId)) return;
  getTabMedia(tabId).then((media) => {
    rebuildDetectedMediaMenu(tabId, media);
    chrome.contextMenus.refresh?.();
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  const tabId = tab?.id;

  if (info.menuItemId === "tur-download-link") {
    queueUrlFromContext(info, tab);
    return;
  }

  if (info.menuItemId === "tur-download-page") {
    queuePage(tab, info);
    return;
  }

  if (typeof info.menuItemId === "string" && info.menuItemId.startsWith(MEDIA_MENU_PREFIX)) {
    const selected = dynamicMenuItems.get(info.menuItemId);
    if (!selected) return;
    queueMediaItem(selected, tab, info);
  }
});

actionApi?.onClicked.addListener((tab) => {
  if (!Number.isInteger(tab?.id)) return;
  getTabMedia(tab.id).then((media) => {
    const selected = media.find((item) => item.playable) || media[0];
    if (selected) {
      queueMediaItem(selected, tab, { pageUrl: tab.url });
    } else {
      queuePage(tab, { pageUrl: tab.url });
    }
  });
});

function createContextMenus() {
  if (!chrome.contextMenus) return;
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "tur-download-link",
      title: "Download with Tur",
      contexts: ["link", "video", "audio", "image"],
    });
    chrome.contextMenus.create({
      id: "tur-download-page",
      title: "Resolve page with Tur",
      contexts: ["page", "frame"],
    });
    chrome.contextMenus.create({
      id: MEDIA_MENU_ROOT,
      title: "Download detected media with Tur",
      contexts: ["page", "frame", "video", "audio"],
    });
    chrome.contextMenus.create({
      id: MEDIA_MENU_EMPTY,
      parentId: MEDIA_MENU_ROOT,
      title: "No detected media yet",
      contexts: ["page", "frame", "video", "audio"],
      enabled: false,
    });
  });
}

function rebuildDetectedMediaMenu(tabId, media) {
  menuGeneration += 1;

  for (const id of dynamicMenuItems.keys()) {
    chrome.contextMenus.remove(id, () => void chrome.runtime.lastError);
  }
  dynamicMenuItems.clear();

  const visible = media
    .filter((item) => item.url && !item.url.startsWith("blob:") && !item.url.startsWith("data:"))
    .slice(0, MAX_CONTEXT_ITEMS);

  if (visible.length === 0) {
    chrome.contextMenus.update(MEDIA_MENU_EMPTY, {
      title: "No detected media yet",
      enabled: false,
      visible: true,
    }, () => void chrome.runtime.lastError);
    return;
  }

  chrome.contextMenus.update(MEDIA_MENU_EMPTY, {
    visible: false,
  }, () => void chrome.runtime.lastError);

  for (const [index, item] of visible.entries()) {
    const id = `${MEDIA_MENU_PREFIX}${menuGeneration}-${index}`;
    dynamicMenuItems.set(id, item);
    chrome.contextMenus.create({
      id,
      parentId: MEDIA_MENU_ROOT,
      title: contextLabel(item),
      contexts: ["page", "frame", "video", "audio"],
    });
  }

  if (tabId >= 0 && visible.length < media.length) {
    const id = `${MEDIA_MENU_PREFIX}${menuGeneration}-more`;
    dynamicMenuItems.set(id, media[0]);
    chrome.contextMenus.create({
      id,
      parentId: MEDIA_MENU_ROOT,
      title: `Show first ${MAX_CONTEXT_ITEMS} of ${media.length} detected items`,
      contexts: ["page", "frame", "video", "audio"],
      enabled: false,
    });
  }
}

function queueUrlFromContext(info, tab) {
  const url = info.linkUrl || info.srcUrl || info.frameUrl || info.pageUrl;
  if (!url) return;

  const classified = TurDownloadClassifier.classifyDownload(url);
  queueMediaItem({
    url,
    mediaType: info.menuItemId === "tur-download-page" ? "page" : classified.mediaType,
    category: classified.category,
    pageUrl: info.pageUrl || tab?.url || url,
    playable: classified.playable,
  }, tab, info);
}

function queuePage(tab, info) {
  const url = info.pageUrl || tab?.url;
  if (!url) return;

  queueMediaItem({
    url,
    mediaType: "page",
    category: "page",
    pageUrl: url,
    playable: true,
    label: "Resolve page with yt-dlp",
  }, tab, info);
}

function queueMediaItem(item, tab, info = {}) {
  const pageUrl = item.pageUrl || item.page_url || info.pageUrl || tab?.url || item.url;
  sendToHost({
    action: "QUEUE_DOWNLOAD",
    payload: {
      url: item.url,
      page_url: pageUrl,
      page_title: tab?.title || "",
      referer: pageUrl,
      user_agent: navigator.userAgent,
      mediaType: item.mediaType || item.media_type || "direct",
      category: item.category || "download",
      label: item.label || "",
    },
  });
}

function mediaItemFromClassification(url, classified, pageUrl) {
  return {
    url,
    mediaType: classified.mediaType || "direct",
    category: classified.category || "unknown",
    extension: classified.extension || "",
    filename: classified.filename || "",
    playable: classified.playable || false,
    attachment: classified.attachment || false,
    pageUrl,
    source: "webRequest",
  };
}

function notifyTab(tabId, item) {
  chrome.tabs.sendMessage(tabId, {
    type: "MEDIA_DETECTED_NETWORK",
    url: item.url,
    mediaType: item.mediaType,
    category: item.category,
    pageUrl: item.pageUrl || "",
  }).catch(() => {});
}

function rememberTabMediaBatch(tabId, items) {
  for (const item of items) {
    rememberTabMedia(tabId, normalizeMediaItem(item), false);
  }
  persistTabMedia(tabId);
}

function rememberTabMedia(tabId, item, persist = true) {
  if (!Number.isInteger(tabId) || !item?.url) return;
  const normalized = normalizeMediaItem(item);
  const current = tabMedia.get(tabId) || [];
  const existingIndex = current.findIndex((entry) => entry.url === normalized.url);

  if (existingIndex >= 0) {
    current[existingIndex] = {
      ...current[existingIndex],
      ...normalized,
      playable: current[existingIndex].playable || normalized.playable,
      label: normalized.label || current[existingIndex].label || "",
    };
  } else {
    current.push(normalized);
  }

  current.sort((a, b) => mediaRank(a) - mediaRank(b) || a.url.localeCompare(b.url));
  tabMedia.set(tabId, current.slice(0, MAX_MEDIA_PER_TAB));
  if (persist) persistTabMedia(tabId);
}

function normalizeMediaItem(item) {
  const classified = TurDownloadClassifier.classifyDownload(item.url || "");
  return {
    url: item.url || "",
    mediaType: item.mediaType || item.media_type || classified.mediaType || "direct",
    category: item.category || classified.category || "unknown",
    extension: item.extension || classified.extension || "",
    filename: item.filename || classified.filename || "",
    playable: Boolean(item.playable || classified.playable),
    attachment: Boolean(item.attachment || classified.attachment),
    pageUrl: item.pageUrl || item.page_url || "",
    source: item.source || "content",
    label: item.label || "",
    width: item.width || null,
    height: item.height || null,
    duration: item.duration || null,
  };
}

async function getTabMedia(tabId) {
  if (tabMedia.has(tabId)) return tabMedia.get(tabId) || [];
  const key = storageMediaKey(tabId);
  const result = await extensionSessionStorage?.get(key).catch(() => ({})) || {};
  const media = result[key] || [];
  tabMedia.set(tabId, media);
  return media;
}

function persistTabMedia(tabId) {
  const key = storageMediaKey(tabId);
  extensionSessionStorage?.set({ [key]: tabMedia.get(tabId) || [] }).catch(() => {});
}

function persistTabTarget(tabId, payload) {
  const key = `tab_${tabId}_target`;
  extensionSessionStorage?.set({ [key]: payload }).catch(() => {});
}

function storageMediaKey(tabId) {
  return `tab_${tabId}_media`;
}

function contextLabel(item) {
  if (item.label) return truncate(item.label, 96);
  const details = [];
  if (item.width && item.height) details.push(`${item.width}x${item.height}`);
  if (item.duration) details.push(formatDuration(item.duration));
  details.push(labelKind(item.mediaType));
  details.push(shortName(item.url));
  return truncate(details.join(" | "), 96);
}

function mediaRank(item) {
  if (item.mediaType === "hls") return 0;
  if (item.mediaType === "dash") return 1;
  if (item.mediaType === "video") return 2;
  if (item.mediaType === "audio") return 3;
  if (item.category === "download") return 4;
  if (item.mediaType === "page") return 5;
  return 6;
}

function labelKind(kind) {
  if (kind === "hls") return "HLS";
  if (kind === "dash") return "DASH";
  if (kind === "audio") return "Audio";
  if (kind === "page") return "yt-dlp";
  if (kind === "direct") return "Direct";
  return "Video";
}

function shortName(rawUrl) {
  try {
    const parsed = new URL(rawUrl);
    const last = parsed.pathname.split("/").filter(Boolean).pop();
    return last || parsed.hostname;
  } catch (_) {
    return rawUrl;
  }
}

function formatDuration(seconds) {
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`
    : `${minutes}:${String(secs).padStart(2, "0")}`;
}

function truncate(value, max) {
  const text = String(value || "");
  return text.length > max ? `${text.slice(0, max - 3)}...` : text;
}

function findHeader(headers, name) {
  const header = headers.find((item) => item.name?.toLowerCase() === name);
  return header?.value || "";
}

// Helper URL resolver
function resolveUrl(base, relative) {
  try {
    return new URL(relative, base).href;
  } catch (_) {
    return relative;
  }
}

// Unified Option String Formatting Engine
function makeJSLabel(width, height, durationSecs, fps, codecs, sizeBytes, bandwidthBps) {
  const resBlock = (width > 0 && height > 0) ? `${width}x${height}` : "Audio";
  
  let durBlock = "";
  if (durationSecs > 0) {
    const totalSecs = Math.round(durationSecs);
    const hours = Math.floor(totalSecs / 3600);
    const mins = Math.floor((totalSecs % 3600) / 60);
    const secs = totalSecs % 60;
    
    const parts = [];
    if (hours > 0) {
      parts.push(`${hours}hr`);
    }
    if (mins > 0) {
      parts.push(`${mins}min`);
    }
    if (secs > 0 || parts.length === 0) {
      parts.push(`${secs}sec`);
    }
    durBlock = ` | ${parts.join(" ")}`;
  }
  
  const fpsStr = fps > 0 ? `${Math.round(fps)}fps/` : "";
  const codecBlock = ` | ${fpsStr}${cleanJSCodec(codecs)}`;
  
  let sizeVal = 0;
  if (sizeBytes > 0) {
    sizeVal = sizeBytes / (1024 * 1024);
  } else if (bandwidthBps > 0 && durationSecs > 0) {
    sizeVal = (bandwidthBps * durationSecs) / (8 * 1024 * 1024);
  }
  
  let sizeBlock = "";
  if (sizeVal > 0) {
    if (sizeVal >= 1000) {
      sizeBlock = ` | ~${(sizeVal / 1024).toFixed(2)}GB`;
    } else {
      sizeBlock = ` | ~${Math.round(sizeVal)}MB`;
    }
  }
  
  return `${resBlock}${durBlock}${codecBlock}${sizeBlock}`.trim();
}

function cleanJSCodec(raw) {
  if (!raw) return "Video";
  const parts = raw.split(',');
  const cleaned = parts.map(p => {
    const pTrim = p.trim().toLowerCase();
    if (pTrim.includes("avc1") || pTrim.includes("h264")) return "h.264";
    if (pTrim.includes("hev1") || pTrim.includes("hvc1") || pTrim.includes("h265")) return "h.265";
    if (pTrim.includes("vp09") || pTrim.includes("vp9")) return "VP9";
    if (pTrim.includes("av01") || pTrim.includes("av1")) return "AV1";
    if (pTrim.includes("mp4a") || pTrim.includes("opus") || pTrim.includes("aac")) return "Audio";
    return pTrim;
  });
  return cleaned.join(" + ");
}

function parseHLS(playlistText, manifestUrl) {
  const lines = playlistText.split('\n');
  const formats = [];
  let currentStreamInf = null;

  for (let line of lines) {
    line = line.trim();
    if (line.startsWith('#EXT-X-STREAM-INF:')) {
      currentStreamInf = line;
    } else if (line && !line.startsWith('#') && currentStreamInf) {
      const variantUrl = resolveUrl(manifestUrl, line);
      let width = 0;
      let height = 0;
      let bandwidth = 0;
      let frameRate = 30;
      let codecs = "HLS";

      const resMatch = currentStreamInf.match(/RESOLUTION=(\d+)x(\d+)/i);
      if (resMatch) {
        width = parseInt(resMatch[1], 10);
        height = parseInt(resMatch[2], 10);
      }

      const bwMatch = currentStreamInf.match(/BANDWIDTH=(\d+)/i);
      if (bwMatch) {
        bandwidth = parseInt(bwMatch[1], 10);
      }

      const fpsMatch = currentStreamInf.match(/FRAME-RATE=([\d.]+)/i);
      if (fpsMatch) {
        frameRate = parseFloat(fpsMatch[1]);
      }

      const codecsMatch = currentStreamInf.match(/CODECS="([^"]+)"/i);
      if (codecsMatch) {
        codecs = codecsMatch[1];
      }

      const label = makeJSLabel(width, height, 0, frameRate, codecs, 0, bandwidth);
      formats.push({
        label,
        videoUrl: variantUrl,
        audioUrl: "",
        resolution: `${width}x${height}`,
      });

      currentStreamInf = null;
    }
  }
  return formats;
}

function parseDASH(xmlText, manifestUrl, duration) {
  const parser = new DOMParser();
  const xml = parser.parseFromString(xmlText, "text/xml");
  
  let mpdBase = "";
  const mpdBaseEl = xml.querySelector("MPD > BaseURL");
  if (mpdBaseEl) mpdBase = mpdBaseEl.textContent.trim();
  
  const periods = xml.querySelectorAll("Period");
  const formats = [];
  
  for (const period of periods) {
    let periodBase = "";
    const periodBaseEl = period.querySelector("BaseURL");
    if (periodBaseEl) periodBase = periodBaseEl.textContent.trim();
    
    const adaptationSets = period.querySelectorAll("AdaptationSet");
    const videoRepresentations = [];
    const audioRepresentations = [];
    
    for (const adapt of adaptationSets) {
      let adaptBase = "";
      const adaptBaseEl = adapt.querySelector("BaseURL");
      if (adaptBaseEl) adaptBase = adaptBaseEl.textContent.trim();
      
      const mimeType = adapt.getAttribute("mimeType") || "";
      const contentType = adapt.getAttribute("contentType") || "";
      const isVideo = mimeType.startsWith("video") || contentType === "video" || adapt.querySelector("Representation[width]");
      const isAudio = mimeType.startsWith("audio") || contentType === "audio";
      
      const representations = adapt.querySelectorAll("Representation");
      for (const rep of representations) {
        let repBase = "";
        const repBaseEl = rep.querySelector("BaseURL");
        if (repBaseEl) repBase = repBaseEl.textContent.trim();
        
        let relativeUrl = "";
        if (repBase) relativeUrl = repBase;
        else if (adaptBase) relativeUrl = adaptBase;
        else if (periodBase) relativeUrl = periodBase;
        else if (mpdBase) relativeUrl = mpdBase;
        
        const absoluteUrl = resolveUrl(manifestUrl, relativeUrl || "");
        
        const repData = {
          id: rep.getAttribute("id") || "",
          bandwidth: parseInt(rep.getAttribute("bandwidth") || "0", 10),
          codecs: rep.getAttribute("codecs") || adapt.getAttribute("codecs") || "",
          url: absoluteUrl,
        };
        
        if (isVideo) {
          repData.width = parseInt(rep.getAttribute("width") || "0", 10);
          repData.height = parseInt(rep.getAttribute("height") || "0", 10);
          repData.frameRate = parseFloat(rep.getAttribute("frameRate") || "30");
          videoRepresentations.push(repData);
        } else if (isAudio) {
          audioRepresentations.push(repData);
        }
      }
    }
    
    let bestAudio = null;
    if (audioRepresentations.length > 0) {
      bestAudio = audioRepresentations.reduce((best, current) => {
        return (current.bandwidth > best.bandwidth) ? current : best;
      }, audioRepresentations[0]);
    }
    
    for (const v of videoRepresentations) {
      const combinedBandwidth = v.bandwidth + (bestAudio ? bestAudio.bandwidth : 0);
      const label = makeJSLabel(
        v.width,
        v.height,
        duration || 0,
        v.frameRate,
        v.codecs,
        0,
        combinedBandwidth
      );
      
      formats.push({
        label,
        videoUrl: v.url,
        audioUrl: bestAudio ? bestAudio.url : "",
        resolution: `${v.width}x${v.height}`,
      });
    }
  }
  return formats;
}
