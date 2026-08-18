//! Serve daemon composition root.
//!
//! Owns the job model ([`jobs`]: `simple`/`parallel`/`harvest` search plus the
//! `extract`/`ax`/`pdf-url`/`pdf-file` ops), the in-process machinery
//! ([`core`]: bounded job queue, job workers, and drain-on-SIGTERM shutdown),
//! the SSE event projection ([`sse`]), and the HTTP API layer ([`api`]:
//! `POST /job` + `GET /healthz` + `GET /metrics` on `:9080`).
//!
//! [`run`] is the composition root: it locates the browser, opens the warm
//! CDP pool ([`gthings_cdp::SharedConnection`]), builds the search router and
//! job worker, and starts the axum server bound to `config.serve_bind`. The
//! returned [`ServeHandle`] keeps the daemon alive until
//! [`ServeHandle::shutdown`] drains it after a termination signal.

pub(crate) mod api;
pub(crate) mod core;
pub(crate) mod jobs;
pub(crate) mod sse;

use std::sync::Arc;

use gthings_cdp::SharedConnection;
use gthings_common::config::Config;
use gthings_search::engine::router::SearchRouter;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::api::{AppState, cdp_port, router};
use crate::core::queue::{DEFAULT_MAX_CONCURRENT, DEFAULT_QUEUE_CAPACITY, JobQueue};
use crate::core::shutdown::{DRAIN_TIMEOUT, Shutdown, drain_on_signal};
use crate::core::workers::{JobRegistry, JobWorker, worker_handler};

/// Handle returned by [`run`], kept by the embedding process for the daemon
/// lifetime. Call [`ServeHandle::shutdown`] to wait for a termination signal,
/// drain in-flight jobs, close every live tab, and stop the HTTP listener.
#[derive(Debug)]
pub struct ServeHandle {
    /// Resolved daemon configuration.
    pub config: Config,
    shutdown: Shutdown,
    state: Option<AppState>,
    worker: Option<JoinHandle<()>>,
    server: Option<JoinHandle<std::io::Result<()>>>,
    connection: Option<Arc<SharedConnection>>,
}

impl ServeHandle {
    /// Gracefully shut the daemon down and return the exit code.
    ///
    /// [`drain_on_signal`] waits for SIGTERM/SIGINT, flips the accepting flag
    /// (so `POST /job` answers 503), and closes every live browser tab. In-
    /// flight jobs get their drain window here: after the signal arrives the
    /// server task is aborted (dropping its `AppState` and releasing the queue
    /// send half), so the worker loop ends once the buffer drains.
    pub async fn shutdown(mut self) -> i32 {
        let code = drain_on_signal(&self.shutdown, self.connection.clone()).await;

        // Stop accepting new requests now that the signal has arrived. Aborting
        // the server task drops its `AppState`, releasing the queue send half.
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }

        // Drop the daemon state, then give in-flight jobs their drain window.
        drop(self.state.take());
        if let Some(worker) = self.worker.take() {
            if tokio::time::timeout(DRAIN_TIMEOUT, worker).await.is_err() {
                tracing::warn!("in-flight jobs did not finish within the drain timeout");
            }
        }

        gthings_common::telemetry::StderrEvent::new(
            "info",
            String::new(),
            serde_json::json!({"event": "drain-complete", "code": code}),
        )
        .emit()
        .ok();

        code
    }
}

/// Composition root: start the CDP pool, job worker, and axum HTTP server.
///
/// Binds `config.serve_bind` (default `127.0.0.1:9080`). The returned
/// [`ServeHandle`] keeps the daemon running until [`ServeHandle::shutdown`].
#[must_use]
pub async fn run(config: Config) -> ServeHandle {
    let port = cdp_port();

    // Locate the browser (metadata feeds `/healthz`), then open one warm
    // session. A missing browser is not fatal: the daemon serves without it.
    let (connection, browser, browser_status, browser_reason) = match gthings_cdp::detect(port)
        .await
    {
        Ok(detected) => {
            let ws_url = detected.ws_url.clone();
            match SharedConnection::connect_ws(&ws_url).await {
                Ok(conn) => {
                    tracing::info!(port, %ws_url, "warm CDP session established");
                    (
                        Some(conn),
                        Some(detected),
                        Some("connected".to_string()),
                        None,
                    )
                }
                Err(error) => {
                    tracing::warn!(error = %error, "warm CDP session failed; serving without browser");
                    (
                        None,
                        Some(detected),
                        Some("detected-no-session".to_string()),
                        Some(error.to_string()),
                    )
                }
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, port, "no browser detected; serving without browser");
            (
                None,
                None,
                Some("not-detected".to_string()),
                Some(error.to_string()),
            )
        }
    };

    // The search router shares the warm session (Google backend only when a
    // browser was found) and the process-wide pacing store.
    let search_router = Arc::new(SearchRouter::new(
        connection.as_ref().map(|conn| conn.session()),
    ));

    let registry = Arc::new(JobRegistry::new());
    let (queue, rx) = JobQueue::new(DEFAULT_QUEUE_CAPACITY, DEFAULT_MAX_CONCURRENT);
    let queue = Arc::new(queue);

    let worker = Arc::new(JobWorker::new(
        search_router,
        Arc::clone(&registry),
        connection.clone(),
    ));
    let worker_handle = queue.spawn_worker(rx, worker_handler(worker));

    let shutdown = Shutdown::new();
    let state = AppState {
        queue: Arc::clone(&queue),
        registry: Arc::clone(&registry),
        browser,
        browser_status,
        browser_reason,
        shutdown: shutdown.clone(),
    };
    let app = router(state.clone());

    let listener = TcpListener::bind(&config.serve_bind)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {}: {error}", config.serve_bind));
    tracing::info!(bind = %config.serve_bind, "HTTP API listening");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    ServeHandle {
        config,
        shutdown,
        state: Some(state),
        worker: Some(worker_handle),
        server: Some(server),
        connection,
    }
}
