pub mod config;
pub mod reputation;
pub use reputation as domain_reputation;
pub mod envelope;
pub mod pagination;
pub mod provenance;
pub mod taxonomy;
pub mod telemetry;
pub mod url_normalizer;
pub mod user_agent;
pub mod util;

pub use config::Config;
pub use domain_reputation::{DomainReputation, QualityFlag, quality_flag_is_blocking};
pub use envelope::{Envelope, ErrorBody};
pub use pagination::{ExtractParams, Pagination};
pub use provenance::Provenance;
pub use taxonomy::ErrorCode;
pub use telemetry::{StderrEvent, trace_id};
pub use url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain,
};
pub use user_agent::{ENV_AGENT, gthings_agent};

pub use util::str::strip_suffix_and_trim;
pub use util::time::unix_now_ms;
pub use util::url::extract_host;
