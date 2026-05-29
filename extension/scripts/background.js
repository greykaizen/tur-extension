// scripts/background.js
if (typeof importScripts === "function") {
  importScripts("classifier.js");
}

const MEDIA_MENU_ROOT = "tur-detected-media-root";
const MEDIA_MENU_EMPTY = "tur-detected-media-empty";
const MEDIA_MENU_PREFIX = "tur-detected-media-";
const MAX_MEDIA_PER_TAB = 48;
const MAX_CONTEXT_ITEMS = 16;

let nativePort = null;
const tabMedia = new Map();
const tabTargets = new Map();
const dynamicMenuItems = new Map();
let menuGeneration = 0;
let focusedBrowserWindowId = chrome.windows.WINDOW_ID_NONE;
const extensionSessionStorage = chrome.storage?.session ?? chrome.storage?.local;
const actionApi = chrome.action ?? chrome.browserAction;

async function setupOffscreenDocument() {
  if (typeof chrome.offscreen === "undefined") {
    console.log("[Background] Offscreen API not supported on this browser.");
    return;
  }
  const OFFSCREEN_PATH = "offscreen.html";
  if (await chrome.offscreen.hasDocument()) return;
  await chrome.offscreen.createDocument({
    url: OFFSCREEN_PATH,
    reasons: ["DOM_PARSER"],
    justification: "Keep Native Messaging available for tur",
  });
  console.log("[Background] Offscreen document created.");
}

chrome.runtime.onStartup.addListener(setupOffscreenDocument);
chrome.runtime.onInstalled.addListener(setupOffscreenDocument);
setupOffscreenDocument();
initializeOverlayVisibilityState();

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

chrome.windows.onFocusChanged.addListener((windowId) => {
  focusedBrowserWindowId = windowId;
  refreshOverlayVisibility().catch((error) => {
    console.warn("[Background] Failed to refresh overlay on window focus change.", error);
  });
});

function connectNative() {
  if (nativePort) return;
  try {
    nativePort = chrome.runtime.connectNative("com.tur.native_host");

    nativePort.onMessage.addListener((msg) => {
      console.log("[Background] Received from TUR native host:", msg);
    });

    nativePort.onDisconnect.addListener(() => {
      console.log("[Background] Disconnected from TUR native host.", chrome.runtime.lastError);
      nativePort = null;
    });
    console.log("[Background] Connected to TUR native host.");
  } catch (err) {
    console.error("[Background] Failed to connect native messaging.", err);
  }
}

connectNative();

function sendToHost(payload) {
  if (!nativePort) connectNative();
  if (!nativePort) {
    console.warn("[Background] Native port not connected, cannot send.");
    return false;
  }

  nativePort.postMessage(payload);
  console.log("[Background] Sent payload to TUR native host.", payload);
  return true;
}

chrome.runtime.onInstalled.addListener(createContextMenus);
chrome.runtime.onStartup.addListener(createContextMenus);
createContextMenus();

const beforeSendHeadersHandler = function(details) {
  if (details.tabId < 0) return;
  if (!["media", "xmlhttprequest", "object", "other"].includes(details.type)) return;

  const classified = TurDownloadClassifier.classifyDownload(details.url);
  if (!classified.downloadable && !classified.playable) return;

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

  if (message.type === "MEDIA_CANDIDATES" && Number.isInteger(tabId)) {
    rememberTabMediaBatch(tabId, message.payload?.media || []);
    sendResponse?.({ ok: true });
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

async function initializeOverlayVisibilityState() {
  try {
    const win = await chrome.windows.getLastFocused();
    focusedBrowserWindowId = Number.isInteger(win?.id) ? win.id : chrome.windows.WINDOW_ID_NONE;
  } catch (_) {
    focusedBrowserWindowId = chrome.windows.WINDOW_ID_NONE;
  }
}

function shouldDisplayOverlayForTab(tab) {
  return Number.isInteger(tab?.id) &&
    Number.isInteger(tab?.windowId) &&
    tab.active === true &&
    tab.windowId === focusedBrowserWindowId;
}

function buildHiddenPayload(tabId, prior = {}) {
  return {
    type: "MEDIA_TARGET_UPDATE",
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
  if (focusedBrowserWindowId === chrome.windows.WINDOW_ID_NONE) {
    for (const tabId of tabTargets.keys()) {
      hideOverlayForTab(tabId);
    }
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
