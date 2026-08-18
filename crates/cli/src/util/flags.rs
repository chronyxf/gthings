//! Universal CLI flags shared across all gthings subcommands.

/// Default CDP debugging port when `--cdp-port` / `GTHINGS_CDP_PORT` is unset.
pub(crate) const DEFAULT_CDP_PORT: u16 = 9222;
/// Default timeout (seconds) for CDP calls and extraction when `--timeout` is unset.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Output format for command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable text output (default).
    Text,
    /// Pretty-printed JSON.
    Json,
    /// Compact JSON lines (one JSON value per line).
    NdJson,
}

/// Universal flags shared across all gthings subcommands.
///
/// Every field is `global = true`: the struct is flattened once at the top
/// level, and clap propagates these args to every subcommand, so they do not
/// need to be re-declared (or merged) per subcommand.
#[derive(Debug, clap::Args)]
pub(crate) struct UniversalFlags {
    /// Output format. Overridden by --json for backward compatibility.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,

    /// JMESPath-like query filter (dot notation, e.g. '.title' or '.[].url'). Applied after the output envelope is built.
    #[arg(long, global = true, value_name = "QUERY")]
    pub query: Option<String>,

    /// Override CDP port (default: 9222, or GTHINGS_CDP_PORT env var).
    #[arg(long, global = true, value_name = "PORT", env = "GTHINGS_CDP_PORT")]
    pub cdp_port: Option<u16>,

    /// Override CDP WebSocket URL (takes priority over port detection).
    #[arg(long, global = true, value_name = "URL")]
    pub cdp_url: Option<String>,

    /// Timeout in seconds for CDP calls and extraction (default: 30). Connection setup may take longer.
    #[arg(long, global = true, value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Increase verbosity (can be repeated: -v -v for debug, -v -v -v for trace).
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error output.
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Backward-compatible alias for --output json.
    #[arg(long, global = true)]
    pub json: bool,
}

impl UniversalFlags {
    /// Resolve the effective output format, honoring the --json backward-compat alias.
    pub(crate) fn resolved_output(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output
        }
    }

    /// Effective CDP port, defaulting to [`DEFAULT_CDP_PORT`] when unset.
    pub(crate) fn effective_cdp_port(&self) -> u16 {
        self.cdp_port.unwrap_or(DEFAULT_CDP_PORT)
    }

    /// Effective timeout in seconds, defaulting to [`DEFAULT_TIMEOUT_SECS`] when unset.
    pub(crate) fn effective_timeout(&self) -> u64 {
        self.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    /// Determine the tracing log level based on verbosity, quiet flag, and output format.
    ///
    /// - `--quiet` or JSON output → only ERROR
    /// - NdJson output → WARN (reduce noise for streaming)
    /// - default (no flags) → INFO
    /// - `-v` → DEBUG
    /// - `-vv` (or more) → TRACE
    pub(crate) fn tracing_level(&self) -> &str {
        if self.quiet || self.resolved_output() == OutputFormat::Json {
            "error"
        } else {
            let base = if self.resolved_output() == OutputFormat::NdJson {
                "warn"
            } else {
                "info"
            };
            match self.verbose {
                0 => base,
                1 => "debug",
                _ => "trace",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_output_json_flag() {
        let flags = UniversalFlags {
            output: OutputFormat::Text,
            query: None,
            cdp_port: None,
            cdp_url: None,
            timeout: None,
            verbose: 0,
            quiet: false,
            json: true,
        };
        assert_eq!(flags.resolved_output(), OutputFormat::Json);
    }

    #[test]
    fn test_resolved_output_explicit() {
        let flags = UniversalFlags {
            output: OutputFormat::NdJson,
            query: None,
            cdp_port: None,
            cdp_url: None,
            timeout: None,
            verbose: 0,
            quiet: false,
            json: false,
        };
        assert_eq!(flags.resolved_output(), OutputFormat::NdJson);
    }

    #[test]
    fn test_effective_defaults_when_unset() {
        let flags = UniversalFlags {
            output: OutputFormat::Text,
            query: None,
            cdp_port: None,
            cdp_url: None,
            timeout: None,
            verbose: 0,
            quiet: false,
            json: false,
        };
        assert_eq!(flags.effective_cdp_port(), 9222);
        assert_eq!(flags.effective_timeout(), 30);
    }
}
