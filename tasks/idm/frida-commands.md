# Frida Capture Commands

Frida was installed through Python 3.12:

```powershell
C:\Users\Shah\AppData\Local\Programs\Python\Python312\python.exe -m pip install frida-tools
```

Frida binaries:

```text
C:\Users\Shah\AppData\Local\Programs\Python\Python312\Scripts
```

Hook script:

```text
D:\tur-tauri\tools\idm-frida-panel.js
```

Log path:

```text
D:\tur-tauri\logs\idm-frida-panel.log
```

## Find Candidate Processes

Use this when IDM panel is visible:

```powershell
& "$env:LOCALAPPDATA\Programs\Python\Python312\Scripts\frida-ps.exe" -a | Select-String -Pattern "IDM|explorer|chrome|brave|thorium|msedge"
```

The successful capture attached to `explorer.exe`, because `idmbrbtn64.dll` was loaded there during the test.

## Run Capture

Replace `explorer.exe` with the PID/name that has `idmbrbtn64.dll` loaded if needed.

```powershell
& "$env:LOCALAPPDATA\Programs\Python\Python312\Scripts\frida.exe" `
  -n explorer.exe `
  -l D:\tur-tauri\tools\idm-frida-panel.js `
  -o D:\tur-tauri\logs\idm-frida-panel.log
```

Then trigger the IDM browser panel in Brave/Thorium by hovering visible media.

## What To Look For

Useful lines:

```text
[enter] CreateIDMButton3
[CreateWindowExW] ... class="IDM Download Button class"
[SetWindowPos] hwnd=<button> ... 226,26
[FindWindowExW] ... class="IDM Download Button class"
[TrackPopupMenu]
```

The key fields to preserve:

```text
parent HWND passed to CreateWindowExW
class name
title
exstyle
style
initial xywh
visible xywh
hide xywh/flags
reuse argument shape
```
