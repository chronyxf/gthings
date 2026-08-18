use serde::{Deserialize, Serialize};

/// Parameters controlling extraction offset and maximum length.
///
/// Passed to every `Extractor::extract()` call. Default values
/// (`offset = 0`, `max_chars = usize::MAX`) mean "extract everything".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractParams {
    pub offset: usize,
    /// Maximum number of characters to extract.
    ///
    /// `usize::MAX` is the sentinel for "no limit" (extract everything) — see
    /// [`Default`]. [`build_pagination`] treats it as unbounded via saturating
    /// arithmetic, so `offset + usize::MAX` never overflows.
    pub max_chars: usize,
}

impl Default for ExtractParams {
    fn default() -> Self {
        Self {
            offset: 0,
            max_chars: usize::MAX,
        }
    }
}

/// Pagination state returned inside every `Article`.
///
/// Tells the consumer whether the content was truncated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    pub truncated: bool,
}

/// Build a [`Pagination`] from extraction parameters and content lengths.
///
/// Computes `truncated`: `true` when `offset + max_chars` falls short of
/// `total_len`.
pub fn build_pagination(params: &ExtractParams, total_len: usize) -> Pagination {
    let truncated =
        params.offset.saturating_add(params.max_chars) < total_len && params.max_chars > 0;
    Pagination { truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_pagination_truncated() {
        // offset=0, max_chars=100, total_len=1000 → truncated=true
        let params = ExtractParams {
            offset: 0,
            max_chars: 100,
        };
        let result = build_pagination(&params, 1000);
        assert!(result.truncated);
    }

    #[test]
    fn test_build_pagination_not_truncated() {
        // offset=0, max_chars=1000, total_len=100 (content shorter than max) → truncated=false
        let params = ExtractParams {
            offset: 0,
            max_chars: 1000,
        };
        let result = build_pagination(&params, 100);
        assert!(!result.truncated);
    }

    #[test]
    fn test_build_pagination_offset_boundary() {
        // offset + max_chars saturating (not overflowing) — test with usize::MAX
        let params = ExtractParams {
            offset: usize::MAX - 10,
            max_chars: 20,
        };
        let result = build_pagination(&params, usize::MAX);
        // Should not panic; truncated should be false since we got everything
        assert!(!result.truncated);
    }
}
