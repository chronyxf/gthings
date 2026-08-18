pub mod article;
pub mod dispatch;
pub mod extractor;
pub mod jsonld;
pub mod pdf;
pub mod quality;

pub use article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, Section, SourceInfo,
};
pub use dispatch::AutoExtractor;
pub use extractor::{Extractor, SourceType, domain_authority};
pub use pdf::PdfExtractor;
pub use quality::{ContentQuality, NAV_TOKENS, is_nav_dense};
pub use web::WebExtractor;

pub mod web;
