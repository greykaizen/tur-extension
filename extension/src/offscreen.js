// offscreen.js

let keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });

keepAlivePort.onDisconnect.addListener(() => {
  setTimeout(() => {
    keepAlivePort = chrome.runtime.connect({ name: "keepAlive" });
  }, 1000);
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.type === "PARSE_MANIFEST") {
    const { url, mediaType, duration } = message;
    console.log(`[Offscreen] Received PARSE_MANIFEST: url=${url}, type=${mediaType}`);
    
    fetch(url, { credentials: "include" })
      .then(response => {
        console.log(`[Offscreen] Fetch manifest response status: ${response.status} for url=${url}`);
        return response.text();
      })
      .then(text => {
        console.log(`[Offscreen] Manifest content preview (first 120 chars): "${text.slice(0, 120).replace(/\n/g, '\\n')}"`);
        let formats = [];
        if (mediaType === "dash") {
          formats = parseDASH(text, url, duration);
        } else if (mediaType === "hls") {
          formats = parseHLS(text, url);
        }
        console.log(`[Offscreen] Parsing completed: url=${url}, type=${mediaType}, formatsFound=${formats.length}`);
        
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
  if (formats.length === 0 && playlistText.includes("#EXTINF")) {
    console.log("[Offscreen] Detected single-variant HLS media playlist instead of master playlist.");
    formats.push({
      label: "HLS Stream (Direct)",
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
