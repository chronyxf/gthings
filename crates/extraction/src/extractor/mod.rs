//! Source extraction entry points.
//!
//! - [`trait`](self) — the [`Extractor`] trait every extractor implements.
//! - [`source`](self) — [`SourceType`] detection from a URL.
//! - [`authority`](self) — domain authority scoring for AI trust assessment.

mod authority;
mod source;
mod r#trait;

pub use authority::{compute_domain_authority, domain_authority};
pub use source::SourceType;
pub use r#trait::Extractor;
