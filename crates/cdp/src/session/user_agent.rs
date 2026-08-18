use gthings_common::user_agent::ENV_AGENT;

use crate::env_or;

/// Fallback desktop Chrome user-agent used when `GTHINGS_AGENT` is unset.
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// Resolve the user-agent from a `GTHINGS_AGENT` value, falling back to
/// the built-in default when unset or empty.
pub(crate) fn user_agent_from(env_value: Option<&str>) -> String {
    env_or(env_value, DEFAULT_USER_AGENT)
}

/// Resolve the user-agent to report to the browser: the `GTHINGS_AGENT`
/// environment variable when set (non-empty), otherwise the built-in default.
pub(crate) fn user_agent() -> String {
    user_agent_from(std::env::var(ENV_AGENT).ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_agent_default_and_override() {
        // Override: env var wins.
        assert_eq!(
            user_agent_from(Some("CustomUA/1.0 (Test)")),
            "CustomUA/1.0 (Test)"
        );

        // Empty string → fall back to default.
        assert_eq!(user_agent_from(Some("")), DEFAULT_USER_AGENT);

        // Unset → fall back to default.
        assert_eq!(user_agent_from(None), DEFAULT_USER_AGENT);
    }
}
