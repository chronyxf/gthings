// End-to-end tests for gthings agent workflows.
// These tests simulate the full 3-agent finance research scenario.
// Requires GTHINGS_TEST_DAEMON=1 and the daemon to be running.
//
// Run: GTHINGS_TEST_DAEMON=1 cargo test --test e2e -- --ignored

#[path = "e2e/tests.rs"]
mod tests;

mod common;
