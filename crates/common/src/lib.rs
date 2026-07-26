use std::io;
use std::path::Path;

pub mod cache;
pub mod domain_reputation;
pub mod error;
pub mod pagination;
pub mod provenance;
pub mod url_normalizer;

pub use cache::Sha256DiskCache;
pub use domain_reputation::{DomainRecord, DomainReputation, QualityFlag};
pub use error::GthingsError;
pub use pagination::{ExtractParams, Pagination};
pub use provenance::Provenance;
pub use url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain, strip_all_fragments,
    strip_google_fragment, strip_tracking_params,
};

// ---------------------------------------------------------------------------
// Agent identification — compile-time, driven by Cargo.toml version
// ---------------------------------------------------------------------------

/// Agent string sent in provenance records.
///
/// Resolved at compile time via `env!("CARGO_PKG_VERSION")`, eliminating manual
/// version bumps.
pub const GTHINGS_AGENT: &str = concat!("gthings/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// File expiry — checks file mtime against a TTL in seconds
// ---------------------------------------------------------------------------

/// Returns `true` if `path`'s mtime is older than `ttl_secs`.
///
/// Missing files are treated as expired. Files whose metadata is inaccessible
/// are conservatively treated as **not** expired (never delete what we cannot
/// verify).
pub fn is_file_expired(path: &Path, ttl_secs: u64) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    return elapsed.as_secs() >= ttl_secs;
                }
            }
            false
        }
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// Atomic file write — temp-file + rename
// ---------------------------------------------------------------------------

/// Atomically write `data` to `path` via a temporary sibling file and rename.
///
/// The temporary file is created as `{path}.tmp.{PID}`. If the rename succeeds
/// the write is complete; if the process crashes between write and rename the
/// temp file is harmless garbage left behind.
pub fn atomic_write(path: &Path, data: &str) -> io::Result<()> {
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Host extraction — parse a URL and extract its host string
// ---------------------------------------------------------------------------

/// Extract the host portion of a URL string.
///
/// Returns `None` when the URL cannot be parsed or has no host (e.g.
/// `mailto:` links).
pub fn extract_host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(String::from)
}
