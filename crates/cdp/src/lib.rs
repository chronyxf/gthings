pub mod ax_tree;
pub use ax_tree::AxTreeResult;
pub mod browser;
pub mod connection;
pub mod error;
pub mod session;
pub mod tab;

pub use browser::{DetectedBrowser, connect, detect, dismiss_allow_debugging_dialog};
pub use connection::{CdpEvent, Connection};
pub use error::{CdpError, Result};
pub use session::Session;
pub use tab::Tab;

/// The `about:blank` URL constant used for background tab creation.
pub const ABOUT_BLANK: &str = "about:blank";
