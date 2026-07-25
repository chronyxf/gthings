---
"gthings-cdp": minor
---

- Feat: macOS default browser detection — uses Launch Services to find the user's default Chromium browser (Chrome, Dia, Arc, Brave, Edge)
- Feat: browser-profile matching — automatically selects the correct profile directory for the detected browser, preventing profile format mismatch errors
- Feat: config env vars (GTHINGS_BROWSER_PATH, GTHINGS_PROFILE_DIR, GTHINGS_CDP_PORT) now wired into browser launch
- Feat: fallback to temp profile `/tmp/gthings-{port}` when no real profile is found
- Chore: added `--no-default-browser-check` and `--disable-sync` flags for launch stability
