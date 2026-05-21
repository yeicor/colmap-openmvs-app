//! Structured logging setup for the application.
//!
//! This module provides a centralized logging configuration that works across
//! the entire application. It uses the `tracing` crate with `tracing-subscriber`
//! for structured, hierarchical logging.
//!
//! ## Usage
//!
//! Initialize logging once at application startup:
//! ```no_run
//! colmap_openmvs_app::logging::init();
//! ```
//!
//! Then use the logging macros throughout the codebase:
//! ```no_run
//! use tracing::{info, warn, error, debug, trace, span, Level};
//!
//! info!("Application started");
//! warn!("This is a warning");
//! error!("An error occurred: {}", err);
//! debug!("Debug information");
//! trace!("Detailed trace");
//! ```
//!
//! ## Log Levels
//!
//! The logging level can be controlled via the `RUST_LOG` environment variable:
//! ```sh
//! RUST_LOG=debug cargo run
//! RUST_LOG=colmap_openmvs_app=debug,colmap_openmvs_backend=info cargo run
//! ```

use tracing_subscriber::prelude::*;

/// Initialize structured logging for the application.
///
/// This should be called once at application startup, typically in `main()`.
/// It sets up:
/// - Formatted output to stdout with timestamps
/// - Color support on terminals that support it
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_init() {
        // This should not panic
        init();
    }
}
