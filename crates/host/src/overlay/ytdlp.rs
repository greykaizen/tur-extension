// crates/host/src/overlay/ytdlp.rs

use std::process::{Command, Stdio};
use std::time::Duration;
use std::ffi::c_void;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::types::FormatInfo;
use crate::ytdlp_parse::parse_ytdlp_output;

pub const WM_USER_TARGET_READY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 2;

pub struct YtDlpResultPayload {
    pub element_id: String,
    pub formats: Vec<FormatInfo>,
}

fn log_ytdlp(msg: &str) {
    let path = std::env::temp_dir().join("tur-overlay-debug.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
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
    let mut cmd = Command::new(cmd_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let child = cmd.spawn();

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
    
    let formats = parse_ytdlp_output(&stdout_str);
    log_ytdlp(&format!("Successfully parsed {} formats from yt-dlp JSON output.", formats.len()));
    formats
}
