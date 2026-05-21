//! Structured logging setup for the backend service.
//!
//! This module provides centralized logging configuration for the backend.
//! It uses the `tracing` crate with `tracing-subscriber` for structured logging.

use tracing_subscriber::prelude::*;

/// Initialize structured logging for the backend.
///
/// This should be called once at server startup.
/// It sets up:
/// - Formatted output to stdout with timestamps
/// - Color support on terminals
/// - Environment variable filtering (RUST_LOG)
/// - Hierarchical span support
pub fn init() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Default to info level if RUST_LOG is not set
        tracing_subscriber::EnvFilter::new("info")
    });

    let fmt_layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_line_number(true)
        .with_file(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();
}
