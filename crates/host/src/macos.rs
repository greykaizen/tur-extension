// crates/host/src/macos.rs
// Full macOS AppKit overlay — per-target NSPanel with custom NSView HUD buttons.
//
// Coordinate system:
//   Browser sends Y from top of screen (CSS convention).
//   Cocoa uses Y from bottom of screen. Convert via:
//     cocoa_y = screen_height - browser_y - panel_height
//
// HUD panel is sized to only the button area (~133x24 pt), NOT the full video,
// so mouse events pass through to the browser for the video player itself.

#![allow(non_snake_case, dead_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

use dispatch::Queue;

use objc2::*;
use objc2::runtime::{Sel, AnyObject, ClassBuilder};
use objc2::ffi::BOOL;
use objc2_foundation::*;
use objc2_app_kit::*;

use crate::types::{CanvasUpdate, FormatInfo, TargetInfo, TargetsUpdate, TargetStatus};
use crate::ytdlp_parse::parse_ytdlp_output;

// ── HUD layout constants (logical points, matching Windows HUD_H=24) ────

const HUD_H: f64 = 24.0;
const HUD_GAP: f64 = 4.0;
const LOGO_L: f64 = 5.0;
const LOGO_SZ: f64 = 14.0;
const PILL_GAP_L: f64 = 4.0;
const PILL_PAD_X: f64 = 8.0;
const PILL_GAP_M: f64 = 3.0;
const X_W: f64 = 22.0;
const R_PAD: f64 = 5.0;
const PILL_R: f64 = 4.0;
const FONT_SIZE: f64 = 10.0;

const PILL_START_X: f64 = LOGO_L + LOGO_SZ + PILL_GAP_L; // = 23.0

/// Compute the HUD button container width (depends only on layout constants).
fn container_width() -> f64 {
    LOGO_L + LOGO_SZ + PILL_GAP_L + 80.0 + PILL_GAP_M + X_W + R_PAD
}

// ── Global state ─────────────────────────────────────────────────────────────

struct PanelRecord {
    panel: *mut NSObject,
    view: *mut NSObject,
    element_id: String,
    /// The C「lient/raw screen x of the associated video element (CSS coords).
    screen_x: i32,
    /// The client/raw screen y of the associated video element (CSS coords).
    screen_y: i32,
    /// Video element width.
    width: i32,
    /// Video element height.
    height: i32,
}

unsafe impl Send for PanelRecord {}

static PANELS: OnceLock<Mutex<Vec<PanelRecord>>> = OnceLock::new();
static IS_DARK: AtomicBool = AtomicBool::new(false);

fn panels() -> &'static Mutex<Vec<PanelRecord>> {
    PANELS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Per-target metadata cache — used for yt-dlp state machine & quality menu.
struct TargetCacheEntry {
    status: TargetStatus,
    formats: Vec<FormatInfo>,
    tab_id: i32,
    media_url: String,
    referer: String,
    user_agent: String,
    cookie: String,
    drag_offset_x: i32,
    drag_offset_y: i32,
}

static TARGET_CACHE: OnceLock<Mutex<HashMap<String, TargetCacheEntry>>> = OnceLock::new();

fn target_cache() -> &'static Mutex<HashMap<String, TargetCacheEntry>> {
    TARGET_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn log_msg(msg: &str) {
    let path = std::env::temp_dir().join("tur-overlay.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true).open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[macos] {}", msg);
    }
}

// ── Native messaging IPC ─────────────────────────────────────────────────────
/// Write a JSON response back to the browser extension via stdout
/// (native messaging protocol: 4-byte LE length prefix + UTF-8 JSON body).
fn write_response(value: &serde_json::Value) {
    use std::io::Write;
    if let Ok(json) = serde_json::to_string(value) {
        let len = (json.len() as u32).to_le_bytes();
        let mut out = std::io::stdout();
        let _ = out.write_all(&len);
        let _ = out.write_all(json.as_bytes());
        let _ = out.flush();
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise NSApp. Called once from main() before the channel is created.
pub fn init() {
    log_msg("init");

    // Register the custom ButtonOverlayView class before any panel creation
    ensure_view_class();

    unsafe {
        let app = class!(NSApplication);
        let shared: *mut NSObject = msg_send![app, sharedApplication];
        // .accessory = no Dock icon, no menu bar takeover
        let accessory = NSApplicationActivationPolicy::Accessory;
        let _: () = msg_send![shared, setActivationPolicy: accessory];
        // Register for screen change notifications
        let ws: *mut NSObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let nc: *mut NSObject = msg_send![ws, notificationCenter];
        let sel = sel!(activeDisplayDidChangeNotification);
        let obs: *mut NSObject = msg_send![class!(NSObject), new];
        let _: () = msg_send![nc, addObserver: obs
                                          selector: sel!(screenParamsChanged:)
                                              name: sel
                                            object: std::ptr::null_mut::<NSObject>()];
    }
    IS_DARK.store(detect_dark_mode(), Ordering::Relaxed);
}

/// Own the macOS event loop. Drains `rx` via GCD dispatch to the main queue,
/// then calls `NSApplication::run()` on the main thread (this blocks forever).
pub fn run(rx: mpsc::Receiver<TargetsUpdate>) {
    log_msg("run: entering event loop");

    // Spawn a background thread that drains the channel and dispatches
    // updates to the main queue via GCD.
    std::thread::spawn(move || {
        while let Ok(update) = rx.recv() {
            let cu = build_canvas_update(&update);
            // Perform yt-dlp resolution on the background thread, then dispatch
            // the panel update to the main thread.
            let targets_needing_resolve = resolve_pending_targets(&update);
            Queue::main().exec_async(move || {
                update_inner(cu);
            });
            // Kick off async yt-dlp for targets that need it (after the main
            // thread has created their panels).
            for eid in targets_needing_resolve {
                if let Some(t) = update.targets.iter().find(|t| t.element_id == eid) {
                    let resolve_url = if t.media_url.starts_with("blob:")
                        || t.media_url.starts_with("data:")
                        || t.media_url.is_empty()
                    {
                        update.page_url.clone()
                    } else {
                        t.media_url.clone()
                    };
                    spawn_ytdlp_resolve(
                        eid,
                        resolve_url,
                        t.cookie.clone(),
                        update.user_agent.clone(),
                        update.referer.clone(),
                    );
                }
            }
        }
        // Channel closed — tell main thread to quit
        log_msg("run: channel closed, posting NSApp terminate");
        Queue::main().exec_async(|| {
            unsafe {
                let app: *mut NSObject = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![app, terminate: std::ptr::null_mut::<NSObject>()];
            }
        });
    });

    // Block the main thread on NSApp run.
    unsafe {
        let app: *mut NSObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, run];
    }
    log_msg("run: NSApp.run() returned");
}

/// Called from main thread via GCD dispatch. Reconciles overlay panels
/// with the incoming canvas update.
fn update_inner(cu: CanvasUpdate) {
    IS_DARK.store(detect_dark_mode(), Ordering::Relaxed);

    if cu.targets.is_empty() || cu.viewport_width <= 0 || cu.viewport_height <= 0 {
        hide_inner();
        return;
    }

    let mut current_ids: Vec<&str> = Vec::new();
    let screen_h = primary_screen_height();
    let cw = container_width();

    for t in &cu.targets {
        current_ids.push(&t.element_id);

        // ── Compute HUD panel frame ────────────────────────────────────────
        // CSS position: top-right of video element, above it by HUD_GAP.
        //   css_hud_x = t.screen_x + t.width - cw
        //   css_hud_y = t.screen_y - HUD_H - HUD_GAP    (top edge of HUD)
        //
        // Cocoa (y-flip): origin is the BOTTOM of the rect.
        //   cocoa_hud_bottom = screen_h - (t.screen_y - HUD_GAP)
        //                    = screen_h - t.screen_y + HUD_GAP
        //
        // Apply saved drag offset (CSS coords → flip Y for Cocoa):
        //   cocoa_x = css_x + drag_offset_x
        //   cocoa_y = cocoa_bottom - drag_offset_y   (negative CSS up = up in Cocoa)

        let css_hud_x = t.screen_x as f64 + t.width as f64 - cw;
        let cocoa_hud_bottom = screen_h - t.screen_y as f64 + HUD_GAP;

        let panel_x = css_hud_x + t.drag_offset_x as f64;
        let panel_y = cocoa_hud_bottom - t.drag_offset_y as f64;

        // Panel is sized to only the HUD button, not the full video
        let panel_w = cw;
        let panel_h = HUD_H;

        // ── State machine: preserve cached resolved status ──────────────────
        let _needs_resolve = {
            let mut cache = target_cache().lock().unwrap();
            match t.status {
                TargetStatus::Ready | TargetStatus::Resolving => {
                    // Extension has newer info — update cache
                    cache.insert(t.element_id.clone(), TargetCacheEntry {
                        status: t.status,
                        formats: t.formats.clone(),
                        tab_id: cu.tab_id,
                        media_url: t.media_url.clone(),
                        referer: cu.referer.clone(),
                        user_agent: cu.user_agent.clone(),
                        cookie: t.cookie.clone(),
                        drag_offset_x: t.drag_offset_x,
                        drag_offset_y: t.drag_offset_y,
                    });
                    false
                }
                TargetStatus::Pending => {
                    if let Some(entry) = cache.get(&t.element_id) {
                        if entry.status == TargetStatus::Resolving
                            || entry.status == TargetStatus::Ready
                        {
                            // Preserve cached resolved state
                            false
                        } else {
                            true // needs fresh resolve
                        }
                    } else {
                        true // new target, needs resolve
                    }
                }
            }
        };

        // Update cache with current drag offset if needed
        {
            let mut cache = target_cache().lock().unwrap();
            if let Some(entry) = cache.get_mut(&t.element_id) {
                entry.drag_offset_x = t.drag_offset_x;
                entry.drag_offset_y = t.drag_offset_y;
            }
        }

        // ── Create or reposition panel ─────────────────────────────────────
        let mut lock = panels().lock().unwrap();
        let existing_idx = lock.iter().position(|p| p.element_id == t.element_id);

        if let Some(idx) = existing_idx {
            // Reposition AND resize existing panel
            unsafe {
                let rec = &lock[idx];
                let frame = NSRect::new(NSPoint::new(panel_x, panel_y), NSSize::new(panel_w, panel_h));
                let _: () = msg_send![rec.panel, setFrame: frame display: YES];
            }
            let rec = &mut lock[idx];
            rec.screen_x = t.screen_x;
            rec.screen_y = t.screen_y;
            rec.width = t.width;
            rec.height = t._height;
        } else {
            // Create a new HUD-sized NSPanel
            let panel = unsafe {
                create_panel(panel_x, panel_y, panel_w, panel_h, &t.element_id)
            };
            if !panel.is_null() {
                lock.push(PanelRecord {
                    panel,
                    view: std::ptr::null_mut(),
                    element_id: t.element_id.clone(),
                    screen_x: t.screen_x,
                    screen_y: t.screen_y,
                    width: t.width,
                    height: t._height,
                });
            }
        }
    }

    // Remove panels for elements no longer in the update
    {
        let mut lock = panels().lock().unwrap();
        lock.retain(|p| {
            if current_ids.contains(&&*p.element_id) { true }
            else {
                unsafe {
                    let _: () = msg_send![p.panel, orderOut: std::ptr::null_mut::<NSObject>()];
                    // Break retain cycle: nil out parent panel association before releasing
                    if !p.view.is_null() {
                        set_parent_panel(p.view, std::ptr::null_mut());
                    }
                    let _: () = msg_send![p.panel, release];
                    if !p.view.is_null() {
                        let _: () = msg_send![p.view, release];
                    }
                }
                false
            }
        });
    }
}

/// Build a CanvasUpdate from a TargetsUpdate, reusing the shared logic.
fn build_canvas_update(update: &TargetsUpdate) -> CanvasUpdate {
    let mut targets: Vec<TargetInfo> = Vec::with_capacity(update.targets.len());

    for t in &update.targets {
        targets.push(TargetInfo {
            element_id: t.element_id.clone(),
            screen_x: t.screen_x,
            screen_y: t.screen_y,
            width: t.width,
            _height: t.height,
            media_url: t.media_url.clone(),
            drag_offset_x: t.drag_offset_x,
            drag_offset_y: t.drag_offset_y,
            duration: t.duration,
            status: t.status,
            formats: t.formats.clone(),
            cookie: t.cookie.clone(),
        });
    }

    CanvasUpdate {
        tab_id: update.tab_id,
        viewport_screen_x: update.viewport_screen_x,
        viewport_screen_y: update.viewport_screen_y,
        viewport_width: update.viewport_width,
        viewport_height: update.viewport_height,
        device_pixel_ratio: update.device_pixel_ratio,
        targets,
        owner: 0,
        is_dark: IS_DARK.load(Ordering::Relaxed),
        referer: update.referer.clone(),
        user_agent: update.user_agent.clone(),
    }
}

/// Check the yt-dlp state machine: return element_ids that need async yt-dlp
/// resolution (new Pending targets not already cached as Resolving/Ready).
fn resolve_pending_targets(update: &TargetsUpdate) -> Vec<String> {
    let mut needs_resolve = Vec::new();
    let mut cache = target_cache().lock().unwrap();

    for t in &update.targets {
        if t.status != TargetStatus::Pending {
            // Extension has definitive status — update cache
            cache.insert(t.element_id.clone(), TargetCacheEntry {
                status: t.status,
                formats: t.formats.clone(),
                tab_id: update.tab_id,
                media_url: t.media_url.clone(),
                referer: update.referer.clone(),
                user_agent: update.user_agent.clone(),
                cookie: t.cookie.clone(),
                drag_offset_x: t.drag_offset_x,
                drag_offset_y: t.drag_offset_y,
            });
            continue;
        }

        // Pending — check if we already have a resolved/ing state
        let already_resolved = cache.get(&t.element_id)
            .map(|e| e.status == TargetStatus::Resolving || e.status == TargetStatus::Ready)
            .unwrap_or(false);

        if !already_resolved {
            // Mark as resolving in cache and queue for yt-dlp
            cache.insert(t.element_id.clone(), TargetCacheEntry {
                status: TargetStatus::Resolving,
                formats: Vec::new(),
                tab_id: update.tab_id,
                media_url: t.media_url.clone(),
                referer: update.referer.clone(),
                user_agent: update.user_agent.clone(),
                cookie: t.cookie.clone(),
                drag_offset_x: t.drag_offset_x,
                drag_offset_y: t.drag_offset_y,
            });
            needs_resolve.push(t.element_id.clone());
        }
    }

    needs_resolve
}

/// Hide all overlay panels.
pub fn hide() {
    Queue::main().exec_async(|| {
        hide_inner();
    });
}

fn hide_inner() {
    let lock = panels().lock().unwrap();
    for p in lock.iter() {
        unsafe {
            let _: () = msg_send![p.panel, orderOut: std::ptr::null_mut::<NSObject>()];
        }
    }
}

/// Destroy all overlay panels and release resources.
pub fn destroy() {
    log_msg("destroy");
    unsafe {
        let mut lock = panels().lock().unwrap();
        for p in lock.drain(..) {
            let _: () = msg_send![p.panel, orderOut: std::ptr::null_mut::<NSObject>()];
            // Break retain cycle: nil out parent panel association before releasing
            if !p.view.is_null() {
                set_parent_panel(p.view, std::ptr::null_mut());
            }
            let _: () = msg_send![p.panel, release];
            if !p.view.is_null() {
                let _: () = msg_send![p.view, release];
            }
        }
    }
}

// ── NSPanel creation ──────────────────────────────────────────────────────────

/// Create a single transparent NSPanel sized to just the HUD button area.
///
/// The panel is created borderless with a clear background, floating above
/// all normal windows, and only covers ~133×24 pt so mouse events pass
/// through to the browser video player naturally.
unsafe fn create_panel(x: f64, y: f64, w: f64, h: f64,
                       element_id: &str) -> *mut NSObject {
    let panel_cls = class!(NSPanel);
    let panel: *mut NSObject = msg_send![panel_cls, alloc];

    let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let panel: *mut NSObject = msg_send![panel, initWithContentRect: frame
                                               styleMask: NSWindowStyleMask::Borderless
                                                 backing: 2
                                                   defer: NO];

    if panel.is_null() {
        log_msg(&format!("create_panel: failed to alloc NSPanel for {}", element_id));
        return std::ptr::null_mut();
    }

    // Configure panel properties
    let _: () = msg_send![panel, setOpaque: NO];
    let clear: *mut NSObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![panel, setBackgroundColor: clear];
    let _: () = msg_send![panel, setIgnoresMouseEvents: NO];
    let _: () = msg_send![panel, setLevel: NSFloatingWindowLevel + 1];
    let _: () = msg_send![panel, setCollectionBehavior:
        NSWindowCollectionBehavior::CanJoinAllSpaces |
        NSWindowCollectionBehavior::Stationary];

    // Create the custom ButtonOverlayView as the content view
    let view_cls = class!(ButtonOverlayView);
    let view: *mut NSObject = msg_send![view_cls, alloc];
    let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
    let view: *mut NSObject = msg_send![view, initWithFrame: view_frame];

    if !view.is_null() {
        // Store parent panel + element_id via associated objects (ASSIGN policy
        // to avoid retain cycles — the panel retains the view as contentView).
        set_parent_panel(view, panel);
        set_element_id_str(view, element_id);
        let _: () = msg_send![panel, setContentView: view];

        // Update the panel's record to point to this view
        // Note: no manual retain — setContentView: retains the view,
        // and the panel is already at retain=1 from alloc/init.
        let mut lock = panels().lock().unwrap();
        if let Some(last) = lock.last_mut() {
            last.view = view;
        }

        // Show the panel
        let _: () = msg_send![panel, orderFront: std::ptr::null_mut::<NSObject>()];
    } else {
        log_msg("create_panel: ButtonOverlayView alloc/init failed");
        let _: () = msg_send![panel, release];
        return std::ptr::null_mut();
    }

    panel
}

// ── Dark mode detection ───────────────────────────────────────────────────────

fn detect_dark_mode() -> bool {
    unsafe {
        let stds: *mut NSObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let key = NSString::from_str("AppleInterfaceStyle");
        let value: *mut NSObject = msg_send![stds, stringForKey: &*key];
        if !value.is_null() {
            let dark_str = NSString::from_str("Dark");
            let is_dark: BOOL = msg_send![value, isEqualToString: &*dark_str];
            is_dark == YES
        } else {
            false
        }
    }
}

// ── Screen helpers ────────────────────────────────────────────────────────────

fn primary_screen_height() -> f64 {
    unsafe {
        let screens: *mut NSObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() { return 768.0; }
        let count: NSUInteger = msg_send![screens, count];
        if count == 0 { return 768.0; }
        let primary: *mut NSObject = msg_send![screens, objectAtIndex: 0];
        let frame: NSRect = msg_send![primary, frame];
        frame.size.height
    }
}

// ── yt-dlp integration ───────────────────────────────────────────────────────

/// Spawn yt-dlp in a background thread, then dispatch results to the main
/// thread to update the target cache and repaint the matching panel.
fn spawn_ytdlp_resolve(
    element_id: String,
    url: String,
    cookie: String,
    user_agent: String,
    referer: String,
) {
    log_msg(&format!("spawn_ytdlp_resolve: element_id={} url={}", element_id, url));

    std::thread::spawn(move || {
        let formats = run_ytdlp_macos(&url, &cookie, &user_agent, &referer);
        let found = formats.len();
        log_msg(&format!("spawn_ytdlp_resolve: found {} formats for {}", found, element_id));

        let eid = element_id.clone();
        Queue::main().exec_async(move || {
            // Update cache with resolved formats
            {
                let mut cache = target_cache().lock().unwrap();
                if let Some(entry) = cache.get_mut(&eid) {
                    entry.status = TargetStatus::Ready;
                    entry.formats = formats;
                }
            }
            // Force repaint of the matching view
            unsafe {
                let lock = panels().lock().unwrap();
                for p in lock.iter() {
                    if p.element_id == eid {
                        let _: () = msg_send![p.view, setNeedsDisplay: YES];
                    }
                }
            }
        });
    });
}

fn find_ytdlp_path() -> Option<String> {
    let path_check = std::process::Command::new("which")
        .arg("yt-dlp")
        .output();
    if let Ok(out) = path_check {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }
    if std::path::Path::new("/opt/homebrew/bin/yt-dlp").exists() {
        return Some("/opt/homebrew/bin/yt-dlp".to_string());
    }
    if std::path::Path::new("/usr/local/bin/yt-dlp").exists() {
        return Some("/usr/local/bin/yt-dlp".to_string());
    }
    None
}

fn run_ytdlp_macos(url: &str, cookie: &str, user_agent: &str, referer: &str) -> Vec<FormatInfo> {
    let ytpath = match find_ytdlp_path() {
        Some(p) => p,
        None => {
            log_msg("yt-dlp not found on this system");
            return Vec::new();
        }
    };

    let mut args = vec![
        "--dump-json".to_string(),
        "--socket-timeout".to_string(), "5".to_string(),
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

    log_msg(&format!("spawning yt-dlp: {} {}", ytpath, args.join(" ")));

    let child = std::process::Command::new(&ytpath)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            log_msg(&format!("yt-dlp spawn failed: {:?}", e));
            return Vec::new();
        }
    };

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = match rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            log_msg(&format!("yt-dlp wait failed: {:?}", e));
            return Vec::new();
        }
        Err(_) => {
            log_msg("yt-dlp timed out after 15s");
            return Vec::new();
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_msg(&format!("yt-dlp failed: {}", stderr));
        return Vec::new();
    }

    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(_) => {
            log_msg("yt-dlp stdout not valid UTF-8");
            return Vec::new();
        }
    };

    parse_ytdlp_output(&stdout)
}

// ── Custom NSView subclass: ButtonOverlayView ─────────────────────────────────
//
// This view is the content view of each NSPanel. It draws the HUD button
// (logo + text pill + X pill) using Core Graphics in drawRect: and handles
// mouse events for drag, quality menu, and dismiss.
//
// Ivars (declared in register_button_overlay_view):
//   _parentPanel  *mut NSObject  — owning NSPanel (ASSIGN, not retained)

unsafe extern "C" fn button_overlay_view_draw_rect(this: *mut AnyObject, _: Sel, _dirty_rect: NSRect) {
    let this: &NSObject = unsafe { &*(this as *mut NSObject) };
    unsafe {
        let ctx: *mut NSObject = msg_send![class!(NSGraphicsContext), currentContext];
        if ctx.is_null() { return; }
        let cg: *mut c_void = msg_send![ctx, CGContext];
        if cg.is_null() { return; }

        let _bounds: NSRect = msg_send![this, bounds];
        let is_dark = IS_DARK.load(Ordering::Relaxed);

        // ── Colors ────────────────────────────────────────────────────────
        let bg_r: f64 = if is_dark { 0.15 } else { 0.95 };
        let bg_g: f64 = if is_dark { 0.15 } else { 0.95 };
        let bg_b: f64 = if is_dark { 0.15 } else { 0.95 };
        let bg_a: f64 = 0.92;
        let text_r: f64 = if is_dark { 1.0 } else { 0.1 };
        let text_g: f64 = if is_dark { 1.0 } else { 0.1 };
        let text_b: f64 = if is_dark { 1.0 } else { 0.1 };
        let accent_r: f64 = 0.2;
        let accent_g: f64 = 0.5;
        let accent_b: f64 = 1.0;

        // ── Draw container rounded rect ───────────────────────────────────
        let container_w = container_width();
        // Since the panel is exactly container_w × HUD_H, the container fills
        // the entire bounds.
        let container_x = 0.0;
        let container_y = 0.0;

        CGContextSetRGBFillColor(cg, bg_r, bg_g, bg_b, bg_a);
        let r = PILL_R;
        CGContextBeginPath(cg);
        CGContextMoveToPoint(cg, container_x + r, container_y);
        CGContextAddLineToPoint(cg, container_x + container_w - r, container_y);
        CGContextAddArcToPoint(cg, container_x + container_w, container_y,
                               container_x + container_w, container_y + r, r);
        CGContextAddLineToPoint(cg, container_x + container_w, container_y + HUD_H - r);
        CGContextAddArcToPoint(cg, container_x + container_w, container_y + HUD_H,
                               container_x + container_w - r, container_y + HUD_H, r);
        CGContextAddLineToPoint(cg, container_x + r, container_y + HUD_H);
        CGContextAddArcToPoint(cg, container_x, container_y + HUD_H,
                               container_x, container_y + HUD_H - r, r);
        CGContextAddLineToPoint(cg, container_x, container_y + r);
        CGContextAddArcToPoint(cg, container_x, container_y,
                               container_x + r, container_y, r);
        CGContextClosePath(cg);
        CGContextFillPath(cg);

        // ── Draw logo circle ──────────────────────────────────────────────
        let logo_cx = container_x + LOGO_L + LOGO_SZ / 2.0;
        let logo_cy = container_y + HUD_H / 2.0;
        let logo_r = LOGO_SZ / 2.0 - 1.0;

        CGContextSetRGBFillColor(cg, accent_r, accent_g, accent_b, 1.0);
        CGContextBeginPath(cg);
        CGContextAddArc(cg, logo_cx, logo_cy, logo_r, 0.0, 2.0 * std::f64::consts::PI, 0);
        CGContextFillPath(cg);

        CGContextSetRGBFillColor(cg, 1.0, 1.0, 1.0, 1.0);
        CGContextSelectFont(cg, b"Helvetica\0" as *const u8 as *const i8, 10.0, kCGEncodingMacRoman);
        CGContextShowTextAtPoint(cg, logo_cx - 3.0, logo_cy - 4.0, b"T\0" as *const u8 as *const i8, 1);

        // ── Draw text pill ────────────────────────────────────────────────
        let text_pill_x = container_x + PILL_START_X;
        let text_pill_w = 80.0;
        let text_pill_h = HUD_H - 4.0;
        let text_pill_y = container_y + 2.0;

        CGContextSetRGBFillColor(cg, 0.3, 0.3, 0.3, 0.3);
        let r2 = PILL_R;
        CGContextBeginPath(cg);
        CGContextMoveToPoint(cg, text_pill_x + r2, text_pill_y);
        CGContextAddLineToPoint(cg, text_pill_x + text_pill_w - r2, text_pill_y);
        CGContextAddArcToPoint(cg, text_pill_x + text_pill_w, text_pill_y,
                               text_pill_x + text_pill_w, text_pill_y + r2, r2);
        CGContextAddLineToPoint(cg, text_pill_x + text_pill_w, text_pill_y + text_pill_h - r2);
        CGContextAddArcToPoint(cg, text_pill_x + text_pill_w, text_pill_y + text_pill_h,
                               text_pill_x + text_pill_w - r2, text_pill_y + text_pill_h, r2);
        CGContextAddLineToPoint(cg, text_pill_x + r2, text_pill_y + text_pill_h);
        CGContextAddArcToPoint(cg, text_pill_x, text_pill_y + text_pill_h,
                               text_pill_x, text_pill_y + text_pill_h - r2, r2);
        CGContextAddLineToPoint(cg, text_pill_x, text_pill_y + r2);
        CGContextAddArcToPoint(cg, text_pill_x, text_pill_y,
                               text_pill_x + r2, text_pill_y, r2);
        CGContextClosePath(cg);
        CGContextFillPath(cg);

        // Draw "Download" text
        CGContextSetRGBFillColor(cg, text_r, text_g, text_b, 0.9);
        CGContextSelectFont(cg, b"Helvetica\0" as *const u8 as *const i8, FONT_SIZE, kCGEncodingMacRoman);
        CGContextShowTextAtPoint(cg, text_pill_x + PILL_PAD_X,
                                 text_pill_y + PILL_PAD_X + 1.0,
                                 b"Download\0" as *const u8 as *const i8, 8);

        // ── Draw X pill ───────────────────────────────────────────────────
        let x_pill_x = text_pill_x + text_pill_w + PILL_GAP_M;
        CGContextSetRGBFillColor(cg, 0.5, 0.5, 0.5, 0.3);
        let r3 = PILL_R;
        CGContextBeginPath(cg);
        CGContextMoveToPoint(cg, x_pill_x + r3, text_pill_y);
        CGContextAddLineToPoint(cg, x_pill_x + X_W - r3, text_pill_y);
        CGContextAddArcToPoint(cg, x_pill_x + X_W, text_pill_y,
                               x_pill_x + X_W, text_pill_y + r3, r3);
        CGContextAddLineToPoint(cg, x_pill_x + X_W, text_pill_y + text_pill_h - r3);
        CGContextAddArcToPoint(cg, x_pill_x + X_W, text_pill_y + text_pill_h,
                               x_pill_x + X_W - r3, text_pill_y + text_pill_h, r3);
        CGContextAddLineToPoint(cg, x_pill_x + r3, text_pill_y + text_pill_h);
        CGContextAddArcToPoint(cg, x_pill_x, text_pill_y + text_pill_h,
                               x_pill_x, text_pill_y + text_pill_h - r3, r3);
        CGContextAddLineToPoint(cg, x_pill_x, text_pill_y + r3);
        CGContextAddArcToPoint(cg, x_pill_x, text_pill_y,
                               x_pill_x + r3, text_pill_y, r3);
        CGContextClosePath(cg);
        CGContextFillPath(cg);

        // Draw "×" symbol (MacRoman: 0xD7)
        CGContextSetRGBFillColor(cg, text_r, text_g, text_b, 0.8);
        CGContextSelectFont(cg, b"Helvetica\0" as *const u8 as *const i8, 12.0, kCGEncodingMacRoman);
        CGContextShowTextAtPoint(cg, x_pill_x + 7.0, text_pill_y + 3.0,
                                 b"\xD7\0" as *const u8 as *const i8, 1);
    }
}

unsafe extern "C" fn button_overlay_view_mouse_down(this: *mut AnyObject, _: Sel, event: *mut NSObject) {
    let this: &NSObject = unsafe { &*(this as *mut NSObject) };
    unsafe {
        let loc: NSPoint = msg_send![event, locationInWindow];

        // Hit test zones (same layout as drawRect — panel is container_w × HUD_H)
        let container_w = container_width();
        let container_x = 0.0;
        let container_y = 0.0;
        let text_pill_x = container_x + PILL_START_X;
        let text_pill_w = 80.0;
        let x_pill_x = text_pill_x + text_pill_w + PILL_GAP_M;

        if loc.x >= container_x && loc.x < container_x + container_w
            && loc.y >= container_y && loc.y < container_y + HUD_H
        {
            if loc.x < PILL_START_X + container_x {
                // Drag zone (logo area) — pass through to NSView drag tracking
                log_msg("mouseDown: drag zone");
            } else if loc.x >= text_pill_x && loc.x < text_pill_x + text_pill_w {
                // TextPill — show quality menu
                log_msg("mouseDown: text pill — showing menu");
                show_quality_menu(this, event);
                // Don't pass to super, menu handles it
                return;
            } else if loc.x >= x_pill_x && loc.x < x_pill_x + X_W {
                // X pill — dismiss
                log_msg("mouseDown: X pill — dismiss");
                let panel = get_parent_panel(this as *const NSObject as *mut NSObject);
                if !panel.is_null() {
                    let _: () = msg_send![panel, orderOut: std::ptr::null_mut::<NSObject>()];
                }
                return;
            }
        }

        // Pass to super for drag tracking (NSView handles mouseDragged:)
        let superclass = class!(NSView);
        let _: () = msg_send![super(this, superclass), mouseDown: event];
    }
}

unsafe extern "C" fn button_overlay_view_mouse_dragged(this: *mut AnyObject, _: Sel, event: *mut NSObject) {
    let this: &NSObject = unsafe { &*(this as *mut NSObject) };
    unsafe {
        let panel = get_parent_panel(this as *const NSObject as *mut NSObject);
        if panel.is_null() { return; }

        let delta: NSPoint = msg_send![event, deltaInWindow];
        let current_origin: NSPoint = msg_send![panel, frameOrigin];
        let new_origin = NSPoint::new(current_origin.x + delta.x,
                                       current_origin.y - delta.y); // Y-flip drag
        let _: () = msg_send![panel, setFrameOrigin: new_origin];
    }
}

unsafe extern "C" fn button_overlay_view_mouse_up(this: *mut AnyObject, _: Sel, event: *mut NSObject) {
    let this: &NSObject = unsafe { &*(this as *mut NSObject) };
    unsafe {
        log_msg("mouseUp: drag committed");

        let panel = get_parent_panel(this as *const NSObject as *mut NSObject);
        if !panel.is_null() {
            // Read current panel frame origin
            let current_origin: NSPoint = msg_send![panel, frameOrigin];

            // Look up the element we belong to
            let eid = get_element_id_str(this as *const NSObject as *mut NSObject);
            if let Some(eid) = eid {
                // Find panel record to get current video element geometry
                let geom = {
                    let lock = panels().lock().unwrap();
                    lock.iter().find(|p| p.element_id == eid).map(|p| {
                        (p.screen_x, p.screen_y, p.width, p.height)
                    })
                };

                if let Some((sx, sy, w, _h)) = geom {
                    // Compute the anchor position (no-drag position in Cocoa)
                    let screen_h = primary_screen_height();
                    let cw = container_width();
                    let css_hud_x = sx as f64 + w as f64 - cw;
                    let cocoa_hud_bottom = screen_h - sy as f64 + HUD_GAP;

                    let anchor_x = css_hud_x;
                    let anchor_y = cocoa_hud_bottom;

                    // Drag delta in Cocoa coordinates
                    let cocoa_dx = current_origin.x - anchor_x;
                    let cocoa_dy = current_origin.y - anchor_y;

                    // Convert to CSS coordinates: X is same, Y is flipped
                    let css_dx = cocoa_dx as i32;
                    let css_dy = -(cocoa_dy as i32);

                    log_msg(&format!("mouseUp: sending OVERLAY_DRAG_MOVED dx={} dy={}", css_dx, css_dy));

                    // Read tab_id and media_url from cache
                    let (tab_id, media_url) = {
                        let cache = target_cache().lock().unwrap();
                        cache.get(&eid).map(|e| (e.tab_id, e.media_url.clone()))
                            .unwrap_or((0, String::new()))
                    };

                    // Send OVERLAY_DRAG_MOVED
                    write_response(&serde_json::json!({
                        "type": "OVERLAY_DRAG_MOVED",
                        "tabId": tab_id,
                        "elementId": eid,
                        "mediaUrl": media_url,
                        "dx": css_dx,
                        "dy": css_dy,
                    }));

                    // Update cache with new drag offset
                    let mut cache = target_cache().lock().unwrap();
                    if let Some(entry) = cache.get_mut(&eid) {
                        entry.drag_offset_x = css_dx;
                        entry.drag_offset_y = css_dy;
                    }
                }
            }
        }

        let superclass = class!(NSView);
        let _: () = msg_send![super(this, superclass), mouseUp: event];
    }
}

// ── Quality Menu ──────────────────────────────────────────────────────────────

unsafe fn show_quality_menu(view: &NSObject, event: *mut NSObject) {
    // Look up cached target metadata for this view
    let eid = get_element_id_str(view as *const NSObject as *mut NSObject);
    let cache_data = eid.as_ref().and_then(|eid| {
        let cache = target_cache().lock().unwrap();
        cache.get(eid).map(|e| {
            (e.formats.clone(), e.tab_id, e.media_url.clone(),
             e.referer.clone(), e.user_agent.clone(), e.cookie.clone())
        })
    });        let (formats, _tab_id, _media_url, _referer, _user_agent, _cookie) = cache_data.unwrap_or_default();

    let quality_title = NSString::from_str("Quality");
    let menu: *mut NSObject = msg_send![class!(NSMenu), alloc];
    let menu: *mut NSObject = msg_send![menu, initWithTitle: &*quality_title];

    // Build menu from resolved formats (or fallback default items)
    if formats.is_empty() {
        // No resolved formats — show static defaults (user can still click,
        // but the download will use the raw media URL)
        let items = [
            "Download (Default Quality)",
            "Download (720p)",
            "Download (480p)",
            "Download (Audio only)",
        ];
        for &title in &items {
            add_menu_item(menu, view, title);
        }
    } else {
        for (idx, f) in formats.iter().enumerate() {
            let label = format!("{}. {}", idx + 1, f.label);
            add_menu_item(menu, view, &label);
        }
    }

    // Separator + Copy Media URL
    let empty_str = NSString::from_str("");
    let sep: *mut NSObject = msg_send![class!(NSMenuItem), separatorItem];
    let _: () = msg_send![menu, addItem: sep];

    let copy_item: *mut NSObject = msg_send![class!(NSMenuItem), alloc];
    let copy_title = NSString::from_str("Copy Media URL");
    let copy_item: *mut NSObject = msg_send![copy_item, initWithTitle: &*copy_title
                                                       action: sel!(copyUrlSelected:)
                                                keyEquivalent: &*empty_str];
    let _: () = msg_send![menu, addItem: copy_item];
    let _: () = msg_send![copy_item, setTarget: view as *const NSObject as *mut NSObject];
    let _: () = msg_send![copy_item, release];

    // Show at click location in screen coordinates
    let win: *mut NSObject = msg_send![event, window];
    let loc_in_win: NSPoint = msg_send![event, locationInWindow];
    let loc_on_screen: NSPoint = msg_send![win, convertPointToScreen: loc_in_win];

    let _: () = msg_send![menu, popUpMenuPositioningItem: std::ptr::null_mut::<NSObject>()
                                       atLocation: loc_on_screen
                                           inView: view as *const NSObject as *mut NSObject];
    let _: () = msg_send![menu, release];
}

unsafe fn add_menu_item(menu: *mut NSObject, view: &NSObject, title: &str) {
    let item: *mut NSObject = msg_send![class!(NSMenuItem), alloc];
    let item_title = NSString::from_str(title);
    let empty_key = NSString::from_str("");
    let item: *mut NSObject = msg_send![item, initWithTitle: &*item_title
                                               action: sel!(menuItemSelected:)
                                        keyEquivalent: &*empty_key];
    let _: () = msg_send![menu, addItem: item];
    let _: () = msg_send![item, setTarget: view as *const NSObject as *mut NSObject];
    let _: () = msg_send![item, release];
}

unsafe extern "C" fn button_overlay_view_menu_item_selected(this: *mut AnyObject, _: Sel, sender: *mut NSObject) {
    let this: &NSObject = unsafe { &*(this as *mut NSObject) };
    unsafe {
        let title: *mut NSObject = msg_send![sender, title];
        let title_str: *mut NSString = msg_send![title, description];
        let utf8: *mut i8 = msg_send![title_str, UTF8String];
        if utf8.is_null() { return; }
        let title = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
        log_msg(&format!("menu item selected: {}", title));

        // Look up cached metadata for this view's element
        let eid = get_element_id_str(this as *const NSObject as *mut NSObject);
        let meta = eid.as_ref().and_then(|eid| {
            let cache = target_cache().lock().unwrap();
            cache.get(eid).map(|e| {
                (e.formats.clone(), e.tab_id, e.media_url.clone(),
                 e.referer.clone(), e.user_agent.clone(), e.cookie.clone())
            })
        });

        if let Some((formats, tab_id, media_url, referer, user_agent, cookie)) = meta {
            // Determine which format index was selected (format items are numbered "1. ...")
            let selected_idx = if title.contains(". ") {
                title.split('.').next()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .map(|i| i.wrapping_sub(1))
            } else {
                None
            };

            let (video_url, audio_url) = match selected_idx {
                Some(idx) if idx < formats.len() => {
                    (formats[idx].video_url.clone(), formats[idx].audio_url.clone())
                }
                _ => (media_url.clone(), String::new()),
            };

            log_msg(&format!("menu item: sending OVERLAY_DOWNLOAD_TRIGGER tab={} video={}", tab_id, video_url));

            write_response(&serde_json::json!({
                "type": "OVERLAY_DOWNLOAD_TRIGGER",
                "tabId": tab_id,
                "elementId": eid.unwrap_or_default(),
                "videoUrl": video_url,
                "audioUrl": audio_url,
                "headers": {
                    "User-Agent": user_agent,
                    "Cookie": cookie,
                    "Referer": referer,
                }
            }));
        }
    }
}

unsafe extern "C" fn button_overlay_view_copy_url(this: *mut AnyObject, _: Sel, _sender: *mut NSObject) {
    unsafe {
        let eid = get_element_id_str(this as *const NSObject as *mut NSObject);
        let media_url = eid.as_ref().and_then(|eid| {
            let cache = target_cache().lock().unwrap();
            cache.get(eid).map(|e| e.media_url.clone())
        }).unwrap_or_default();

        if !media_url.is_empty() {
            // Copy to clipboard via NSPasteboard
            let pb: *mut NSObject = msg_send![class!(NSPasteboard), generalPasteboard];
            let _: () = msg_send![pb, clearContents];
            let nsstr = NSString::from_str(&media_url);
            let arr: *mut NSObject = msg_send![class!(NSArray), arrayWithObject: &*nsstr];
            let _: BOOL = msg_send![pb, writeObjects: arr];

            log_msg(&format!("copyUrl: {}", media_url));
        }
    }
}

// ── CGContext raw FFI bindings ────────────────────────────────────────────────

type CGContextRef = *mut c_void;

extern "C" {
    fn CGContextSetRGBFillColor(c: CGContextRef, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetRGBStrokeColor(c: CGContextRef, r: f64, g: f64, b: f64, a: f64);
    fn CGContextBeginPath(c: CGContextRef);
    fn CGContextMoveToPoint(c: CGContextRef, x: f64, y: f64);
    fn CGContextAddLineToPoint(c: CGContextRef, x: f64, y: f64);
    fn CGContextAddArcToPoint(c: CGContextRef, x1: f64, y1: f64, x2: f64, y2: f64, r: f64);
    fn CGContextAddArc(c: CGContextRef, cx: f64, cy: f64, r: f64, sa: f64, ea: f64, cw: i32);
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
    fn CGContextStrokePath(c: CGContextRef);
    fn CGContextSelectFont(c: CGContextRef, name: *const i8, size: f64, encoding: i32);
    fn CGContextShowTextAtPoint(c: CGContextRef, x: f64, y: f64, text: *const i8, len: usize);
}

#[allow(non_upper_case_globals)]
const kCGEncodingMacRoman: i32 = 0;
#[cfg(target_arch = "aarch64")]
const YES: BOOL = true;
#[cfg(target_arch = "aarch64")]
const NO:  BOOL = false;
#[cfg(not(target_arch = "aarch64"))]
const YES: BOOL = 1i8 as BOOL;
#[cfg(not(target_arch = "aarch64"))]
const NO:  BOOL = 0i8 as BOOL;

// ── Associated Object helpers ────────────────────────────────────────────────
// Uses RETAIN policy so the parent panel pointer is always valid.
// The retain cycle (panel→view via contentView, view→panel via associated
// object) is broken explicitly in destroy() by nil-ing the association
// before releasing the panel.

const OBJC_ASSOCIATION_RETAIN: usize = 0x301;

extern "C" {
    fn objc_setAssociatedObject(
        object: *mut NSObject,
        key: *const c_void,
        value: *mut NSObject,
        policy: usize,
    );
    fn objc_getAssociatedObject(
        object: *mut NSObject,
        key: *const c_void,
    ) -> *mut NSObject;
}

fn parent_panel_key() -> *const c_void {
    static KEY: u8 = 0;
    &KEY as *const u8 as *const c_void
}

fn element_id_key() -> *const c_void {
    static KEY: u8 = 1;
    &KEY as *const u8 as *const c_void
}

unsafe fn set_parent_panel(view: *mut NSObject, panel: *mut NSObject) {
    objc_setAssociatedObject(view, parent_panel_key(), panel, OBJC_ASSOCIATION_RETAIN);
}

unsafe fn get_parent_panel(view: *mut NSObject) -> *mut NSObject {
    objc_getAssociatedObject(view, parent_panel_key())
}

unsafe fn set_element_id_str(view: *mut NSObject, eid: &str) {
    let nsstr = NSString::from_str(eid);
    // NSString is retained by the associated object (RETAIN policy on the string itself)
    objc_setAssociatedObject(view, element_id_key(), &*nsstr as *const NSString as *mut NSObject, 0x301);
}

unsafe fn get_element_id_str(view: *mut NSObject) -> Option<String> {
    let obj = objc_getAssociatedObject(view, element_id_key());
    if obj.is_null() { return None; }
    let nsstr: *mut NSString = obj as *mut NSString;
    let utf8: *mut i8 = msg_send![nsstr, UTF8String];
    if utf8.is_null() { return None; }
    let s = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
    Some(s)
}

// ── Class registration: ButtonOverlayView ─────────────────────────────────────

fn register_button_overlay_view() {
    let super_cls = class!(NSView);
    let mut builder = ClassBuilder::new("ButtonOverlayView", super_cls)
        .expect("Failed to allocate ButtonOverlayView class");

    unsafe {
        builder.add_ivar::<*mut NSObject>("_parentPanel");

        builder.add_method(sel!(drawRect:),
            button_overlay_view_draw_rect as unsafe extern "C" fn(*mut AnyObject, Sel, NSRect));
        builder.add_method(sel!(mouseDown:),
            button_overlay_view_mouse_down as unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSObject));
        builder.add_method(sel!(mouseDragged:),
            button_overlay_view_mouse_dragged as unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSObject));
        builder.add_method(sel!(mouseUp:),
            button_overlay_view_mouse_up as unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSObject));
        builder.add_method(sel!(menuItemSelected:),
            button_overlay_view_menu_item_selected as unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSObject));
        builder.add_method(sel!(copyUrlSelected:),
            button_overlay_view_copy_url as unsafe extern "C" fn(*mut AnyObject, Sel, *mut NSObject));

        builder.register();
    }
    log_msg("ButtonOverlayView class registered");
}

fn ensure_view_class() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        register_button_overlay_view();
    });
}
