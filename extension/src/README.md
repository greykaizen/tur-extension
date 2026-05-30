`extension` is the source of truth.

Why this exists:

- Chromium and Firefox do not share a loadable manifest format here.
- Firefox temporary add-ons require `manifest.json` at the add-on root.
- Chromium uses the MV3 `manifest.json` in this source root.
- Firefox cannot load `manifest.firefox.json` directly as an alternate manifest file.

Workflow:

1. Edit shared behavior here:
   - `scripts/`
   - `icons/`
   - `offscreen.*`
2. Build the actual browser load roots:
   - run `extension/build-targets.ps1`
3. Load:
   - Chromium-family browsers: `extension/build/chromium`
   - Firefox-family browsers: `extension/build/firefox`

Rules:

- Shared behavior lives in `scripts/`
- Do not edit files inside `build/`
- If you change `scripts/`, rebuild once and both browser families get the same logic
