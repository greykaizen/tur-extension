use std::ptr::null_mut;
use std::sync::mpsc;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

mod overlay;
mod window;

/// Custom window message posted from stdin worker to main window.
const WM_OVERLAY_UPDATE: u32 = WM_APP + 0;

/// Geometry update from the extension.
#[derive(Debug, Clone)]
struct OverlayUpdate {
    tab_id: i32,
    screen_x: i32,
    screen_y: i32,
    width: i32,
    height: i32,
    viewport_width: i32,
    viewport_height: i32,
    page_url: String,
    device_pixel_ratio: f64,
}

fn main() {
    // Attempt to set per-monitor DPI awareness so coordinates are accurate.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // Load turbrbtn.dll.
    let api = match overlay::load_button_api() {
        Some(api) => {
            eprintln!("[tur] turbrbtn.dll loaded (v2 popup mode)");
            api
        }
        None => {
            eprintln!("[tur] FATAL: turbrbtn.dll not found or exports missing");
            std::process::exit(1);
        }
    };

    // Detect dark mode preference and propagate to the button DLL.
    unsafe {
        let is_dark = detect_dark_mode();
        let _ = (api.set_dark_mode)(if is_dark { true.into() } else { false.into() });
        eprintln!("[tur] dark mode: {}", is_dark);
    }

    // Channel: stdin worker -> main thread.
    let (tx, rx) = mpsc::channel::<OverlayUpdate>();

    // Spawn stdin reader thread.
    let stdin_tx = tx.clone();
    std::thread::spawn(move || {
        stdin_reader(stdin_tx);
    });

    // Create hidden controller window and enter message loop.
    unsafe {
        run_message_pump(&api, rx);
    }
}

/// Detect Windows dark mode preference from the registry.
unsafe fn detect_dark_mode() -> bool {
    // Simple registry check via reg.exe
    let output = std::process::Command::new("reg")
        .args(["query", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "/v", "AppsUseLightTheme"])
        .output();
    match output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            // Look for "0x0" in the output (meaning dark mode)
            !s.contains("0x1")
        }
        Err(_) => false, // Default to light mode
    }
}

/// Reads length-prefixed JSON messages from stdin (native messaging protocol).
fn stdin_reader(tx: mpsc::Sender<OverlayUpdate>) {
    use std::io::Read;

    let mut stdin = std::io::stdin();
    let mut len_buf = [0u8; 4];

    loop {
        // Read 4-byte little-endian length prefix.
        if let Err(e) = stdin.read_exact(&mut len_buf) {
            eprintln!("[tur] stdin read length error: {e}");
            break;
        }
        let msg_len = u32::from_le_bytes(len_buf) as usize;

        // Read JSON message body.
        let mut msg_buf = vec![0u8; msg_len];
        if let Err(e) = stdin.read_exact(&mut msg_buf) {
            eprintln!("[tur] stdin read body error: {e}");
            break;
        }

        // Parse JSON.
        let Ok(msg) = serde_json::from_slice::<serde_json::Value>(&msg_buf) else {
            eprintln!("[tur] invalid JSON from stdin");
            write_response(&serde_json::json!({"error": "invalid json"}));
            continue;
        };

        // Route by message type.
        let msg_type = msg["type"].as_str().unwrap_or("");
        match msg_type {
            "MEDIA_TARGET_UPDATE" => {
                let tab_id = msg["tabId"].as_i64().unwrap_or(0) as i32;

                let dpr = msg["devicePixelRatio"].as_f64().unwrap_or(1.0);

                let update = OverlayUpdate {
                    tab_id,
                    screen_x: (msg["screenX"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    screen_y: (msg["screenY"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    width: (msg["width"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    height: (msg["height"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    viewport_width: (msg["viewportWidth"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    viewport_height: (msg["viewportHeight"].as_f64().unwrap_or(0.0) * dpr).round() as i32,
                    page_url: msg["pageUrl"].as_str().unwrap_or("").to_string(),
                    device_pixel_ratio: dpr,
                };

                if tx.send(update).is_err() {
                    eprintln!("[tur] main thread dropped, exiting stdin reader");
                    break;
                }
                write_response(&serde_json::json!({"ok": true}));
            }
            "MEDIA_CANDIDATES" | "MEDIA_DETECTED_NETWORK" => {
                // Silently acknowledge; no overlay action needed.
                write_response(&serde_json::json!({"ok": true}));
            }
            other => {
                eprintln!("[tur] unknown message type: {other}");
                write_response(&serde_json::json!({"error": format!("unknown type: {other}")}));
            }
        }
    }

    // Signal main thread to quit via WM_QUIT on controller.
    let hwnd = CONTROLLER_HWND.load(std::sync::atomic::Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(
                HWND(hwnd as *mut std::ffi::c_void),
                WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
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

unsafe fn run_message_pump(api: &overlay::ButtonApi, rx: mpsc::Receiver<OverlayUpdate>) {
    // Register a hidden controller window class.
    let instance: HINSTANCE = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .unwrap_or_default()
            .0,
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

    // Create the hidden controller window.
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

    // Store controller HWND for stdin reader to wake us up.
    CONTROLLER_HWND.store(controller.0 as isize, std::sync::atomic::Ordering::Release);

    // Store the channel receiver in user data so wndproc can access.
    let rx_box = Box::new(rx);
    let rx_ptr = Box::into_raw(rx_box);
    SetWindowLongPtrW(controller, GWLP_USERDATA, rx_ptr as isize);

    // Enter the message loop.
    let mut msg = MSG::default();
    loop {
        // Drain pending updates from the stdin thread.
        let rx_ref = &*(rx_ptr as *const mpsc::Receiver<OverlayUpdate>);
        while let Ok(update) = rx_ref.try_recv() {
            handle_geometry_update(api, &update, controller);
        }

        // Process Windows messages (non-blocking).
        let has_msg = PeekMessageW(&mut msg, HWND(null_mut()), 0, 0, PM_REMOVE).as_bool();
        if has_msg {
            if msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        } else {
            // Sleep briefly to avoid busy-waiting.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    // Cleanup all buttons.
    overlay::destroy_all_buttons(api);
    let _ = DestroyWindow(controller);

    // Free the boxed receiver.
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

unsafe fn handle_geometry_update(api: &overlay::ButtonApi, update: &OverlayUpdate, _controller: HWND) {
    if update.width <= 0 || update.height <= 0
        || update.viewport_width <= 0 || update.viewport_height <= 0
    {
        overlay::hide_button(api, update.tab_id);
        return;
    }

    // Find the center of the media element in screen space.
    let target_center_x = update.screen_x + (update.width / 2);
    let target_center_y = update.screen_y + (update.height / 2);

    // Find the Chrome root window that contains this point.
    let Some(root) = window::find_chromium_root_for_point(target_center_x, target_center_y) else {
        eprintln!(
            "[tur] no chromium root for tab={} point=({}, {})",
            update.tab_id, target_center_x, target_center_y
        );
        overlay::hide_button(api, update.tab_id);
        return;
    };

    // Find the actual web content rendering surface (Chrome_RenderWidgetHostHWND).
    let content_surface = window::find_chromium_content_surface(root);

    eprintln!(
        "[tur] tab={} root={:?} content_surface={:?} pos=({}, {}) size=({}x{})",
        update.tab_id,
        root,
        content_surface,
        update.screen_x,
        update.screen_y,
        update.width,
        update.height
    );

    // Update the popup overlay button at absolute screen coordinates.
    overlay::update_button(api, update, root, content_surface);
}
