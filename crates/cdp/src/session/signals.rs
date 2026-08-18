use crate::error::Result;
use crate::session::Session;
use crate::tab::Tab;
use gthings_common::domain_reputation::QualityFlag;
use serde_json::Value;

impl Session {
    /// Runs a compact JS snippet in the page to detect quality issues before extraction.
    /// The JS snippet checks for Cloudflare/Turnstile bot walls, reCAPTCHA/hCaptcha, and paywall text markers.
    /// Keep JS logic in sync with `Session::parse_signal_flags` and the
    /// page-signal detection in `gthings_extraction` (`quality/detect.rs`).
    pub async fn check_page_signals(&self, tab: &Tab) -> Result<Vec<QualityFlag>> {
        let js = r#"
            (() => {
                const flags = [];
                if (document.querySelector('#cf-challenge, .cf-turnstile, [class*="challenge"], [id*="challenge"]'))
                    flags.push("BotWall");
                if (document.title.toLowerCase().includes("just a moment"))
                    flags.push("BotWall");
                if (document.querySelector('iframe[src*="recaptcha"], iframe[src*="hcaptcha"], .h-captcha, .g-recaptcha'))
                    flags.push("Captcha");
                const text = (document.body?.innerText || '').slice(0, 2000).toLowerCase();
                if (/subscribe to continue|sign in to read|you have reached your free article limit|subscribe to read|log in to read this/i.test(text))
                    flags.push("Paywall");
                return flags;
            })()
        "#;

        let result = tab.evaluate(self, js).await?;
        Ok(Self::parse_signal_flags(&result))
    }

    /// Parse a `Runtime.evaluate` result value into quality flags.
    ///
    /// Public and crate-visible for testing. The JS snippet returns an array
    /// of strings like `["BotWall", "Captcha"]`.
    pub(crate) fn parse_signal_flags(value: &Value) -> Vec<QualityFlag> {
        match value
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
        {
            Some(arr) => arr
                .iter()
                .filter_map(|v| {
                    v.as_str().and_then(|s| match s {
                        "BotWall" => Some(QualityFlag::BotWall),
                        "Captcha" => Some(QualityFlag::Captcha),
                        "Paywall" => Some(QualityFlag::Paywall),
                        _ => None,
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_signal_flags_empty() {
        let val = json!({"result": {"type": "object", "value": []}});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_parse_signal_flags_botwall() {
        let val = json!({"result": {"type": "object", "value": ["BotWall"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall]);
    }

    #[test]
    fn test_parse_signal_flags_multiple() {
        let val = json!({"result": {"type": "object", "value": ["BotWall", "Captcha"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall, QualityFlag::Captcha]);
    }

    #[test]
    fn test_parse_signal_flags_paywall() {
        let val = json!({"result": {"type": "object", "value": ["Paywall"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::Paywall]);
    }

    #[test]
    fn test_parse_signal_flags_unknown_ignored() {
        let val =
            json!({"result": {"type": "object", "value": ["BotWall", "UnknownFlag", "Captcha"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall, QualityFlag::Captcha]);
    }

    #[test]
    fn test_parse_signal_flags_missing_result() {
        let val = json!({});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_parse_signal_flags_non_array_value() {
        let val = json!({"result": {"type": "string", "value": "not_an_array"}});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }
}
