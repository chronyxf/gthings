//! CLI subcommand handlers.

mod ax;
mod batch;
mod config;
mod extract;
mod harvest;
mod health;
mod pdf;
mod search;
mod serve;
mod status;
mod update;

pub(crate) use ax::*;
pub(crate) use batch::*;
pub(crate) use config::*;
pub(crate) use extract::*;
pub(crate) use harvest::*;
pub(crate) use health::*;
pub(crate) use pdf::*;
pub(crate) use search::*;
pub(crate) use serve::*;
pub(crate) use status::*;
pub(crate) use update::*;
