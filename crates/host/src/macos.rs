// macOS AppKit NSPanel overlay skeleton.
//
// Mirrors the public API of overlay.rs so that main.rs can dispatch to either
// implementation via #[cfg(target_os = "...")].
//
// Current state: stub-only — logs to stderr, does not paint anything.
// Intended as the compilation-safe foundation for a full AppKit implementation.
//
// Architecture notes (for the future full implementation):
//   - NSPanel with NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
//     is the AppKit equivalent of WS_EX_LAYERED | WS_EX_TOOLWINDOW.
//   - nonactivatingPanel mirrors the Win32 WM_MOUSEACTIVATE → MA_NOACTIVATE
//     behaviour: the user can click overlay buttons without stealing focus from
//     the browser.
//   - Coordinate space: macOS uses bottom-left origin. Convert incoming CSS-pixel
//     screen_y with:  macos_y = NSScreen.main.frame.height - screen_y - height
//   - For transparency: set the panel's backgroundColor to NSColor.clear and
//     enable isOpaque = false. No chroma-key required — AppKit composites natively.

#![cfg(target_os = "macos")]

use crate::types::{CanvasUpdate, TargetInfo};

// Re-export TargetInfo so callers don't need to qualify the path.
pub use crate::types::TargetInfo as MacosTargetInfo;

// ── public API ───────────────────────────────────────────────────────────────

/// Initialise the AppKit overlay subsystem.
/// Must be called once from the main thread before any other overlay call.
pub fn init() {
    eprintln!("[tur/macos] overlay::init — stub (NSPanel not yet created)");
    // Future: call NSApplication.sharedApplication, set activation policy to
    // NSApplicationActivationPolicyAccessory so the app has no Dock icon.
}

/// Create or update the overlay panel to show buttons at the given geometry.
pub fn update(update: CanvasUpdate) {
    if update.targets.is_empty() || update.viewport_width <= 0 || update.viewport_height <= 0 {
        hide();
        return;
    }

    eprintln!(
        "[tur/macos] overlay::update — stub  tab={} targets={} viewport=({},{} {}x{}) dpr={}",
        update.tab_id,
        update.targets.len(),
        update.viewport_screen_x,
        update.viewport_screen_y,
        update.viewport_width,
        update.viewport_height,
        update.device_pixel_ratio,
    );

    // ── Coordinate conversion (reference impl, not yet wired to real AppKit) ──
    // AppKit Y origin is bottom-left; CSS/Win32 Y origin is top-left.
    // When implementing for real:
    //
    //   let screen_height = NSScreen::mainScreen().frame().size.height;
    //   let macos_y = screen_height
    //       - update.viewport_screen_y as f64
    //       - update.viewport_height as f64;
    //
    // Then set the panel frame:
    //   panel.setFrame_display(NSRect::new(
    //       NSPoint::new(update.viewport_screen_x as f64, macos_y),
    //       NSSize::new(update.viewport_width as f64, update.viewport_height as f64),
    //   ), true);

    for (i, t) in update.targets.iter().enumerate() {
        eprintln!(
            "[tur/macos] overlay:   [{}] id={} sx={} sy={} w={}",
            i, t.element_id, t.screen_x, t.screen_y, t.width
        );
    }
}

/// Hide the overlay panel without destroying it.
pub fn hide() {
    eprintln!("[tur/macos] overlay::hide — stub");
    // Future: panel.orderOut(None);
}

/// Destroy the overlay panel and release AppKit resources.
pub fn destroy() {
    eprintln!("[tur/macos] overlay::destroy — stub");
    // Future: panel.close(); drop panel reference.
}
