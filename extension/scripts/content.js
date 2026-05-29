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

const intersectionObserver = new IntersectionObserver(scheduleTargetReport, {
  threshold: [0, 0.2, 0.5, 0.9],
});
const resizeObserver = new ResizeObserver(scheduleTargetReport);

safeRuntimeListener((message) => {
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
    reportTarget();
  });
}

function reportTarget() {
  const target = pickBestTarget();
  const payload = target ? buildTargetPayload(target) : buildPagePayload();
  const signature = JSON.stringify({
    pageUrl: payload.pageUrl,
    x: payload.screenX,
    y: payload.screenY,
    width: payload.width,
    height: payload.height,
    mediaCount: payload.media.length,
    firstUrl: payload.media[0]?.url || "",
  });

  if (signature === lastTargetSignature) return;
  lastTargetSignature = signature;
  renderDebugOverlay(payload);


  safeSendMessage({
    type: "MEDIA_TARGET_UPDATE",
    payload,
  });
}

function buildTargetPayload(target) {
  const { element, rect, metadata } = target;
  const viewport = currentViewportGeometry();
  const media = sortedCandidates().map((item) => ({
    ...metadata,
    ...item,
    label: item.label || candidateLabel(item, metadata),
  }));

  const elementUrl = element.localName === "iframe"
    ? (element.src || element.getAttribute("data-src") || "")
    : (element.currentSrc || element.src || element.data || "");

  if (elementUrl && !elementUrl.startsWith("blob:") && !media.some((item) => item.url === elementUrl)) {
    const classified = window.TurDownloadClassifier?.classifyDownload(elementUrl) || {};
    media.unshift({
      url: elementUrl,
      mediaType: classified.mediaType || guessMediaTypeForElement(element),
      category: classified.category || guessCategoryForElement(element),
      pageUrl: window.location.href,
      playable: classified.playable || true,
      source: "active-element",
      ...metadata,
      label: candidateLabel({
        url: elementUrl,
        mediaType: classified.mediaType || guessMediaTypeForElement(element),
        category: classified.category || guessCategoryForElement(element),
      }, metadata),
    });
  }

  if (media.length === 0) {
    media.push(pageCandidate(metadata));
  }

  return {
    type: "MEDIA_TARGET_UPDATE",
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
    clientX: Math.round(rect.right),
    clientY: Math.round(rect.top),
    screenX: Math.round(viewport.screenX + rect.left),
    screenY: Math.round(viewport.screenY + rect.top),
    width: Math.round(rect.width),
    height: Math.round(rect.height),
    videoWidth: metadata.width,
    videoHeight: metadata.height,
    duration: metadata.duration,
    frameUrl: window.location.href,
    devicePixelRatio: window.devicePixelRatio,
  };
}

function buildPagePayload() {
  const viewport = currentViewportGeometry();
  const media = sortedCandidates();
  return {
    type: "MEDIA_TARGET_UPDATE",
    pageUrl: window.location.href,
    pageTitle: document.title,
    referer: window.location.href,
    userAgent: navigator.userAgent,
    media: media.length > 0 ? media : [pageCandidate({ width: 0, height: 0, duration: null })],
    isTopFrame: window === window.top,
    viewportScreenX: viewport.screenX,
    viewportScreenY: viewport.screenY,
    viewportWidth: viewport.width,
    viewportHeight: viewport.height,
    rawWindowScreenX: Math.round(window.screenX),
    rawWindowScreenY: Math.round(window.screenY),
    outerWidth: Math.round(window.outerWidth),
    outerHeight: Math.round(window.outerHeight),
    clientX: 0,
    clientY: 0,
    screenX: 0,
    screenY: 0,
    width: 0,
    height: 0,
    videoWidth: 0,
    videoHeight: 0,
    duration: null,
    frameUrl: window.location.href,
    devicePixelRatio: window.devicePixelRatio,
  };
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

function pickBestTarget() {
  let best = null;
  let bestScore = Number.NEGATIVE_INFINITY;
  const elements = findMediaElements();

  for (const element of elements) {
    const target = describeTarget(element);
    if (!target) continue;

    const { rect, metadata, visibleRatio, occluded } = target;
    const area = rect.width * rect.height;
    const aspect = rect.height > 0 ? rect.width / rect.height : 0;
    
    let tagBonus = 1.0;
    if (element.localName === "video") {
      tagBonus = 3.0;
    } else if (element.localName === "iframe") {
      tagBonus = 1.8;
    } else if (element.localName === "audio") {
      tagBonus = 0.9;
    } else {
      tagBonus = 1.25;
    }

    const playingBonus = element instanceof HTMLMediaElement && !element.paused ? 1.8 : 1.0;
    const sourceBonus = (element.currentSrc || element.src || element.getAttribute("data-src") || element.getAttribute("data")) ? 1.15 : 1.0;
    const resolutionBonus = metadata.width >= 640 || metadata.height >= 360 ? 1.12 : 1.0;
    const aspectPenalty = aspect > 0 && (aspect < 0.2 || aspect > 6) ? 0.25 : 1.0;
    const occlusionPenalty = occluded ? 0.35 : 1.0;
    
    const score =
      area *
      visibleRatio *
      tagBonus *
      playingBonus *
      sourceBonus *
      resolutionBonus *
      aspectPenalty *
      occlusionPenalty;

    if (score > bestScore) {
      best = target;
      bestScore = score;
    }
  }
  return best;
}

function describeTarget(element) {
  if (!element.isConnected) return null;

  const style = window.getComputedStyle(element);
  if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
    return null;
  }

  const rect = computeClippedRect(element);
  if (!rect) return null;

  const tag = element.localName;
  if (tag === "iframe") {
    if (!isLikelyVideoIframe(element)) return null;
  }

  const minimumWidth = tag === "audio" ? 1 : 96;
  const minimumHeight = tag === "audio" ? 1 : 54;
  if (rect.width < minimumWidth || rect.height < minimumHeight) return null;

  const rawRect = element.getBoundingClientRect();
  const rawArea = Math.max(1, rawRect.width * rawRect.height);
  const visibleRatio = Math.min(1, (rect.width * rect.height) / rawArea);
  const occluded = isLikelyOccluded(element, rect);

  return {
    element,
    rect,
    metadata: mediaMetadata(element),
    visibleRatio,
    occluded,
  };
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

function clipsOverflow(style) {
  const values = [style.overflow, style.overflowX, style.overflowY];
  return values.some((value) => ["hidden", "clip", "scroll", "auto"].includes(value));
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

function isLikelyOccluded(element, rect) {
  return false;
}

function isCoveredAtPoint(element, x, y) {
  const stack = document.elementsFromPoint(x, y);
  for (const node of stack) {
    if (node === element || element.contains(node) || node.contains(element)) return false;
    const style = window.getComputedStyle(node);
    if (style.visibility !== "visible" || style.display === "none" || Number(style.opacity) === 0) {
      continue;
    }
    if (style.pointerEvents === "none") continue;
    if (hasVisiblePaint(style)) return true;
  }
  return false;
}

function hasVisiblePaint(style) {
  return style.backgroundColor !== "rgba(0, 0, 0, 0)" ||
    style.backgroundImage !== "none" ||
    style.borderTopWidth !== "0px" ||
    style.borderRightWidth !== "0px" ||
    style.borderBottomWidth !== "0px" ||
    style.borderLeftWidth !== "0px";
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

function pageCandidate(metadata) {
  return {
    url: window.location.href,
    mediaType: "page",
    category: "page",
    pageUrl: window.location.href,
    playable: true,
    source: "page-fallback",
    ...metadata,
    label: candidateLabel({ mediaType: "page", category: "page", url: window.location.href }, metadata),
  };
}

function mediaMetadata(element) {
  if (element instanceof HTMLVideoElement) {
    return {
      width: Math.round(element.videoWidth || element.getBoundingClientRect().width || 0),
      height: Math.round(element.videoHeight || element.getBoundingClientRect().height || 0),
      duration: Number.isFinite(element.duration) ? element.duration : null,
    };
  }

  if (element instanceof HTMLAudioElement) {
    return {
      width: 0,
      height: 0,
      duration: Number.isFinite(element.duration) ? element.duration : null,
    };
  }

  const rect = element.getBoundingClientRect();
  return {
    width: Math.round(rect.width || 0),
    height: Math.round(rect.height || 0),
    duration: null,
  };
}

function candidateLabel(item, metadata) {
  const details = [];
  if (metadata.width && metadata.height) details.push(`${metadata.width}x${metadata.height}`);
  if (metadata.duration) details.push(formatDuration(metadata.duration));
  details.push(labelKind(item.mediaType || item.category));
  return `Download ${details.join(" | ")}`;
}

function labelKind(kind) {
  if (kind === "hls") return "HLS";
  if (kind === "dash") return "DASH";
  if (kind === "audio") return "Audio";
  if (kind === "page") return "yt-dlp";
  if (kind === "direct") return "Direct";
  return "Video";
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
// Red = target element, Blue = viewport, Yellow = button anchor position
const DEBUG_OVERLAY = false;

function renderDebugOverlay(payload) {
  if (!DEBUG_OVERLAY || window !== window.top) return;
  const root = ensureDebugRoot();
  if (!root) return;
  root.textContent = "";

  const vw = Math.max(0, Number(payload.viewportWidth || 0));
  const vh = Math.max(0, Number(payload.viewportHeight || 0));
  const tw = Math.max(0, Number(payload.width || 0));
  const th = Math.max(0, Number(payload.height || 0));
  const tx = Math.max(0, Number(payload.clientX || 0) - tw);
  const ty = Math.max(0, Number(payload.clientY || 0));

  if (vw <= 0 || vh <= 0) return;

  root.appendChild(makeDebugBox({
    left: 0, top: 0, width: vw, height: vh,
    border: "2px solid rgba(0, 120, 255, 0.5)",
    background: "rgba(0, 120, 255, 0.08)",
  }));

  if (tw <= 0 || th <= 0) return;

  root.appendChild(makeDebugBox({
    left: tx, top: ty, width: tw, height: th,
    border: "2px solid rgba(255, 50, 50, 0.7)",
    background: "rgba(255, 50, 50, 0.10)",
  }));

  let ax = tx + tw - 226;
  const ay = ty - 26 - 2;
  ax = Math.max(0, Math.min(ax, Math.max(0, vw - 226)));

  if (ay >= 0) {
    root.appendChild(makeDebugBox({
      left: ax, top: ay, width: 226, height: 26,
      border: "2px solid rgba(255, 200, 0, 0.8)",
      background: "rgba(255, 200, 0, 0.15)",
    }));

    const label = document.createElement("div");
    label.textContent = "\u25E1 ANCHOR";
    label.style.cssText = "position:absolute;left:" + Math.round(ax) + "px;top:" + Math.round(ay - 14) + "px;font:700 10px/1 ui-sans-serif,sans-serif;color:rgba(255,200,0,0.9);pointer-events:none;user-select:none;white-space:nowrap;z-index:2147483647;";
    root.appendChild(label);
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
