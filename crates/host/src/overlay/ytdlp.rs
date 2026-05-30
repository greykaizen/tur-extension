// crates/host/src/overlay/ytdlp.rs

use std::process::{Command, Stdio};
use std::os::windows::process::CommandExt;
use std::time::Duration;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::types::FormatInfo;

pub const WM_USER_TARGET_READY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 2;

pub struct YtDlpResultPayload {
    pub element_id: String,
    pub formats: Vec<FormatInfo>,
}

#[derive(Debug, serde::Deserialize)]
struct YtDlpOutput {
    duration: Option<f64>,
    formats: Option<Vec<YtDlpFormat>>,
    // Top-level fields present when yt-dlp processes a direct CDN file
    // (no formats[] array in this case — it's a flat single-stream JSON)
    url: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    vcodec: Option<String>,
    #[allow(dead_code)]
    acodec: Option<String>,
    tbr: Option<f64>,
    fps: Option<f64>,
    filesize: Option<u64>,
    ext: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct YtDlpFormat {
    format_id: String,
    url: String,
    width: Option<u32>,
    height: Option<u32>,
    vcodec: Option<String>,
    acodec: Option<String>,
    tbr: Option<f64>,
    fps: Option<f64>,
    filesize: Option<u64>,
}

fn log_ytdlp(msg: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("C:\\Users\\Shah\\.gemini\\antigravity-ide\\brain\\f3fdf00f-ff53-4d50-8779-b8b9f6116f8b\\scratch\\overlay_debug.log")
    {
        use std::io::Write;
        let _ = writeln!(file, "[ytdlp] {}", msg);
    }
}

pub fn resolve_ytdlp_async(
    element_id: String,
    media_url: String,
    cookie: String,
    user_agent: String,
    referer: String,
    controller_hwnd: HWND,
) {
    log_ytdlp(&format!("resolve_ytdlp_async starting for element_id={} url={}", element_id, media_url));
    let element_id_clone = element_id.clone();
    let media_url_clone = media_url.clone();
    let cookie_clone = cookie.clone();
    let ua_clone = user_agent.clone();
    let referer_clone = referer.clone();
    let controller_hwnd_val = controller_hwnd.0 as isize;

    std::thread::spawn(move || {
        let hwnd = HWND(controller_hwnd_val as *mut c_void);
        let formats = run_ytdlp(&media_url_clone, &cookie_clone, &ua_clone, &referer_clone);
        
        log_ytdlp(&format!("resolve_ytdlp_async thread completed for element_id={}. Found {} formats. Posting WM_USER_TARGET_READY...", element_id_clone, formats.len()));
        
        let payload = Box::new(YtDlpResultPayload {
            element_id: element_id_clone,
            formats,
        });
        let payload_ptr = Box::into_raw(payload);
        
        // Post message to trigger main-thread repaint safely
        unsafe {
            let _ = PostMessageW(
                hwnd,
                WM_USER_TARGET_READY,
                WPARAM(0),
                LPARAM(payload_ptr as isize),
            );
        }
    });
}

fn run_ytdlp_process(
    cmd_path: &str,
    url: &str,
    cookie: &str,
    user_agent: &str,
    referer: &str,
) -> Option<std::process::Output> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    
    let mut args = vec![
        "--dump-json".to_string(),
        "--socket-timeout".to_string(),
        "5".to_string(),
        "--no-playlist".to_string(),
    ];

    if !user_agent.is_empty() {
        args.push("--user-agent".to_string());
        args.push(user_agent.to_string());
    }

    if !referer.is_empty() {
        args.push("--referer".to_string());
        args.push(referer.to_string());
    }

    if !cookie.is_empty() {
        args.push("--add-header".to_string());
        args.push(format!("Cookie:{}", cookie));
    }

    args.push(url.to_string());

    log_ytdlp(&format!("Spawning yt-dlp process: {} on URL: {}", cmd_path, url));
    let child = Command::new(cmd_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            log_ytdlp(&format!("Failed to spawn {}: {:?}", cmd_path, e));
            return None;
        }
    };

    // Use a channel to wait for the child process with a 10s watchdog timeout
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let res = child.wait_with_output();
        let _ = tx.send(res);
    });

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(output)) => Some(output),
        Ok(Err(e)) => {
            log_ytdlp(&format!("wait_with_output failed for {}: {:?}", cmd_path, e));
            None
        }
        Err(_) => {
            log_ytdlp(&format!("yt-dlp process {} timed out after 10 seconds", cmd_path));
            None
        }
    }
}

fn run_ytdlp(url: &str, cookie: &str, user_agent: &str, referer: &str) -> Vec<FormatInfo> {
    log_ytdlp(&format!("run_ytdlp: url={}", url));
    
    // First try standard PATH search
    let mut output = run_ytdlp_process("yt-dlp", url, cookie, user_agent, referer);
    
    if output.is_none() {
        log_ytdlp("Standard yt-dlp spawn failed or timed out. Attempting WinGet fallback search...");
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            let fallback_dir = std::path::Path::new(&localappdata)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages");
            log_ytdlp(&format!("WinGet fallback search dir: {:?}", fallback_dir));
            if let Ok(entries) = std::fs::read_dir(fallback_dir) {
                let mut found = false;
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.starts_with("yt-dlp.yt-dlp") {
                            let exe_path = entry.path().join("yt-dlp.exe");
                            log_ytdlp(&format!("Checking fallback path: {:?}", exe_path));
                            if exe_path.exists() {
                                found = true;
                                log_ytdlp(&format!("Found local WinGet yt-dlp.exe at {:?}", exe_path));
                                output = run_ytdlp_process(exe_path.to_str().unwrap_or("yt-dlp"), url, cookie, user_agent, referer);
                                if output.is_some() {
                                    break;
                                }
                            }
                        }
                    }
                }
                if !found {
                    log_ytdlp("Did not find any yt-dlp.yt-dlp folder in local WinGet Packages directory.");
                }
            } else {
                log_ytdlp("Failed to read local WinGet Packages directory.");
            }
        } else {
            log_ytdlp("LOCALAPPDATA environment variable is missing.");
        }
    }
        
    let Some(out) = output else {
        log_ytdlp("Both standard and fallback yt-dlp processes failed to produce output.");
        return Vec::new();
    };
    
    if !out.status.success() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        log_ytdlp(&format!("yt-dlp command exited with failure. status: {:?}, stderr: {}", out.status, err_msg));
        return Vec::new();
    };
    
    let Ok(stdout_str) = String::from_utf8(out.stdout) else {
        log_ytdlp("Failed to convert stdout to valid UTF-8.");
        return Vec::new();
    };
    
    let formats = parse_ytdlp_json(&stdout_str);
    log_ytdlp(&format!("Successfully parsed {} formats from yt-dlp JSON output.", formats.len()));
    formats
}

fn parse_ytdlp_json(json_str: &str) -> Vec<FormatInfo> {
    let Ok(parsed) = serde_json::from_str::<YtDlpOutput>(json_str) else {
        log_ytdlp("Failed to parse yt-dlp JSON output");
        return Vec::new();
    };
    
    let duration = parsed.duration.unwrap_or(0.0);
    let formats = parsed.formats.unwrap_or_default();
    
    // --- Case 1: Full formats[] array (walled gardens: YouTube, Vimeo, etc.) ---
    if !formats.is_empty() {
        let mut videos = Vec::new();
        let mut audios = Vec::new();
        let mut muxed = Vec::new();
        
        for f in formats {
            let vcodec = f.vcodec.as_deref().unwrap_or("none");
            let acodec = f.acodec.as_deref().unwrap_or("none");
            
            let has_video = vcodec != "none" && !vcodec.is_empty();
            let has_audio = acodec != "none" && !acodec.is_empty();
            
            if has_video && !has_audio {
                videos.push(f);
            } else if !has_video && has_audio {
                audios.push(f);
            } else if has_video && has_audio {
                muxed.push(f);
            }
        }
        
        let mut results = Vec::new();
        
        let best_audio = audios.iter().max_by(|a, b| {
            let a_tbr = a.tbr.unwrap_or(0.0);
            let b_tbr = b.tbr.unwrap_or(0.0);
            a_tbr.partial_cmp(&b_tbr).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        if !videos.is_empty() && best_audio.is_some() {
            let audio = best_audio.unwrap();
            for v in videos {
                let width = v.width.unwrap_or(0);
                let height = v.height.unwrap_or(0);
                let fps = v.fps.unwrap_or(0.0);
                let vcodec = v.vcodec.clone().unwrap_or_default();
                
                let combined_tbr = v.tbr.unwrap_or(0.0) + audio.tbr.unwrap_or(0.0);
                let bandwidth_bps = combined_tbr * 1000.0;
                
                let combined_size = match (v.filesize, audio.filesize) {
                    (Some(vs), Some(asize)) => Some(vs + asize),
                    _ => None,
                };
                
                let label = make_label(width, height, duration, fps, &vcodec, combined_size, bandwidth_bps);
                results.push(FormatInfo {
                    label,
                    video_url: v.url.clone(),
                    audio_url: audio.url.clone(),
                    resolution: format!("{}x{}", width, height),
                });
            }
        }
        
        for m in muxed {
            let width = m.width.unwrap_or(0);
            let height = m.height.unwrap_or(0);
            let fps = m.fps.unwrap_or(0.0);
            let vcodec = m.vcodec.clone().unwrap_or_default();
            let tbr = m.tbr.unwrap_or(0.0);
            
            let label = make_label(width, height, duration, fps, &vcodec, m.filesize, tbr * 1000.0);
            results.push(FormatInfo {
                label,
                video_url: m.url.clone(),
                audio_url: String::new(),
                resolution: format!("{}x{}", width, height),
            });
        }
        
        if !results.is_empty() {
            log_ytdlp(&format!("Parsed {} formats from formats[] array", results.len()));
            return results;
        }
    }
    
    // --- Case 2: Flat single-stream JSON (direct CDN files: mp4/webm/mkv etc.) ---
    // yt-dlp emits a flat object when there's only one stream, no formats[] array.
    if let Some(url) = parsed.url {
        if !url.is_empty() {
            let width = parsed.width.unwrap_or(0);
            let height = parsed.height.unwrap_or(0);
            let fps = parsed.fps.unwrap_or(0.0);
            let vcodec = parsed.vcodec.clone().unwrap_or_else(|| {
                // Fall back to ext as a codec label if vcodec is missing
                parsed.ext.clone().unwrap_or_else(|| "Video".to_string())
            });
            // Treat as muxed (audio+video) if acodec is set, otherwise pure video
            let label = make_label(width, height, duration, fps, &vcodec, parsed.filesize, parsed.tbr.unwrap_or(0.0) * 1000.0);
            log_ytdlp(&format!("Parsed single-stream flat format: {}x{} {}", width, height, vcodec));
            return vec![FormatInfo {
                label,
                video_url: url,
                audio_url: String::new(),
                resolution: format!("{}x{}", width, height),
            }];
        }
    }
    
    log_ytdlp("parse_ytdlp_json: no formats found in either formats[] or top-level fields");
    Vec::new()
}

fn make_label(
    width: u32,
    height: u32,
    duration_secs: f64,
    fps: f64,
    codec: &str,
    size_bytes: Option<u64>,
    bandwidth_bps: f64,
) -> String {
    let res_block = if width > 0 && height > 0 {
        format!("{}x{}", width, height)
    } else {
        "Audio".to_string()
    };
    
    let dur_block = if duration_secs > 0.0 {
        let total_secs = duration_secs.round() as u64;
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        
        let mut parts = Vec::new();
        if hours > 0 {
            parts.push(format!("{}hr", hours));
        }
        if mins > 0 {
            parts.push(format!("{}min", mins));
        }
        if secs > 0 || parts.is_empty() {
            parts.push(format!("{}sec", secs));
        }
        
        format!(" | {}", parts.join(" "))
    } else {
        String::new()
    };
    
    let fps_str = if fps > 0.0 {
        format!("{}fps/", fps.round())
    } else {
        String::new()
    };
    
    let codec_block = format!(" | {}{}", fps_str, clean_codec(codec));
    
    let size_val = if let Some(bytes) = size_bytes {
        bytes as f64 / (1024.0 * 1024.0)
    } else if bandwidth_bps > 0.0 && duration_secs > 0.0 {
        (bandwidth_bps * duration_secs) / (8.0 * 1024.0 * 1024.0)
    } else {
        0.0
    };
    
    let size_block = if size_val > 0.0 {
        if size_val >= 1000.0 {
            let val_gb = size_val / 1024.0;
            format!(" | ~{:.2}GB", val_gb)
        } else {
            format!(" | ~{}MB", size_val.round() as u64)
        }
    } else {
        String::new()
    };
    
    format!("{}{}{}{}", res_block, dur_block, codec_block, size_block).trim().to_string()
}

fn clean_codec(raw: &str) -> String {
    let parts: Vec<&str> = raw.split(',').collect();
    let cleaned: Vec<String> = parts.iter().map(|p| {
        let p_trim = p.trim().to_lowercase();
        if p_trim.contains("avc1") || p_trim.contains("h264") {
            "h.264".to_string()
        } else if p_trim.contains("hev1") || p_trim.contains("hvc1") || p_trim.contains("h265") {
            "h.265".to_string()
        } else if p_trim.contains("vp09") || p_trim.contains("vp9") {
            "VP9".to_string()
        } else if p_trim.contains("av01") || p_trim.contains("av1") {
            "AV1".to_string()
        } else if p_trim.contains("mp4a") || p_trim.contains("opus") || p_trim.contains("aac") {
            "Audio".to_string()
        } else {
            p_trim
        }
    }).collect();
    cleaned.join(" + ")
}


