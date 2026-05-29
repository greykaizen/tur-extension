use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::mpsc;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

mod overlay;
mod window;
#[cfg(target_os = "macos")]
mod macos;

/// Parsed geometry update from the extension (MEDIA_TARGETS_UPDATE).
#[derive(Debug, Clone)]
struct TargetsUpdate {
    tab_id: i32,
    _page_url: String,
    viewport_screen_x: i32,
    viewport_screen_y: i32,
    viewport_width: i32,
    viewport_height: i32,
    _device_pixel_ratio: f64,
    targets: Vec<TargetPayload>,
}

#[derive(Debug, Clone)]
struct TargetPayload {
    element_id: String,
    _client_x: i32,
    _client_y: i32,
    width: i32,
    height: i32,
    screen_x: i32,
    screen_y: i32,
    media_url: String,
    drag_offset_x: i32,
    drag_offset_y: i32,
}

fn main() {
    // Per-monitor DPI awareness so coordinates are accurate.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // COM must be initialised exactly once at the process root, before ANY
    // COM object creation (D2D factory, WIC imaging factory, DComp device).
    // Calling CoInitializeEx inside a helper function is an anti-pattern that
    // causes silent RPC_E_CHANGED_MODE failures if the apartment type differs.
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
        .ok() // S_FALSE is acceptable (already initialized) — only panic on true failure
        .expect("CoInitializeEx failed — cannot proceed without COM apartment");
    }

    // Initialise the canvas overlay system.
    #[cfg(not(target_os = "macos"))]
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
    unsafe {
        run_message_pump(rx);
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

        let msg_type = msg["type"].as_str().unwrap_or("");
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
            other => {
                eprintln!("[tur] unknown message type: {other}");
                write_response(&serde_json::json!({"error": format!("unknown type: {other}")}));
            }
        }
    }

    // Signal main thread to quit.
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

fn parse_targets_update(msg: &serde_json::Value) -> Result<TargetsUpdate, String> {
    let dpr = msg["devicePixelRatio"].as_f64().unwrap_or(1.0).max(1.0);
    let raw_targets = msg["targets"].as_array().ok_or("missing targets array")?;
    let mut targets = Vec::with_capacity(raw_targets.len());

    for t in raw_targets {
        let element_id = t["elementId"].as_str().unwrap_or("_unknown_").to_string();
        let _client_x = (t["clientX"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let _client_y = (t["clientY"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let width = (t["width"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let height = (t["height"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let screen_x = (t["screenX"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let screen_y = (t["screenY"].as_f64().unwrap_or(0.0) * dpr).round() as i32;
        let _media_url = t["mediaUrl"].as_str().unwrap_or("").to_string();

        targets.push(TargetPayload {
            element_id,
            _client_x,
            _client_y,
            width,
            height,
            screen_x,
            screen_y,
            media_url: t["mediaUrl"].as_str().unwrap_or("").to_string(),
            drag_offset_x: t["dragOffsetX"].as_i64().unwrap_or(0) as i32,
            drag_offset_y: t["dragOffsetY"].as_i64().unwrap_or(0) as i32,
        });
    }

    Ok(TargetsUpdate {
        tab_id: msg["tabId"].as_i64().unwrap_or(0) as i32,
        _page_url: msg["pageUrl"].as_str().unwrap_or("").to_string(),
        viewport_screen_x: (msg["viewportScreenX"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_screen_y: (msg["viewportScreenY"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_width: (msg["viewportWidth"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        viewport_height: (msg["viewportHeight"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
        _device_pixel_ratio: dpr,
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

static CONTROLLER_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

// ── main thread: controller window + message pump ────────────────────────

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

    #[cfg(not(target_os = "macos"))]
    overlay::destroy();
    let _ = DestroyWindow(controller);
    let _ = Box::from_raw(rx_ptr);
}

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
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── overlay update logic ─────────────────────────────────────────────────

unsafe fn handle_targets_update(update: &TargetsUpdate, _controller: HWND) {
    let is_dark = detect_dark_mode();

    if update.targets.is_empty()
        || update.viewport_width <= 0
        || update.viewport_height <= 0
    {
        #[cfg(not(target_os = "macos"))]
        overlay::hide();
        #[cfg(target_os = "macos")]
        macos::hide();
        return;
    }

    // Find the Chrome window that owns this viewport.
    let center_x = update.viewport_screen_x + update.viewport_width / 2;
    let center_y = update.viewport_screen_y + update.viewport_height / 2;

    // Use the TOP-LEVEL Chrome_WidgetWin_1 as owner, NOT the child
    // Chrome_RenderWidgetHostHWND. The DWM aggressively clips popups
    // owned by deep child windows.
    let root = window::find_chromium_root_for_point(center_x, center_y);
    let owner = root.unwrap_or(HWND(null_mut()));

    eprintln!(
        "[tur] tab={} targets={} viewport=({},{} {}x{}) root={:?} dpr={}",
        update.tab_id,
        update.targets.len(),
        update.viewport_screen_x,
        update.viewport_screen_y,
        update.viewport_width,
        update.viewport_height,
        root,
        update._device_pixel_ratio,
    );

    let overlay_targets: Vec<overlay::TargetInfo> = update
        .targets
        .iter()
        .map(|t| overlay::TargetInfo {
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

    #[cfg(not(target_os = "macos"))]
    overlay::update(overlay::CanvasUpdate {
        tab_id: update.tab_id,
        viewport_screen_x: update.viewport_screen_x,
        viewport_screen_y: update.viewport_screen_y,
        viewport_width: update.viewport_width,
        viewport_height: update.viewport_height,
        device_pixel_ratio: update._device_pixel_ratio,
        targets: overlay_targets.clone(),
        owner: owner.0 as isize,
        is_dark,
    });
    #[cfg(target_os = "macos")]
    macos::update(overlay::CanvasUpdate {
        tab_id: update.tab_id,
        viewport_screen_x: update.viewport_screen_x,
        viewport_screen_y: update.viewport_screen_y,
        viewport_width: update.viewport_width,
        viewport_height: update.viewport_height,
        device_pixel_ratio: update._device_pixel_ratio,
        targets: overlay_targets,
        owner: 0,
        is_dark,
    });
}

// ── dark mode detection (cached once) ────────────────────────────────────

static DARK_MODE_INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

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
