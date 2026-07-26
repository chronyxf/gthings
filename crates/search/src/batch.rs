//! Batch operations — concurrent multi-query search.
//!
//! All operations share a single [`Session`] (one WebSocket connection),
//! creating independent tabs per task with per-task timeout isolation.

use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use tokio::task::JoinSet;

use crate::search::SearchResult;

/// Batch processor for multi-query search pipelines.
///
/// Each operation creates one tab per query, runs them concurrently via
/// [`JoinSet`], and closes all tabs after completion.
pub struct BatchProcessor;

impl BatchProcessor {
    /// Search multiple queries concurrently.
    ///
    /// Creates one tab per query via [`Session::create_tab`], runs each
    /// search independently with a 30-second per-task timeout, and closes
    /// all tabs after completion.
    ///
    /// When `follow_results` is `true`, each search result's URL is also
    /// fetched (best-effort) to verify reachability. The followed content
    /// is not retained — use [`crate::follow::follow`] separately if
    /// content extraction is needed.
    ///
    /// Before following a URL, the domain reputation cache is consulted;
    /// blocked domains are skipped without opening CDP tabs. After a
    /// real extraction, detected quality flags are written back to the
    /// reputation cache.
    ///
    /// # Arguments
    ///
    /// * `session` — Shared CDP session (wrapped in [`Arc`] for concurrent access).
    /// * `queries` — List of search queries.
    /// * `count` — Results per query.
    /// * `follow_results` — If `true`, attempt to load each result URL.
    /// * `follow_max_chars` — Max characters when following (ignored if `follow_results` is `false`).
    /// * `reputation` — Optional domain reputation cache.
    pub async fn search(
        session: Arc<Session>,
        queries: &[String],
        count: usize,
        follow_results: bool,
        follow_max_chars: usize,
        reputation: Option<Arc<DomainReputation>>,
    ) -> Result<Vec<Vec<SearchResult>>, CdpError> {
        let mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>> = JoinSet::new();
        let timeout = Duration::from_secs(30);

        for query in queries {
            let session = Arc::clone(&session);
            let query = query.clone();
            let rep = reputation.clone();

            join_set.spawn(async move {
                // ── Early-exit check: scan results for blocked domains before creating tabs ──
                // We still need a tab for the search itself; the per-URL check happens
                // inside the follow call below.

                // 1. Create tab outside timeout — guarantees we can close it on all paths
                let tab = match session.create_tab("about:blank").await {
                    Ok(t) => t,
                    Err(e) => return Err(e),
                };

                // 2. Search with timeout (tab is alive during this)
                let search_result = tokio::time::timeout(
                    timeout,
                    crate::search::search(&session, &tab, &query, count),
                )
                .await;

                // 3. In-browser pre-check on the search tab: detect bot-walls / captchas
                //    before processing individual results. If the search page itself is
                //    blocked, skip follow entirely for all results.
                let search_tab_blocked = if let Ok(Ok(_)) = &search_result {
                    if let Ok(flags) = session.check_page_signals(&tab).await {
                        let blocked = flags.iter().any(|f| {
                            matches!(
                                f,
                                gthings_common::domain_reputation::QualityFlag::BotWall
                                    | gthings_common::domain_reputation::QualityFlag::Captcha
                                    | gthings_common::domain_reputation::QualityFlag::Paywall
                            )
                        });
                        if blocked {
                            tracing::warn!(
                                "batch: search page blocked (flags={:?}), skipping follow",
                                flags
                            );
                            // Per-URL reputation is written inside follow().
                        }
                        blocked
                    } else {
                        false
                    }
                } else {
                    false
                };

                // 4. If search succeeded and follow_results is enabled, follow best-effort
                if let Ok(Ok(ref results)) = search_result {
                    if follow_results && !search_tab_blocked {
                        for result in results {
                            let host =
                                gthings_common::extract_host(&result.url).unwrap_or_default();

                            // Skip blocked domains without CDP interaction
                            if let Some(ref rep) = rep {
                                if !host.is_empty() && rep.is_blocked(&host).await {
                                    tracing::debug!(
                                        url = %result.url,
                                        "batch: skip blocked domain"
                                    );
                                    continue;
                                }
                            }

                            let params = ExtractParams {
                                offset: 0,
                                max_chars: follow_max_chars,
                            };
                            if let Err(e) = crate::follow::follow(
                                &session,
                                &tab,
                                &result.url,
                                params,
                                rep.as_deref(),
                            )
                            .await
                            {
                                tracing::warn!(
                                    url = %result.url,
                                    error = %e,
                                    "batch: follow_result failed"
                                );
                            }
                        }
                    }
                }

                // 5. ALWAYS close tab — runs even on timeout or search error
                if let Err(e) = session.close_tab(tab).await {
                    tracing::warn!("close_tab failed: {e}");
                }

                // 6. Convert timeout error to CdpError
                match search_result {
                    Ok(Ok(results)) => Ok(results),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(CdpError::CdpCallFailed {
                        method: "batch_search".into(),
                        detail: format!("timeout for query: {query}"),
                    }),
                }
            });
        }

        let mut all_results = Vec::new();
        while let Some(task) = join_set.join_next().await {
            match task {
                Ok(Ok(results)) => all_results.push(results),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(CdpError::CdpCallFailed {
                        method: "batch_search".into(),
                        detail: format!("join error: {join_err}"),
                    });
                }
            }
        }
        Ok(all_results)
    }
}
