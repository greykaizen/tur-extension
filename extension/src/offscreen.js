// offscreen.js

let keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });

keepAlivePort.onDisconnect.addListener(() => {
  setTimeout(() => {
    keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });
  }, 1000);
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.type === "PARSE_MANIFEST") {
    const { url, mediaType, duration, cookie, referer, isTsMime, userAgent } = message;
    console.log(`[Offscreen] Received PARSE_MANIFEST: url=${url}, type=${mediaType}`);
    
    const headers = {};
    if (referer) headers["Referer"] = referer;
    if (cookie) headers["Cookie"] = cookie;
    if (userAgent) headers["User-Agent"] = userAgent;

    fetch(url, { headers, credentials: "include" })
      .then(async response => {
        console.log(`[Offscreen] Fetch manifest response status: ${response.status} for url=${url}`);
        const contentType = response.headers.get("content-type") || "";
        const contentLength = response.headers.get("content-length");
        
        if (contentType.includes("video/") || contentType.includes("audio/") || contentType.includes("image/")) {
          return { text: "", isBinary: true, contentType, contentLength };
        }

        if (!response.body) {
          const text = await response.text();
          return { text, isBinary: false, contentType, contentLength };
        }

        const reader = response.body.getReader();
        let chunks = [];
        let receivedLength = 0;
        
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          chunks.push(value);
          receivedLength += value.length;
          if (receivedLength > 8192) {
            await reader.cancel();
            break;
          }
        }

        const allChunks = new Uint8Array(receivedLength);
        let position = 0;
        for (let chunk of chunks) {
          allChunks.set(chunk, position);
          position += chunk.length;
        }

        const text = new TextDecoder("utf-8").decode(allChunks);
        return { text, isBinary: false, contentType, contentLength };
      })
      .then(({ text, isBinary, contentType, contentLength }) => {
        let formats = [];
        
        const isHls = !isBinary && (text.includes("#EXTM3U") || contentType.includes("mpegurl"));
        const isDash = !isBinary && (text.includes("<MPD") || text.includes("<mpd") || contentType.includes("dash"));

        if (isHls) {
          console.log(`[Offscreen] Sniffed HLS manifest content for url=${url}`);
          formats = parseHLS(text, url, isTsMime);
        } else if (isDash) {
          console.log(`[Offscreen] Sniffed DASH manifest content for url=${url}`);
          formats = parseDASH(text, url, duration);
        } else {
          // Direct file fallback
          console.log(`[Offscreen] Sniffed direct binary/media content for url=${url}`);
          const sizeBytes = contentLength ? parseInt(contentLength, 10) : 0;
          let ext = TurDownloadClassifier.extensionFromUrl(url).toUpperCase();
          if (!ext || ext.length > 4) {
            ext = "TS";
          }
          const label = makeJSLabel(ext, 0, 0, duration, 0, sizeBytes);
          formats = [{
            label,
            videoUrl: url,
            audioUrl: "",
            resolution: ""
          }];
        }

        console.log(`[Offscreen] Parsing completed: url=${url}, formatsFound=${formats.length}`);
        
        chrome.runtime.sendMessage({
          type: "MANIFEST_PARSED",
          url,
          formats,
          success: true
        });
      })
      .catch(error => {
        console.error(`[Offscreen] Fetch/Parse manifest failed for url=${url}. Error:`, error);
        chrome.runtime.sendMessage({
          type: "MANIFEST_PARSED",
          url,
          formats: [],
          success: false
        });
      });
      
    return true; // async reply
  }
});

// Helper URL resolver
function resolveUrl(base, relative) {
  try {
    return new URL(relative, base).href;
  } catch (_) {
    return relative;
  }
}

// Unified Option String Formatting Engine
// Schema: "[TYPE] | WidthxHeight | Xmin Ysec | Bitratekbps | ~SizeMB"
function makeJSLabel(type, width, height, durationSecs, bitrateKbps, sizeBytes) {
  const typeStr = (type || "VIDEO").toUpperCase();
  const parts = [typeStr];

  if (width > 0 && height > 0) {
    parts.push(`${width}x${height}`);
  } else if (["AUDIO", "MP3", "AAC", "M4A", "OGG", "WMA", "FLAC", "OPUS"].includes(typeStr)) {
    parts.push("Audio");
  }

  if (durationSecs > 0) {
    const totalSecs = Math.round(durationSecs);
    if (totalSecs > 0) {
      if (totalSecs < 60) {
        parts.push(`${totalSecs}sec`);
      } else if (totalSecs < 3600) {
        const mins = Math.floor(totalSecs / 60);
        const secs = totalSecs % 60;
        parts.push(`${mins}min ${secs}sec`);
      } else {
        const hours = Math.floor(totalSecs / 3600);
        const mins = Math.floor((totalSecs % 3600) / 60);
        parts.push(`${hours}hr ${mins}min`);
      }
    }
  }

  if (bitrateKbps > 0) {
    parts.push(`${Math.round(bitrateKbps)}kbps`);
  }

  let computedSize = sizeBytes;
  if (!(computedSize > 0) && bitrateKbps > 0 && durationSecs > 0) {
    computedSize = (bitrateKbps * 1000 * durationSecs) / 8;
  }

  if (computedSize > 0) {
    if (computedSize < 1024 * 1024) {
      parts.push(`${Math.round(computedSize / 1024)}KB`);
    } else if (computedSize < 1024 * 1024 * 1024) {
      const mb = computedSize / (1024 * 1024);
      parts.push(`${mb % 1 === 0 ? mb : mb.toFixed(1)}MB`);
    } else {
      const gb = computedSize / (1024 * 1024 * 1024);
      parts.push(`${gb.toFixed(2)}GB`);
    }
  }

  return parts.join(" | ");
}

function parseHLS(playlistText, manifestUrl, isTsMime) {
  const lines = playlistText.split('\n');
  const formats = [];
  let currentStreamInf = null;

  const hasTsSegments = /\.ts(?:[?#]|$)/i.test(playlistText);
  const typeLabel = (hasTsSegments || isTsMime) ? "TS" : "HLS";

  for (let line of lines) {
    line = line.trim();
    if (line.startsWith('#EXT-X-STREAM-INF:')) {
      currentStreamInf = line;
    } else if (line && !line.startsWith('#') && currentStreamInf) {
      const variantUrl = resolveUrl(manifestUrl, line);
      let width = 0;
      let height = 0;
      let bandwidth = 0;

      const resMatch = currentStreamInf.match(/RESOLUTION=(\d+)x(\d+)/i);
      if (resMatch) {
        width = parseInt(resMatch[1], 10);
        height = parseInt(resMatch[2], 10);
      }

      const bwMatch = currentStreamInf.match(/BANDWIDTH=(\d+)/i);
      if (bwMatch) {
        bandwidth = parseInt(bwMatch[1], 10);
      }

      const label = makeJSLabel(typeLabel, width, height, 0, 0, 0);
      formats.push({
        label,
        videoUrl: variantUrl,
        audioUrl: "",
        resolution: `${width}x${height}`,
      });

      currentStreamInf = null;
    }
  }
  if (formats.length === 0 && playlistText.includes("#EXTINF")) {
    console.log("[Offscreen] Detected single-variant HLS media playlist instead of master playlist.");
    let totalDuration = 0;
    const matches = playlistText.match(/#EXTINF:(\d+(?:\.\d+)?)/g);
    if (matches) {
      for (const match of matches) {
        const d = parseFloat(match.replace("#EXTINF:", ""));
        if (!isNaN(d)) {
          totalDuration += d;
        }
      }
    }

    let bitrateKbps = 0;
    const bwMatch = manifestUrl.match(/[\b_](?:bandwidth|bitrate|rate)=(\d+)/i) || 
                    manifestUrl.match(/[_-](\d+k)(?:\b|_|\.)/i) || 
                    manifestUrl.match(/(\d+)kbps/i);
    if (bwMatch) {
      let val = bwMatch[1].toLowerCase();
      if (val.endsWith("k")) {
        bitrateKbps = parseFloat(val);
      } else {
        const num = parseFloat(val);
        if (num > 10000) {
          bitrateKbps = num / 1000;
        } else {
          bitrateKbps = num;
        }
      }
    } else {
      const resMatch = manifestUrl.match(/[_-](1080|720|480|360|240)p?(?:\b|_|\.)/i);
      if (resMatch) {
        const height = parseInt(resMatch[1], 10);
        if (height === 1080) bitrateKbps = 5000;
        else if (height === 720) bitrateKbps = 2500;
        else if (height === 480) bitrateKbps = 1200;
        else if (height === 360) bitrateKbps = 800;
        else if (height === 240) bitrateKbps = 400;
      }
    }

    formats.push({
      label: makeJSLabel(typeLabel, 0, 0, totalDuration, bitrateKbps, 0),
      videoUrl: manifestUrl,
      audioUrl: "",
      resolution: "",
    });
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
      const label = makeJSLabel(
        "DASH",
        v.width,
        v.height,
        0,
        0,
        0
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
