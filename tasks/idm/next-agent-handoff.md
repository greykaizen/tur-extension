# Next Agent Handoff

Current state:

```text
Branch: native-overlays
Working tree has uncommitted native Windows POC files.
The historical Tauri-overlay attempt is preserved on branch wip/windows-native-overlay-attempt.
```

Immediate command to test the POC:

```powershell
powershell -ExecutionPolicy Bypass -File windows-overlay-poc\run.ps1
```

Native-host smoke test already passed:

```text
tur-native-host accepted a length-prefixed MEDIA_TARGET_UPDATE message.
response: {"ok":true}
exit: 0
```

Installed-host smoke test also passed:

```text
installed exe: C:\Users\Shah\AppData\Local\tur-native-host\tur-native-host.exe
installed dll: C:\Users\Shah\AppData\Local\tur-native-host\turbrbtn.dll
response: {"ok":true}
exit: 0
```

The installer was run with Chromium extension ID:

```text
imeiddeomojifhdcnlhbchagckalhllm
```

Expected result:

```text
The controller builds turbrbtn.dll.
The controller finds a Chromium root.
If IDM panel is currently visible, the controller selects the same root IDM uses.
The controller creates one native `Tur Download Button class` child under that root.
```

If the Tur button does not appear:

```text
1. Keep IDM button visible in Brave/Thorium.
2. Rerun the POC.
3. Check console for `selected root from existing IDM button`.
4. Use WinSpy on the visible IDM button and Tur button.
5. Tur should appear as class `Tur Download Button class`; if not, inspect CreateWindowExW failure and layered alpha state.
```

The current highest-confidence direction is:

```text
native host/controller + native button DLL + Chromium root child HWNDs
```

Avoid:

```text
DOM injection
Shadow DOM
Tauri webview overlay windows for the browser panel
top-level WS_POPUP overlays for browser-attached buttons
```

Reason:

```text
IDM's real runtime capture shows direct child HWNDs under the Chromium root, not top-level overlays.
```
