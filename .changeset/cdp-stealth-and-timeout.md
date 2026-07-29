---
"gthings-cdp": patch
---

- Fix: osascript dialog dismiss timeout — replaced unbounded blocking call with mpsc channel + 5s recv_timeout, preventing macOS System Events hang
- Feat: Add `CdpError::CaptchaBlocked` error variant for typed Google CAPTCHA/Sorry detection
- Fix: Add stealth JS injection via `Page.addScriptToEvaluateOnNewDocument` (navigator.webdriver override) and `Network.setUserAgentOverride` to avoid Google CDP detection
- Fix: Add session_id filter to networkIdle lifecycle event predicate — prevents cross-tab event matching with many open tabs
