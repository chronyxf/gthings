---
"gthings-cdp": patch
---

- Fix: switch to temporary profile by default to avoid "Something went wrong when opening your profile" error. Removes `real_profile_dir()` / `browser_profile_suffix()` functions that tried to match browser to real user profile (which caused SingletonLock conflicts).
- Chore: add CDP launch stability flags (`--enable-automation`, `--disable-background-networking`, `--disable-extensions`, `--disable-component-update`, `--disable-default-apps`, `--password-store=basic`, `--use-mock-keychain`)
