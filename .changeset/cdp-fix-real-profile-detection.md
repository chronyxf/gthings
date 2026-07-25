---
"gthings-cdp": patch
---

- Fix: correct browser profile paths to include `/User Data` subdirectory (e.g., `Dia` → `Dia/User Data`) so `Local State` and profiles are found correctly
- Fix: add `detect_last_used_profile()` that reads `Local State` → `profile.last_used` to identify the user's actual profile
- Fix: add `--profile-directory=<last_used>` flag to launch command so the browser opens the user's real profile (with bookmarks, history, cookies) instead of a fresh `Default` profile that triggers onboarding
