# Runtime Frida Capture

Capture source:

```text
script: D:\tur-tauri\tools\idm-frida-panel.js
log:    D:\tur-tauri\logs\idm-frida-panel.log
target: explorer.exe during active IDM browser panel display
browser root observed: 0x210b12
```

The capture successfully observed IDM's real panel creation path.

## Export Entry Point

The active panel path calls:

```text
idmbrbtn64.dll!CreateIDMButton3
```

Representative creation call:

```text
[enter] CreateIDMButton3
  arg0=0x6a00f
  arg1=0x8090080
  arg2=0x1af1dd80
  arg3=0x210b12
  arg4=0x0
  arg5=0x1af1ddc0
  arg6=0x0
  arg7=0x0
  arg8=0x2ba7b498
  arg9=0x24b11688
```

Representative reuse/update call:

```text
[enter] CreateIDMButton3
  arg0=0x6a00b
  arg1=0x8090280
  arg2=0x1af1c380
  arg3=0x210b12
  arg4=0x191048
  arg5=0x1af1c3c0
  arg6=0x1af1c3e0
  arg7=0x0
  arg8=0x28a28b88
  arg9=0x24b11688
[leave] CreateIDMButton3 ret=0x191048
```

Interpretation:

```text
arg3 is the Chromium top-level root HWND used as the button parent.
arg4 is null on creation.
arg4 is the existing IDM button HWND on reuse/update.
arg1 changes from 0x8090080 on creation to 0x8090280 on reuse/update.
```

## Window Creation

IDM creates one native child window per visible panel button:

```text
[CreateWindowExW] ret=0x240fcc parent=0x210b12
  class="IDM Download Button class"
  title="IDM Download Panel"
  ex=0x8000c
  style=0x40000000
  xywh=0,0,0,0
```

Known constants:

```text
class:   IDM Download Button class
title:   IDM Download Panel
parent:  Chromium root HWND, example 0x210b12
exstyle: 0x8000c
style:   0x40000000
size:    226x26
```

`0x40000000` is `WS_CHILD`. The IDM panel is not a popup top-level overlay.

## Tooltip Creation

Immediately after button creation, IDM creates a native tooltip child:

```text
[CreateWindowExW] ret=0x2304c4 parent=0x240fcc
  class="tooltips_class32"
  title="NULL"
  ex=0x0
  style=0x80000042
  xywh=-2147483648,-2147483648,-2147483648,-2147483648
```

## Positioning

IDM positions the panel after creation through `SetWindowPos`.

Examples:

```text
[SetWindowPos] hwnd=0x240fcc after=0x0 xywh=1084,54,226,26 flags=0x80 ret=0x1
[SetWindowPos] hwnd=0x240fcc after=0x0 xywh=1084,54,226,26 flags=0x0 ret=0x1
[SetWindowPos] hwnd=0x2b07c6 after=0x0 xywh=2,245,226,26 flags=0x40 ret=0x1
[SetWindowPos] hwnd=0x930758 after=0x0 xywh=130,245,226,26 flags=0x40 ret=0x1
```

Observed hidden/offscreen state:

```text
[SetWindowPos] hwnd=0x2b07c6 after=0x0 xywh=0,0,0,0 flags=0x40b ret=0x1
[SetWindowPos] hwnd=0x930758 after=0x0 xywh=0,0,0,0 flags=0x40b ret=0x1
```

Interpretation:

```text
IDM keeps/reuses HWNDs and collapses hidden buttons to 0,0,0,0.
Visible panel size is consistently 226x26.
Coordinates are child-window coordinates relative to the Chromium root client area.
```

## Enumeration Behavior

Before creating a new panel, IDM enumerates existing button children under the Chromium root:

```text
[FindWindowExW] ret=0x930758 parent=0x210b12 after=0x0 class="IDM Download Button class" title="NULL"
[FindWindowExW] ret=0x191048 parent=0x210b12 after=0x930758 class="IDM Download Button class" title="NULL"
[FindWindowExW] ret=0x2b07c6 parent=0x210b12 after=0x191048 class="IDM Download Button class" title="NULL"
[FindWindowExW] ret=0x0 parent=0x210b12 after=0x2b07c6 class="IDM Download Button class" title="NULL"
```

Interpretation:

```text
IDM maintains multiple native button HWNDs under one browser root.
This matches grid pages like Imgur where several media targets can exist at once.
```

## Confirmed Architecture

```text
browser extension/native host -> IDM monitor/controller -> idmbrbtn64.dll -> native child HWNDs under Chromium root
```

The root HWND is the attachment target for the button. `Chrome_RenderWidgetHostHWND` remains useful for detecting viewport geometry, but the visible IDM button is parented to the browser root, not to the render child.
