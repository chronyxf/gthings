pub mod browser;
pub mod connection;
pub mod error;
pub mod tab;

pub use browser::Browser;
pub use connection::Connection;
pub use error::{CdpError, Result};
pub use tab::Tab;
