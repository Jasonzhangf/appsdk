//! Codex App Server transport over the host-owned Unix WebSocket endpoint.
//!
//! This module speaks the native JSON-RPC surface exposed by Codex TUI and
//! Desktop. It does not start an App Server, invent a namespace, or treat
//! turn acceptance as execution. Explicit coordination and background wakeups
//! use `turn/start` or `turn/steer`. Every operation is
//! bounded and preserves the exact native error on failure.

include!("codex_app_server_production_part1.rs");
include!("codex_app_server_production_part2.rs");

#[cfg(test)]
#[path = "codex_app_server_tests.rs"]
mod tests;
