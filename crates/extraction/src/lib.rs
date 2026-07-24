pub mod html;
pub mod pdf;
/// Content extraction and quality validation crate.
///
/// Pure functions for content quality, PDF text extraction, and HTML extraction.
pub mod quality;

pub use html::{ExtractedContent, HtmlExtractor, Section};
pub use pdf::PdfExtractor;
pub use quality::{ContentQuality, QualityReason, QualityResult, SecondaryResult};
