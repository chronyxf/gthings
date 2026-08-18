pub mod ax_tree;
pub use ax_tree::AxTreeResult;
pub mod browser;
pub mod connection;
pub mod discovery;
pub mod error;
pub mod pool;
pub mod session;
pub mod tab;

pub use browser::{DetectedBrowser, detect};
pub use connection::{CdpEvent, Connection};
pub use discovery::{check_alive, rewrite_ws_host};
pub use error::{CdpError, Result};
pub use pool::SharedConnection;
pub use session::Session;
pub use tab::{Tab, TabGuard};

/// The `about:blank` URL constant used for background tab creation.
///
/// Canonical copy for CDP; `gthings_common` keeps an identical constant for
/// other crates.
pub const ABOUT_BLANK: &str = "about:blank";

/// Resolve an environment value to a string, falling back to `default` when
/// the variable is unset or empty. Shared by the CDP host and user-agent
/// resolvers.
pub(crate) fn env_or(env_value: Option<&str>, default: &str) -> String {
    match env_value {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => default.to_string(),
    }
}
