# IDM Native Browser Panel Research

This folder contains the current Windows native-overlay evidence for Project Tur.

Scope:

```text
Goal: implement Tur's browser media button as native Win32 UI, not DOM or Shadow DOM.
Primary target: Chromium-family browsers on Windows, tested with Brave/Thorium.
Reference app: Internet Download Manager's native browser download panel.
```

Key conclusion:

```text
IDM's visible browser button is a native Win32 child window.
It is not injected browser DOM.
It is not Shadow DOM.
It is not a separate unowned Tauri-style top-level overlay.
```

Most important files:

```text
tasks/idm/runtime-frida-capture.md      Confirmed runtime calls and HWND/style data.
tasks/idm/implementation-notes.md       Tur implementation direction based on the capture.
tasks/idm/frida-commands.md             Commands and scripts for repeating the capture.
tools/idm-frida-panel.js                Frida hook used for the current capture.
turbrbtn/                               Tur native button DLL POC.
windows-overlay-poc/                    Controller POC that loads turbrbtn.dll and attaches to Chromium.
```

Do not regress to browser-rendered UI. The extension can remain a sensor/metadata producer, but the button/menu UI must be native.
