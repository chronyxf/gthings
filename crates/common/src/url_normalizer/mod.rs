// ---------------------------------------------------------------------------
// URL canonicalization and normalisation
// ---------------------------------------------------------------------------

mod canonicalize;
mod classify;
mod strip;
mod tracking;

use url::Url;

pub use canonicalize::{canonicalize_url, dedup_key};
pub use classify::{is_arxiv_url, is_pdf_url, registered_domain};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse a URL string, returning `None` for unparseable inputs.
///
/// Delegates to the shared [`crate::util::url::parse_url`] helper.
fn try_parse_url(url: &str) -> Option<Url> {
    crate::util::url::parse_url(url)
}
