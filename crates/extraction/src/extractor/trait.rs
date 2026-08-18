use gthings_common::pagination::ExtractParams;

use crate::article::{Article, ExtractionError};

/// Every extractor implements this trait.
/// Input varies by source, output is always Article.
/// `params` controls offset/max_chars slicing of the extracted text.
#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    type Input: Send + 'static;
    async fn extract(
        &self,
        input: Self::Input,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError>;
}
