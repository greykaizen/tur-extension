// crates/host/src/overlay/menu.rs

use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};
use windows::core::{PCWSTR, PCSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::overlay::write_response;

/// Custom message posted to the canvas window to trigger deferred menu open.
/// This ensures the menu is shown from within the canvas wndproc where
/// SetForegroundWindow actually works (process has mouse/key ownership).
pub const WM_APP_SHOW_PENDING_MENU: u32 = WM_APP + 1;

// ── Deferred menu: saved click params for while yt-dlp is still resolving ────

#[derive(Clone)]
pub struct PendingMenu {
    pub element_id: String,
    pub hwnd: isize,
    pub pt_x: i32,
    pub pt_y: i32,
    pub tab_id: i32,
    pub media_url: String,
}

pub fn pending_menu() -> &'static Mutex<Option<PendingMenu>> {
    static PM: OnceLock<Mutex<Option<PendingMenu>>> = OnceLock::new();
    PM.get_or_init(|| Mutex::new(None))
}

/// Called from WM_USER_TARGET_READY after formats are written into canvas state.
/// Posts WM_APP_SHOW_PENDING_MENU to the CANVAS window rather than calling
/// show_quality_menu directly — this is critical because the canvas wndproc
/// has the correct foreground/input context for TrackPopupMenu to work.
pub unsafe fn fire_pending_menu(element_id: &str) {
    let saved = {
        let mut lock = pending_menu().lock().unwrap();
        if lock.as_ref().map(|p| p.element_id == element_id).unwrap_or(false) {
            lock.take()
        } else {
            None
        }
    };

    if let Some(p) = saved {
        // Post to the CANVAS hwnd (stored in p.hwnd) so the menu appears
        // in the correct window message context with valid foreground rights.
        let canvas_hwnd = HWND(p.hwnd as *mut std::ffi::c_void);
        // Re-store params for canvas_wndproc to pick up
        {
            let mut lock = pending_menu().lock().unwrap();
            *lock = Some(p);
        }
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            canvas_hwnd,
            WM_APP_SHOW_PENDING_MENU,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Clear any stale pending-menu state (e.g. when the overlay is reset/dismissed).
pub fn cancel_pending_menu() {
    if let Ok(mut lock) = pending_menu().lock() {
        *lock = None;
    }
}
pub unsafe fn apply_menu_dark_mode(hwnd: HWND, enable: bool) {
    let uxtheme_name: Vec<u16> = "uxtheme.dll".encode_utf16().chain(std::iter::once(0)).collect();
    if let Ok(hmodule) = windows::Win32::System::LibraryLoader::LoadLibraryW(PCWSTR(uxtheme_name.as_ptr())) {
        // SetPreferredAppMode (Ordinal 135)
        if let Some(set_preferred_app_mode) = windows::Win32::System::LibraryLoader::GetProcAddress(
            hmodule,
            PCSTR(135 as *const u8),
        ) {
            let set_preferred_app_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(set_preferred_app_mode);
            let mode = if enable { 2 } else { 3 }; // 2 is ForceDark, 3 is ForceLight
            set_preferred_app_mode(mode);
        }

        // FlushMenuThemes (Ordinal 136)
        if let Some(flush_menu_themes) = windows::Win32::System::LibraryLoader::GetProcAddress(
            hmodule,
            PCSTR(136 as *const u8),
        ) {
            let flush_menu_themes: extern "system" fn() = std::mem::transmute(flush_menu_themes);
            flush_menu_themes();
        }

        // SetWindowTheme (from uxtheme.dll)
        if let Some(set_window_theme) = windows::Win32::System::LibraryLoader::GetProcAddress(
            hmodule,
            PCSTR("SetWindowTheme\0".as_ptr()),
        ) {
            let set_window_theme: extern "system" fn(HWND, PCWSTR, PCWSTR) -> windows::core::HRESULT = std::mem::transmute(set_window_theme);
            if enable {
                let explorer: Vec<u16> = "Explorer\0".encode_utf16().collect();
                let _ = set_window_theme(hwnd, PCWSTR(explorer.as_ptr()), PCWSTR(std::ptr::null()));
            } else {
                let _ = set_window_theme(hwnd, PCWSTR(std::ptr::null()), PCWSTR(std::ptr::null()));
            }

            // Also execute on the parent controller HWND to flush non-client borders
            let controller_val = crate::CONTROLLER_HWND.load(std::sync::atomic::Ordering::Acquire);
            if controller_val != 0 {
                let controller_hwnd = HWND(controller_val as *mut std::ffi::c_void);
                if enable {
                    let explorer: Vec<u16> = "Explorer\0".encode_utf16().collect();
                    let _ = set_window_theme(controller_hwnd, PCWSTR(explorer.as_ptr()), PCWSTR(std::ptr::null()));
                } else {
                    let _ = set_window_theme(controller_hwnd, PCWSTR(std::ptr::null()), PCWSTR(std::ptr::null()));
                }
            }
        }
    }
}

/// quality popup menu (anchored to button bottom-left)
pub unsafe fn show_quality_menu(
    hwnd: HWND,
    pt: POINT,
    element_id: &str,
    tab_id: i32,
    media_url: &str,
) {
    // Lock canvas state briefly to copy the status, formats, cookie, referer, user_agent, is_dark
    let (status, formats, cookie, referer, user_agent, is_dark) = {
        let lock = crate::overlay::canvas().lock().unwrap();
        if let Some(ref state) = *lock {
            if let Some(target) = state.targets.iter().find(|t| t.element_id == element_id) {
                (
                    target.status,
                    target.formats.clone(),
                    target.cookie.clone(),
                    state.referer.clone(),
                    state.user_agent.clone(),
                    state.is_dark,
                )
            } else {
                return; // target not found
            }
        } else {
            return;
        }
    };

    macro_rules! wstr {
        ($s:expr) => {{
            let v: Vec<u16> = $s.encode_utf16().chain(std::iter::once(0)).collect();
            v
        }};
    }

    // ── If still resolving: save click silently, show nothing ─────────────────
    // The button label already communicates the resolving state.
    // show_quality_menu will be called automatically via WM_APP_SHOW_PENDING_MENU
    // once yt-dlp finishes — no annoying popup needed here.
    if status == crate::types::TargetStatus::Resolving {
        let mut lock = pending_menu().lock().unwrap();
        *lock = Some(PendingMenu {
            element_id: element_id.to_string(),
            hwnd: hwnd.0 as isize,
            pt_x: pt.x,
            pt_y: pt.y,
            tab_id,
            media_url: media_url.to_string(),
        });
        // Don't show any popup — return immediately.
        return;
    }

    // ── Normal path: formats are ready ───────────────────────────────────────
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };

    // Apply dark mode theme if enabled
    apply_menu_dark_mode(hwnd, is_dark);

    // Dynamic profile listing
    if formats.is_empty() {
        let label = wstr!("Download (Default Quality)");
        let _ = AppendMenuW(menu, MF_STRING, 10000, PCWSTR(label.as_ptr()));
    } else {
        for (idx, f) in formats.iter().enumerate() {
            let label_str = format!("{}. {}", idx + 1, f.label);
            let label = wstr!(&label_str);
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                10000 + idx,
                PCWSTR(label.as_ptr()),
            );
        }
    }

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null_mut()));
    let copy_label = wstr!("Copy Media URL");
    let _ = AppendMenuW(menu, MF_STRING, 9999, PCWSTR(copy_label.as_ptr()));

    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    let cmd = cmd.0 as usize;
    if cmd == 0 {
        return;
    }

    if cmd == 9999 {
        let text_to_copy = if formats.is_empty() {
            media_url
        } else {
            &formats[0].video_url
        };
        copy_to_clipboard(text_to_copy);
    } else if cmd >= 10000 {
        let idx = cmd - 10000;
        let (video_url, audio_url) = if formats.is_empty() {
            (media_url.to_string(), String::new())
        } else if idx < formats.len() {
            (formats[idx].video_url.clone(), formats[idx].audio_url.clone())
        } else {
            return;
        };

        write_response(&serde_json::json!({
            "type": "OVERLAY_DOWNLOAD_TRIGGER",
            "tabId": tab_id,
            "elementId": element_id,
            "videoUrl": video_url,
            "audioUrl": audio_url,
            "headers": {
                "User-Agent": user_agent,
                "Cookie": cookie,
                "Referer": referer
            }
        }));
    }
}

fn copy_to_clipboard(text: &str) {
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::DataExchange::{OpenClipboard, EmptyClipboard, SetClipboardData, CloseClipboard};
        use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
        use windows::Win32::Foundation::HWND;

        if OpenClipboard(HWND(std::ptr::null_mut())).is_err() {
            return;
        }
        let _ = EmptyClipboard();

        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes_len = wide.len() * std::mem::size_of::<u16>();

        if let Ok(h_mem) = GlobalAlloc(GHND, bytes_len) {
            let mem_ptr = GlobalLock(h_mem);
            if !mem_ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), mem_ptr as *mut u16, wide.len());
                let _ = GlobalUnlock(h_mem);
                let _ = SetClipboardData(13, HANDLE(h_mem.0)); // CF_UNICODETEXT is 13
            }
        }
        let _ = CloseClipboard();
    }
}
