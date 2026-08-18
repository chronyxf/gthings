//! GitHub URL handling: intelligent routing, raw-file fetch, and repo-root
//! README resolution on the resolved default branch.

use gthings_common::pagination::{ExtractParams, build_pagination};
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};
use std::time::Instant;
use url::Url;

use crate::article::{Article, ExtractionError, ExtractionMethod};
use crate::extractor::Extractor;

use super::AutoExtractor;
use super::rate_limit::rate_limit_status;

impl AutoExtractor {
    /// Handle GitHub URLs with intelligent routing:
    ///
    /// | Pattern                | Action                                          |
    /// |------------------------|-------------------------------------------------|
    /// | `/blob/` or `/tree/`   | Rewrite to `raw.githubusercontent.com`           |
    /// | `.diff` / `.patch`     | Direct HTTP fetch (follows to `patch-diff...`)   |
    /// | `/owner/repo` (root)   | Fetch `README.md` on the resolved default branch |
    /// |                        | (API-backed, falls back to `main` then `master`)|
    /// | Everything else        | Fall through to `WebExtractor` (Readability)     |
    pub(super) async fn extract_github(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let parsed = Url::parse(url)
            .map_err(|e| ExtractionError::Http(format!("invalid github url: {e}")))?;
        let path = parsed.path();

        // Non-root GitHub paths rewrite to a single raw URL, or fall through
        // to web extraction.
        let maybe_raw: Option<String> = {
            if path.contains("/blob/") {
                Some(
                    url.replace("github.com", "raw.githubusercontent.com")
                        .replace("/blob/", "/"),
                )
            } else if path.contains("/tree/") {
                Some(
                    url.replace("github.com", "raw.githubusercontent.com")
                        .replace("/tree/", "/"),
                )
            } else if path.ends_with(".diff") || path.ends_with(".patch") {
                // Fetch directly from github.com; reqwest follows the redirect
                // to patch-diff.githubusercontent.com automatically.
                Some(url.to_string())
            } else {
                None
            }
        };

        if let Some(raw_url) = maybe_raw {
            return self.fetch_raw_github(&raw_url, url, params).await;
        }

        // Repo root: path has exactly two non-empty segments (owner/repo).
        // Fetch the README from the resolved default branch (never the
        // hardcoded `master`, which 404s on main-only repositories).
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() == 2 {
            self.fetch_repo_readme(segments[0], segments[1], url, params)
                .await
        } else {
            self.web.extract(url.to_string(), params).await
        }
    }

    /// Fetch a raw GitHub file and wrap it in an [`Article`] with [`ContentTree::Code`].
    async fn fetch_raw_github(
        &self,
        raw_url: &str,
        original_url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let start = Instant::now();
        let content_bytes = super::fetch_bytes(&self.client, raw_url, "github raw").await?;
        let content = String::from_utf8_lossy(&content_bytes).into_owned();

        let language = Self::detect_language(original_url);
        let total_len = content.len();

        // Apply offset and max_chars slicing
        let effective_content = super::slice_text(&content, &params);
        let effective_len = effective_content.len();
        let line_count = effective_content.lines().count();

        let pagination = build_pagination(&params, total_len);

        let validated = crate::ContentQuality::validate(&effective_content);
        let duration_ms = start.elapsed().as_millis() as u64;
        let is_empty_shell = crate::quality::is_empty_shell(&effective_content);

        let provenance = Provenance::new(
            original_url.to_string(),
            ProvenanceMethod::Follow,
            duration_ms,
        );

        Ok(super::build_article(
            original_url.to_string(),
            String::new(),
            crate::article::SourceInfo {
                author: None,
                published: None,
                site_name: "GitHub".into(),
                domain_authority: crate::extractor::compute_domain_authority(original_url),
                language: None,
            },
            ExtractionMethod::RawFile,
            crate::article::round_score(1.0),
            duration_ms,
            crate::article::ContentTree::Code {
                language,
                content: effective_content,
                file_path: original_url.to_string(),
                line_count,
            },
            crate::article::ContinuationSignals {
                truncated: pagination.truncated,
                total_length: total_len,
                returned_length: effective_len,
                is_paywall: false,
                is_bot_blocked: false,
                is_empty_shell,
                related_urls: Vec::new(),
            },
            crate::article::QualityScore::from_result(&validated),
            provenance,
            pagination,
        ))
    }

    /// Fetch the repo-root `README.md` for `owner/repo`.
    ///
    /// Uses the resolved default branch (from the GitHub API, cached) and,
    /// when the API is unavailable, falls back to `main` then `master`.
    /// A rate-limited fetch is never swallowed: [`ExtractionError::RateLimited`]
    /// (with its `Retry-After` hint) always propagates to the caller.
    async fn fetch_repo_readme(
        &self,
        owner: &str,
        repo: &str,
        original_url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let branches = self.repo_readme_branches(owner, repo).await?;
        let mut last_err: Option<ExtractionError> = None;
        for branch in branches {
            let raw_url =
                format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/README.md");
            match self
                .fetch_raw_github(&raw_url, original_url, params.clone())
                .await
            {
                Ok(article) => return Ok(article),
                Err(err) => {
                    if matches!(err, ExtractionError::RateLimited { .. }) {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            ExtractionError::Http(format!("GitHub README not found for {owner}/{repo}"))
        }))
    }

    /// Resolve the ordered branch candidates for a repo's README fetch,
    /// cached per `owner/repo` for the lifetime of the extractor so the API
    /// is only consulted once per repository.
    async fn repo_readme_branches(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<String>, ExtractionError> {
        let key = format!("{owner}/{repo}");
        if let Some(candidates) = self
            .default_branches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(candidates.clone());
        }

        let branches = Self::branch_candidates(self.fetch_default_branch(owner, repo).await)?;
        self.default_branches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, branches.clone());
        Ok(branches)
    }

    /// Turn the GitHub API resolution into the ordered list of branches to
    /// try. A successful resolution yields exactly that branch; a failed one
    /// falls back to the conventional `main` then `master`. A rate-limited
    /// resolution is propagated (never dropped) so `Retry-After` reaches
    /// callers.
    fn branch_candidates(
        api: Result<Option<String>, ExtractionError>,
    ) -> Result<Vec<String>, ExtractionError> {
        match api {
            Ok(Some(branch)) => Ok(vec![branch]),
            Ok(None) => Ok(vec!["main".to_string(), "master".to_string()]),
            Err(err) => Err(err),
        }
    }

    /// Fetch the default branch of a GitHub repo via
    /// `GET https://api.github.com/repos/{owner}/{repo}` → `default_branch`.
    ///
    /// Returns `Ok(None)` when the API is unreachable, returns a non-429
    /// status, or the payload lacks `default_branch` — callers then fall back
    /// to conventional branch names. A 429 is surfaced as
    /// [`ExtractionError::RateLimited`] with `Retry-After` intact rather than
    /// being swallowed into the fallback.
    async fn fetch_default_branch(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<String>, ExtractionError> {
        let api_url = format!("https://api.github.com/repos/{owner}/{repo}");
        let resp = match self
            .client
            .get(&api_url)
            .header(
                reqwest::header::USER_AGENT,
                gthings_common::user_agent::gthings_agent(),
            )
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    %owner,
                    %repo,
                    "github API unreachable while resolving default branch; falling back to main/master"
                );
                return Ok(None);
            }
        };

        let status = resp.status();
        if !status.is_success() {
            rate_limit_status(
                status.as_u16(),
                resp.headers().get("retry-after"),
                format!("Rate limited while resolving default branch for {owner}/{repo}"),
            )?;
            return Ok(None);
        }

        let body = match resp.text().await {
            Ok(body) => body,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    %owner,
                    %repo,
                    "could not read default-branch API response body; falling back to main/master"
                );
                return Ok(None);
            }
        };
        let branch = Self::parse_default_branch(&body);
        if branch.is_none() {
            tracing::warn!(
                %owner,
                %repo,
                body_len = body.len(),
                "default-branch API payload missing or unparsable; falling back to main/master"
            );
        }
        Ok(branch)
    }

    /// Parse the GitHub repo API payload to extract the `default_branch` field.
    ///
    /// Pure and total: returns `None` for malformed JSON, an empty body, or a
    /// payload without a non-empty string `default_branch` so callers can fall
    /// back to the conventional `main` then `master` branch names.
    fn parse_default_branch(body: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|json| {
                json.get("default_branch")
                    .and_then(|b| b.as_str())
                    .map(str::to_string)
            })
            .filter(|b| !b.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── F10: default-branch resolution fallback (main → master) ──

    #[test]
    fn test_branch_candidates_falls_back_to_main_then_master() {
        // API unavailable → try `main` first, then `master`.
        assert_eq!(
            AutoExtractor::branch_candidates(Ok(None)).unwrap(),
            vec!["main".to_string(), "master".to_string()]
        );
    }

    #[test]
    fn test_branch_candidates_uses_resolved_default_branch() {
        // A successful API resolution is preferred over the fallbacks.
        assert_eq!(
            AutoExtractor::branch_candidates(Ok(Some("trunk".to_string()))).unwrap(),
            vec!["trunk".to_string()]
        );
    }

    // ── F12: default-branch payload parsing (pure, no network) ──

    #[test]
    fn test_parse_default_branch_valid() {
        assert_eq!(
            AutoExtractor::parse_default_branch(r#"{"default_branch":"main"}"#),
            Some("main".to_string())
        );
    }

    #[test]
    fn test_parse_default_branch_empty_body() {
        assert_eq!(AutoExtractor::parse_default_branch(""), None);
    }

    #[test]
    fn test_parse_default_branch_malformed_json() {
        assert_eq!(AutoExtractor::parse_default_branch("{not json"), None);
    }

    #[test]
    fn test_parse_default_branch_missing_field() {
        assert_eq!(
            AutoExtractor::parse_default_branch(r#"{"name":"repo"}"#),
            None
        );
    }

    #[test]
    fn test_parse_default_branch_non_string_field() {
        assert_eq!(
            AutoExtractor::parse_default_branch(r#"{"default_branch":42}"#),
            None
        );
    }

    #[test]
    fn test_parse_default_branch_empty_string_value() {
        assert_eq!(
            AutoExtractor::parse_default_branch(r#"{"default_branch":""}"#),
            None
        );
    }

    #[test]
    fn test_branch_candidates_propagates_rate_limited_resolution() {
        // F11: a rate-limited default-branch resolution must NOT be swallowed
        // into the main/master fallback — `Retry-After` must reach the caller.
        let err = AutoExtractor::branch_candidates(Err(ExtractionError::RateLimited {
            detail: "slow down".to_string(),
            retry_after: Some(7),
        }))
        .unwrap_err();
        match err {
            ExtractionError::RateLimited { retry_after, .. } => {
                assert_eq!(retry_after, Some(7));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}
