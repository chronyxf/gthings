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

    let js = format!(
        r#"try {{ let title = document.title || ''; let text = document.body ? document.body.innerText : ''; let t = text.substring({}, {}); let trunc = text.length > {} + {}; JSON.stringify({{title, content: t, error: ''}}) }} catch(e) {{ JSON.stringify({{title: document.title || '', content: '', error: e.message}}) }}"#,
        params.offset, params.max_chars, params.offset, params.max_chars
    );

    let result = tab.evaluate(session, &js).await?;
    let json_str = result["result"]["value"].as_str().unwrap_or("{}");
    let mut follow_result: FollowResult = serde_json::from_str(json_str)?;
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
}
