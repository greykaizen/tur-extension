use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Find the Chromium top-level root HWND that contains the given screen-space point.
/// Searches visible top-level windows whose class is Chrome_WidgetWin_1
/// (or Chrome_WidgetWin_0) and whose rectangle contains the point.
pub unsafe fn find_chromium_root_for_point(screen_x: i32, screen_y: i32) -> Option<HWND> {
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
        if class != "Chrome_WidgetWin_1" && class != "Chrome_WidgetWin_0" {
            return true.into();
        }

        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);

        // Check if the point is inside this window.
        if search.screen_x >= rect.left && search.screen_x < rect.right
            && search.screen_y >= rect.top && search.screen_y < rect.bottom
        {
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

/// Given a top-level Chrome_WidgetWin_1/0 HWND, find the child window that
/// represents the actual web content rendering surface (Chrome_RenderWidgetHostHWND).
pub unsafe fn find_chromium_content_surface(root: HWND) -> Option<HWND> {
    struct Search {
        result: Option<HWND>,
    }

    unsafe extern "system" fn enum_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let class = get_class_name(hwnd);

        if class == "Chrome_RenderWidgetHostHWND"
            || class == "Chrome_WebViewWindow"
        {
            search.result = Some(hwnd);
            return false.into();
        }

        true.into()
    }

    let mut search = Search { result: None };

    let _ = EnumChildWindows(
        root,
        Some(enum_child_proc),
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
