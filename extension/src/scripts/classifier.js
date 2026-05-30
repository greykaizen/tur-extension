// Shared URL/content classifier for Tur's extension.
// Keep this dependency-free so it can run in both MV3 service workers and content scripts.

(function initTurClassifier(globalScope) {
  "use strict";

  const STREAM_EXTENSIONS = {
    m3u8: "hls",
    mpd: "dash",
    f4m: "f4m",
  };

  const VIDEO_EXTENSIONS = new Set([
    "3g2", "3gp", "avi", "divx", "f4f", "flv", "m2ts", "m4s", "m4v",
    "mkv", "mov", "mp4", "mpeg", "mpg", "ogv", "qt", "ts", "vob", "webm",
    "wmv",
  ]);

  const AUDIO_EXTENSIONS = new Set([
    "aac", "aiff", "amr", "flac", "m4a", "mid", "midi", "mp3", "oga",
    "ogg", "opus", "wav", "weba", "wma",
  ]);

  const DOWNLOAD_EXTENSIONS = new Set([
    "7z", "apk", "appimage", "bin", "bz2", "crx", "deb", "dmg", "doc",
    "docx", "epub", "exe", "gz", "iso", "jar", "msi", "msix", "odp",
    "ods", "odt", "pdf", "pkg", "ppt", "pptx", "rar", "rpm", "tar",
    "tbz2", "tgz", "torrent", "txz", "war", "whl", "xapk", "xls",
    "xlsx", "xz", "zip", "zst",
  ]);

  const IMAGE_EXTENSIONS = new Set([
    "apng", "avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "svg",
    "tif", "tiff", "webp",
  ]);

  const CONTENT_TYPE_RULES = [
    [/application\/(x-mpegurl|vnd\.apple\.mpegurl|octet-stream-m3u8)/i, ["hls", "stream"]],
    [/application\/dash\+xml|video\/vnd\.mpeg\.dash\.mpd/i, ["dash", "stream"]],
    [/video\//i, ["video", "video"]],
    [/audio\//i, ["audio", "audio"]],
    [/application\/(pdf|zip|x-zip|x-rar|x-rar-compressed|x-7z-compressed|x-tar|gzip|x-gzip|x-bzip2|x-xz|vnd\.android\.package-archive|x-msdownload|octet-stream)/i, ["direct", "download"]],
    [/application\/(msword|vnd\.openxmlformats|vnd\.ms-|epub\+zip)/i, ["direct", "download"]],
    [/image\//i, ["image", "image"]],
  ];

  function extensionFromUrl(rawUrl) {
    try {
      const parsed = new URL(rawUrl, globalScope.location?.href || "https://tur.invalid/");
      const pathname = parsed.pathname.toLowerCase();
      const last = pathname.split("/").pop() || "";
      const match = /\.([a-z0-9]{1,12})$/.exec(last);
      return match ? match[1] : "";
    } catch (_) {
      const clean = String(rawUrl || "").split(/[?#]/, 1)[0].toLowerCase();
      const match = /\.([a-z0-9]{1,12})$/.exec(clean);
      return match ? match[1] : "";
    }
  }

  function filenameFromDisposition(disposition) {
    if (!disposition) return "";
    const utf = /filename\*\s*=\s*UTF-8''([^;]+)/i.exec(disposition);
    if (utf) return decodeURIComponentSafe(utf[1].trim().replace(/^["']|["']$/g, ""));
    const ascii = /filename\s*=\s*([^;]+)/i.exec(disposition);
    return ascii ? ascii[1].trim().replace(/^["']|["']$/g, "") : "";
  }

  function decodeURIComponentSafe(value) {
    try {
      return decodeURIComponent(value);
    } catch (_) {
      return value;
    }
  }

  function classifyDownload(rawUrl, contentType = "", disposition = "") {
    const url = String(rawUrl || "");
    const lowerType = String(contentType || "").toLowerCase().split(";", 1)[0].trim();
    const filename = filenameFromDisposition(disposition);
    const ext = extensionFromUrl(filename || url);
    let mediaType = null;
    let category = null;

    if (Object.prototype.hasOwnProperty.call(STREAM_EXTENSIONS, ext)) {
      mediaType = STREAM_EXTENSIONS[ext];
      category = "stream";
    } else if (VIDEO_EXTENSIONS.has(ext)) {
      mediaType = "video";
      category = "video";
    } else if (AUDIO_EXTENSIONS.has(ext)) {
      mediaType = "audio";
      category = "audio";
    } else if (DOWNLOAD_EXTENSIONS.has(ext)) {
      mediaType = "direct";
      category = "download";
    } else if (IMAGE_EXTENSIONS.has(ext)) {
      mediaType = "image";
      category = "image";
    }

    if (!mediaType && lowerType) {
      for (const [pattern, result] of CONTENT_TYPE_RULES) {
        if (pattern.test(lowerType)) {
          [mediaType, category] = result;
          break;
        }
      }
    }

    const attachment = /(^|;)\s*attachment\s*(;|$)/i.test(disposition || "");
    const playable = category === "stream" || category === "video" || category === "audio";
    const downloadable = Boolean(mediaType) || attachment;

    return {
      url,
      mediaType: mediaType || "direct",
      category: category || (attachment ? "download" : "unknown"),
      extension: ext,
      filename,
      downloadable,
      playable,
      attachment,
    };
  }

  globalScope.TurDownloadClassifier = {
    classifyDownload,
    extensionFromUrl,
  };
})(globalThis);
