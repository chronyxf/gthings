pub mod html;
pub mod pdf;
/// Content extraction and quality validation crate.
///
/// Provides pure functions for:
/// - Content quality validation (bot detection, paywall, CAPTCHA, etc.)
/// - PDF text extraction (pure Rust, FlateDecode decompression)
/// - HTML content extraction (CSS selector-based, section detection)
pub mod quality;

pub use html::{ExtractedContent, HtmlExtractor, Section};
pub use pdf::PdfExtractor;
pub use quality::{ContentQuality, QualityResult, SecondaryResult};
