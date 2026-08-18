//! Per-command timeout configuration (built-in defaults + `GTHINGS_<CMD>_TIMEOUT`
//! env override) and a generic timeout wrapper for command futures.

use std::time::Duration;

/// Single source of truth for per-command timeout defaults, keyed by the same
/// command name used for the `GTHINGS_<CMD>_TIMEOUT` env override. Keeping the
/// name and duration in one table means the two cannot drift.
pub(crate) const COMMAND_TIMEOUTS: &[(&str, Duration)] = &[
    // Google single queries can legitimately reach ~22s, and brave pin-mode can
    // wait up to 30s of pacing before the search even starts, so a 30s cap
    // produced spurious "timed out" failures on healthy runs.
    ("search", Duration::from_secs(60)),
    ("parallel_search", Duration::from_secs(60)),
    ("harvest", Duration::from_secs(120)),
    ("status", Duration::from_secs(10)),
    ("health", Duration::from_secs(10)),
    ("update", Duration::from_secs(60)),
    ("extract", Duration::from_secs(30)),
    ("ax", Duration::from_secs(30)),
    ("pdf_url", Duration::from_secs(30)),
    ("pdf_file", Duration::from_secs(15)),
];

/// Look up the default timeout for a command name.
pub(crate) fn command_timeout(name: &str) -> Duration {
    COMMAND_TIMEOUTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| *d)
        .unwrap_or(Duration::from_secs(30))
}

/// Run a command future under a per-command timeout, returning the future's
/// exit code directly. On timeout, prints an error and returns the
/// conventional exit code `2` (the command never ran to completion).
///
/// Generic over any future that yields an exit code (`Future<Output = i32>`),
/// matching every `commands::cmd_*` handler in the direct-dispatch layer.
pub(crate) async fn run_with_timeout<F: std::future::Future<Output = i32>>(
    name: &str,
    secs: u64,
    fut: F,
) -> i32 {
    if let Ok(code) = tokio::time::timeout(Duration::from_secs(secs), fut).await {
        code
    } else {
        eprintln!("gthings: {name} timed out after {secs}s");
        2
    }
}
