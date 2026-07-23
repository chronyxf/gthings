pub mod browser;
pub mod connection;
pub mod tab;
pub mod error;

pub use browser::Browser;
pub use connection::Connection;
pub use tab::Tab;
pub use error::{CdpError, Result};
