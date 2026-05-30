// crates/host/src/ytdlp_parse.rs
// Cross-platform yt-dlp JSON output parser — no OS-specific imports.
// Used by overlay/ytdlp.rs (Windows) and macos.rs (macOS).

use crate::types::FormatInfo;

#[derive(Debug, serde::Deserialize)]
pub struct YtDlpOutput {
    pub duration: Option<f64>,
    pub formats:  Option<Vec<YtDlpFormat>>,
    // Flat single-stream fields (direct CDN files)
    pub url:      Option<String>,
    pub width:    Option<u32>,
    pub height:   Option<u32>,
    #[allow(dead_code)]
    pub vcodec:   Option<String>,
    #[allow(dead_code)]
    pub acodec:   Option<String>,
    pub tbr:      Option<f64>,
    #[allow(dead_code)]
    pub fps:      Option<f64>,
    pub filesize: Option<u64>,
    pub ext:      Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub struct YtDlpFormat {
    pub format_id: String,
    pub url:       String,
    pub width:     Option<u32>,
    pub height:    Option<u32>,
    pub vcodec:    Option<String>,
    pub acodec:    Option<String>,
    pub tbr:       Option<f64>,
    pub fps:       Option<f64>,
    pub filesize:  Option<u64>,
    pub ext:       Option<String>,
}

/// Parse yt-dlp `--dump-json` output into a list of `FormatInfo` entries.
pub fn parse_ytdlp_output(json_str: &str) -> Vec<FormatInfo> {
    let Ok(parsed) = serde_json::from_str::<YtDlpOutput>(json_str) else {
        return Vec::new();
    };

    let duration = parsed.duration.unwrap_or(0.0);
    let formats  = parsed.formats.unwrap_or_default();

    // ── Case 1: formats[] array (YouTube, Vimeo, etc.) ───────────────────────
    if !formats.is_empty() {
        let mut videos = Vec::new();
        let mut audios = Vec::new();
        let mut muxed  = Vec::new();

        for f in formats {
            let has_v = f.vcodec.as_deref().map(|c| c != "none" && !c.is_empty()).unwrap_or(false);
            let has_a = f.acodec.as_deref().map(|c| c != "none" && !c.is_empty()).unwrap_or(false);
            if has_v && !has_a { videos.push(f); }
            else if !has_v && has_a { audios.push(f); }
            else if has_v && has_a  { muxed.push(f); }
        }

        let mut results = Vec::new();

        let best_audio = audios.iter().max_by(|a, b| {
            a.tbr.unwrap_or(0.0).partial_cmp(&b.tbr.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if !videos.is_empty() && best_audio.is_some() {
            let audio = best_audio.unwrap();
            for v in videos {
                let w = v.width.unwrap_or(0);
                let h = v.height.unwrap_or(0);
                let ext = v.ext.as_deref().unwrap_or("video");
                let tbr = v.tbr.unwrap_or(0.0) + audio.tbr.unwrap_or(0.0);
                let size = match (v.filesize, audio.filesize) {
                    (Some(vs), Some(aus)) => Some(vs + aus),
                    _ => None,
                };
                results.push(FormatInfo {
                    label:     make_label(ext, w, h, duration, size, tbr),
                    video_url: v.url.clone(),
                    audio_url: audio.url.clone(),
                    resolution: format!("{}x{}", w, h),
                    size:     size.and_then(format_size),
                    duration: format_duration(duration),
                });
            }
        }

        for m in muxed {
            let w = m.width.unwrap_or(0);
            let h = m.height.unwrap_or(0);
            let ext = m.ext.as_deref().unwrap_or("video");
            results.push(FormatInfo {
                label:     make_label(ext, w, h, duration, m.filesize, m.tbr.unwrap_or(0.0)),
                video_url: m.url.clone(),
                audio_url: String::new(),
                resolution: format!("{}x{}", w, h),
                size:     m.filesize.and_then(format_size),
                duration: format_duration(duration),
            });
        }

        if !results.is_empty() { return results; }
    }

    // ── Case 2: flat single-stream JSON ──────────────────────────────────────
    if let Some(url) = parsed.url {
        if !url.is_empty() {
            let w = parsed.width.unwrap_or(0);
            let h = parsed.height.unwrap_or(0);
            let ext = parsed.ext.as_deref().unwrap_or("video");
            return vec![FormatInfo {
                label:     make_label(ext, w, h, duration, parsed.filesize, parsed.tbr.unwrap_or(0.0)),
                video_url: url,
                audio_url: String::new(),
                resolution: format!("{}x{}", w, h),
                size:     parsed.filesize.and_then(format_size),
                duration: format_duration(duration),
            }];
        }
    }

    Vec::new()
}

pub fn make_label(ext: &str, w: u32, h: u32, dur_secs: f64, size_bytes: Option<u64>, tbr_kbps: f64) -> String {
    let type_str = if ext.is_empty() { "VIDEO".to_string() } else { ext.to_uppercase() };
    let mut parts = vec![type_str.clone()];

    if w > 0 && h > 0 {
        parts.push(format!("{}x{}", w, h));
    } else if ["AUDIO","MP3","AAC","M4A","OGG","OPUS","FLAC","WMA"].contains(&type_str.as_str()) {
        parts.push("Audio".to_string());
    }

    if dur_secs > 0.0 {
        let secs = dur_secs.round() as u64;
        if secs > 0 {
            parts.push(if secs < 60 {
                format!("{}sec", secs)
            } else if secs < 3600 {
                format!("{}min {}sec", secs / 60, secs % 60)
            } else {
                format!("{}hr {}min", secs / 3600, (secs % 3600) / 60)
            });
        }
    }

    if tbr_kbps > 0.0 {
        parts.push(format!("{}kbps", tbr_kbps.round() as u64));
    }

    let computed = size_bytes.filter(|&b| b > 0).or_else(|| {
        if tbr_kbps > 0.0 && dur_secs > 0.0 {
            let v = (tbr_kbps * 1000.0 * dur_secs / 8.0).round() as u64;
            if v > 0 { Some(v) } else { None }
        } else { None }
    });

    if let Some(b) = computed {
        parts.push(format_size_str(b));
    }

    parts.join(" | ")
}

pub fn format_duration(secs: f64) -> Option<String> {
    if secs <= 0.0 { return None; }
    let s = secs.round() as u64;
    if s == 0 { return None; }
    Some(if s < 60 {
        format!("{}sec", s)
    } else if s < 3600 {
        format!("{}min {}sec", s / 60, s % 60)
    } else {
        format!("{}hr {}min", s / 3600, (s % 3600) / 60)
    })
}

pub fn format_size(bytes: u64) -> Option<String> {
    if bytes == 0 { None } else { Some(format_size_str(bytes)) }
}

fn format_size_str(b: u64) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        let kb = b as f64 / 1024.0;
        if (kb * 10.0).round() % 10.0 == 0.0 { format!("{:.0}KB", kb) } else { format!("{:.1}KB", kb) }
    } else if b < 1024 * 1024 * 1024 {
        let mb = b as f64 / (1024.0 * 1024.0);
        if (mb * 10.0).round() % 10.0 == 0.0 { format!("{:.0}MB", mb) } else { format!("{:.1}MB", mb) }
    } else {
        let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
        if (gb * 100.0).round() % 100.0 == 0.0 { format!("{:.0}GB", gb) } else { format!("{:.2}GB", gb) }
    }
}
