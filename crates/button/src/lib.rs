use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Once;
use windows::core::w;
use windows::Win32::Foundation::{
    GetLastError, BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetStockObject, RoundRect, SelectObject, SetBkMode, SetTextColor, DEFAULT_GUI_FONT, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, HBRUSH, HGDIOBJ, PAINTSTRUCT, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleHandleW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetClientRect, GetWindowLongPtrW, GetWindowRect, LoadCursorW, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, TrackPopupMenu,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU, IDC_ARROW, LWA_ALPHA,
    MENU_ITEM_FLAGS, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE,
    SW_SHOWNA, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_PAINT, WNDCLASSW, WS_CHILD,
};

const CLASS_NAME: windows::core::PCWSTR = w!("Tur Download Button class");
const BUTTON_WIDTH: i32 = 226;
const BUTTON_HEIGHT: i32 = 26;

static REGISTER_CLASS: Once = Once::new();
static LAST_ERROR: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
struct ButtonState {
    menu_owner: HWND,
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    instance: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason == 1 {
        let _ = DisableThreadLibraryCalls(instance);
    }
    true.into()
}

#[no_mangle]
pub unsafe extern "system" fn GetTurButtonVersion() -> u32 {
    1
}

#[no_mangle]
pub unsafe extern "system" fn GetTurLastError() -> u32 {
    LAST_ERROR.load(Ordering::Relaxed)
}

#[no_mangle]
pub unsafe extern "system" fn CreateTurButton(
    parent: HWND,
    x: i32,
    y: i32,
    menu_owner: HWND,
) -> HWND {
    register_button_class();
    create_button_window(parent, x, y, menu_owner)
}

#[no_mangle]
pub unsafe extern "system" fn SetTurButtonValue(
    button: HWND,
    x: i32,
    y: i32,
    visible: BOOL,
) -> BOOL {
    if button.0.is_null() {
        return false.into();
    }

    if visible.as_bool() {
        let _ = SetWindowPos(
            button,
            HWND(null_mut()),
            x,
            y,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    } else {
        let _ = SetWindowPos(
            button,
            HWND(null_mut()),
            0,
            0,
            0,
            0,
            SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(button, SW_HIDE);
    }
    true.into()
}

#[no_mangle]
pub unsafe extern "system" fn DestroyTurButton(button: HWND) -> BOOL {
    if button.0.is_null() {
        return false.into();
    }
    DestroyWindow(button)
        .map(|_| true.into())
        .unwrap_or(false.into())
}

unsafe fn register_button_class() {
    REGISTER_CLASS.call_once(|| {
        let hinstance = HINSTANCE(
            GetModuleHandleW(None)
                .map(|module| module.0)
                .unwrap_or_default(),
        );
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(button_wndproc),
            hInstance: hinstance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        unsafe {
            let _ = RegisterClassW(&class);
        }
    });
}

unsafe fn create_button_window(parent: HWND, x: i32, y: i32, menu_owner: HWND) -> HWND {
    let state = Box::new(ButtonState { menu_owner });
    let raw_state = Box::into_raw(state);
    let hinstance = HINSTANCE(
        GetModuleHandleW(None)
            .map(|module| module.0)
            .unwrap_or_default(),
    );

    let hwnd = create_button_window_with_style(
        parent,
        x,
        y,
        raw_state,
        hinstance,
        WINDOW_EX_STYLE(0x8000c),
    )
    .or_else(|| {
        create_button_window_with_style(parent, x, y, raw_state, hinstance, WINDOW_EX_STYLE(0))
    });

    match hwnd {
        Some(hwnd) => hwnd,
        None => {
            let _ = Box::from_raw(raw_state);
            HWND(null_mut())
        }
    }
}

unsafe fn create_button_window_with_style(
    parent: HWND,
    x: i32,
    y: i32,
    raw_state: *mut ButtonState,
    hinstance: HINSTANCE,
    ex_style: WINDOW_EX_STYLE,
) -> Option<HWND> {
    match CreateWindowExW(
        ex_style,
        CLASS_NAME,
        w!("Tur Download Panel"),
        WINDOW_STYLE(WS_CHILD.0),
        x,
        y,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
        parent,
        HMENU(null_mut()),
        hinstance,
        Some(raw_state.cast()),
    ) {
        Ok(hwnd) => {
            if ex_style.0 & 0x80000 != 0 {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            Some(hwnd)
        }
        Err(_) => {
            LAST_ERROR.store(GetLastError().0, Ordering::Relaxed);
            None
        }
    }
}

unsafe extern "system" fn button_wndproc(
    hwnd: HWND,
    msg: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let state = cs.lpCreateParams as *mut ButtonState;
            if state.is_null() {
                return LRESULT(0);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            LRESULT(1)
        }
        WM_PAINT => {
            paint_button(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            show_menu(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ButtonState;
            if !ptr.is_null() {
                let _ = Box::from_raw(ptr);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, WPARAM(0), lparam),
    }
}

unsafe fn paint_button(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);

    let bg = CreateSolidBrush(COLORREF(0x00F5F5F5));
    let border = CreatePen(PS_SOLID, 1, COLORREF(0x00B8B8B8));
    let old_brush = SelectObject(hdc, HGDIOBJ(bg.0));
    let old_pen = SelectObject(hdc, HGDIOBJ(border.0));
    let _ = FillRect(hdc, &rect, HBRUSH(bg.0));
    let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 10, 10);
    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);

    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(0x00111111));
    let old_font = SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
    let mut text_rect = rect;
    let mut text: Vec<u16> = "Download with tur".encode_utf16().chain(Some(0)).collect();
    let _ = DrawTextW(
        hdc,
        &mut text,
        &mut text_rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    let _ = SelectObject(hdc, old_font);

    let _ = DeleteObject(bg);
    let _ = DeleteObject(border);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), 1, w!("Download video"));
    let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), 2, w!("Download audio only"));
    let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), 3, w!("Copy media URL"));
    let _ = AppendMenuW(menu, MENU_ITEM_FLAGS(0), 4, w!("Open download details"));

    let mut rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rect);
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ButtonState;
    let owner = if ptr.is_null() {
        hwnd
    } else {
        (*ptr).menu_owner
    };
    let owner = if owner.0.is_null() { hwnd } else { owner };
    let _ = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD,
        rect.left + 12,
        rect.bottom + 6,
        0,
        owner,
        None,
    );
    let _ = DestroyMenu(menu);
}
