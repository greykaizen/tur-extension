// crates/host/src/overlay/menu.rs

use std::ptr::null_mut;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::overlay::types::*;
use crate::overlay::write_response;

/// quality popup menu (anchored to button bottom-left)
pub unsafe fn show_quality_menu(
    hwnd: HWND,
    pt: POINT,
    element_id: &str,
    tab_id: i32,
    media_url: &str,
) {
    macro_rules! wstr {
        ($s:expr) => {{
            let v: Vec<u16> = $s.encode_utf16().chain(std::iter::once(0)).collect();
            v
        }};
    }
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };
    let s1 = wstr!("Download 1080p (stub)");
    let s2 = wstr!("Download 720p (stub)");
    let s3 = wstr!("Download 480p (stub)");
    let s4 = wstr!("Copy Media URL");
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_1080P, PCWSTR(s1.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_720P, PCWSTR(s2.as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_DL_480P, PCWSTR(s3.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null_mut()));
    let _ = AppendMenuW(menu, MF_STRING, MENU_COPY_URL, PCWSTR(s4.as_ptr()));
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
    let quality = match cmd {
        MENU_DL_1080P => "1080p",
        MENU_DL_720P => "720p",
        MENU_DL_480P => "480p",
        _ => "",
    };
    if cmd == MENU_COPY_URL {
        write_response(&serde_json::json!({
            "type": "OVERLAY_COPY_URL",
            "tabId": tab_id,
            "elementId": element_id,
            "mediaUrl": media_url,
        }));
    } else if !quality.is_empty() {
        write_response(&serde_json::json!({
            "type": "OVERLAY_MENU_SELECTED",
            "tabId": tab_id,
            "elementId": element_id,
            "quality": quality,
            "mediaUrl": media_url,
        }));
    }
}
