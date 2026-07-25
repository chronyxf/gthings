---
"gthings-cdp": patch
---

- Fix: restore real profile detection to avoid browser onboarding/login prompts. Uses `real_profile_dir()` to find the user's real browser profile (Dia, Chrome, Arc, etc.) via macOS Launch Services.
- Fix: prevent SingletonLock conflicts by relying on `find_existing()` reuse — only one browser instance is ever launched per profile. If a fresh launch is needed, lock files are cleaned first.
