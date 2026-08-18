//! `gthings config` — print the resolved environment configuration.
//!
//! PROPOSAL §9: prints the resolved env+defaults (the same [`Config`] the
//! CLI and serve daemon run with) as the standard `{status, data, error}`
//! envelope so the Go backend can validate its boot-time assumptions against
//! a single parse path.

use gthings_common::config::Config;

use crate::util::{UniversalFlags, emit_success};

/// Print the resolved configuration as an envelope and exit `0`.
pub(crate) fn cmd_config(flags: &UniversalFlags) -> i32 {
    let config = Config::load();
    let value = serde_json::json!({
        "cdp_host": config.cdp_host,
        "user_agent": config.user_agent,
        "update_disabled": config.update_disabled,
        "reputation_dir": config.reputation_dir.map(|dir| dir.display().to_string()),
        "reputation_ttl_secs": config.reputation_ttl_secs,
        "serve_bind": config.serve_bind,
        "command_timeouts": config.command_timeouts,
    });
    emit_success(flags, value);
    0
}
