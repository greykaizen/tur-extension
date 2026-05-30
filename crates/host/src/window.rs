use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn log_debug(msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;
    let path = std::env::temp_dir().join("tur-overlay-debug.log");
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{}", msg);
    }
}

/// Get the window title of an HWND.
unsafe fn get_window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut buffer) as usize;
    String::from_utf16_lossy(&buffer[..len])
}

/// Find the browser top-level root HWND that contains the given screen-space point.
/// Searches visible top-level windows whose class is Chrome_WidgetWin_1
/// (or Chrome_WidgetWin_0, MozillaWindowClass) and whose rectangle contains the point.
pub unsafe fn find_browser_root_for_point(screen_x: i32, screen_y: i32) -> Option<HWND> {
    struct Search {
        screen_x: i32,
        screen_y: i32,
        result: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);

        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }

        let class = get_class_name(hwnd);
        if class != "Chrome_WidgetWin_1"
            && class != "Chrome_WidgetWin_0"
            && class != "MozillaWindowClass"
        {
            return true.into();
        }

        let title = get_window_title(hwnd);
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);

        log_debug(&format!(
            "[tur] Window candidate: hwnd={:?} class='{}' title='{}' rect={:?}",
            hwnd, class, title, rect
        ));

        // Skip windows that are empty/invisible utility wrappers (which often have no title and small size)
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if (width <= 100 || height <= 100) && title.is_empty() {
            log_debug(&format!("[tur]   -> skipping candidate due to empty title and small size"));
            return true.into();
        }

        // Check if the point is inside this window.
        if search.screen_x >= rect.left && search.screen_x < rect.right
            && search.screen_y >= rect.top && search.screen_y < rect.bottom
        {
            log_debug(&format!("[tur]   -> matched point ({}, {})!", search.screen_x, search.screen_y));
            search.result = Some(hwnd);
            return false.into(); // Stop enumeration.
        }

        true.into()
    }

    let mut search = Search {
        screen_x,
        screen_y,
        result: None,
    };

    let _ = EnumWindows(
        Some(enum_proc),
        LPARAM((&mut search as *mut Search) as isize),
    );

    search.result
}

/// Get the class name of an HWND.
unsafe fn get_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut buffer) as usize;
    String::from_utf16_lossy(&buffer[..len])
}
