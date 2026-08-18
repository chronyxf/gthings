//! Page following and content extraction via CDP.
//!
//! Navigates to a URL, waits for network idle, then extracts title and body
//! text via in-browser JavaScript evaluation.
//!
//! This module is split into sub-modules:
//! - [`clean`] — Boilerplate stripping and whitespace collapse helpers
//! - [`result`] — Error-result and provenance construction
//! - [`tests`] — Unit tests (cfg(test))

mod clean;
mod result;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use clean::collapse_whitespace;
pub(crate) use clean::{strip_boilerplate, strip_leading_title};
pub(crate) use result::{error_provenance, make_error_result};

use std::time::Instant;

use chrono::Utc;
use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::domain_reputation::{DomainReputation, QualityFlag};
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::Provenance;
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
    /// Quality flags detected on the extracted content (single scan, shared
    /// with the reputation write-back and downstream quality scoring).
    #[serde(default)]
    pub quality_flags: Vec<QualityFlag>,
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
    let host = gthings_common::extract_host(url).unwrap_or_else(|| {
        tracing::warn!("follow: failed to parse host from URL: {url}");
        String::new()
    });

    // ── Early-exit if domain reputation says blocked ──
    if let Some(rep) = reputation {
        if !host.is_empty() && rep.is_blocked(&host).await {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(make_error_result(
                url,
                "blocked by domain reputation (BotWall/Paywall)",
                duration_ms,
            ));
        }
    }

    // ── Real CDP extraction ──
    tab.navigate(session, url).await?;

    // ── In-browser pre-check (before full extraction) ──
    // Runs a lightweight JS snippet (< 50ms) to detect bot-walls, captchas,
    // and paywalls early. If any quality flags are found, skip the expensive
    // extraction JS and return immediately with those flags.
    if let Ok(flags) = session.check_page_signals(tab).await {
        let has_blockers = flags.iter().any(gthings_common::quality_flag_is_blocking);
        if has_blockers {
            // Write flags into reputation cache so future requests skip this domain
            if let Some(rep) = reputation {
                if !host.is_empty() {
                    rep.write(&host, &flags).await;
                }
            }
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(make_error_result(
                url,
                &format!("early abort: blocked by {:?}", flags),
                duration_ms,
            ));
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
    let js = include_str!("../../templates/follow_extract.js")
        .replace("__OFFSET__", &params.offset.to_string())
        .replace("__MAX_CHARS__", &params.max_chars.to_string());

    let result = tab.evaluate(session, &js).await?;
    let raw = result.pointer("/result/value").and_then(|v| v.as_str());
    let json_str = raw.unwrap_or_else(|| {
        tracing::warn!("follow: CDP result missing /result/value field");
        r#"{"title":"","content":"","error":"CDP result missing value field"}"#
    });
    let mut follow_result: FollowResult = parse_follow_json(json_str).map_err(|e| *e)?;
    follow_result.url = url.to_string();
    // Post-extraction content cleaning:
    // 1) Strip only unambiguous image-view chrome (Medium image captions).
    //    Real prose is preserved verbatim — no boilerplate phrases that could
    //    appear in genuine article text are removed.
    // 2) Collapse whitespace while preserving paragraph structure: runs of
    //    spaces/tabs within a line become a single space, but newlines are
    //    kept as paragraph breaks (2+ consecutive newlines collapse to one).
    // 3) Remove a leading run of title text duplicated at the top of the body.
    let cleaned = strip_boilerplate(&follow_result.content);
    let cleaned = strip_leading_title(&cleaned, &follow_result.title);
    follow_result.content = cleaned;

    let duration_ms = start.elapsed().as_millis() as u64;
    let content_len = follow_result.content.len();

    follow_result.pagination = Some(gthings_common::pagination::build_pagination(
        &params,
        content_len,
    ));

    let prov = &mut follow_result.provenance;
    prov.source_url = follow_result.url.clone();
    prov.agent = gthings_common::user_agent::gthings_agent();
    prov.accessed_at = Utc::now();
    prov.duration_ms = duration_ms;

    // ── Post-extraction: detect quality flags ONCE and share the result ──
    // A single full-text scan feeds both the reputation write-back below and
    // the downstream quality scoring (via `FollowResult::quality_flags`).
    let detected = detect_quality_flags(&follow_result.content);
    follow_result.quality_flags = detected.clone();
    if let Some(rep) = reputation {
        if !host.is_empty() {
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

/// Parse the JSON string returned by the in-browser extraction JS,
/// returning a [`FollowResult`] or a boxed [`CdpError::Json`] on failure.
fn parse_follow_json(json_str: &str) -> Result<FollowResult, Box<CdpError>> {
    serde_json::from_str(json_str).map_err(|e| {
        let mut end = json_str.len().min(200);
        while end > 0 && !json_str.is_char_boundary(end) {
            end -= 1;
        }
        let preview = &json_str[..end];
        tracing::warn!("follow: failed to parse extraction JSON: {e} (preview: {preview:?})");
        Box::new(CdpError::Json(e))
    })
}

/// Run quality heuristics on extracted content and return matching flags.
fn detect_quality_flags(content: &str) -> Vec<QualityFlag> {
    gthings_extraction::ContentQuality::detect_all(content)
}

/// Outcome of a timed search inside a temporary tab.
pub(crate) enum TimedSearchOutcome {
    /// Search succeeded.
    Success(Vec<crate::SearchResult>),
    /// Search returned an error.
    Error(CdpError),
    /// Search timed out.
    Timeout,
}
