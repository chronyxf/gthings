//! Job workers — consume [`QueuedJob`]s off the [`JobQueue`](super::queue::JobQueue)
//! and execute them.
//!
//! A [`JobWorker`] owns the daemon's search router, trace_id → SSE-sender
//! [`JobRegistry`], and the optional warm CDP [`SharedConnection`]. Each job
//! is validated once (by the API layer, into a [`QueuedJob`]), bounded by
//! [`QueuedJob::timeout`], executed (search ops through
//! [`gthings_search::search_streaming`], non-search ops
//! `extract`/`ax`/`pdf-url`/`pdf-file` through the extraction and CDP
//! backends), and streamed to the job's SSE sender. Every job publishes
//! exactly one terminal SSE frame — always `done`, always last — carrying a
//! JSON `content` object (the real result on success, an error envelope on
//! failure). `error` frames, when emitted, are non-terminal informational
//! frames that precede the terminal `done`.
//!
//! Tab-owning jobs (those pinning the Google engine, plus the `ax` op, which
//! opens a background tab) are serialized behind a single mutex so the daemon
//! holds at most one warm session's tabs at a time.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use gthings_cdp::ax_tree::{AxTreeResult, ax_tree};
use gthings_cdp::{CdpError, SharedConnection};
use gthings_common::envelope::{Envelope, ErrorBody};
use gthings_common::pagination::ExtractParams;
use gthings_common::taxonomy::ErrorCode;
use gthings_common::telemetry::StderrEvent;
use gthings_extraction::{Article, AutoExtractor, ExtractionError, Extractor, PdfExtractor};
use gthings_search::engine::SearchOptions;
use gthings_search::engine::router::SearchRouter;
use gthings_search::{
    BatchHarvestRequest, EngineChoice, RankStrategy, SearchEngine, SearchEngineError, SearchEvent,
    SearchResult, harvest, search_streaming,
};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;
use tracing::{debug, warn};

use crate::jobs::args::{EngineArg, JobArgs, Strategy};
use crate::jobs::{Op, QueuedJob};
use crate::sse::{SseEvent, error_engine, error_retry_after_ms, project_event};

/// One [`search_streaming`] dispatch target produced from validated args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchTarget {
    /// The query to search.
    pub query: String,
    /// Max results requested.
    pub count: usize,
    /// Engine choice (from the validated `engine` arg).
    pub choice: EngineChoice,
    /// Recency filter (`day`/`week`/`month`/`year` or an ISO date).
    pub freshness: Option<String>,
    /// Search depth (`basic`/`advanced`).
    pub search_depth: Option<String>,
}

/// Map validated job args to the [`search_streaming`] calls that satisfy them.
///
/// Search ops only: `simple` dispatches one stream for its single query,
/// while `parallel` and `harvest` fan out one stream per query (`harvest`
/// runs the full search → dedup → rank → follow pipeline through the warm
/// CDP session in [`JobWorker::run_harvest`]). Non-search ops return no
/// dispatch targets and are executed by [`JobWorker::run_non_search`].
#[must_use]
pub(crate) fn dispatch_plan(args: &JobArgs) -> Vec<DispatchTarget> {
    let choice = args.engine.to_engine_choice();
    match args.strategy {
        Strategy::Simple => {
            let query = args
                .query
                .clone()
                .expect("validated 'simple' args always carry a query");
            vec![DispatchTarget {
                query,
                count: args.count,
                choice,
                freshness: args.freshness.clone(),
                search_depth: args.search_depth.clone(),
            }]
        }
        Strategy::Parallel | Strategy::Harvest => args
            .queries
            .iter()
            .map(|query| DispatchTarget {
                query: query.clone(),
                count: args.count,
                choice,
                freshness: args.freshness.clone(),
                search_depth: args.search_depth.clone(),
            })
            .collect(),
        // Non-search ops are executed by run_non_search, not dispatched.
        Strategy::NonSearch => vec![],
    }
}

/// Whether executing `op` with `args` may open CDP tabs, requiring the
/// connection lock.
///
/// The Google engine backend owns tabs, and the `ax` op creates a background
/// tab in the warm session; plain-HTTP engines and the remaining non-search
/// ops do not.
#[must_use]
pub(crate) fn needs_connection(op: Op, args: &JobArgs) -> bool {
    op == Op::Ax || matches!(args.engine, EngineArg::Google)
}

/// trace_id → SSE-sender registry.
///
/// The API layer registers the send half of a job's SSE channel before
/// enqueueing it, so the worker can publish progress events by trace id. The
/// channel carries already-projected [`SseEvent`]s so the worker can publish
/// the terminal `done` frame with the complete result envelope.
#[derive(Debug, Default)]
pub(crate) struct JobRegistry {
    inner: Mutex<HashMap<String, mpsc::Sender<SseEvent>>>,
}

impl JobRegistry {
    /// Create an empty registry.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Associate `trace_id` with its SSE sender.
    ///
    /// Returns `false` when `trace_id` is already registered — the registry is
    /// keyed by trace id, so a duplicate would silently overwrite the original
    /// job's sender and cross-wire its SSE stream. Callers must reject the
    /// duplicate (HTTP 409) rather than proceed.
    pub(crate) async fn register(
        &self,
        trace_id: impl Into<String>,
        tx: mpsc::Sender<SseEvent>,
    ) -> bool {
        let mut inner = self.inner.lock().await;
        let key = trace_id.into();
        if inner.contains_key(&key) {
            return false;
        }
        inner.insert(key, tx);
        true
    }

    /// The SSE sender for `trace_id`, if registered.
    pub(crate) async fn sender(&self, trace_id: &str) -> Option<mpsc::Sender<SseEvent>> {
        self.inner.lock().await.get(trace_id).cloned()
    }

    /// Drop the association for `trace_id` once its stream has ended.
    pub(crate) async fn unregister(&self, trace_id: &str) {
        self.inner.lock().await.remove(trace_id);
    }
}

/// Executes jobs pulled off the queue.
pub(crate) struct JobWorker {
    router: Arc<SearchRouter>,
    registry: Arc<JobRegistry>,
    connection: Option<Arc<SharedConnection>>,
    conn_lock: Arc<Mutex<()>>,
    http: reqwest::Client,
}

impl JobWorker {
    /// Build a worker sharing the daemon's router, registry, and warm CDP
    /// connection.
    #[must_use]
    pub(crate) fn new(
        router: Arc<SearchRouter>,
        registry: Arc<JobRegistry>,
        connection: Option<Arc<SharedConnection>>,
    ) -> Self {
        Self {
            router,
            registry,
            connection,
            conn_lock: Arc::new(Mutex::new(())),
            http: reqwest::Client::builder()
                .user_agent(gthings_common::user_agent::BROWSER_UA)
                .default_headers({
                    let mut headers = reqwest::header::HeaderMap::new();
                    headers.insert(
                        reqwest::header::ACCEPT,
                        reqwest::header::HeaderValue::from_static(
                            gthings_common::user_agent::BROWSER_ACCEPT,
                        ),
                    );
                    headers
                })
                .build()
                .expect("failed to build daemon HTTP client"),
        }
    }

    /// Execute one pre-validated job end-to-end: dispatch → stream.
    ///
    /// The API layer parses and validates the wire args exactly once and
    /// enqueues a [`QueuedJob`], so `run` trusts `job.args` without re-parsing
    /// the raw JSON. Tab-owning jobs serialize on the connection mutex and are
    /// refused once the connection is shutting down. Every executed job —
    /// success, failure, or timeout — publishes exactly one terminal SSE frame
    /// (`done`, always last) carrying a JSON `content` object. On failure
    /// `done.content` is an error envelope; `error` frames (if any) are
    /// non-terminal informational frames emitted before the terminal `done`.
    pub(crate) async fn run(&self, job: QueuedJob) {
        // Keep the client-supplied trace id (the API layer falls back to a
        // generated id only when the request carried none) so it can be echoed
        // verbatim in the terminal `done` envelope.
        let trace_id = job.trace_id.clone();
        let sender = self
            .registry
            .sender(trace_id.as_deref().unwrap_or_default())
            .await;
        let started = Instant::now();

        let timeout = job.timeout();
        let args = job.args;

        // Serialize tab-owning jobs so the daemon never opens more than one
        // warm session's tabs at a time; refuse them once draining.
        let _guard = if needs_connection(job.op, &args) {
            if let Some(connection) = &self.connection {
                if connection.is_shutdown() {
                    let envelope = Envelope::error(ErrorBody::new(
                        ErrorCode::ConnectionFailed,
                        "CDP connection is shutting down; tab-owning job refused",
                    ));
                    // Informational error frame, then the terminal done with
                    // the error envelope in content — done is always last.
                    emit(
                        &sender,
                        SseEvent::Error {
                            code: ErrorCode::ConnectionFailed,
                            message: "CDP connection is shutting down; tab-owning job refused"
                                .into(),
                            engine: None,
                            retry_after_ms: None,
                        },
                    )
                    .await;
                    emit(
                        &sender,
                        SseEvent::Done {
                            query: None,
                            results: Vec::new(),
                            trace_id: trace_id.clone(),
                            count: 0,
                            engine: None,
                            duration_ms: started.elapsed().as_millis() as u64,
                            sla_ms: timeout.as_millis() as u64,
                            queries: Vec::new(),
                            content: Some(error_content_from_envelope(
                                &envelope,
                                args.url.as_deref(),
                            )),
                        },
                    )
                    .await;
                    return;
                }
            }
            Some(self.conn_lock.lock().await)
        } else {
            None
        };

        debug!(op = %job.op, timeout_ms = timeout.as_millis(), "running job");
        emit(
            &sender,
            SseEvent::JobStarted {
                trace_id: trace_id.clone(),
            },
        )
        .await;
        telemetry("info", &trace_id, job.op, 0);

        let queries = target_queries(&args);
        let outcome =
            match tokio::time::timeout(timeout, self.execute(job.op, &args, &sender)).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    warn!(op = %job.op, timeout_ms = timeout.as_millis(), "job timed out");
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    // The stream's budget elapsed: publish an informational error
                    // frame; the terminal done (with the timeout error envelope in
                    // content) is emitted below, always last.
                    emit(
                        &sender,
                        SseEvent::Error {
                            code: ErrorCode::Timeout,
                            message: format!("job '{}' timed out after {:?}", job.op, timeout),
                            engine: None,
                            retry_after_ms: None,
                        },
                    )
                    .await;
                    telemetry("warn", &trace_id, job.op, elapsed_ms);
                    JobOutcome {
                        results: Vec::new(),
                        query: None,
                        content: Some(error_content(
                            ErrorCode::Timeout,
                            format!("job '{}' timed out after {:?}", job.op, timeout),
                        )),
                    }
                }
            };

        // The terminal event is ALWAYS `done`, emitted as the last event for
        // every job. On success `content` carries the real result (or `None`
        // for search ops, whose results ride in the `results` field); on
        // failure it carries the error envelope.
        let count = outcome.results.len();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let engine = winning_engine(&outcome.results);
        emit(
            &sender,
            SseEvent::Done {
                query: outcome.query,
                results: outcome.results,
                trace_id: trace_id.clone(),
                count,
                engine,
                duration_ms: elapsed_ms,
                sla_ms: timeout.as_millis() as u64,
                queries,
                content: outcome.content,
            },
        )
        .await;
        telemetry("info", &trace_id, job.op, elapsed_ms);
    }

    /// Execute `op` to its final envelope.
    ///
    /// Search ops run the dispatch plan and stream results; non-search ops
    /// (`extract`/`ax`/`pdf-url`/`pdf-file`) run their backend directly. The
    /// returned [`JobOutcome`] carries the terminal `done` content; `run`
    /// always emits the terminal `done` as the last event. Any `error` frame
    /// emitted here is informational (non-terminal) and precedes that `done`.
    async fn execute(
        &self,
        op: Op,
        args: &JobArgs,
        sender: &Option<mpsc::Sender<SseEvent>>,
    ) -> JobOutcome {
        if args.strategy == Strategy::NonSearch {
            let envelope = self.run_non_search(op, args).await;
            // On failure emit an informational `error` frame; the terminal
            // `done` (with the error envelope in content) is emitted by `run`.
            if let Some(error) = &envelope.error {
                emit(
                    sender,
                    SseEvent::Error {
                        code: error.code,
                        message: error.detail.clone(),
                        engine: None,
                        retry_after_ms: None,
                    },
                )
                .await;
            }
            let content = if envelope.is_err() {
                let url = match op {
                    Op::Extract | Op::Ax | Op::PdfUrl => args.url.as_deref(),
                    _ => None,
                };
                Some(error_content_from_envelope(&envelope, url))
            } else {
                envelope.data.clone()
            };
            return JobOutcome {
                results: Vec::new(),
                query: None,
                content,
            };
        }
        // The harvest pipeline runs the full search → dedup → rank → follow
        // flow through the warm CDP session, so it is handled before the
        // generic dispatch plan.
        if args.strategy == Strategy::Harvest {
            return self.run_harvest(op, args, sender).await;
        }
        let targets = dispatch_plan(args);
        let (results, failure) = match targets.len() {
            1 => match pump_stream(self.search_stream(&targets[0]), sender).await {
                Ok(results) => (results, None),
                Err(failure) => (Vec::new(), Some(failure)),
            },
            _ => self.fan_out(&targets, sender).await,
        };
        match failure {
            None => JobOutcome {
                results,
                query: done_query(args),
                content: None,
            },
            Some(failure) => {
                // Emit an informational `error` frame (non-terminal); the
                // terminal `done` is emitted by `run`, always last. When any
                // partial results were collected, `done` carries them in its
                // `results` field; otherwise `done.content` carries the error
                // envelope.
                let message = failure
                    .envelope
                    .error
                    .as_ref()
                    .map(|e| e.detail.clone())
                    .unwrap_or_else(|| "search failed".to_string());
                let code = failure
                    .envelope
                    .error
                    .as_ref()
                    .map(|e| e.code)
                    .unwrap_or(ErrorCode::EngineFailed);
                emit(
                    sender,
                    SseEvent::Error {
                        code,
                        message,
                        engine: failure.engine,
                        retry_after_ms: failure.retry_after_ms,
                    },
                )
                .await;
                if results.is_empty() {
                    let content = Some(error_content_from_envelope(&failure.envelope, None));
                    JobOutcome {
                        results: Vec::new(),
                        query: None,
                        content,
                    }
                } else {
                    JobOutcome {
                        results,
                        query: done_query(args),
                        content: None,
                    }
                }
            }
        }
    }

    /// Run the harvest pipeline: search → dedup → rank → follow through the
    /// warm CDP session, streaming each harvested result as an SSE `result`
    /// frame.
    async fn run_harvest(
        &self,
        _op: Op,
        args: &JobArgs,
        sender: &Option<mpsc::Sender<SseEvent>>,
    ) -> JobOutcome {
        let Some(connection) = &self.connection else {
            let envelope = failure_envelope(
                ErrorCode::ConnectionFailed,
                "harvest requires a warm CDP connection; none is available",
            );
            // Informational error frame; the terminal done (with the error
            // envelope in content) is emitted by `run`, always last.
            emit(
                sender,
                SseEvent::Error {
                    code: ErrorCode::ConnectionFailed,
                    message: "harvest requires a warm CDP connection; none is available".into(),
                    engine: None,
                    retry_after_ms: None,
                },
            )
            .await;
            let content = Some(error_content_from_envelope(&envelope, None));
            return JobOutcome {
                results: Vec::new(),
                query: None,
                content,
            };
        };
        let session = connection.session();
        let request = BatchHarvestRequest {
            queries: args.queries.clone(),
            rank_by: RankStrategy::Composite,
            follow_top_n: args.follow_top,
            extract_params: ExtractParams {
                offset: 0,
                max_chars: args.max_chars.unwrap_or(usize::MAX),
            },
            reputation: None,
            engine: pinned_engine(args.engine.to_engine_choice()),
        };
        match harvest(session, request).await {
            Ok((harvested, _summary)) => {
                let results: Vec<SearchResult> =
                    harvested.iter().map(|h| h.search_result.clone()).collect();
                for h in &harvested {
                    emit(sender, SseEvent::Result(h.search_result.clone())).await;
                }
                JobOutcome {
                    results,
                    query: None,
                    content: None,
                }
            }
            Err(error) => {
                let envelope =
                    failure_envelope(cdp_error_code(&error), format!("harvest: {error}"));
                // Informational error frame; the terminal done (with the error
                // envelope in content) is emitted by `run`, always last.
                emit(
                    sender,
                    SseEvent::Error {
                        code: cdp_error_code(&error),
                        message: format!("harvest: {error}"),
                        engine: None,
                        retry_after_ms: None,
                    },
                )
                .await;
                let content = Some(error_content_from_envelope(&envelope, None));
                JobOutcome {
                    results: Vec::new(),
                    query: None,
                    content,
                }
            }
        }
    }

    /// Execute a non-search op against the extraction/CDP backends.
    async fn run_non_search(&self, op: Op, args: &JobArgs) -> Envelope {
        match op {
            Op::Extract => {
                let url = args
                    .url
                    .clone()
                    .expect("validated extract args always carry a url");
                let params = ExtractParams {
                    offset: 0,
                    max_chars: args.max_chars.unwrap_or(usize::MAX),
                };
                match AutoExtractor::new(&self.http)
                    .extract(url.clone(), params)
                    .await
                {
                    Ok(article) => {
                        Envelope::ok(serde_json::json!({ "url": url, "article": article }))
                    }
                    Err(error) => failure_envelope(
                        extraction_error_code(&error),
                        format!("extract '{url}': {error}"),
                    ),
                }
            }
            Op::Ax => {
                let url = args
                    .url
                    .clone()
                    .expect("validated ax args always carry a url");
                let Some(connection) = &self.connection else {
                    return failure_envelope(
                        ErrorCode::ConnectionFailed,
                        "ax requires a warm CDP connection; none is available",
                    );
                };
                let session = connection.session();
                match ax_tree(&session, &url, None).await {
                    Ok(tree) => ax_envelope(&url, &tree),
                    Err(error) => {
                        failure_envelope(cdp_error_code(&error), format!("ax '{url}': {error}"))
                    }
                }
            }
            Op::PdfUrl => {
                let url = args
                    .url
                    .clone()
                    .expect("validated pdf-url args always carry a url");
                match self.pdf_from_url(&url).await {
                    Ok(article) => {
                        Envelope::ok(serde_json::json!({ "url": url, "article": article }))
                    }
                    Err(error) => failure_envelope(
                        extraction_error_code(&error),
                        format!("pdf-url '{url}': {error}"),
                    ),
                }
            }
            Op::PdfFile => {
                let path = args
                    .path
                    .clone()
                    .expect("validated pdf-file args always carry a path");
                match self.pdf_from_file(&path).await {
                    Ok(article) => {
                        Envelope::ok(serde_json::json!({ "path": path, "article": article }))
                    }
                    Err(error) => failure_envelope(
                        extraction_error_code(&error),
                        format!("pdf-file '{path}': {error}"),
                    ),
                }
            }
            _ => unreachable!("search ops are dispatched by dispatch_plan"),
        }
    }

    /// Fetch a PDF over HTTP and extract its text via [`PdfExtractor`].
    async fn pdf_from_url(&self, url: &str) -> Result<Article, ExtractionError> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| ExtractionError::Http(format!("pdf fetch: {error}")))?;
        if !response.status().is_success() {
            return Err(ExtractionError::Http(format!(
                "pdf fetch returned HTTP {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| ExtractionError::Http(format!("pdf read: {error}")))?
            .to_vec();
        let params = ExtractParams {
            offset: 0,
            max_chars: usize::MAX,
        };
        PdfExtractor.extract_article(url, &bytes, &params)
    }

    /// Read a local PDF and extract its text via [`PdfExtractor`].
    async fn pdf_from_file(&self, path: &str) -> Result<Article, ExtractionError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| ExtractionError::Http(format!("pdf read '{path}': {error}")))?;
        let params = ExtractParams {
            offset: 0,
            max_chars: usize::MAX,
        };
        PdfExtractor.extract_article(path, &bytes, &params)
    }

    /// One stream per query, run concurrently and merged. The first failing
    /// stream wins: its failure (envelope + engine/retry metadata) becomes the
    /// job's outcome. Results collected from the sibling streams before the
    /// failure are returned alongside it so the caller can surface partial
    /// results. No terminal `error` frame is emitted here — sibling streams may
    /// still be producing `result` frames, so the caller emits the terminal
    /// frame only after every stream has joined.
    async fn fan_out(
        &self,
        targets: &[DispatchTarget],
        sender: &Option<mpsc::Sender<SseEvent>>,
    ) -> (Vec<SearchResult>, Option<StreamFailure>) {
        let mut set = JoinSet::new();
        for target in targets {
            let sender = sender.clone();
            let rx = self.search_stream(target);
            set.spawn(async move { pump_stream(rx, &sender).await });
        }
        let mut results = Vec::new();
        let mut failure = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(mut rs)) => {
                    results.append(&mut rs);
                }
                Ok(Err(f)) => {
                    failure.get_or_insert(f);
                }
                Err(_) => {}
            }
        }
        (results, failure)
    }

    /// Produce the event stream for one target.
    fn search_stream(&self, target: &DispatchTarget) -> mpsc::Receiver<SearchEvent> {
        let options = SearchOptions {
            freshness: target.freshness.clone(),
            search_depth: target.search_depth.clone(),
        };
        search_streaming(
            Arc::clone(&self.router),
            target.query.clone(),
            target.count,
            target.choice,
            &options,
        )
    }
}

/// Outcome of executing one job: the data echoed in the terminal `done` event.
/// `run` always emits the terminal `done` as the last event for every job.
struct JobOutcome {
    /// Results collected for the job, carried by the terminal `done` event so
    /// a client can assemble the full result set from it.
    results: Vec<SearchResult>,
    /// The single-query echo carried by the terminal `done` event
    /// (`simple` jobs only).
    query: Option<String>,
    /// The terminal `done` event's `content` object: the raw backend payload
    /// for non-search ops, or the error envelope `{"error": {code, detail}}`
    /// on failure. `None` for successful search ops, whose results ride in the
    /// `results` field.
    content: Option<serde_json::Value>,
}

/// A terminal search-stream failure: the failure envelope plus the engine and
/// retry metadata needed to build the terminal `error` SSE frame.
///
/// `pump_stream`/`fan_out` return this instead of a bare [`Envelope`] so the
/// caller (`execute`) can emit the terminal `error` frame — after every
/// sub-stream has joined — while preserving the engine/retry attribution that
/// the inner `SearchEngineError` carried.
#[derive(Debug)]
struct StreamFailure {
    /// The failure envelope carrying the engine/retry attribution.
    envelope: Envelope,
    /// The engine implicated by the failure, when a single engine is at fault.
    engine: Option<SearchEngine>,
    /// Milliseconds to wait before retrying, when the backend supplied a
    /// `Retry-After` header; `None` when it did not.
    retry_after_ms: Option<u64>,
}

/// The query echo for a job's terminal `done` event: `simple` jobs echo their
/// single query, while multi-query (`parallel`/`harvest`) and non-search ops
/// carry none.
#[must_use]
fn done_query(args: &JobArgs) -> Option<String> {
    match args.strategy {
        Strategy::Simple => args.query.clone(),
        _ => None,
    }
}

/// Every query that contributed to a job, from its dispatch plan. Populates
/// the terminal `done` event's `queries` field for per-query attribution.
#[must_use]
fn target_queries(args: &JobArgs) -> Vec<String> {
    dispatch_plan(args)
        .into_iter()
        .map(|target| target.query)
        .collect()
}

/// The engine that served a job's results, when a single engine answered
/// every result; `None` when results span engines or the set is empty.
#[must_use]
fn winning_engine(results: &[SearchResult]) -> Option<SearchEngine> {
    let mut engines = results.iter().map(|r| r.engine);
    let first = engines.next()?;
    engines.all(|e| e == first).then_some(first)
}

/// Resolve an [`EngineChoice`] to the pinned engine, if any, for the harvest
/// pipeline's `engine` field (`Auto` → `None`).
#[must_use]
fn pinned_engine(choice: EngineChoice) -> Option<SearchEngine> {
    match choice {
        EngineChoice::Auto => None,
        EngineChoice::Pin(engine) => Some(engine),
    }
}

/// Emit a structured stderr telemetry line for a job lifecycle event.
fn telemetry(level: &str, trace_id: &Option<String>, op: Op, elapsed_ms: u64) {
    let _ = StderrEvent::new(
        level,
        trace_id.clone().unwrap_or_default(),
        serde_json::json!({ "op": op.to_string(), "elapsed_ms": elapsed_ms }),
    )
    .emit();
}

/// Publish `event` to `sender`, if any, with backpressure.
async fn emit(sender: &Option<mpsc::Sender<SseEvent>>, event: SseEvent) {
    if let Some(tx) = sender {
        let _ = tx.send(event).await;
    }
}

/// Forward one search stream's progress events to `sender` while collecting
/// its results. The inner stream's `JobStarted`/`Done` events are suppressed —
/// the worker emits one terminal pair for the whole job. An inner `Error` is
/// NOT forwarded here: it is returned as a [`StreamFailure`] so the caller can
/// emit the terminal `error` frame after every sub-stream has joined.
async fn pump_stream(
    mut rx: mpsc::Receiver<SearchEvent>,
    sender: &Option<mpsc::Sender<SseEvent>>,
) -> Result<Vec<SearchResult>, StreamFailure> {
    let mut results = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            SearchEvent::Result(result) => {
                if let Some(tx) = sender {
                    let _ = tx.send(SseEvent::Result((*result).clone())).await;
                }
                results.push(*result);
            }
            SearchEvent::EngineEvent { engine, kind } => {
                if let Some(tx) = sender {
                    let _ = tx.send(project_event(engine, kind)).await;
                }
            }
            SearchEvent::JobStarted | SearchEvent::Done => {}
            SearchEvent::Error(error) => {
                let envelope =
                    failure_envelope(search_error_code(&error), format!("search failed: {error}"));
                return Err(StreamFailure {
                    envelope,
                    engine: error_engine(&error),
                    retry_after_ms: error_retry_after_ms(&error),
                });
            }
        }
    }
    Ok(results)
}

/// Build the success envelope for an `ax` op — or an error envelope when the
/// compressed tree is empty. A settled capture can still compress to nothing
/// (e.g. a blank page, or an unnamed `RootWebArea` whose children all dropped),
/// and the client must see a proper failure instead of `ok` with `tree:""` and
/// `total_nodes:0`. Mirrors the CLI guard in `cli/commands/ax.rs`.
#[must_use]
fn ax_envelope(url: &str, tree: &AxTreeResult) -> Envelope {
    if tree.tree.is_empty() || tree.total_nodes == 0 {
        return failure_envelope(
            ErrorCode::ExtractFailed,
            format!(
                "ax '{url}': AX_TREE_EMPTY: AX tree is empty; page produced no accessible content"
            ),
        );
    }
    Envelope::ok(serde_json::json!({
        "url": url,
        "tree": tree.tree,
        "total_nodes": tree.total_nodes,
        "truncated": tree.truncated,
    }))
}

/// Build an error envelope from a canonical code and detail message.
#[must_use]
fn failure_envelope(code: ErrorCode, detail: impl Into<String>) -> Envelope {
    Envelope::error(ErrorBody::new(code, detail))
}

/// The terminal `done` `content` object for a failure: an error envelope
/// `{"error": {"code": ..., "detail": ...}}` carrying a canonical [`ErrorCode`].
#[must_use]
fn error_content(code: ErrorCode, detail: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "detail": detail.into() } })
}

/// The terminal `done` `content` object derived from a failure [`Envelope`].
///
/// When `url` is `Some`, the requested URL is echoed alongside the error so
/// the client can correlate a failed job with the URL it was asked to process.
#[must_use]
fn error_content_from_envelope(envelope: &Envelope, url: Option<&str>) -> serde_json::Value {
    let mut value = match &envelope.error {
        Some(body) => error_content(body.code, body.detail.clone()),
        None => error_content(ErrorCode::EngineFailed, "unknown failure"),
    };
    if let Some(url) = url {
        value["url"] = serde_json::Value::String(url.to_string());
    }
    value
}

/// Canonical [`ErrorCode`] for an [`ExtractionError`].
#[must_use]
fn extraction_error_code(error: &ExtractionError) -> ErrorCode {
    match error {
        ExtractionError::RateLimited { .. } => ErrorCode::RateLimited,
        ExtractionError::Timeout(_) => ErrorCode::Timeout,
        ExtractionError::Http(_)
        | ExtractionError::Parse(_)
        | ExtractionError::Unsupported(_)
        | ExtractionError::Empty(_)
        | ExtractionError::BotBlocked(_) => ErrorCode::ExtractFailed,
    }
}

/// Canonical [`ErrorCode`] for a [`CdpError`].
#[must_use]
fn cdp_error_code(error: &CdpError) -> ErrorCode {
    match error {
        CdpError::BrowserNotFound { .. } => ErrorCode::BrowserNotFound,
        CdpError::ConnectionFailed { .. } => ErrorCode::ConnectionFailed,
        CdpError::CaptchaBlocked { .. } => ErrorCode::Captcha,
        CdpError::NavigationTimeout { .. } => ErrorCode::Timeout,
        CdpError::UnsupportedUrl { .. } => ErrorCode::InvalidInput,
        CdpError::CdpCallFailed { .. }
        | CdpError::Json(_)
        | CdpError::Ws(_)
        | CdpError::Io(_)
        | CdpError::Http(_) => ErrorCode::ExtractFailed,
    }
}

/// Canonical [`ErrorCode`] for a [`SearchEngineError`] — the code paired with
/// the failure envelope when an inner search stream fails.
#[must_use]
pub(crate) fn search_error_code(error: &SearchEngineError) -> ErrorCode {
    match error {
        SearchEngineError::RateLimited { .. } => ErrorCode::RateLimited,
        SearchEngineError::Captcha { .. } => ErrorCode::Captcha,
        SearchEngineError::QuotaExceeded { .. } => ErrorCode::QuotaExceeded,
        SearchEngineError::Network { .. }
        | SearchEngineError::Parse { .. }
        | SearchEngineError::Unavailable { .. }
        | SearchEngineError::AllEnginesFailed(_) => ErrorCode::EngineFailed,
    }
}

/// Adapt `worker` to [`JobQueue::spawn_worker`]'s handler shape.
///
/// The returned closure owns a clone of `worker`, so it is `'static`.
pub(crate) fn worker_handler(
    worker: Arc<JobWorker>,
) -> impl Fn(QueuedJob) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync + 'static
{
    move |job| {
        let worker = Arc::clone(&worker);
        Box::pin(async move { worker.run(job).await })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gthings_common::provenance::Provenance;
    use gthings_common::taxonomy::ErrorCode;
    use gthings_search::{
        EngineChoice, EngineEventKind, EngineMode, SearchEngine, SearchEngineError, SearchEvent,
        SearchResult,
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::{
        JobRegistry, JobWorker, ax_envelope, cdp_error_code, dispatch_plan, extraction_error_code,
        needs_connection, pump_stream, search_error_code,
    };
    use crate::jobs::args::JobArgs;
    use crate::jobs::{Op, QueuedJob};
    use crate::sse::SseEvent;

    fn parse(op: Op, args: serde_json::Value) -> JobArgs {
        JobArgs::parse(op, &args).unwrap()
    }

    fn fake_result(title: &str) -> SearchResult {
        SearchResult {
            title: title.to_string(),
            url: format!("https://{title}.example"),
            snippet: "snippet".to_string(),
            position: 1,
            provenance: Provenance::default(),
            domain_authority: 0.8,
            source_type: "web".to_string(),
            engine: SearchEngine::Brave,
            score: 0.0,
            published_date: None,
            favicon: None,
            mode: EngineMode::Hybrid,
        }
    }

    fn job(op: Op, args: serde_json::Value) -> QueuedJob {
        QueuedJob {
            op,
            args: JobArgs::parse(op, &args).unwrap(),
            timeout_ms: None,
            trace_id: Some("t1".to_string()),
        }
    }

    /// Build a worker sharing a fresh router/registry and no warm CDP
    /// connection (enough for the offline non-search and harvest paths).
    fn worker() -> (Arc<JobWorker>, Arc<JobRegistry>) {
        let router = Arc::new(gthings_search::engine::router::SearchRouter::new(None));
        let registry = Arc::new(JobRegistry::new());
        (
            Arc::new(JobWorker::new(router, Arc::clone(&registry), None)),
            registry,
        )
    }

    #[test]
    fn dispatch_plan_maps_simple_to_one_stream() {
        let args = parse(
            Op::Simple,
            json!({"query": "rust", "count": 3, "engine": "brave"}),
        );
        let targets = dispatch_plan(&args);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].query, "rust");
        assert_eq!(targets[0].count, 3);
        assert_eq!(targets[0].choice, EngineChoice::Pin(SearchEngine::Brave));
    }

    #[test]
    fn dispatch_plan_fans_out_parallel_and_harvest() {
        for op in [Op::Parallel, Op::Harvest] {
            let args = parse(op, json!({"queries": ["a", "b", "c"]}));
            let targets = dispatch_plan(&args);
            assert_eq!(targets.len(), 3);
            let queries: Vec<&str> = targets.iter().map(|t| t.query.as_str()).collect();
            assert_eq!(queries, ["a", "b", "c"]);
            assert!(
                targets
                    .iter()
                    .all(|t| t.count == 5 && t.choice == EngineChoice::Auto)
            );
        }
    }

    #[test]
    fn needs_connection_for_google_engine_and_ax() {
        assert!(needs_connection(
            Op::Simple,
            &parse(Op::Simple, json!({"query": "x", "engine": "google"}))
        ));
        assert!(!needs_connection(
            Op::Simple,
            &parse(Op::Simple, json!({"query": "x"}))
        ));
        assert!(!needs_connection(
            Op::Simple,
            &parse(Op::Simple, json!({"query": "x", "engine": "brave"}))
        ));
        assert!(needs_connection(
            Op::Ax,
            &parse(Op::Ax, json!({"url": "https://example.com"}))
        ));
        assert!(!needs_connection(
            Op::Extract,
            &parse(Op::Extract, json!({"url": "https://example.com"}))
        ));
        assert!(!needs_connection(
            Op::PdfUrl,
            &parse(Op::PdfUrl, json!({"url": "https://example.com/doc.pdf"}))
        ));
        assert!(!needs_connection(
            Op::PdfFile,
            &parse(Op::PdfFile, json!({"path": "/tmp/a.pdf"}))
        ));
    }

    #[test]
    fn extraction_error_maps_to_canonical_codes() {
        use gthings_extraction::ExtractionError;

        assert_eq!(
            extraction_error_code(&ExtractionError::Http("boom".into())),
            ErrorCode::ExtractFailed
        );
        assert_eq!(
            extraction_error_code(&ExtractionError::Parse("boom".into())),
            ErrorCode::ExtractFailed
        );
        assert_eq!(
            extraction_error_code(&ExtractionError::Empty("none".into())),
            ErrorCode::ExtractFailed
        );
        assert_eq!(
            extraction_error_code(&ExtractionError::RateLimited {
                detail: "slow".into(),
                retry_after: Some(3),
            }),
            ErrorCode::RateLimited
        );
        assert_eq!(
            extraction_error_code(&ExtractionError::Timeout("slow".into())),
            ErrorCode::Timeout
        );
    }

    #[test]
    fn cdp_error_maps_to_canonical_codes() {
        use gthings_cdp::CdpError;

        assert_eq!(
            cdp_error_code(&CdpError::BrowserNotFound { port: 9222 }),
            ErrorCode::BrowserNotFound
        );
        assert_eq!(
            cdp_error_code(&CdpError::ConnectionFailed {
                detail: "down".into()
            }),
            ErrorCode::ConnectionFailed
        );
        assert_eq!(
            cdp_error_code(&CdpError::CaptchaBlocked {
                detail: "sorry".into()
            }),
            ErrorCode::Captcha
        );
        assert_eq!(
            cdp_error_code(&CdpError::NavigationTimeout {
                url: "https://x".into(),
                timeout: 25
            }),
            ErrorCode::Timeout
        );
        assert_eq!(
            cdp_error_code(&CdpError::CdpCallFailed {
                method: "Accessibility.getFullAXTree".into(),
                detail: "AX_TREE_EMPTY".into(),
            }),
            ErrorCode::ExtractFailed
        );
    }

    #[test]
    fn search_error_maps_to_canonical_codes() {
        assert_eq!(
            search_error_code(&SearchEngineError::RateLimited {
                engine: SearchEngine::Brave,
                detail: "429".into(),
                retry_after_ms: None,
            }),
            ErrorCode::RateLimited
        );
        assert_eq!(
            search_error_code(&SearchEngineError::Captcha {
                engine: SearchEngine::Google,
                detail: "x".into(),
            }),
            ErrorCode::Captcha
        );
        assert_eq!(
            search_error_code(&SearchEngineError::QuotaExceeded {
                engine: SearchEngine::Bing,
                detail: "x".into(),
            }),
            ErrorCode::QuotaExceeded
        );
        assert_eq!(
            search_error_code(&SearchEngineError::Network {
                engine: SearchEngine::Brave,
                detail: "x".into(),
            }),
            ErrorCode::EngineFailed
        );
        assert_eq!(
            search_error_code(&SearchEngineError::AllEnginesFailed(vec![])),
            ErrorCode::EngineFailed
        );
    }

    #[test]
    fn ax_envelope_empty_tree_returns_error_not_ok() {
        use gthings_cdp::AxTreeResult;

        // A settled capture that compresses to nothing (blank page, or an
        // unnamed RootWebArea with no surviving children) must surface as a
        // failure — never ok-with-empty-tree.
        let empty = AxTreeResult {
            tree: String::new(),
            total_nodes: 0,
            truncated: false,
        };
        let envelope = ax_envelope("https://example.com", &empty);
        assert_eq!(envelope.status, "error");
        let body = envelope.error.expect("error body present");
        assert_eq!(body.code, ErrorCode::ExtractFailed);
        assert!(body.detail.contains("AX_TREE_EMPTY"));
    }

    #[test]
    fn ax_envelope_rejects_total_nodes_zero_even_with_text() {
        use gthings_cdp::AxTreeResult;

        // Compression can never produce text with zero counted nodes — treat
        // any such inconsistent result as empty too.
        let inconsistent = AxTreeResult {
            tree: "[1] RootWebArea \"Example\"".into(),
            total_nodes: 0,
            truncated: false,
        };
        let envelope = ax_envelope("https://example.com", &inconsistent);
        assert_eq!(envelope.status, "error");
        assert_eq!(
            envelope.error.map(|body| body.code),
            Some(ErrorCode::ExtractFailed)
        );
    }

    #[test]
    fn ax_envelope_populated_tree_is_ok() {
        use gthings_cdp::AxTreeResult;

        let tree = AxTreeResult {
            tree: "[1] RootWebArea \"Example\"\n  [2] button \"Click\"".into(),
            total_nodes: 2,
            truncated: false,
        };
        let envelope = ax_envelope("https://example.com", &tree);
        assert!(envelope.is_ok());
        assert_eq!(
            envelope.data,
            Some(json!({
                "url": "https://example.com",
                "tree": "[1] RootWebArea \"Example\"\n  [2] button \"Click\"",
                "total_nodes": 2,
                "truncated": false,
            }))
        );
    }

    #[tokio::test]
    async fn pump_stream_forwards_frames_and_collects_results() {
        let (tx, rx) = mpsc::channel::<SearchEvent>(8);
        let (sse_tx, mut sse_rx) = mpsc::channel::<SseEvent>(8);

        tx.send(SearchEvent::Result(Box::new(fake_result("Hit A"))))
            .await
            .unwrap();
        tx.send(SearchEvent::EngineEvent {
            engine: SearchEngine::Bing,
            kind: EngineEventKind::Captcha,
        })
        .await
        .unwrap();
        tx.send(SearchEvent::Done).await.unwrap();
        drop(tx);

        let results = pump_stream(rx, &Some(sse_tx))
            .await
            .expect("stream completes");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Hit A");
        assert!(matches!(sse_rx.recv().await, Some(SseEvent::Result(_))));
        assert!(matches!(
            sse_rx.recv().await,
            Some(SseEvent::EngineEvent { .. })
        ));
        assert!(sse_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn pump_stream_maps_inner_error_to_stream_failure() {
        let (tx, rx) = mpsc::channel::<SearchEvent>(8);
        tx.send(SearchEvent::Error(SearchEngineError::RateLimited {
            engine: SearchEngine::Brave,
            detail: "429".into(),
            retry_after_ms: Some(2500),
        }))
        .await
        .unwrap();
        drop(tx);

        let failure = pump_stream(rx, &None)
            .await
            .expect_err("inner error fails the stream");
        assert_eq!(
            failure.envelope.error.as_ref().map(|e| e.code),
            Some(ErrorCode::RateLimited)
        );
        assert_eq!(failure.engine, Some(SearchEngine::Brave));
        assert_eq!(failure.retry_after_ms, Some(2500));
    }

    #[tokio::test]
    async fn ax_without_connection_done_carries_connection_failed_and_url() {
        let (worker, registry) = worker();
        let (sse_tx, mut sse_rx) = mpsc::channel::<SseEvent>(8);
        registry.register("t1", sse_tx).await;

        worker
            .run(job(Op::Ax, json!({"url": "https://example.com"})))
            .await;

        assert!(matches!(
            sse_rx.recv().await,
            Some(SseEvent::JobStarted { .. })
        ));
        assert!(matches!(
            sse_rx.recv().await,
            Some(SseEvent::Error {
                code: ErrorCode::ConnectionFailed,
                ..
            })
        ));
        match sse_rx.recv().await {
            Some(SseEvent::Done { content, .. }) => {
                let content = content.expect("done carries an error envelope on failure");
                assert_eq!(content["error"]["code"], "connection-failed");
                assert_eq!(content["url"], "https://example.com");
            }
            other => panic!("expected terminal done with error envelope, got {other:?}"),
        }
        registry.unregister("t1").await;
        assert!(
            sse_rx.recv().await.is_none(),
            "exactly one terminal event per job"
        );
    }

    #[tokio::test]
    async fn harvest_without_connection_emits_terminal_error() {
        let (worker, registry) = worker();
        let (sse_tx, mut sse_rx) = mpsc::channel::<SseEvent>(8);
        registry.register("t1", sse_tx).await;

        worker
            .run(job(Op::Harvest, json!({"queries": ["a", "b"]})))
            .await;

        assert!(matches!(
            sse_rx.recv().await,
            Some(SseEvent::JobStarted { .. })
        ));
        assert!(matches!(
            sse_rx.recv().await,
            Some(SseEvent::Error {
                code: ErrorCode::ConnectionFailed,
                ..
            })
        ));
        match sse_rx.recv().await {
            Some(SseEvent::Done { content, .. }) => {
                let content = content.expect("done carries an error envelope on failure");
                assert_eq!(content["error"]["code"], "connection-failed");
                assert!(content["error"]["detail"].as_str().is_some());
            }
            other => panic!("expected terminal done with error envelope, got {other:?}"),
        }
        registry.unregister("t1").await;
        assert!(
            sse_rx.recv().await.is_none(),
            "exactly one terminal event per job"
        );
    }

    #[tokio::test]
    async fn registry_rejects_duplicate_trace_id() {
        let registry = JobRegistry::new();
        let (tx1, _rx1) = mpsc::channel::<SseEvent>(4);
        let (tx2, _rx2) = mpsc::channel::<SseEvent>(4);
        assert!(registry.register("t1", tx1).await);
        assert!(
            !registry.register("t1", tx2).await,
            "a duplicate trace_id must be rejected"
        );
        assert!(
            registry.sender("t1").await.is_some(),
            "the original sender is preserved"
        );
    }

    #[tokio::test]
    async fn registry_round_trips_senders() {
        let registry = JobRegistry::new();
        let (tx, _rx) = mpsc::channel::<SseEvent>(4);
        assert!(registry.sender("t1").await.is_none());
        assert!(registry.register("t1", tx).await);
        assert!(registry.sender("t1").await.is_some());
        registry.unregister("t1").await;
        assert!(registry.sender("t1").await.is_none());
    }
}
