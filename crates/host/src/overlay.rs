// Single transparent canvas overlay.
//
// One layered window maps to the browser viewport and paints all overlay
// buttons using GDI. The background uses a colour-key (magenta) so only the
// button UI is visible. Clicks are intercepted via WM_NCHITTEST — points
// over a button return HTCLIENT, everything else falls through to the browser.
//
// Replaces the old per-HWND turbrbtn.dll approach.  No DOM injection,
// no multiple HWNDs per tab — just one Win32 layered window.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ── constants ─────────────────────────────────────────────────────────────

const BUTTON_WIDTH: i32 = 180;
const BUTTON_HEIGHT: i32 = 26;
const BUTTON_GAP: i32 = 2;
const CORNER_RADIUS: i32 = 10;

/// Colour used as transparency key – any pixel painted with this exact
/// RGB value becomes fully transparent via LWA_COLORKEY.
const KEY_COLOR: u32 = 0x00FF00FF; // magenta

// ── public types ────────────────────────────────────────────────────

/// One media-target that should get a download button on the canvas.
#[derive(Debug, Clone)]
pub struct TargetInfo {
    pub element_id: String,
    pub screen_x: i32,
    pub screen_y: i32,
    pub width: i32,
    pub _height: i32,
}

/// Sent from the main thread to the overlay when geometry changes.
#[derive(Debug, Clone)]
pub struct CanvasUpdate {
    pub tab_id: i32,
    pub viewport_screen_x: i32,
    pub viewport_screen_y: i32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub device_pixel_ratio: f64,
    pub targets: Vec<TargetInfo>,
    pub owner: isize, // HWND stored as isize (!Send workaround)
    pub is_dark: bool,
}

// ── static state ────────────────────────────────────────────────────

struct CanvasState {
    hwnd: isize, // stored as isize so CanvasState is Send
    tab_id: i32,
    viewport_screen_x: i32,
    viewport_screen_y: i32,
    viewport_width: i32,
    viewport_height: i32,
    dpr: f64,
    targets: Vec<TargetInfo>,
    is_dark: bool,
}

static REGISTERED: OnceLock<Mutex<bool>> = OnceLock::new();
static CANVAS: OnceLock<Mutex<Option<CanvasState>>> = OnceLock::new();

fn canvas() -> &'static Mutex<Option<CanvasState>> {
    CANVAS.get_or_init(|| Mutex::new(None))
}

// ── public API ────────────────────────────────────────────────────

/// Must be called once before any other overlay operation.
pub fn init() {
    let mut r = REGISTERED.get_or_init(|| Mutex::new(false)).lock().unwrap();
    if *r {
        return;
    }
    *r = true;
    eprintln!("[tur] overlay: registering window class TurOverlayCanvas");

    unsafe {
        let instance = HINSTANCE(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map(|m| m.0)
                .unwrap_or_default(),
        );

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(canvas_wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(null_mut()),
            lpszClassName: w!("TurOverlayCanvas"),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        eprintln!("[tur] overlay: RegisterClassW returned atom={}", atom);
    }
}

/// Create or update the single overlay canvas with fresh target geometry.
/// If `update.targets` is empty the canvas is hidden.
pub fn update(update: CanvasUpdate) {
    init();

    let mut guard = canvas().lock().unwrap();

    if update.targets.is_empty() || update.viewport_width <= 0 || update.viewport_height <= 0 {
        eprintln!(
            "[tur] overlay: hiding (targets={}, vw={}, vh={})",
            update.targets.len(),
            update.viewport_width,
            update.viewport_height
        );
        if let Some(ref c) = *guard {
            unsafe {
                let _ = ShowWindow(HWND(c.hwnd as *mut c_void), SW_HIDE);
            }
            eprintln!(
                "[tur] overlay: hidden hwnd={:?}",
                HWND(c.hwnd as *mut c_void)
            );
        }
        return;
    }

    eprintln!(
        "[tur] overlay: update tab={} targets={} viewport=({},{},{}x{}) owner={}",
        update.tab_id,
        update.targets.len(),
        update.viewport_screen_x,
        update.viewport_screen_y,
        update.viewport_width,
        update.viewport_height,
        update.owner
    );
    for (i, t) in update.targets.iter().enumerate() {
        eprintln!(
            "[tur] overlay:   [{}] id={} sx={} sy={} w={}",
            i, t.element_id, t.screen_x, t.screen_y, t.width
        );
    }

    if let Some(ref mut c) = *guard {
        eprintln!(
            "[tur] overlay: updating existing canvas hwnd={:?}",
            HWND(c.hwnd as *mut c_void)
        );
        c.tab_id = update.tab_id;
        c.viewport_screen_x = update.viewport_screen_x;
        c.viewport_screen_y = update.viewport_screen_y;
        c.viewport_width = update.viewport_width;
        c.viewport_height = update.viewport_height;
        c.dpr = update.device_pixel_ratio.max(1.0);
        c.targets = update.targets;
        c.is_dark = update.is_dark;

        unsafe {
            let result = SetWindowPos(
                HWND(c.hwnd as *mut c_void),
                HWND((-1_isize) as *mut c_void), // HWND_TOPMOST
                update.viewport_screen_x,
                update.viewport_screen_y,
                update.viewport_width,
                update.viewport_height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            eprintln!("[tur] overlay: SetWindowPos result={:?}", result);
            let _ = InvalidateRect(HWND(c.hwnd as *mut c_void), None, TRUE);
            eprintln!("[tur] overlay: invalidated");
        }
    } else {
        eprintln!("[tur] overlay: creating new canvas window");
        unsafe {
            let instance = HINSTANCE(
                windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                    .map(|m| m.0)
                    .unwrap_or_default(),
            );
            let owner = HWND(update.owner as *mut c_void);

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0),
                w!("TurOverlayCanvas"),
                w!("TurOverlay"),
                WINDOW_STYLE(WS_POPUP.0),
                update.viewport_screen_x,
                update.viewport_screen_y,
                update.viewport_width,
                update.viewport_height,
                owner,
                HMENU(null_mut()),
                instance,
                None,
            );

            match hwnd {
                Ok(hwnd) => {
                    eprintln!("[tur] overlay: created hwnd={:?}", hwnd);
                    // Single call: colour-key + alpha combined.
                    let lwa = SetLayeredWindowAttributes(
                        hwnd,
                        COLORREF(KEY_COLOR),
                        255,
                        LWA_COLORKEY | LWA_ALPHA,
                    );
                    eprintln!("[tur] overlay: SetLayeredWindowAttributes result={:?}", lwa);

                    *guard = Some(CanvasState {
                        hwnd: hwnd.0 as isize,
                        tab_id: update.tab_id,
                        viewport_screen_x: update.viewport_screen_x,
                        viewport_screen_y: update.viewport_screen_y,
                        viewport_width: update.viewport_width,
                        viewport_height: update.viewport_height,
                        dpr: update.device_pixel_ratio.max(1.0),
                        targets: update.targets,
                        is_dark: update.is_dark,
                    });

                    let sw = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    eprintln!(
                        "[tur] overlay: ShowWindow(SW_SHOWNOACTIVATE) result={}",
                        sw.as_bool()
                    );
                }
                Err(e) => {
                    eprintln!("[tur] overlay: FAILED to create canvas window: {}", e);
                }
            }
        }
    }
}

/// Hide the overlay canvas.
pub fn hide() {
    let guard = canvas().lock().unwrap();
    if let Some(ref c) = *guard {
        eprintln!(
            "[tur] overlay: hide canvas hwnd={:?}",
            HWND(c.hwnd as *mut c_void)
        );
        unsafe {
            let _ = ShowWindow(HWND(c.hwnd as *mut c_void), SW_HIDE);
        }
    } else {
        eprintln!("[tur] overlay: hide called but no canvas exists");
    }
}

/// Destroy the overlay canvas entirely.
pub fn destroy() {
    let mut guard = canvas().lock().unwrap();
    if let Some(c) = guard.take() {
        unsafe {
            let _ = DestroyWindow(HWND(c.hwnd as *mut c_void));
        }
    }
}

// ── window procedure ─────────────────────────────────────────────────

unsafe extern "system" fn canvas_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => {
            return LRESULT(MA_NOACTIVATE as isize);
        }

        WM_NCHITTEST => {
            // lparam contains screen coordinates.
            let screen_x = (lparam.0 & 0xFFFF) as i16 as i32;
            let screen_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let g = canvas().lock().unwrap();
            if let Some(ref state) = *g {
                if is_over_button(state, screen_x, screen_y) {
                    return LRESULT(HTCLIENT as isize);
                }
            }

            // Pass click through to the window below.
            return LRESULT(HTTRANSPARENT as isize);
        }

        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let mut screen_pt = POINT { x, y };
            let _ = ClientToScreen(hwnd, &mut screen_pt);

            let g = canvas().lock().unwrap();
            if let Some(ref state) = *g {
                let dpr = state.dpr;
                let bw = (BUTTON_WIDTH as f64 * dpr).round() as i32;
                let bh = (BUTTON_HEIGHT as f64 * dpr).round() as i32;
                let bg = (BUTTON_GAP as f64 * dpr).round() as i32;

                for target in &state.targets {
                    let bx = target.screen_x + target.width - bw;
                    let by = target.screen_y - bh - bg;

                    if screen_pt.x >= bx
                        && screen_pt.x < bx + bw
                        && screen_pt.y >= by
                        && screen_pt.y < by + bh
                    {
                        let response = serde_json::json!({
                            "type": "OVERLAY_BUTTON_CLICKED",
                            "tabId": state.tab_id,
                            "elementId": target.element_id,
                        });
                        write_response(&response);
                        break;
                    }
                }
            }

            return LRESULT(0);
        }

        WM_ERASEBKGND => {
            return LRESULT(1);
        }

        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.is_invalid() {
                eprintln!("[tur] overlay: WM_PAINT BeginPaint failed");
                return LRESULT(0);
            }
            eprintln!(
                "[tur] overlay: WM_PAINT hwnd={:?} paint_rect=({},{})-({},{})",
                hwnd, ps.rcPaint.left, ps.rcPaint.top, ps.rcPaint.right, ps.rcPaint.bottom
            );
            let g = canvas().lock().unwrap();
            if let Some(ref state) = *g {
                paint_canvas(hdc, state);
            } else {
                eprintln!("[tur] overlay: WM_PAINT but no canvas state");
            }
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }

        WM_DESTROY => {
            return LRESULT(0);
        }

        _ => {}
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ── painting helpers ─────────────────────────────────────────────────

unsafe fn paint_canvas(hdc: HDC, state: &CanvasState) {
    eprintln!(
        "[tur] overlay: paint_canvas vp=({},{}) size=({}x{}) targets={} dpr={}",
        state.viewport_screen_x,
        state.viewport_screen_y,
        state.viewport_width,
        state.viewport_height,
        state.targets.len(),
        state.dpr
    );

    // Compute DPR-scaled button dimensions so they match CSS-pixel positions
    // from the debug overlay.
    let dpr = state.dpr;
    let bw = (BUTTON_WIDTH as f64 * dpr).round() as i32;
    let bh = (BUTTON_HEIGHT as f64 * dpr).round() as i32;
    let bg = (BUTTON_GAP as f64 * dpr).round() as i32;
    let cr = (CORNER_RADIUS as f64 * dpr).round() as i32;

    // 1. Fill entire canvas with the transparent key colour.
    let key_brush = CreateSolidBrush(COLORREF(KEY_COLOR));
    let rect = RECT {
        left: 0,
        top: 0,
        right: state.viewport_width,
        bottom: state.viewport_height,
    };
    let _ = FillRect(hdc, &rect, HBRUSH(key_brush.0));
    let _ = DeleteObject(HGDIOBJ(key_brush.0));
    eprintln!("[tur] overlay: filled background with key color");

    // 2. Paint each button.
    let mut painted = 0;
    for target in &state.targets {
        // Button position relative to the canvas (which is at viewport_screen_*).
        // btn_x = dpr * (clientX - bw)  — same as debug overlay's (clientX - 226) in CSS px
        // btn_y = dpr * (clientY - bh - bg) — same as debug overlay's (clientY - 28) in CSS px
        let btn_x = target.screen_x + target.width - bw - state.viewport_screen_x;
        let btn_y = target.screen_y - bh - bg - state.viewport_screen_y;

        eprintln!(
            "[tur] overlay: target {} -> btn at ({}, {}) bw={} bh={}",
            target.element_id, btn_x, btn_y, bw, bh
        );

        // SKIP clipping for now — we want to SEE where the button renders,
        // even if it's partially off-canvas. Re-enable after debug boxes confirm
        // positions are correct.
        // if btn_y < 0 || btn_x < 0 || btn_x + bw > state.viewport_width {
        //     eprintln!("[tur] overlay:   -> OUTSIDE canvas, skipping");
        //     continue;
        // }

        paint_button(hdc, btn_x, btn_y, bw, bh, cr, state.is_dark);
        painted += 1;
    }
    eprintln!("[tur] overlay: painted {} buttons", painted);
}

unsafe fn paint_button(hdc: HDC, x: i32, y: i32, w: i32, h: i32, cr: i32, is_dark: bool) {
    let rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };

    let (bg, border, text_clr) = if is_dark {
        (
            COLORREF(0x0022262A),
            COLORREF(0x0040484F),
            COLORREF(0x00E8EAED),
        )
    } else {
        (
            COLORREF(0x00F5F5F5),
            COLORREF(0x00B8B8B8),
            COLORREF(0x00111111),
        )
    };

    eprintln!(
        "[tur] overlay:   paint_button at ({},{}) dark={} size={}x{} cr={}",
        x, y, is_dark, w, h, cr
    );

    // Fill background
    let bg_brush = CreateSolidBrush(bg);
    let _ = FillRect(hdc, &rect, HBRUSH(bg_brush.0));
    let _ = DeleteObject(HGDIOBJ(bg_brush.0));

    // Draw rounded-rect border
    let border_pen = CreatePen(PS_SOLID, 1, border);
    let old_pen = SelectObject(hdc, HGDIOBJ(border_pen.0));
    let old_brush = SelectObject(hdc, HGDIOBJ(GetStockObject(NULL_BRUSH).0));

    let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, cr, cr);

    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(border_pen.0));

    // Draw text — use DT_LEFT with padding so the icon gap works.
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, text_clr);

    let old_font = SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    let text_pad = (12.0 * (w as f64) / BUTTON_WIDTH as f64).round() as i32;
    let mut text_rect = rect;
    text_rect.left += text_pad;
    let mut text: Vec<u16> = "Download with tur".encode_utf16().chain(Some(0)).collect();
    let dt = DrawTextW(
        hdc,
        &mut text,
        &mut text_rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
    eprintln!("[tur] overlay: DrawTextW returned {}", dt);

    let _ = SelectObject(hdc, old_font);
}

// ── hit-test helper ─────────────────────────────────────────────────

unsafe fn is_over_button(state: &CanvasState, screen_x: i32, screen_y: i32) -> bool {
    let dpr = state.dpr;
    let bw = (BUTTON_WIDTH as f64 * dpr).round() as i32;
    let bh = (BUTTON_HEIGHT as f64 * dpr).round() as i32;
    let bg = (BUTTON_GAP as f64 * dpr).round() as i32;

    for target in &state.targets {
        let bx = target.screen_x + target.width - bw;
        let by = target.screen_y - bh - bg;

        if screen_x >= bx && screen_x < bx + bw && screen_y >= by && screen_y < by + bh {
            return true;
        }
    }
    false
}

// ── IPC back to extension ─────────────────────────────────────────

fn write_response(value: &serde_json::Value) {
    use std::io::Write;
    let json = serde_json::to_string(value).unwrap_or_default();
    let len = (json.len() as u32).to_le_bytes();
    let mut out = std::io::stdout();
    let _ = out.write_all(&len);
    let _ = out.write_all(json.as_bytes());
    let _ = out.flush();
}
