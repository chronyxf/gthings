---
"gthings-cdp": patch
---

- Fix: correct browser profile paths to include `/User Data` subdirectory (e.g., `Dia` → `Dia/User Data`) so the browser finds its `Local State` and opens the real user profile instead of creating a fresh `Default` profile
