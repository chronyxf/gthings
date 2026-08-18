//! Job model for the serve daemon.
//!
//! [`Op`] enumerates the accepted operations (search ops plus the non-search
//! ops `extract`, `ax`, `pdf-url`, `pdf-file`); [`Job`] is the wire shape of
//! a `POST /job` payload. Per-op argument parsing and validation lives in
//! [`args`].

pub(crate) mod args;

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default timeout for a `simple` search.
pub(crate) const TIMEOUT_SIMPLE_SECS: u64 = 10;
/// Default timeout for a `parallel` search.
pub(crate) const TIMEOUT_PARALLEL_SECS: u64 = 20;
/// Default timeout for a `harvest` run.
pub(crate) const TIMEOUT_HARVEST_SECS: u64 = 45;
/// Default timeout for an `extract` run.
pub(crate) const TIMEOUT_EXTRACT_SECS: u64 = 30;
/// Default timeout for an `ax` run.
pub(crate) const TIMEOUT_AX_SECS: u64 = 30;
/// Default timeout for a `pdf-url` run.
pub(crate) const TIMEOUT_PDF_URL_SECS: u64 = 30;
/// Default timeout for a `pdf-file` run.
pub(crate) const TIMEOUT_PDF_FILE_SECS: u64 = 15;
/// Hard cap applied to every job timeout, regardless of the request.
pub(crate) const TIMEOUT_HARD_CAP_SECS: u64 = 120;

/// Operations accepted by the daemon.
///
/// Serialized lowercase: `simple`, `parallel`, `harvest`, `extract`, `ax`,
/// `pdf-url`, `pdf-file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Op {
    /// Single-query search (the streaming core, [`gthings_search::search_streaming`]).
    #[serde(alias = "search")]
    Simple,
    /// Multi-query search, one entry per query.
    Parallel,
    /// Full research pipeline: search → dedup → rank → follow → quality.
    Harvest,
    /// Readable-content extraction from a page URL.
    Extract,
    /// Accessibility-tree extraction from a page URL.
    Ax,
    /// Fetch a PDF from a URL and extract its text.
    #[serde(rename = "pdf-url")]
    PdfUrl,
    /// Extract text from a local PDF file.
    #[serde(rename = "pdf-file")]
    PdfFile,
}

impl Op {
    /// Default timeout for this operation (the timeout ladder).
    #[must_use]
    pub(crate) fn default_timeout(self) -> Duration {
        let secs = match self {
            Self::Simple => TIMEOUT_SIMPLE_SECS,
            Self::Parallel => TIMEOUT_PARALLEL_SECS,
            Self::Harvest => TIMEOUT_HARVEST_SECS,
            Self::Extract => TIMEOUT_EXTRACT_SECS,
            Self::Ax => TIMEOUT_AX_SECS,
            Self::PdfUrl => TIMEOUT_PDF_URL_SECS,
            Self::PdfFile => TIMEOUT_PDF_FILE_SECS,
        };
        Duration::from_secs(secs)
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Simple => "simple",
            Self::Parallel => "parallel",
            Self::Harvest => "harvest",
            Self::Extract => "extract",
            Self::Ax => "ax",
            Self::PdfUrl => "pdf-url",
            Self::PdfFile => "pdf-file",
        })
    }
}

/// A job submitted to the daemon via `POST /job`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Job {
    /// Which operation to run.
    pub op: Op,
    /// Raw per-op arguments, validated by [`args::JobArgs::parse`].
    #[serde(default)]
    pub args: serde_json::Value,
    /// Caller-requested timeout in milliseconds; `None` uses the op ladder.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Correlation id for the job; `None` falls back to a generated UUID.
    #[serde(default)]
    pub trace_id: Option<String>,
}

/// Resolve an effective timeout budget shared by [`QueuedJob`].
#[must_use]
fn resolve_timeout(op: Op, timeout_ms: Option<u64>) -> Duration {
    let requested = timeout_ms.filter(|ms| *ms > 0).map(Duration::from_millis);
    requested
        .unwrap_or_else(|| op.default_timeout())
        .min(Duration::from_secs(TIMEOUT_HARD_CAP_SECS))
}

/// A job ready for execution: the wire [`Job`] plus its validated, typed args.
///
/// `POST /job` parses and validates [`Job::args`] exactly once
/// ([`args::JobArgs::parse`]) and enqueues a [`QueuedJob`] in its place; the
/// worker trusts the pre-validated args and never re-parses the raw JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedJob {
    /// Which operation to run.
    pub op: Op,
    /// Validated per-op arguments (never raw wire JSON).
    pub args: args::JobArgs,
    /// Caller-requested timeout in milliseconds; `None` uses the op ladder.
    pub timeout_ms: Option<u64>,
    /// Correlation id for the job.
    pub trace_id: Option<String>,
}

impl QueuedJob {
    /// Resolve the effective timeout budget (the per-op ladder, capped).
    #[must_use]
    pub(crate) fn timeout(&self) -> Duration {
        resolve_timeout(self.op, self.timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{
        Job, Op, QueuedJob, TIMEOUT_HARD_CAP_SECS, TIMEOUT_PARALLEL_SECS, args, resolve_timeout,
    };

    fn queued(op: Op, timeout_ms: Option<u64>) -> QueuedJob {
        let raw = match op {
            Op::Simple => json!({"query": "x"}),
            Op::Parallel | Op::Harvest => json!({"queries": ["x"]}),
            Op::Extract | Op::Ax | Op::PdfUrl => json!({"url": "https://example.com"}),
            Op::PdfFile => json!({"path": "/tmp/a.pdf"}),
        };
        QueuedJob {
            op,
            args: args::JobArgs::parse(op, &raw).unwrap(),
            timeout_ms,
            trace_id: None,
        }
    }

    #[test]
    fn ladder_defaults_when_timeout_absent() {
        assert_eq!(resolve_timeout(Op::Simple, None), Duration::from_secs(10));
        assert_eq!(
            resolve_timeout(Op::Parallel, None),
            Duration::from_secs(TIMEOUT_PARALLEL_SECS)
        );
        assert_eq!(resolve_timeout(Op::Harvest, None), Duration::from_secs(45));
    }

    #[test]
    fn explicit_timeout_is_respected() {
        assert_eq!(
            resolve_timeout(Op::Harvest, Some(5_000)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn timeout_is_hard_capped() {
        let capped = resolve_timeout(Op::Simple, Some(10_000_000));
        assert_eq!(capped, Duration::from_secs(TIMEOUT_HARD_CAP_SECS));
    }

    #[test]
    fn queued_job_shares_the_timeout_ladder() {
        assert_eq!(queued(Op::Simple, None).timeout(), Duration::from_secs(10));
        assert_eq!(
            queued(Op::Parallel, Some(5_000)).timeout(),
            Duration::from_secs(5)
        );
        assert_eq!(
            queued(Op::PdfFile, Some(10_000_000)).timeout(),
            Duration::from_secs(TIMEOUT_HARD_CAP_SECS)
        );
    }

    #[test]
    fn zero_timeout_falls_back_to_ladder() {
        assert_eq!(
            resolve_timeout(Op::Harvest, Some(0)),
            Duration::from_secs(45)
        );
    }

    #[test]
    fn ladder_covers_non_search_ops() {
        assert_eq!(resolve_timeout(Op::Extract, None), Duration::from_secs(30));
        assert_eq!(resolve_timeout(Op::Ax, None), Duration::from_secs(30));
        assert_eq!(resolve_timeout(Op::PdfUrl, None), Duration::from_secs(30));
        assert_eq!(resolve_timeout(Op::PdfFile, None), Duration::from_secs(15));
    }

    #[test]
    fn non_search_ops_are_still_hard_capped() {
        let capped = resolve_timeout(Op::PdfFile, Some(10_000_000));
        assert_eq!(capped, Duration::from_secs(TIMEOUT_HARD_CAP_SECS));
    }

    #[test]
    fn deserializes_pdf_op_wire_names() {
        let job: Job = serde_json::from_value(json!({"op": "pdf-url"})).unwrap();
        assert_eq!(job.op, Op::PdfUrl);
        let job: Job = serde_json::from_value(json!({"op": "pdf-file"})).unwrap();
        assert_eq!(job.op, Op::PdfFile);
        let job: Job = serde_json::from_value(json!({"op": "extract"})).unwrap();
        assert_eq!(job.op, Op::Extract);
    }

    #[test]
    fn deserializes_wire_shape() {
        let payload = json!({
            "op": "simple",
            "args": {"query": "rust async"},
            "timeout_ms": 8000,
            "trace_id": "abc-123"
        });
        let job: Job = serde_json::from_value(payload).unwrap();
        assert_eq!(job.op, Op::Simple);
        assert_eq!(job.args["query"], "rust async");
        assert_eq!(job.timeout_ms, Some(8000));
        assert_eq!(job.trace_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn missing_fields_default() {
        let job: Job = serde_json::from_value(json!({"op": "parallel"})).unwrap();
        assert_eq!(job.args, serde_json::Value::Null);
        assert_eq!(job.timeout_ms, None);
        assert_eq!(job.trace_id, None);
    }
}
