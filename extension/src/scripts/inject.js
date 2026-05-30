// scripts/inject.js
// This runs in the MAIN world to intercept fetch and XHR for m3u8/mpd detection

(function() {
  // Store original functions
  const originalFetch = window.fetch;
  const originalXHROpen = XMLHttpRequest.prototype.open;

  const DOWNLOAD_URL_RE = /\.(m3u8|mpd|f4m|mp4|m4v|m4s|mkv|webm|avi|mov|flv|wmv|mp3|m4a|aac|flac|wav|ogg|opus|zip|rar|7z|tar|gz|bz2|xz|zst|pdf|epub|exe|msi|deb|rpm|appimage|dmg|iso|torrent)(\?|#|$)/i;
  const DOWNLOAD_CONTENT_RE = /^(application\/(x-mpegurl|vnd\.apple\.mpegurl|dash\+xml|octet-stream|pdf|zip|x-rar|x-7z|x-tar|gzip|x-gzip|x-bzip2|x-xz)|video\/|audio\/)/i;

  function isDownloadLikeUrl(url) {
    if (!url || typeof url !== 'string') return false;
    return DOWNLOAD_URL_RE.test(url);
  }

  function classifyUrl(url, contentType = '') {
    const lower = String(url || '').toLowerCase().split(/[?#]/, 1)[0];
    const type = String(contentType || '').toLowerCase();
    if (lower.endsWith('.m3u8') || type.includes('mpegurl')) return { mediaType: 'hls', category: 'stream' };
    if (lower.endsWith('.mpd') || type.includes('dash+xml')) return { mediaType: 'dash', category: 'stream' };
    if (/\.(mp4|m4v|m4s|mkv|webm|avi|mov|flv|wmv|ts|m2ts|3gp|ogv)$/.test(lower) || type.startsWith('video/')) {
      return { mediaType: 'video', category: 'video' };
    }
    if (/\.(mp3|m4a|aac|flac|wav|ogg|opus|weba|wma)$/.test(lower) || type.startsWith('audio/')) {
      return { mediaType: 'audio', category: 'audio' };
    }
    return { mediaType: 'direct', category: 'download' };
  }

  function notifyExtension(url, source, contentType = '') {
    const classification = classifyUrl(url, contentType);
    const detail = {
      url,
      source,
      pageUrl: window.location.href,
      mediaType: classification.mediaType,
      category: classification.category,
      contentType,
      title: document.title
    };

    window.dispatchEvent(new CustomEvent('__tur_media_found__', { detail }));
    window.postMessage({
      source: 'TUR_INTERCEPTOR',
      payload: {
        type: 'MEDIA_DETECTED',
        ...detail
      }
    }, '*');
  }

  // Intercept Fetch
  window.fetch = async function(...args) {
    const url = typeof args[0] === 'string' ? args[0] : args[0]?.url;
    if (isDownloadLikeUrl(url)) {
      notifyExtension(url, 'fetch');
    }
    const response = await originalFetch.apply(this, args);
    const contentType = response?.headers?.get?.('content-type') || '';
    const disposition = response?.headers?.get?.('content-disposition') || '';
    if (url && (DOWNLOAD_CONTENT_RE.test(contentType) || /attachment/i.test(disposition))) {
      notifyExtension(url, 'fetch-response', contentType);
    }
    return response;
  };

  // Intercept XHR
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    if (isDownloadLikeUrl(url)) {
      notifyExtension(url, 'xhr');
    }
    this.addEventListener('readystatechange', function() {
      if (this.readyState !== 2 && this.readyState !== 4) return;
      const contentType = this.getResponseHeader?.('content-type') || '';
      const disposition = this.getResponseHeader?.('content-disposition') || '';
      if (url && (DOWNLOAD_CONTENT_RE.test(contentType) || /attachment/i.test(disposition))) {
        notifyExtension(url, 'xhr-response', contentType);
      }
    }, { once: true });
    return originalXHROpen.call(this, method, url, ...rest);
  };
})();
