#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::mpsc;

mod types;
use types::{TargetPayload, TargetsUpdate};

#[cfg(target_os = "windows")]
use std::ffi::c_void;
#[cfg(target_os = "windows")]
use std::ptr::null_mut;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::*;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::*;

#[cfg(target_os = "windows")]
mod overlay;
#[cfg(target_os = "windows")]
mod window;
#[cfg(target_os = "macos")]
mod macos;

fn main() {
    // Per-monitor DPI awareness so coordinates are accurate.
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // COM must be initialised exactly once at the process root, before ANY
    // COM object creation (D2D factory, WIC imaging factory, DComp device).
    // Calling CoInitializeEx inside a helper function is an anti-pattern that
    // causes silent RPC_E_CHANGED_MODE failures if the apartment type differs.
    #[cfg(target_os = "windows")]
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
        .ok() // S_FALSE is acceptable (already initialized) — only panic on true failure
        .expect("CoInitializeEx failed — cannot proceed without COM apartment");
    }

    // Initialise the canvas overlay system.
    #[cfg(target_os = "windows")]
    overlay::init();
    #[cfg(target_os = "macos")]
    macos::init();
    eprintln!("[tur] overlay system initialised");

    // Channel: stdin worker -> main thread.
    let (tx, rx) = mpsc::channel::<TargetsUpdate>();

    // Spawn stdin reader thread.
    let stdin_tx = tx.clone();
    std::thread::spawn(move || {
        stdin_reader(stdin_tx);
    });

    // Create hidden controller window and enter message loop.
    #[cfg(target_os = "windows")]
    unsafe {
        run_message_pump(rx);
    }
    #[cfg(target_os = "macos")]
    {
        while let Ok(update) = rx.recv() {
            unsafe {
                handle_targets_update_macos(&update);
            }
        }
    }
}

// ── stdin reader (native messaging protocol) ─────────────────────────────

fn stdin_reader(tx: mpsc::Sender<TargetsUpdate>) {
    use std::io::Read;

    let mut stdin = std::io::stdin();
    let mut len_buf = [0u8; 4];

    loop {
        if let Err(e) = stdin.read_exact(&mut len_buf) {
            eprintln!("[tur] stdin read length error: {e}");
            break;
        }
        let msg_len = u32::from_le_bytes(len_buf) as usize;

        let mut msg_buf = vec![0u8; msg_len];
        if let Err(e) = stdin.read_exact(&mut msg_buf) {
            eprintln!("[tur] stdin read body error: {e}");
            break;
        }

        let Ok(msg) = serde_json::from_slice::<serde_json::Value>(&msg_buf) else {
            eprintln!("[tur] invalid JSON from stdin");
            write_response(&serde_json::json!({"error": "invalid json"}));
            continue;
        };

        let msg_type = msg["type"].as_str().or(msg["action"].as_str()).unwrap_or("");
        match msg_type {
            "MEDIA_TARGETS_UPDATE" => {
                let result = parse_targets_update(&msg);
                match result {
                    Ok(update) => {
                        if tx.send(update).is_err() {
                            eprintln!("[tur] main thread dropped, exiting");
                            break;
                        }
                        write_response(&serde_json::json!({"ok": true}));
                    }
                    Err(e) => {
                        eprintln!("[tur] parse error: {e}");
                        write_response(&serde_json::json!({"error": format!("parse: {e}")}));
                    }
                }
            }
            "MEDIA_TARGET_UPDATE" | "MEDIA_CANDIDATES" | "MEDIA_DETECTED_NETWORK" => {
                write_response(&serde_json::json!({"ok": true}));
            }
            "QUEUE_DOWNLOAD" => {
                eprintln!("[tur] RECEIVED QUEUE_DOWNLOAD message");
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(r"C:\Users\Shah\.gemini\antigravity-ide\brain\f3fdf00f-ff53-4d50-8779-b8b9f6116f8b\scratch\overlay_debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(file, "[main] QUEUE_DOWNLOAD received: {}", serde_json::to_string_pretty(&msg).unwrap_or_default());
                }
                write_response(&serde_json::json!({"ok": true, "status": "queued"}));
            }
            other => {
                eprintln!("[tur] unknown message type: {other}");
                write_response(&serde_json::json!({"error": format!("unknown type: {other}")}));
            }
        }
    }

    // Signal main thread to quit.
    #[cfg(target_os = "windows")]
    {
        let hwnd = CONTROLLER_HWND.load(std::sync::atomic::Ordering::Acquire);
        if hwnd != 0 {
            unsafe {
                let _ = PostMessageW(
                    HWND(hwnd as *mut c_void),
                    WM_QUIT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}


fn parse_targets_update(msg: &serde_json::Value) -> Result<TargetsUpdate, String> {
    let dpr = msg["devicePixelRatio"].as_f64().unwrap_or(1.0).max(1.0);
    let raw_targets = msg["targets"].as_array().ok_or("missing targets array")?;
    let mut targets = Vec::with_capacity(raw_targets.len());

    for t in raw_targets {
        let element_id = t["elementId"].as_str().unwrap_or("_unknown_").to_string();
        let client_x = (t["clientX"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let client_y = (t["clientY"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let width = (t["width"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let height = (t["height"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let screen_x = (t["screenX"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let screen_y = (t["screenY"].as_f64().unwrap_or(0.0) * dpr).round() as i32;

        let status = match t["status"].as_str().unwrap_or("pending") {
            "resolving" => types::TargetStatus::Resolving,
            "ready" => types::TargetStatus::Ready,
            _ => types::TargetStatus::Pending,
        };

        let mut formats = Vec::new();
        if let Some(formats_arr) = t["formats"].as_array() {
            for f in formats_arr {
                formats.push(types::FormatInfo {
                    label: f["label"].as_str().unwrap_or("").to_string(),
                    video_url: f["videoUrl"].as_str().unwrap_or("").to_string(),
                    audio_url: f["audioUrl"].as_str().unwrap_or("").to_string(),
                    resolution: f["resolution"].as_str().unwrap_or("").to_string(),
                });
            }
        }

        let cookie = t["cookie"].as_str().unwrap_or("").to_string();
        let duration = t["duration"].as_f64().unwrap_or(0.0);

        targets.push(TargetPayload {
            element_id,
            client_x,
            client_y,
            width,
            height,
            screen_x,
            screen_y,
            media_url: t["mediaUrl"].as_str().unwrap_or("").to_string(),
            drag_offset_x: t["dragOffsetX"].as_i64().unwrap_or(0) as i32,
            drag_offset_y: t["dragOffsetY"].as_i64().unwrap_or(0) as i32,
            duration,
            status,
            formats,
            cookie,
        });
    }

    Ok(TargetsUpdate {
        tab_id: msg["tabId"].as_i64().unwrap_or(0) as i32,
        page_url: msg["pageUrl"].as_str().unwrap_or("").to_string(),
        referer: msg["referer"].as_str().unwrap_or("").to_string(),
        user_agent: msg["userAgent"].as_str().unwrap_or("").to_string(),
        viewport_screen_x: (msg["viewportScreenX"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_screen_y: (msg["viewportScreenY"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_width: (msg["viewportWidth"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_height: (msg["viewportHeight"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        device_pixel_ratio: dpr,
        targets,
    })
}

fn write_response(value: &serde_json::Value) {
    use std::io::Write;
    let json = serde_json::to_string(value).unwrap_or_default();
    let len = (json.len() as u32).to_le_bytes();
    let mut out = std::io::stdout();
    let _ = out.write_all(&len);
    let _ = out.write_all(json.as_bytes());
    let _ = out.flush();
}

#[cfg(target_os = "windows")]
pub static CONTROLLER_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

// ── main thread: controller window + message pump ────────────────────────

#[cfg(target_os = "windows")]
unsafe fn run_message_pump(rx: mpsc::Receiver<TargetsUpdate>) {
    let instance = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|m| m.0)
            .unwrap_or_default(),
    );

    let class_name = windows::core::w!("TurOverlayController");

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(controller_wndproc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);

    let controller = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        windows::core::w!("TurOverlayController"),
        WINDOW_STYLE(0),
        0, 0, 0, 0,
        HWND(null_mut()),
        HMENU(null_mut()),
        instance,
        None,
    );

    if controller.is_err() {
        eprintln!("[tur] failed to create controller window");
        return;
    }
    let controller = controller.unwrap();

    CONTROLLER_HWND.store(controller.0 as isize, std::sync::atomic::Ordering::Release);

    let rx_box = Box::new(rx);
    let rx_ptr = Box::into_raw(rx_box);
    SetWindowLongPtrW(controller, GWLP_USERDATA, rx_ptr as isize);

    let mut msg = MSG::default();
    loop {
        let rx_ref = &*(rx_ptr as *const mpsc::Receiver<TargetsUpdate>);
        while let Ok(update) = rx_ref.try_recv() {
            handle_targets_update(&update, controller);
        }

        let has_msg = PeekMessageW(&mut msg, HWND(null_mut()), 0, 0, PM_REMOVE).as_bool();
        if has_msg {
            if msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    overlay::destroy();
    let _ = DestroyWindow(controller);
    let _ = Box::from_raw(rx_ptr);
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn controller_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
 ) -> LRESULT {
    match msg {
        WM_NCCREATE => LRESULT(1),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        overlay::ytdlp::WM_USER_TARGET_READY => {
            let payload_ptr = lparam.0 as *mut overlay::ytdlp::YtDlpResultPayload;
            if !payload_ptr.is_null() {
                let payload = Box::from_raw(payload_ptr);
                let canvas_hwnd_val = {
                    let mut lock = overlay::canvas().lock().unwrap();
                    if let Some(ref mut state) = *lock {
                        if let Some(target) = state.targets.iter_mut().find(|t| t.element_id == payload.element_id) {
                            target.formats = payload.formats;
                            target.status = types::TargetStatus::Ready;
                        }
                        state.hwnd
                    } else {
                        0
                    }
                };
                // Repaint the canvas so the button text reflects Ready state
                if canvas_hwnd_val != 0 {
                    let canvas_hwnd = HWND(canvas_hwnd_val as *mut std::ffi::c_void);
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(canvas_hwnd, None, false);
                }
                // Auto-reopen the menu if user already clicked while we were resolving
                overlay::menu::fire_pending_menu(&payload.element_id);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── overlay update logic ─────────────────────────────────────────────────

#[cfg(target_os = "windows")]
unsafe fn handle_targets_update(update: &TargetsUpdate, _controller: HWND) {
    let is_dark = detect_dark_mode();

    if update.targets.is_empty()
        || update.viewport_width <= 0
        || update.viewport_height <= 0
    {
        overlay::menu::cancel_pending_menu();
        overlay::hide();
        return;
    }

    let center_x = update.viewport_screen_x + update.viewport_width / 2;
    let center_y = update.viewport_screen_y + update.viewport_height / 2;

    let root = window::find_browser_root_for_point(center_x, center_y);
    let owner = root.unwrap_or(HWND(null_mut()));

    log_debug(&format!(
        "[tur] tab={} targets={} viewport=({},{} {}x{}) root={:?} dpr={}",
        update.tab_id,
        update.targets.len(),
        update.viewport_screen_x,
        update.viewport_screen_y,
        update.viewport_width,
        update.viewport_height,
        root,
        update.device_pixel_ratio,
    ));

    let mut overlay_targets = Vec::with_capacity(update.targets.len());

    for incoming in &update.targets {
        let incoming_token = canonical_media_token(&incoming.media_url);

        let existing_opt = {
            let lock = overlay::canvas().lock().unwrap();
            if let Some(ref state) = *lock {
                state.targets.iter().find(|t| {
                    t.element_id == incoming.element_id &&
                    canonical_media_token(&t.media_url) == incoming_token
                }).cloned()
            } else {
                None
            }
        };

        log_debug(&format!(
            "[tur] Incoming target: id={}, url={}, status={:?}, token={}, existing={}",
            incoming.element_id,
            incoming.media_url,
            incoming.status,
            incoming_token,
            existing_opt.is_some()
        ));

        let mut final_target = overlay::TargetInfo {
            element_id: incoming.element_id.clone(),
            screen_x: incoming.screen_x,
            screen_y: incoming.screen_y,
            width: incoming.width,
            _height: incoming.height,
            media_url: incoming.media_url.clone(),
            drag_offset_x: incoming.drag_offset_x,
            drag_offset_y: incoming.drag_offset_y,
            duration: incoming.duration,
            status: incoming.status,
            formats: incoming.formats.clone(),
            cookie: incoming.cookie.clone(),
        };

        if let Some(ref existing) = existing_opt {
            log_debug(&format!(
                "[tur]   Target matched existing: status={:?}, formats_len={}",
                existing.status,
                existing.formats.len()
            ));
            final_target.drag_offset_x = existing.drag_offset_x;
            final_target.drag_offset_y = existing.drag_offset_y;
        }

        // State Machine Resolution:
        // 1. If extension says target is Ready/Resolving, trust it completely.
        // 2. If extension says Pending:
        //    a. If we already have a resolving or ready state in cache, preserve it.
        //    b. Else, transition to Resolving and spin up yt-dlp worker.
        match incoming.status {
            types::TargetStatus::Ready | types::TargetStatus::Resolving => {
                log_debug(&format!(
                    "[tur]   Target {} updated by extension to status={:?}, formats={}",
                    incoming.element_id, incoming.status, incoming.formats.len()
                ));
                final_target.status = incoming.status;
                final_target.formats = incoming.formats.clone();
            }
            types::TargetStatus::Pending => {
                let mut needs_resolve = true;
                if let Some(ref existing) = existing_opt {
                    if existing.status == types::TargetStatus::Resolving || existing.status == types::TargetStatus::Ready {
                        log_debug(&format!(
                            "[tur]   Preserving host-resolved status for {}: status={:?}, formats={}",
                            incoming.element_id, existing.status, existing.formats.len()
                        ));
                        final_target.status = existing.status;
                        if final_target.formats.is_empty() {
                            final_target.formats = existing.formats.clone();
                        }
                        needs_resolve = false;
                    }
                }
                if needs_resolve {
                    let media_url = &incoming.media_url;
                    let resolve_url = if media_url.starts_with("blob:")
                        || media_url.starts_with("data:")
                        || media_url.is_empty()
                    {
                        update.page_url.clone()
                    } else {
                        media_url.clone()
                    };
                    log_debug(&format!(
                        "[tur]   Dispatching to yt-dlp fallback: element_id={} url={}",
                        final_target.element_id, resolve_url
                    ));
                    final_target.status = types::TargetStatus::Resolving;
                    overlay::ytdlp::resolve_ytdlp_async(
                        final_target.element_id.clone(),
                        resolve_url,
                        incoming.cookie.clone(),
                        update.user_agent.clone(),
                        update.referer.clone(),
                        _controller,
                    );
                }
            }
        }

        overlay_targets.push(final_target);
    }

    overlay::update(overlay::CanvasUpdate {
        tab_id: update.tab_id,
        viewport_screen_x: update.viewport_screen_x,
        viewport_screen_y: update.viewport_screen_y,
        viewport_width: update.viewport_width,
        viewport_height: update.viewport_height,
        device_pixel_ratio: update.device_pixel_ratio,
        targets: overlay_targets,
        owner: owner.0 as isize,
        is_dark,
        referer: update.referer.clone(),
        user_agent: update.user_agent.clone(),
    });
}

#[cfg(target_os = "macos")]
unsafe fn handle_targets_update_macos(update: &TargetsUpdate) {
    let overlay_targets: Vec<macos::MacosTargetInfo> = update
        .targets
        .iter()
        .map(|t| macos::MacosTargetInfo {
            element_id: t.element_id.clone(),
            screen_x:   t.screen_x,
            screen_y:   t.screen_y,
            width:      t.width,
            _height:    t.height,
            media_url:  t.media_url.clone(),
            drag_offset_x: t.drag_offset_x,
            drag_offset_y: t.drag_offset_y,
        })
        .collect();

    macos::update(types::CanvasUpdate {
        tab_id: update.tab_id,
        viewport_screen_x: update.viewport_screen_x,
        viewport_screen_y: update.viewport_screen_y,
        viewport_width: update.viewport_width,
        viewport_height: update.viewport_height,
        device_pixel_ratio: update.device_pixel_ratio,
        targets: overlay_targets,
        owner: 0,
        is_dark: false,
        referer: update.referer.clone(),
        user_agent: update.user_agent.clone(),
    });
}

// ── dark mode detection (cached once) ────────────────────────────────────

#[cfg(target_os = "windows")]
static DARK_MODE_INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(target_os = "windows")]
fn detect_dark_mode() -> bool {
    *DARK_MODE_INIT.get_or_init(|| {
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output();
        match output {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                !s.contains("0x1")
            }
            Err(_) => false,
        }
    })
}

fn log_debug(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"C:\Users\Shah\.gemini\antigravity-ide\brain\f3fdf00f-ff53-4d50-8779-b8b9f6116f8b\scratch\overlay_debug.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

fn canonical_media_token(url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    if url.contains("youtube.com") || url.contains("youtu.be") {
        if let Some(pos) = url.find("v=") {
            let start = pos + 2;
            let end = url[start..].find('&').map(|idx| start + idx).unwrap_or(url.len());
            return format!("yt:{}", &url[start..end]);
        }
    }
    if url.contains("vimeo.com") {
        if let Some(pos) = url.find("vimeo.com/") {
            let start = pos + 10;
            let end = url[start..].find('?').map(|idx| start + idx).unwrap_or(url.len());
            return format!("vimeo:{}", &url[start..end]);
        }
    }
    if let Some(pos) = url.find('?') {
        url[..pos].to_string()
    } else {
        url.to_string()
    }
}


