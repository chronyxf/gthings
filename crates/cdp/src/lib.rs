pub mod browser;
pub mod connection;
pub mod error;
pub mod session;
pub mod tab;

pub use browser::{connect, detect, dismiss_allow_debugging_dialog, DetectedBrowser};
pub use connection::{CdpEvent, Connection};
pub use error::{CdpError, Result};
pub use session::Session;
pub use tab::Tab;
