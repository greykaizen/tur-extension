# macOS Setup Guide — tur-overlay

This guide covers building, installing, and testing the tur-overlay native host on macOS (both Intel and Apple Silicon).

---

## Prerequisites

- **macOS 12.0+** (Monterey or later)
- **Rust toolchain** — install from [rustup.rs](https://rustup.rs) if you don't have it:
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **yt-dlp** — optional but recommended for highest-quality downloads:
  ```bash
  brew install yt-dlp
  # or install via pip: pip3 install yt-dlp
  ```
- **A browser**: Google Chrome, Chromium, Brave, or Firefox (latest versions)

---

## Quick Install

```bash
# Clone the repo
git clone <repo-url> tur-overlay
cd tur-overlay

# Run the installer (interactive)
./tools/install-macos.sh
```

The installer will:
1. Build the Rust host binary (`target/release/tur-overlay-host`)
2. Prompt you to select browsers to install the native messaging host for
3. Write the native messaging host manifest to the right location for each browser

---

## Step-by-Step Installation

### 1. Build the Rust host

```bash
cd tur-overlay
cargo build --release --manifest-path crates/host/Cargo.toml
```

This produces `target/release/tur-overlay-host` — the native messaging host daemon.

### 2. Install the native messaging host

You can use the installer script:

```bash
# Install for all browsers
./tools/install-macos.sh --all

# Or just for one browser
./tools/install-macos.sh --chrome
./tools/install-macos.sh --firefox

# Uninstall
./tools/install-macos.sh --uninstall
```

The script writes a JSON manifest to the browser's `NativeMessagingHosts` directory so the extension can launch the daemon.

### 3. Build the browser extension

```bash
# Create dist folder with browser-specific manifests
mkdir -p extension/dist/chromium extension/dist/firefox

# Copy shared files
cp -r extension/src/* extension/dist/chromium/
cp -r extension/src/* extension/dist/firefox/

# Copy browser-specific manifests
cp extension/chromium/manifest.json extension/dist/chromium/manifest.json
cp extension/firefox/manifest.json extension/dist/firefox/manifest.json
```

> **Note:** Windows uses `tools/build.ps1` for this — the above is the manual equivalent for macOS.

### 4. Load the extension in your browser

#### Chrome / Chromium / Brave

1. Open `chrome://extensions`
2. Enable **Developer mode** (toggle top-right)
3. Click **Load unpacked**
4. Select the folder: `extension/dist/chromium/`
5. Note the extension ID shown on the card (e.g., `abcdefghijklmnop123456`)

**Important:** After loading, copy the extension ID and update the native messaging host manifest:

```bash
# Replace YOUR_CHROME_EXTENSION_ID with the ID shown in chrome://extensions
# Edit ~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.tur.native_host.json
# Change "allowed_origins" to: ["chrome-extension://YOUR_EXTENSION_ID_HERE/"]
```

Then restart Chrome.

#### Firefox

1. Open `about:debugging#/runtime/this-firefox`
2. Click **Load Temporary Add-on**
3. Select the file: `extension/dist/firefox/manifest.json`

Firefox uses the extension ID `tur@project.local` (set in `manifest.json` under `browser_specific_settings.gecko.id`), which already matches the native messaging host manifest.

---

## What to Expect

When everything is working:

1. **Open any page with a video** (YouTube, Vimeo, Imgur with video, Reddit video, or any `<video>` element with a downloadable stream)
2. **Hover over the video** — a small overlay button appears
3. **Click the button** (the "Download with tur" pill) — a quality menu pops up if formats are available
4. **Select a quality** — the download is queued via the native host
5. **Drag the overlay** — the position is persisted so it stays where you put it

If the button doesn't appear:
- Check the browser console (`⌘⌥J` on Chrome, `⌘⌥K` on Firefox) for any errors
- Check that the native host process is running (`ps aux | grep tur-overlay`)
- Try reloading the page

---

## Smoke Test Checklist

Use this checklist when testing on your Mac. Each item should pass.

### Build & Compilation

| # | Check | Expected |
|---|-------|----------|
| 1 | `cargo build --release` succeeds | Exit code 0, binary at `target/release/tur-overlay-host` |
| 2 | `file target/release/tur-overlay-host` | Output: `Mach-O 64-bit executable x86_64` (Intel) or `arm64` (Apple Silicon) |
| 3 | Binary is executable | `ls -l` shows `-rwxr-xr-x` or similar with `x` permission |

### Native Messaging Host Installation

| # | Check | Expected |
|---|-------|----------|
| 4 | Chrome manifest exists | `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.tur.native_host.json` |
| 5 | Firefox manifest exists | `~/Library/Application Support/Mozilla/NativeMessagingHosts/com.tur.native_host.json` |
| 6 | Chrome manifest `name` field is `com.tur.native_host` | ✓ |
| 7 | Chrome manifest `path` points to the built binary | ✓ (absolute path) |
| 8 | Firefox manifest `allowed_extensions` contains `tur@project.local` | ✓ |

### Extension Loading

| # | Check | Expected |
|---|-------|----------|
| 9 | Chrome extension loads without errors | No red errors in `chrome://extensions` |
| 10 | Firefox add-on loads without errors | No errors in `about:debugging#/runtime/this-firefox` |
| 11 | Background worker activates | Chrome: "Service worker" shows "Running" in `chrome://extensions` |
| 12 | Native port connects | Check `chrome://extensions` → tur → Inspect views background page → Console shows `"Connected to TUR native host."` |

### Overlay Appearance

| # | Check | Expected |
|---|-------|----------|
| 13 | Open YouTube video page | Hover over video area |
| 14 | Overlay button appears | A small pill button (logo + "Download with tur" + "×") near the video |
| 15 | Overlay has correct position | Aligned to the top-right of the video element |
| 16 | Overlay disappears on non-video pages | Go to google.com — no overlay should appear |
| 17 | Overlay re-appears when navigating back to a video | ✓ |
| 18 | New tab/window shows no overlay until a video is present | ✓ |

### Overlay Interaction

| # | Check | Expected |
|---|-------|----------|
| 19 | Click "Download with tur" pill | A quality/resolution menu appears (if formats resolved) |
| 20 | Click a quality option | Download is triggered (QUEUE_DOWNLOAD message sent) |
| 21 | Click "×" (dismiss) | Overlay hides for that video element |
| 22 | Drag the overlay by the logo area | Overlay follows mouse; position is persisted on drop (console shows `Drag offset persisted`) |
| 23 | Reload the page after dragging | Overlay remembers its dragged position |

### Format Resolution

| # | Check | Expected |
|---|-------|----------|
| 24 | HLS playlists (.m3u8) | Resolved by offscreen document → shows multiple quality options |
| 25 | DASH manifests (.mpd) | Parsed → shows quality options with video+audio URLs |
| 26 | Direct MP4 files | No yt-dlp needed (HEAD request resolves size) |
| 27 | YouTube | Status shows "pending" → yt-dlp resolves in background → updates to "ready" with formats |
| 28 | Check debug log | `cat $TMPDIR/tur-overlay-debug.log` (or `echo $TMPDIR` to find the exact path) shows target processing |

### Cross-Browser Consistency

| # | Check | Expected |
|---|-------|----------|
| 29 | Same page works identically in Chrome and Firefox | Same overlay behavior, same format options |
| 30 | Both browsers can have active overlays simultaneously | Works independently per browser window |

---

## Troubleshooting

### "Native host has exited" / port disconnects
The background service worker suspends after ~30s of inactivity. This is normal — it auto-reconnects when a new media target is detected. If it doesn't reconnect, try reloading the extension.

### Overlay doesn't appear
1. Open the console (`⌘⌥J` on Chrome, `⌘⌥K` on Firefox)
2. Look for: `"Connected to TUR native host."` — if missing, the native messaging host isn't installed correctly
3. Check: `~/Library/Application Support/[Browser]/NativeMessagingHosts/com.tur.native_host.json` exists and has the correct binary path
4. Check binary permissions: `ls -l target/release/tur-overlay-host`

### yt-dlp not found
The binary searches for yt-dlp in this order:
1. `yt-dlp` (from PATH)
2. `/opt/homebrew/bin/yt-dlp` (Apple Silicon Homebrew)
3. `/usr/local/bin/yt-dlp` (Intel Homebrew / manual)

Install it: `brew install yt-dlp` or `pip3 install yt-dlp`

### Debug logging
Logs are written to your system's temp directory. Find the exact path and tail them in real-time:
```bash
ls -la "$TMPDIR/tur-overlay-debug.log"  # verify the file exists
tail -f "$TMPDIR/tur-overlay-debug.log"
```

(On macOS, `$TMPDIR` is typically `/var/folders/xx/xxxxxx/T/`. If the log doesn't appear there, run `echo $TMPDIR` to see the correct path for your system.)

---

## File Reference

| File | Purpose |
|------|---------|
| `crates/host/src/macos.rs` | macOS overlay implementation (AppKit NSPanel, GCD dispatch) |
| `tools/install-macos.sh` | Install/uninstall native messaging host manifests |
| `extension/dist/chromium/` | Chrome extension build output |
| `extension/dist/firefox/` | Firefox extension build output |
| `extension/src/` | Shared extension source code |
| `target/release/tur-overlay-host` | Built native host binary |

---

## Architecture Notes (for the curious)

The macOS port mirrors the Windows implementation but uses native macOS APIs:

- **AppKit NSPanel** — transparent floating panels positioned per video target
- **Core Graphics (Quartz 2D)** — custom drawing for the HUD button (CGContext)
- **GCD (Grand Central Dispatch)** — async message handling from the stdin thread to the main thread
- **Associated Objects** — attach panel references to NSView instances via `objc_setAssociatedObject`
- **NSMenu** — quality selection popup menu
- **Dark mode detection** — reads `NSUserDefaults AppleInterfaceStyle`

The native messaging protocol is identical to Windows — the extension sends the same JSON payloads, and the Rust host parses them the same way. Only the rendering and event loop differ per platform.
