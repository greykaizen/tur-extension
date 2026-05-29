use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::OverlayUpdate;

const BUTTON_WIDTH: i32 = 226;
const BUTTON_HEIGHT: i32 = 26;
const BUTTON_GAP: i32 = 2;

type CreateTurButtonFn = unsafe extern "system" fn(HWND, i32, i32, HWND) -> HWND;
type SetTurButtonValueFn = unsafe extern "system" fn(HWND, i32, i32, BOOL) -> BOOL;
type DestroyTurButtonFn = unsafe extern "system" fn(HWND) -> BOOL;
type SetTurDarkModeFn = unsafe extern "system" fn(BOOL) -> BOOL;

pub struct ButtonApi {
    pub create: CreateTurButtonFn,
    pub set_value: SetTurButtonValueFn,
    pub destroy: DestroyTurButtonFn,
    pub set_dark_mode: SetTurDarkModeFn,
}

// Store button HWND as isize (HWND contains *mut c_void which is !Send).
// Value is (button_hwnd_raw, owner_hwnd_raw).
// All button operations happen on the main thread only.
static BUTTONS: OnceLock<Mutex<HashMap<i32, (isize, isize)>>> = OnceLock::new();

fn buttons() -> &'static Mutex<HashMap<i32, (isize, isize)>> {
    BUTTONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load turbrbtn.dll and resolve exported functions.
pub fn load_button_api() -> Option<ButtonApi> {
    let dll_path = find_turbrbtn_dll()?;

    let mut wide: Vec<u16> = dll_path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();

    unsafe {
        let module = LoadLibraryW(PCWSTR(wide.as_mut_ptr())).ok()?;
        let create = GetProcAddress(module, windows::core::s!("CreateTurButton"))?;
        let set_value = GetProcAddress(module, windows::core::s!("SetTurButtonValue"))?;
        let destroy = GetProcAddress(module, windows::core::s!("DestroyTurButton"))?;
        let set_dark_mode = GetProcAddress(module, windows::core::s!("SetTurDarkMode"))?;

        Some(ButtonApi {
            create: std::mem::transmute(create),
            set_value: std::mem::transmute(set_value),
            destroy: std::mem::transmute(destroy),
            set_dark_mode: std::mem::transmute(set_dark_mode),
        })
    }
}

/// Find turbrbtn.dll by searching common locations.
fn find_turbrbtn_dll() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    // Current directory
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("crates").join("button").join("target").join("debug").join("turbrbtn.dll"));
        candidates.push(cwd.join("crates").join("button").join("target").join("release").join("turbrbtn.dll"));
        candidates.push(cwd.join("turbrbtn.dll"));
    }

    // Exe-relative paths
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors() {
            candidates.push(ancestor.join("crates").join("button").join("target").join("debug").join("turbrbtn.dll"));
            candidates.push(ancestor.join("crates").join("button").join("target").join("release").join("turbrbtn.dll"));
            candidates.push(ancestor.join("turbrbtn.dll"));
        }
    }

    // Also search alongside the exe itself
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("turbrbtn.dll"));
        }
    }

    candidates.into_iter().find(|p| p.exists())
}

/// Create or update a popup overlay button for the given tab/geometry.
/// The button is positioned at absolute screen coordinates (no child window offset).
pub unsafe fn update_button(api: &ButtonApi, update: &OverlayUpdate, root: HWND, content_surface: Option<HWND>) {
    // Position the button above the top-right of the video element.
    let x_screen = update.screen_x + update.width - BUTTON_WIDTH;
    let y_screen = update.screen_y - BUTTON_HEIGHT - BUTTON_GAP;

    if y_screen < 0 {
        hide_button(api, update.tab_id);
        return;
    }

    // Use the content surface HWND as the owner for proper z-ordering.
    let owner = content_surface.unwrap_or(root);

    let mut map = buttons().lock().unwrap();
    let stored = map.get(&update.tab_id).copied();
    let (hwnd_raw, _old_owner_raw) = stored.unwrap_or((0, 0));
    let button = HWND(hwnd_raw as *mut std::ffi::c_void);

    let button = if hwnd_raw != 0 && IsWindow(button).as_bool() {
        button
    } else {
        // Create new popup button at absolute screen coordinates.
        let created = (api.create)(owner, x_screen, y_screen, owner);
        if created.0.is_null() {
            eprintln!("[tur] CreateTurButton failed for tab={}", update.tab_id);
            return;
        }
        map.insert(update.tab_id, (created.0 as isize, owner.0 as isize));
        created
    };

    let _ = (api.set_value)(button, x_screen, y_screen, true.into());
}

/// Hide (but keep alive) the button for a given tab.
pub unsafe fn hide_button(api: &ButtonApi, tab_id: i32) {
    let mut map = buttons().lock().unwrap();
    if let Some(&(hwnd_raw, _owner_raw)) = map.get(&tab_id) {
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        if IsWindow(hwnd).as_bool() {
            let _ = (api.set_value)(hwnd, 0, 0, false.into());
        } else {
            map.remove(&tab_id);
        }
    }
}

/// Destroy a button and remove it from tracking.
pub unsafe fn destroy_button(api: &ButtonApi, tab_id: i32) {
    let mut map = buttons().lock().unwrap();
    if let Some((hwnd_raw, _owner_raw)) = map.remove(&tab_id) {
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        let _ = (api.destroy)(hwnd);
    }
}

/// Destroy all tracked buttons.
pub unsafe fn destroy_all_buttons(api: &ButtonApi) {
    let mut map = buttons().lock().unwrap();
    for (_, (hwnd_raw, _owner_raw)) in map.drain() {
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        let _ = (api.destroy)(hwnd);
    }
    map.clear();
}
