// crates/host/src/overlay.rs — Direct2D + DirectComposition hardware overlay
// Orchestrates submodules and defines the public API interface.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub mod types;
pub mod gpu;
pub mod wic;
pub mod menu;
pub mod wndproc;
pub mod render;
pub mod ytdlp;

// Re-export common public types
pub use types::{CanvasUpdate, TargetInfo};

use types::*;
use wndproc::canvas_wndproc;

// ── global canvas singleton ───────────────────────────────────────────────────
pub fn canvas() -> &'static Mutex<Option<CanvasState>> {
    static C: OnceLock<Mutex<Option<CanvasState>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

// ── public API ────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe { register_class() };
}

pub fn update(u: CanvasUpdate) {
    unsafe { do_update(u) };
}

pub fn hide() {
    unsafe { do_hide() };
}

pub fn destroy() {
    unsafe { do_destroy() };
}

pub fn render_frame() {
    let mut g = canvas().lock().unwrap();
    if let Some(ref mut state) = *g {
        unsafe {
            render::do_render(state);
        }
    }
}

// ── window class registration ─────────────────────────────────────────────────
static CLASS_ATOM: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

unsafe fn register_class() {
    let instance = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|m| m.0)
            .unwrap_or_default(),
    );
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(canvas_wndproc),
        hInstance: instance,
        lpszClassName: w!("TurOverlayCanvas"),
        ..Default::default()
    };
    let atom = RegisterClassW(&wc);
    CLASS_ATOM.store(atom, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn log_debug(msg: &str) {
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

// ── update (create / resize / repaint) ───────────────────────────────────────
unsafe fn do_update(u: CanvasUpdate) {
    log_debug(&format!(
        "do_update: tab_id={} targets_len={} vp_screen_x={}, vp_screen_y={}, vp_width={}, vp_height={}, dpr={}",
        u.tab_id, u.targets.len(), u.viewport_screen_x, u.viewport_screen_y, u.viewport_width, u.viewport_height, u.device_pixel_ratio
    ));
    for (i, t) in u.targets.iter().enumerate() {
        log_debug(&format!(
            "  target[{}]: id={} screen_x={}, screen_y={}, width={}, dx={}, dy={}",
            i, t.element_id, t.screen_x, t.screen_y, t.width, t.drag_offset_x, t.drag_offset_y
        ));
    }
    let mut different_tab = false;
    let mut old_hwnd: Option<HWND> = None;
    let mut existing_hwnd: Option<HWND> = None;

    {
        let mut g = canvas().lock().unwrap();
        if let Some(ref mut state) = *g {
            if state.tab_id != u.tab_id {
                different_tab = true;
                if let Some(ref gpu) = state.gpu {
                    gpu.d2d_ctx.SetTarget(None);
                }
                state.gpu = None;
                if state.hwnd != 0 {
                    old_hwnd = Some(HWND(state.hwnd as *mut c_void));
                }
            } else if state.hwnd != 0 {
                existing_hwnd = Some(HWND(state.hwnd as *mut c_void));
            }
        }
    }

    if different_tab {
        if let Some(h) = old_hwnd {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }

    let instance = HINSTANCE(
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|m| m.0)
            .unwrap_or_default(),
    );

    let owner = if u.owner != 0 {
        HWND(u.owner as *mut c_void)
    } else {
        HWND(null_mut())
    };

    let new_hwnd = if let Some(hwnd) = existing_hwnd {
        // Reposition / resize existing window.
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            u.viewport_screen_x,
            u.viewport_screen_y,
            u.viewport_width,
            u.viewport_height,
            SWP_NOACTIVATE,
        );
        hwnd
    } else {
        // Create the overlay window.
        let ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE;
        CreateWindowExW(
            ex_style,
            w!("TurOverlayCanvas"),
            w!(""),
            WS_POPUP,
            u.viewport_screen_x,
            u.viewport_screen_y,
            u.viewport_width,
            u.viewport_height,
            owner,
            HMENU(null_mut()),
            instance,
            None,
        )
        .unwrap_or(HWND(null_mut()))
    };

    if new_hwnd.is_invalid() || new_hwnd == HWND(null_mut()) {
        eprintln!("[tur] overlay: CreateWindowExW failed");
        return;
    }

    // ── Update or create canvas state ────────────────────────────────────────
    let phys_w = u.viewport_width.max(1) as u32;
    let phys_h = u.viewport_height.max(1) as u32;
    let dpi = (u.device_pixel_ratio * 96.0) as f32;

    let mut need_init = false;
    let mut dpr_changed = false;

    {
        let mut g = canvas().lock().unwrap();
        if g.is_none() {
            *g = Some(CanvasState {
                hwnd: new_hwnd.0 as isize,
                tab_id: u.tab_id,
                viewport_screen_x: u.viewport_screen_x,
                viewport_screen_y: u.viewport_screen_y,
                viewport_width: u.viewport_width,
                viewport_height: u.viewport_height,
                dpr: u.device_pixel_ratio,
                targets: u.targets.clone(),
                is_dark: u.is_dark,
                gpu: None,
                referer: u.referer.clone(),
                user_agent: u.user_agent.clone(),
                potential_drag: false,
                potential_zone: HitZone::None,
                dragging: false,
                drag_idx: 0,
                drag_start_x: 0,
                drag_start_y: 0,
                live_dx: 0,
                live_dy: 0,
            });
            need_init = true;
        } else if let Some(ref mut state) = *g {
            // Capture DPR delta BEFORE updating, so the resize check
            // below can detect cross-monitor moves even when physical
            // viewport size is identical.
            dpr_changed = state.dpr != u.device_pixel_ratio;
            state.dpr = u.device_pixel_ratio;
            state.hwnd = new_hwnd.0 as isize;
            state.tab_id = u.tab_id;
            state.viewport_screen_x = u.viewport_screen_x;
            state.viewport_screen_y = u.viewport_screen_y;
            state.viewport_width = u.viewport_width;
            state.viewport_height = u.viewport_height;
            state.targets = u.targets.clone();
            state.is_dark = u.is_dark;
            state.referer = u.referer.clone();
            state.user_agent = u.user_agent.clone();
            if state.gpu.is_none() {
                need_init = true;
            }
        }
    }

    if need_init {
        match gpu::init_gpu(new_hwnd, phys_w, phys_h, dpi) {
            Ok(g) => {
                let mut lock = canvas().lock().unwrap();
                if let Some(ref mut state) = *lock {
                    state.gpu = Some(g);
                }
            }
            Err(e) => {
                eprintln!("[tur] overlay: GPU init failed: {e:?}");
                return;
            }
        }
    } else {
        // Resize swapchain if dimensions or DPR changed.
        // dpr_changed was captured above BEFORE state.dpr was updated,
        // so it correctly reflects a real DPI transition.
        let mut lock = canvas().lock().unwrap();
        if let Some(ref mut state) = *lock {
            if let Some(ref mut g) = state.gpu {
                if g.sc_size != (phys_w, phys_h) || dpr_changed {
                    if let Err(e) = gpu::resize_swapchain(g, phys_w, phys_h, dpi) {
                        eprintln!("[tur] overlay: swapchain resize failed: {e:?}");
                    }
                }
            }
        }
    }

    let _ = ShowWindow(new_hwnd, SW_SHOWNOACTIVATE);

    // Render immediately — don't wait for WM_PAINT.
    let mut lock = canvas().lock().unwrap();
    if let Some(ref mut state) = *lock {
        render::do_render(state);
    }
}

// ── hide / destroy ────────────────────────────────────────────────────────────
unsafe fn do_hide() {
    let g = canvas().lock().unwrap();
    if let Some(ref state) = *g {
        if state.hwnd != 0 {
            let _ = ShowWindow(HWND(state.hwnd as *mut c_void), SW_HIDE);
        }
    }
}

unsafe fn do_destroy() {
    let mut g = canvas().lock().unwrap();
    if let Some(ref mut state) = *g {
        if let Some(ref gpu) = state.gpu {
            gpu.d2d_ctx.SetTarget(None);
        }
        drop(state.gpu.take());
        if state.hwnd != 0 {
            let _ = DestroyWindow(HWND(state.hwnd as *mut c_void));
            state.hwnd = 0;
        }
    }
}

// ── IPC back to extension ─────────────────────────────────────────────────────
pub(crate) fn write_response(value: &serde_json::Value) {
    use std::io::Write;
    let json = serde_json::to_string(value).unwrap_or_default();
    let len = (json.len() as u32).to_le_bytes();
    let mut out = std::io::stdout();
    let _ = out.write_all(&len);
    let _ = out.write_all(json.as_bytes());
    let _ = out.flush();
}
