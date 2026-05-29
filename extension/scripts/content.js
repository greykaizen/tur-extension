// Detector-only content script.
//
// This script intentionally renders no UI. IDM-style browser overlays must be
// native/browser-owned surfaces, not DOM injected into arbitrary websites.

"use strict";

const MEDIA_EVENT = "__tur_media_found__";
const observedElements = new WeakSet();
const detectedUrls = new Map();

let extensionAlive = true;
let rafId = 0;
let lastTargetSignature = "";
let lastCandidatesSignature = "";
let elementIdCounter = 0;

const intersectionObserver = new IntersectionObserver(scheduleTargetReport, {
  threshold: [0, 0.2, 0.5, 0.9],
});
const resizeObserver = new ResizeObserver(scheduleTargetReport);

safeRuntimeListener((message) => {
  if (message.type === "TUR_PING") {
    return { ok: true };
  }
  if (message.type !== "MEDIA_DETECTED_NETWORK") return;
  rememberMedia(message.url, message.mediaType, message.pageUrl, message.category, "network");
});

window.addEventListener(MEDIA_EVENT, (event) => {
  const detail = event.detail || {};
  rememberMedia(detail.url, detail.mediaType, detail.pageUrl, detail.category, detail.source || "main-world");
});

window.addEventListener("message", (event) => {
  if (event.source !== window) return;
  if (event.data?.source !== "TUR_INTERCEPTOR") return;
  const payload = event.data?.payload || {};
  if (payload.type !== "MEDIA_DETECTED") return;
  rememberMedia(payload.url, payload.mediaType, payload.pageUrl, payload.category, payload.source || "main-world");
});

const mutationObserver = new MutationObserver((mutations) => {
  try {
    for (const mutation of mutations) {
      for (const node of mutation.addedNodes) {
        if (node.nodeType !== Node.ELEMENT_NODE) continue;
        processMediaElement(node);
        node.querySelectorAll?.("video, audio, object, embed").forEach(processMediaElement);
      }
    }
    scheduleTargetReport();
  } catch (error) {
    if (isContextInvalidated(error)) teardown();
    else console.debug("[tur] media observer failed", error);
  }
});

mutationObserver.observe(document.documentElement, {
  attributes: true,
  attributeFilter: ["class", "style", "hidden", "src", "poster", "controls"],
  childList: true,
  subtree: true,
});

refreshMediaElements();
window.addEventListener("scroll", scheduleTargetReport, true);
window.addEventListener("resize", scheduleTargetReport, true);
window.addEventListener("fullscreenchange", scheduleTargetReport, true);
document.addEventListener("visibilitychange", scheduleTargetReport, true);

setInterval(() => {
  if (!extensionAlive) return;
  refreshMediaElements();
  reportCandidates();
  scheduleTargetReport();
}, 1200);

scheduleTargetReport();

function ensureElementId(element) {
  if (element.dataset.turId) return element.dataset.turId;
  elementIdCounter++;
  const id = "tur_target_" + elementIdCounter;
  element.dataset.turId = id;
  return id;
}

function rememberMedia(url, mediaType = "direct", pageUrl = window.location.href, category = "unknown", source = "unknown") {
  if (!url || typeof url !== "string") return;

  const classified = window.TurDownloadClassifier?.classifyDownload(url) || {};
  const finalMediaType = mediaType || classified.mediaType || "direct";
  const finalCategory = category === "unknown" ? (classified.category || "unknown") : category;

  detectedUrls.set(url, {
    url,
    mediaType: finalMediaType,
    pageUrl: pageUrl || window.location.href,
    category: finalCategory,
    extension: classified.extension || "",
    filename: classified.filename || "",
    playable: classified.playable || ["hls", "dash", "f4m", "video", "audio"].includes(finalMediaType),
    attachment: classified.attachment || false,
    source,
  });

  reportCandidates();
  scheduleTargetReport();
}

function refreshMediaElements() {
  findMediaElements().forEach(processMediaElement);
}

function processMediaElement(element) {
  if (!(element instanceof Element)) return;
  const tag = element.localName;
  if (!["video", "audio", "object", "embed"].includes(tag)) return;
  if (observedElements.has(element)) return;

  observedElements.add(element);
  intersectionObserver.observe(element);
  resizeObserver.observe(element);

  element.addEventListener("loadedmetadata", () => {
    rememberElementSource(element, "metadata");
    scheduleTargetReport();
  }, true);
  element.addEventListener("play", scheduleTargetReport, true);
  element.addEventListener("pause", scheduleTargetReport, true);
  element.addEventListener("playing", scheduleTargetReport, true);
  element.addEventListener("durationchange", scheduleTargetReport, true);
  element.addEventListener("emptied", scheduleTargetReport, true);

  rememberElementSource(element, "dom");
}

function rememberElementSource(element, source) {
  const src = element.currentSrc || element.src || element.data || "";
  if (src && !src.startsWith("blob:") && !src.startsWith("data:")) {
    const classified = window.TurDownloadClassifier?.classifyDownload(src) || {};
    rememberMedia(src, classified.mediaType || guessMediaTypeForElement(element), window.location.href, classified.category || guessCategoryForElement(element), source);
  }

  element.querySelectorAll?.("source").forEach((sourceElement) => {
    const sourceUrl = sourceElement.src || "";
    if (!sourceUrl || sourceUrl.startsWith("blob:") || sourceUrl.startsWith("data:")) return;
    const classified = window.TurDownloadClassifier?.classifyDownload(sourceUrl, sourceElement.type || "") || {};
    rememberMedia(sourceUrl, classified.mediaType || guessMediaTypeForElement(element), window.location.href, classified.category || guessCategoryForElement(element), "source-element");
  });
}

function reportCandidates() {
  const media = sortedCandidates();
  const signature = media.map((item) => `${item.url}|${item.mediaType}`).join("\n");
  if (signature === lastCandidatesSignature) return;
  lastCandidatesSignature = signature;

  safeSendMessage({
    type: "MEDIA_CANDIDATES",
    payload: {
      pageUrl: window.location.href,
      pageTitle: document.title,
      referer: window.location.href,
      userAgent: navigator.userAgent,
      media,
    },
  });
}

function scheduleTargetReport() {
  if (!extensionAlive || rafId) return;
  rafId = requestAnimationFrame(() => {
    rafId = 0;
    reportTargets();
  });
}

function reportTargets() {
  const targets = collectAllTargets();
  const media = sortedCandidates();
  const viewport = currentViewportGeometry();

  const payload = {
    type: "MEDIA_TARGETS_UPDATE",
    pageUrl: window.location.href,
    pageTitle: document.title,
    referer: window.location.href,
    userAgent: navigator.userAgent,
    media,
    isTopFrame: window === window.top,
    viewportScreenX: viewport.screenX,
    viewportScreenY: viewport.screenY,
    viewportWidth: viewport.width,
    viewportHeight: viewport.height,
    rawWindowScreenX: Math.round(window.screenX),
    rawWindowScreenY: Math.round(window.screenY),
    outerWidth: Math.round(window.outerWidth),
    outerHeight: Math.round(window.outerHeight),
    devicePixelRatio: window.devicePixelRatio,
    targets,
  };

  const signature = JSON.stringify({
    targetCount: targets.length,
    targets: targets.map(function(t) {
      return t.elementId + ":" + t.clientX + ":" + t.clientY + ":" + t.width + ":" + t.height;
    }).join("|"),
    mediaCount: media.length,
    vp: viewport.screenX + "," + viewport.screenY + "," + viewport.width + "," + viewport.height,
  });

  if (signature === lastTargetSignature) return;
  lastTargetSignature = signature;
  renderDebugOverlay(payload);

  safeSendMessage({
    type: "MEDIA_TARGETS_UPDATE",
    payload,
  });
}

function collectAllTargets() {
  const elements = findMediaElements();
  const results = [];

  for (const element of elements) {
    if (!element.isConnected) continue;

    const style = window.getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
      continue;
    }

    const rect = computeClippedRect(element);
    if (!rect) continue;

    const tag = element.localName;
    if (tag === "iframe" && !isLikelyVideoIframe(element)) continue;

    const minimumWidth = tag === "audio" ? 1 : 96;
    const minimumHeight = tag === "audio" ? 1 : 54;
    if (rect.width < minimumWidth || rect.height < minimumHeight) continue;

    const elementId = ensureElementId(element);
    const elementUrl = tag === "iframe"
      ? (element.src || element.getAttribute("data-src") || "")
      : (element.currentSrc || element.src || element.data || "");

    // Only include elements that have a detectable media source or are iframes
    if (!elementUrl && tag !== "iframe") continue;

    results.push({
      elementId,
      clientX: Math.round(rect.right),
      clientY: Math.round(rect.top),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
      mediaUrl: elementUrl,
    });
  }

  return results;
}

function currentViewportGeometry() {
  const border = Math.max(0, (window.outerWidth - window.innerWidth) / 2);
  const topChrome = Math.max(0, window.outerHeight - window.innerHeight - border);
  return {
    screenX: Math.round(window.screenX + border),
    screenY: Math.round(window.screenY + topChrome),
    width: Math.round(window.innerWidth),
    height: Math.round(window.innerHeight),
  };
}

function findMediaElements(root = document) {
  const list = [];
  function traverse(node) {
    if (!node) return;
    if (node.nodeType === Node.ELEMENT_NODE) {
      const tag = node.localName;
      if (["video", "audio", "object", "embed"].includes(tag)) {
        list.push(node);
      }
      if (tag === "iframe" && window === window.top) {
        list.push(node);
      }
      if (node.shadowRoot) {
        traverse(node.shadowRoot);
      }
    }
    let child = node.firstChild;
    while (child) {
      traverse(child);
      child = child.nextSibling;
    }
  }
  traverse(root);
  return list;
}

function isLikelyVideoIframe(iframe) {
  try {
    const src = (iframe.src || iframe.getAttribute("data-src") || "").toLowerCase();
    if (src.includes("googleads") || src.includes("doubleclick") || src.includes("adnxs") || src.includes("amazon-adsystem")) {
      return false;
    }
    const rect = iframe.getBoundingClientRect();
    if (rect.width < 250 || rect.height < 140) return false;
    const aspect = rect.width / rect.height;
    if (aspect < 1.1 || aspect > 2.2) return false;
    
    const keywords = ["embed", "player", "video", "stream", "mcloud", "vidsrc", "youtube", "vimeo", "streamtape", "dood", "voe", "filemoon", "rapidcloud", "megacloud", "play", "anilist", "anime"];
    const hasKeyword = keywords.some(kw => src.includes(kw));
    const allow = (iframe.getAttribute("allow") || "").toLowerCase();
    const hasFullscreen = allow.includes("fullscreen") || iframe.hasAttribute("allowfullscreen");
    return hasKeyword || hasFullscreen;
  } catch (_) {
    return false;
  }
}

function computeClippedRect(element) {
  let rect = toMutableRect(element.getBoundingClientRect());
  if (rect.width <= 0 || rect.height <= 0) return null;

  rect = intersectRects(rect, {
    left: 0,
    top: 0,
    right: window.innerWidth,
    bottom: window.innerHeight,
  });

  return rect;
}

function intersectRects(a, b) {
  const left = Math.max(a.left, b.left);
  const top = Math.max(a.top, b.top);
  const right = Math.min(a.right, b.right);
  const bottom = Math.min(a.bottom, b.bottom);

  if (right - left <= 0 || bottom - top <= 0) return null;

  return {
    left,
    top,
    right,
    bottom,
    width: right - left,
    height: bottom - top,
  };
}
function toMutableRect(rect) {
  return {
    left: rect.left,
    top: rect.top,
    right: rect.right,
    bottom: rect.bottom,
    width: rect.width,
    height: rect.height,
  };
}

function sortedCandidates() {
  return [...detectedUrls.values()]
    .filter((item) => item.url && !item.url.startsWith("blob:") && !item.url.startsWith("data:"))
    .sort((a, b) => candidateRank(a) - candidateRank(b) || a.url.localeCompare(b.url))
    .slice(0, 24);
}

function candidateRank(item) {
  if (item.mediaType === "hls") return 0;
  if (item.mediaType === "dash") return 1;
  if (item.mediaType === "video") return 2;
  if (item.mediaType === "audio") return 3;
  if (item.category === "download") return 4;
  return 5;
}

function guessMediaTypeForElement(element) {
  if (element.localName === "audio") return "audio";
  if (element.localName === "video") return "video";
  return "direct";
}

function guessCategoryForElement(element) {
  if (element.localName === "audio") return "audio";
  if (element.localName === "video") return "video";
  return "download";
}
// ── Debug overlay boxes ──────────────────────────────────────────────
// Blue = viewport, Red = target elements, Yellow = button anchors
const DEBUG_OVERLAY = false;

function renderDebugOverlay(payload) {
  if (!DEBUG_OVERLAY || window !== window.top) return;
  const root = ensureDebugRoot();
  if (!root) return;
  root.textContent = "";

  const vw = Math.max(0, Number(payload.viewportWidth || 0));
  const vh = Math.max(0, Number(payload.viewportHeight || 0));
  if (vw <= 0 || vh <= 0) return;

  // Viewport box
  root.appendChild(makeDebugBox({
    left: 0, top: 0, width: vw, height: vh,
    border: "2px solid rgba(0, 120, 255, 0.5)",
    background: "rgba(0, 120, 255, 0.08)",
  }));

  // Target boxes for each element
  var targets = payload.targets || [];
  for (var i = 0; i < targets.length; i++) {
    var t = targets[i];
    var tw = Math.max(0, Number(t.width || 0));
    var th = Math.max(0, Number(t.height || 0));
    if (tw <= 0 || th <= 0) continue;

    var tx = Math.max(0, Number(t.clientX || 0) - tw);
    var ty = Math.max(0, Number(t.clientY || 0));

    root.appendChild(makeDebugBox({
      left: tx, top: ty, width: tw, height: th,
      border: "2px solid rgba(255, 50, 50, 0.7)",
      background: "rgba(255, 50, 50, 0.10)",
    }));

    // Button anchor
    var ax = Math.max(0, Math.min(tx + tw - 226, Math.max(0, vw - 226)));
    var ay = ty - 26 - 2;

    if (ay >= 0) {
      root.appendChild(makeDebugBox({
        left: ax, top: ay, width: 226, height: 26,
        border: "2px solid rgba(255, 200, 0, 0.8)",
        background: "rgba(255, 200, 0, 0.15)",
      }));

      var label = document.createElement("div");
      label.textContent = "\u25E1 " + (t.elementId || "?") + " (" + Math.round(ax) + ", " + Math.round(ay) + ")";
      label.style.cssText = "position:absolute;left:" + Math.round(ax) + "px;top:" + Math.round(ay - 14) + "px;font:700 10px/1 ui-sans-serif,sans-serif;color:rgba(255,200,0,0.9);pointer-events:none;user-select:none;white-space:nowrap;z-index:2147483647;";
      root.appendChild(label);
    }
  }
}

function ensureDebugRoot() {
  let root = document.getElementById("tur-debug-overlay-root");
  if (root) return root;
  root = document.createElement("div");
  root.id = "tur-debug-overlay-root";
  root.style.cssText = "position:fixed;left:0;top:0;width:100vw;height:100vh;pointer-events:none;z-index:2147483647;overflow:hidden;";
  if (!document.documentElement) return null;
  document.documentElement.appendChild(root);
  return root;
}

function makeDebugBox({ left, top, width, height, border, background }) {
  const node = document.createElement("div");
  node.style.cssText = "position:absolute;left:" + Math.round(left) + "px;top:" + Math.round(top) + "px;width:" + Math.max(0, Math.round(width)) + "px;height:" + Math.max(0, Math.round(height)) + "px;box-sizing:border-box;border:" + border + ";background:" + background + ";pointer-events:none;";
  return node;
}

function safeRuntimeListener(listener) {
  try {
    chrome.runtime.onMessage.addListener(listener);
  } catch (error) {
    if (isContextInvalidated(error)) teardown();
    else throw error;
  }
}

function safeSendMessage(message) {
  if (!extensionAlive) return;
  try {
    chrome.runtime.sendMessage(message, () => {
      const lastError = chrome.runtime.lastError?.message || "";
      if (/context invalidated|receiving end does not exist|extension context/i.test(lastError)) {
        teardown();
      }
    });
  } catch (error) {
    if (isContextInvalidated(error)) teardown();
    else console.debug("[tur] sendMessage failed", error);
  }
}

function teardown() {
  extensionAlive = false;
  try { mutationObserver.disconnect(); } catch (_) {}
  try { intersectionObserver.disconnect(); } catch (_) {}
  try { resizeObserver.disconnect(); } catch (_) {}
}

function isContextInvalidated(error) {
  return /extension context invalidated/i.test(String(error?.message || error));
}
