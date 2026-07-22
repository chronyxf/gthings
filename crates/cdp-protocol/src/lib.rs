#![allow(non_snake_case, non_camel_case_types, dead_code, unused_imports)]

/// CDP Protocol types — auto-generated from browser_protocol.json + js_protocol.json.
///
/// Each CDP domain (`Page`, `Runtime`, `Target`, etc.) becomes a Rust module.
/// Commands get `{Name}Params` / `{Name}Return` structs; events get `{Name}Params` structs.
///
/// Top-level enums:
/// - [`CdpMethod`] — one variant per command, with `Display` giving the `"Domain.method"` string.
/// - [`CdpEvent`] — serde-tagged enum over all CDP events.
/// - [`Command`] trait — implemented by each `*Params` struct, tying it to its `*Return` type.
///
include!(concat!(env!("OUT_DIR"), "/cdp.rs"));
