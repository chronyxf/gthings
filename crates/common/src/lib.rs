use std::path::Path;
use std::sync::Arc;

pub mod domain_reputation;
pub mod error;
pub mod pagination;
pub mod provenance;
pub mod url_normalizer;

pub use domain_reputation::{DomainRecord, DomainReputation, QualityFlag};
pub use error::GthingsError;
pub use pagination::{ExtractParams, Pagination};
pub use provenance::Provenance;
pub use url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain, strip_all_fragments,
    strip_google_fragment, strip_tracking_params,
};

/// The `about:blank` URL constant, used for empty/inert page navigation.
pub const ABOUT_BLANK: &str = "about:blank";

/// Agent string sent in provenance records.
///
/// Resolved at compile time via `env!("CARGO_PKG_VERSION")`, eliminating manual
/// version bumps.
pub const GTHINGS_AGENT: &str = concat!("gthings/", env!("CARGO_PKG_VERSION"));

/// Returns `true` if `path`'s mtime is older than `ttl_secs`.
///
/// Missing files are treated as expired. Files whose metadata is inaccessible
/// are conservatively treated as **not** expired (never delete what we cannot
/// verify).
#[must_use]
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

/// Atomically write `data` to `path` via a temporary sibling file and rename.
///
/// The temporary file is created as `{path}.tmp.{PID}`. If the rename succeeds
/// the write is complete; if the process crashes between write and rename the
/// temp file is harmless garbage left behind.
///
/// # Errors
///
/// Returns [`std::io::Error`] if the temporary file cannot be written or
/// the rename fails.
pub fn atomic_write(path: &Path, data: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Extract the host portion of a URL string.
///
/// Returns `None` when the URL cannot be parsed or has no host (e.g.
/// `mailto:` links).
pub fn extract_host(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(String::from)
}

/// Safely truncate a suffix from the end of a string, respecting UTF-8 character boundaries.
///
/// Returns the remainder of the string (trimmed) if the suffix is present,
/// or the original string unchanged if the suffix is not found.
///
/// Unlike byte-level slicing (`s[..s.len()-n]`), this avoids panicking on
/// multi-byte characters such as non-breaking space (U+00A0), CJK, or emoji.
#[must_use]
pub fn safe_truncate_end(s: &str, suffix: &str) -> String {
    s.strip_suffix(suffix)
        .map_or_else(|| s.to_string(), |trimmed| trimmed.trim().to_string())
}

/// Attempt to unwrap an `Arc<T>`, returning the inner value if the ref-count is one.
///
/// This is a safe wrapper around [`Arc::try_unwrap`] that returns `None`
/// instead of panicking when there are other references.
///
/// # Example
/// ```
/// use std::sync::Arc;
/// use gthings_common::disconnect_arc;
/// let arc = Arc::new(vec![1, 2, 3]);
/// assert_eq!(disconnect_arc(arc), Some(vec![1, 2, 3]));
/// ```
pub fn disconnect_arc<T>(arc: Arc<T>) -> Option<T> {
    Arc::try_unwrap(arc).ok()
}

/// Determine whether a quality flag indicates a hard (blocking) failure.
///
/// Returns `true` for flags that should stop extraction immediately:
/// `BotWall`, `Captcha`, or `Paywall`.
///
/// **Note:** This logically belongs in the `gthings-extraction` crate to avoid
/// a circular dependency (`gthings-extraction` already depends on
/// `gthings-common`). It is defined here as a free function so that callers in
/// other crates can use it without reaching into extraction internals. The
/// extraction crate may re-export or override this as needed.
#[must_use]
pub const fn quality_flag_is_blocking(flag: &QualityFlag) -> bool {
    matches!(
        flag,
        QualityFlag::BotWall | QualityFlag::Captcha | QualityFlag::Paywall
    )
}
