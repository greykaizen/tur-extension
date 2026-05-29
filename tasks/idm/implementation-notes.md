# Tur Native Button Implementation Notes

Current local code:

```text
turbrbtn/             Rust cdylib that exports native button functions.
windows-overlay-poc/  Rust controller POC that loads turbrbtn.dll and creates a button under Chromium.
tur-native-host/      Existing Chromium native host, now also has a Windows direct native-panel path.
```

## Confirmed Target Behavior

Tur should mirror these IDM behaviors:

```text
Create native child HWND under Chromium root, not a top-level always-on-top overlay.
Use a dedicated class name for discovery/debugging.
Use child-window coordinates relative to the Chromium root.
Use SetWindowPos for updates.
Use one HWND per visible target, with reuse/collapse for hidden targets.
Render the button and context menu natively.
```

The first Tur native class is:

```text
Tur Download Button class
```

The current DLL tries IDM's captured layered child exstyle first:

```text
exstyle 0x8000c
style   0x40000000 / WS_CHILD
```

If `CreateWindowExW` refuses the layered child in the standalone controller process, it falls back to a plain native `WS_CHILD`. That fallback is intentionally in the POC because it gives us a visible child-window validation even when the host process manifest/style combination rejects layered child windows.

The first exported DLL API is:

```text
GetTurButtonVersion() -> u32
CreateTurButton(parent: HWND, x: i32, y: i32, menu_owner: HWND) -> HWND
SetTurButtonValue(button: HWND, x: i32, y: i32, visible: BOOL) -> BOOL
DestroyTurButton(button: HWND) -> BOOL
```

This intentionally copies IDM's separation of concerns without copying proprietary code:

```text
controller owns target selection and browser HWND resolution
button DLL owns Win32 class registration, painting, click handling, and native menu
```

## Immediate POC Flow

```text
1. Build turbrbtn.dll.
2. Run windows-overlay-poc.
3. Controller loads turbrbtn.dll through LoadLibraryW.
4. Controller finds a Chromium root.
5. If IDM is visible, controller prefers the root containing `IDM Download Button class`.
6. Controller creates `Tur Download Button class` as a child of that same root.
7. Controller updates position with SetTurButtonValue.
```

## Extension-Driven Native Host Flow

`tur-native-host` still reads Chromium native messages with the required 4-byte little-endian length prefix. On Windows, each `MEDIA_TARGET_UPDATE` now also goes through a direct native-panel handler before the existing Tauri named-pipe forward.

Current flow:

```text
extension background.js
-> chrome.runtime.connectNative("com.tur.native_host")
-> tur-native-host framed stdin loop
-> native_panel::handle_geometry_update
-> load turbrbtn.dll
-> find Chrome_WidgetWin_1 root containing the target screen point
-> create/update Tur Download Button class child HWND
```

Current root selection:

```text
Use the media target center point from the extension payload.
Enum top-level windows.
Keep visible Chrome_WidgetWin_1 roots whose window rect contains that point.
Prefer the foreground root when scores tie.
```

This is still a bridge implementation. A production version should bind extension tab/window identity to root HWND more explicitly, but the current path is enough to test native child-window behavior from the real extension message stream.

## Important Correction From Failed Attempts

The sticky-workspace problem came from top-level overlay windows. Native child windows solve a different problem:

```text
top-level overlay: independent desktop/DWM object, can lag or appear over other apps/workspaces
root child window: belongs to the browser window hierarchy and naturally follows browser z-order/minimize/workspace behavior
```

This is why the IDM path matters.

## Remaining Production Work

The POC still needs these before merging into the real app path:

```text
native host stdin/stdout Chromium message loop
extension payload parser for multiple target boxes
per-browser-root map of Tur button HWNDs
button reuse instead of create/destroy churn
accurate root HWND association from extension tab/window metadata
DPI-aware coordinate conversion
SetWinEventHook or browser-move event handling for root movement
native menu option callback bridge back to Tur
```

Do not build the production path around a Tauri webview overlay. Tauri remains useful for the main app, but the browser panel path should be a small native Windows controller plus the native button DLL.
