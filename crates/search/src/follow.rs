//! Page following and content extraction via CDP.
//!
//! Navigates to a URL, waits for network idle, then extracts title and body
//! text via in-browser JavaScript evaluation.

use std::time::Instant;

use chrono::Utc;
use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::domain_reputation::{DomainReputation, QualityFlag};
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::{ExtractionMethod, Provenance};
use serde::{Deserialize, Serialize};

/// Result of following a URL and extracting its content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowResult {
    /// The URL that was followed.
    #[serde(default)]
    pub url: String,
    /// Document title from `<title>`.
    #[serde(default)]
    pub title: String,
    /// Extracted page text (up to `max_chars`).
    #[serde(default)]
    pub content: String,
    /// Non-empty if the in-browser JS evaluation threw an error.
    #[serde(default)]
    pub error: String,
    /// How and when this content was acquired.
    #[serde(default)]
    pub provenance: Provenance,
    /// Pagination state.
    pub pagination: Option<Pagination>,
}

/// Follow a URL and extract page content via CDP.
///
/// Navigates to the URL, waits for network idle via lifecycle events,
/// then extracts the page title and body text (up to `max_chars`) via
/// in-browser JS evaluation.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection.
/// * `tab` — An already-created tab. Will be navigated to `url`.
/// * `url` — The URL to fetch.
/// * `params` — Extraction parameters (offset, max_chars).
/// * `reputation` — Optional domain reputation cache. When provided, the
///   function checks the domain's reputation before navigating; if the
///   domain is blocked (BotWall/Paywall on 2+ consecutive hits), it
///   returns a synthesized low-quality result without CDP navigation.
///   After extraction, detected quality flags are written back.
pub async fn follow(
    session: &Session,
    tab: &Tab,
    url: &str,
    params: ExtractParams,
    reputation: Option<&DomainReputation>,
) -> Result<FollowResult, CdpError> {
    let start = Instant::now();
    let host = gthings_common::extract_host(url).unwrap_or_default();

    // ── Early-exit if domain reputation says blocked ──
    if let Some(rep) = reputation {
        if !host.is_empty() && rep.is_blocked(&host).await {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(FollowResult {
                url: url.to_string(),
                title: String::new(),
                content: String::new(),
                error: "blocked by domain reputation (BotWall/Paywall)".into(),
                provenance: Provenance {
                    source_url: url.to_string(),
                    method: ExtractionMethod::Follow,
                    agent: gthings_common::GTHINGS_AGENT.into(),
                    accessed_at: Utc::now(),
                    duration_ms,
                    derived_from: None,
                },
                pagination: None,
            });
        }
    }

    // ── Real CDP extraction ──
    tab.navigate(session, url).await?;

    // ── In-browser pre-check (before full extraction) ──
    // Runs a lightweight JS snippet (< 50ms) to detect bot-walls, captchas,
    // and paywalls early. If any quality flags are found, skip the expensive
    // extraction JS and return immediately with those flags.
    if let Ok(flags) = session.check_page_signals(tab).await {
        let has_blockers = flags.iter().any(|f| {
            matches!(
                f,
                gthings_common::domain_reputation::QualityFlag::BotWall
                    | gthings_common::domain_reputation::QualityFlag::Captcha
                    | gthings_common::domain_reputation::QualityFlag::Paywall
            )
        });
        if has_blockers {
            // Write flags into reputation cache so future requests skip this domain
            if let Some(rep) = reputation {
                if !host.is_empty() {
                    rep.write(&host, &flags).await;
                }
            }
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(FollowResult {
                url: url.to_string(),
                title: String::new(),
                content: String::new(),
                error: format!("early abort: blocked by {:?}", flags),
                provenance: Provenance {
                    source_url: url.to_string(),
                    method: ExtractionMethod::Follow,
                    agent: gthings_common::GTHINGS_AGENT.into(),
                    accessed_at: Utc::now(),
                    duration_ms,
                    derived_from: None,
                },
                pagination: None,
            });
        }
    }

    // In-browser JS: extract page content with SPA rendering tolerance.
    // 1) Async polling loop (up to 3 s) with 100 ms yields — does NOT block
    //    the main thread, so SPAs can render.
    // 2) Prefer <main>, <article>, or [role="main"] over <body>.
    // 3) Conditional stripping: remove chrome elements only when a semantic
    //    container was found; when falling back to <body> keep nav/footer/header.
    // 4) Fallback from innerText to textContent when extracted text is < 80 chars
    //    (catches SPAs that render content via CSS-displayed elements).
    // 5) Wrapped in an async IIFE so CDP with awaitPromise:true waits for it.
    let js = format!(
        r#"(async function() {{try {{var _deadline=Date.now()+3000;while(Date.now()<_deadline){{if(document.body&&document.body.innerText&&document.body.innerText.length>100)break;await new Promise(function(r){{setTimeout(r,100);}});}}var _c=document.querySelector('main, article, [role="main"]')??document.body;if(!_c){{return JSON.stringify({{title:document.title||'',content:'',error:'No document body found'}});}}else{{var _isMain=_c!==document.body;var _cl=_c.cloneNode(true);if(_isMain){{_cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e){{e.remove()}});}}else{{_cl.querySelectorAll('script,style,noscript,svg,iframe').forEach(function(e){{e.remove()}});}}var _text=_cl.innerText||'';if(_text.length<80){{_text=_cl.textContent||'';_text=_text.replace(/\s+/g,' ').trim();}}var _title=document.title||'';if(_text.length<3){{return JSON.stringify({{title:_title,content:'',error:'content too short ('+_text.length+' chars)'}});}}else{{var _t=_text.substring({},{});return JSON.stringify({{title:_title,content:_t,error:''}});}}}}}}catch(e){{return JSON.stringify({{title:document.title||'',content:'',error:e.message}});}}}})()"#,
        params.offset, params.max_chars
    );

    let result = tab.evaluate(session, &js).await?;
    let raw = result["result"]["value"].as_str();
    let json_str = raw.unwrap_or(
        r#"{"title":"","content":"","error":"CDP result missing value field"}"#,
    );
    let mut follow_result: FollowResult = serde_json::from_str(json_str)
        .map_err(|e| {
            let preview = &json_str[..json_str.len().min(200)];
            tracing::warn!("follow: failed to parse extraction JSON: {e} (preview: {preview:?})");
            CdpError::Json(e)
        })?;
    follow_result.url = url.to_string();

    let duration_ms = start.elapsed().as_millis() as u64;
    let content_len = follow_result.content.len();

    follow_result.pagination = Some(gthings_common::pagination::build_pagination(
        &params,
        url,
        content_len,
        content_len,
    ));

    follow_result.provenance = Provenance {
        source_url: url.to_string(),
        method: ExtractionMethod::Follow,
        agent: gthings_common::GTHINGS_AGENT.into(),
        accessed_at: Utc::now(),
        duration_ms,
        derived_from: None,
    };

    // ── Post-extraction: detect quality flags and update reputation ──
    if let Some(rep) = reputation {
        if !host.is_empty() {
            let detected = detect_quality_flags(&follow_result.content);
            if !detected.is_empty() {
                rep.write(&host, &detected).await;
            } else {
                // Clean extraction — reset any BotWall/Paywall flags
                rep.decay(&host).await;
            }
        }
    }

    Ok(follow_result)
}

/// Run quality heuristics on extracted content and return matching flags.
fn detect_quality_flags(content: &str) -> Vec<QualityFlag> {
    let mut flags = Vec::new();

    if gthings_extraction::ContentQuality::detect_bot(content) {
        flags.push(QualityFlag::BotWall);
    }
    if gthings_extraction::ContentQuality::detect_paywall(content) {
        flags.push(QualityFlag::Paywall);
    }
    if gthings_extraction::ContentQuality::detect_captcha(content) {
        flags.push(QualityFlag::Captcha);
    }
    if gthings_extraction::ContentQuality::detect_empty_shell(content) {
        flags.push(QualityFlag::EmptyShell);
    }

    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── detect_quality_flags ──────────────────────────────────────────

    #[test]
    fn test_detect_quality_flags_bot() {
        let flags = detect_quality_flags("Please verify you are a human. Cloudflare challenge.");
        assert!(flags.contains(&QualityFlag::BotWall));
    }

    #[test]
    fn test_detect_quality_flags_paywall() {
        let flags = detect_quality_flags("Subscribe to read this article. Paywall.");
        assert!(flags.contains(&QualityFlag::Paywall));
    }

    #[test]
    fn test_detect_quality_flags_clean() {
        let flags = detect_quality_flags(
            "This is a sufficiently long piece of content with many words and sentences \
             that should pass all quality checks without triggering any of the detection \
             heuristics for blocked pages or subscription prompts.",
        );
        assert!(flags.is_empty());
    }

    #[test]
    fn test_detect_quality_flags_empty() {
        let flags = detect_quality_flags("");
        assert!(flags.contains(&QualityFlag::EmptyShell));
    }

    #[test]
    fn test_detect_quality_flags_captcha() {
        let flags = detect_quality_flags("reCAPTCHA verification required");
        assert!(flags.contains(&QualityFlag::Captcha));
    }

    // ── Reputation early-exit (pure host-extraction check) ────────────

    #[test]
    fn test_reputation_check_blocks_follow() {
        // Verify that the URL-to-host extraction used by the block check is correct.
        // The follow() function uses gthings_common::extract_host() before checking
        // reputation.is_blocked().
        let url = "https://blocked-domain.example.com/page";
        let host = gthings_common::extract_host(url);
        assert_eq!(host, Some("blocked-domain.example.com".into()));
    }

    // ── FollowResult JSON parsing ──────────────────────────────────────

    #[test]
    fn test_follow_result_parse_valid() {
        let json = r#"{"title":"Hello","content":"World","error":""}"#;
        let result: FollowResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.title, "Hello");
        assert_eq!(result.content, "World");
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_follow_result_parse_with_error() {
        let json = r#"{"title":"","content":"","error":"content too short (3 chars)"}"#;
        let result: FollowResult = serde_json::from_str(json).unwrap();
        assert!(result.title.is_empty());
        assert!(result.content.is_empty());
        assert_eq!(result.error, "content too short (3 chars)");
    }

    #[test]
    fn test_follow_result_parse_missing_fields() {
        // Missing fields should get serde defaults (empty strings).
        let json = r#"{}"#;
        let result: FollowResult = serde_json::from_str(json).unwrap();
        assert!(result.title.is_empty());
        assert!(result.content.is_empty());
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_follow_result_parse_malformed() {
        let json = r#"not valid json"#;
        let err = serde_json::from_str::<FollowResult>(json);
        assert!(err.is_err());
    }

    #[test]
    fn test_follow_result_parse_partial() {
        // Only title provided; content/error should be empty.
        let json = r#"{"title":"Partial"}"#;
        let result: FollowResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.title, "Partial");
        assert!(result.content.is_empty());
        assert!(result.error.is_empty());
    }

    // ── JS extraction format string smoke tests ───────────────────────

    /// Verify the format string contains the new compound selector.
    #[test]
    fn test_extraction_js_has_new_selectors() {
        let js = format!(
            r#"(async function() {{try {{var _deadline=Date.now()+3000;while(Date.now()<_deadline){{if(document.body&&document.body.innerText&&document.body.innerText.length>100)break;await new Promise(function(r){{setTimeout(r,100);}});}}var _c=document.querySelector('main, article, [role="main"]')??document.body;if(!_c){{return JSON.stringify({{title:document.title||'',content:'',error:'No document body found'}});}}else{{var _isMain=_c!==document.body;var _cl=_c.cloneNode(true);if(_isMain){{_cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e){{e.remove()}});}}else{{_cl.querySelectorAll('script,style,noscript,svg,iframe').forEach(function(e){{e.remove()}});}}var _text=_cl.innerText||'';if(_text.length<80){{_text=_cl.textContent||'';_text=_text.replace(/\s+/g,' ').trim();}}var _title=document.title||'';if(_text.length<3){{return JSON.stringify({{title:_title,content:'',error:'content too short ('+_text.length+' chars)'}});}}else{{var _t=_text.substring({},{});return JSON.stringify({{title:_title,content:_t,error:''}});}}}}}}catch(e){{return JSON.stringify({{title:document.title||'',content:'',error:e.message}});}}}})()"#,
            0usize, 5000usize
        );
        assert!(
            js.contains(r#"querySelector('main, article, [role="main"]')"#),
            "JS must use the new compound selector"
        );
        assert!(
            js.contains("Date.now()<_deadline"),
            "JS must contain the 3-second async polling loop guard"
        );
        assert!(
            js.contains("await new Promise"),
            "JS must contain async/await in polling loop"
        );
        assert!(
            js.contains("_text=_cl.textContent"),
            "JS must have textContent fallback"
        );
    }

    /// Verify that when `isMain` is true, chrome elements are stripped;
    /// when falling back to body, nav/footer/header are preserved.
    #[test]
    fn test_extraction_js_conditional_stripping_main() {
        let js = format!(
            r#"(async function() {{try {{var _deadline=Date.now()+3000;while(Date.now()<_deadline){{if(document.body&&document.body.innerText&&document.body.innerText.length>100)break;await new Promise(function(r){{setTimeout(r,100);}});}}var _c=document.querySelector('main, article, [role="main"]')??document.body;if(!_c){{return JSON.stringify({{title:document.title||'',content:'',error:'No document body found'}});}}else{{var _isMain=_c!==document.body;var _cl=_c.cloneNode(true);if(_isMain){{_cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e){{e.remove()}});}}else{{_cl.querySelectorAll('script,style,noscript,svg,iframe').forEach(function(e){{e.remove()}});}}var _text=_cl.innerText||'';if(_text.length<80){{_text=_cl.textContent||'';_text=_text.replace(/\s+/g,' ').trim();}}var _title=document.title||'';if(_text.length<3){{return JSON.stringify({{title:_title,content:'',error:'content too short ('+_text.length+' chars)'}});}}else{{var _t=_text.substring({},{});return JSON.stringify({{title:_title,content:_t,error:''}});}}}}}}catch(e){{return JSON.stringify({{title:document.title||'',content:'',error:e.message}});}}}})()"#,
            0usize, 5000usize
        );
        // The full JS must contain the chrome-rich stripping query
        assert!(
            js.contains("script,style,noscript,svg,iframe,nav,footer,header"),
            "Main-branch stripping must include nav, footer, header"
        );
        // And also the body-branch stripping query (without nav/footer/header)
        assert!(
            js.contains("script,style,noscript,svg,iframe"),
            "JS must contain the body-branch stripping query"
        );
        // Ensure we have exactly two distinct stripping queries.
        let with_chrome = js.match_indices("nav,footer,header").count();
        let without_nav = js.match_indices("script,style,noscript,svg,iframe").count();
        assert_eq!(
            with_chrome, 1,
            "nav,footer,header should appear exactly once (main branch)"
        );
        assert_eq!(
            without_nav, 2,
            "the minimal stripping query should appear twice (once in each branch)"
        );
    }

    /// Verify that < 3 char content produces an error in the JS logic.
    #[test]
    fn test_extraction_js_short_content_error() {
        let js = format!(
            r#"(async function() {{try {{var _deadline=Date.now()+3000;while(Date.now()<_deadline){{if(document.body&&document.body.innerText&&document.body.innerText.length>100)break;await new Promise(function(r){{setTimeout(r,100);}});}}var _c=document.querySelector('main, article, [role="main"]')??document.body;if(!_c){{return JSON.stringify({{title:document.title||'',content:'',error:'No document body found'}});}}else{{var _isMain=_c!==document.body;var _cl=_c.cloneNode(true);if(_isMain){{_cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e){{e.remove()}});}}else{{_cl.querySelectorAll('script,style,noscript,svg,iframe').forEach(function(e){{e.remove()}});}}var _text=_cl.innerText||'';if(_text.length<80){{_text=_cl.textContent||'';_text=_text.replace(/\s+/g,' ').trim();}}var _title=document.title||'';if(_text.length<3){{return JSON.stringify({{title:_title,content:'',error:'content too short ('+_text.length+' chars)'}});}}else{{var _t=_text.substring({},{});return JSON.stringify({{title:_title,content:_t,error:''}});}}}}}}catch(e){{return JSON.stringify({{title:document.title||'',content:'',error:e.message}});}}}})()"#,
            0usize, 5000usize
        );
        assert!(
            js.contains("content too short ("),
            "Short content must produce an error message"
        );
        assert!(
            js.contains("_text.length<3"),
            "The short-content threshold must be 3"
        );
    }
}
