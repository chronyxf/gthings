---
"gthings-search": patch
---

- Feat: Add CAPTCHA/Sorry page detection after Google navigation — checks URL for `/sorry/` and page title for "Accessibility"/"Learn more", returns `CdpError::CaptchaBlocked` instead of fake results
