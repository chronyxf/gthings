// Integration tests for gthings library crates.
// Tests cover internal logic: cache, extraction, quality, types.
//
// Run: cargo test --test integration

#[path = "integration/tests.rs"]
mod tests;

mod common;
