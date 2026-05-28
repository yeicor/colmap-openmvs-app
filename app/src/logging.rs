/// Initializes the logger for the application.
///
/// - On native targets (not wasm32), sets the `RUST_LOG` environment variable to a default value if not already set.
/// - On wasm32 (WebAssembly), determines the log level from the URL query parameters or hash fragment (`log` or `level`),
///   or falls back to `DEBUG` in debug builds and `INFO` otherwise.
pub fn init() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Set a default RUST_LOG if not already set
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "info,colmap_openmvs_backend=trace");
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        use std::str::FromStr;
        use tracing::Level;
        // Default log level: DEBUG in debug builds, INFO otherwise
        let mut level = if cfg!(debug_assertions) {
            Level::DEBUG
        } else {
            Level::INFO
        };
        // TODO: Override with URL parameters for easier debugging in production builds.
        // TODO: Override per logger, like the RUST_LOG env var on native.
        dioxus::logger::init(level).expect("Failed to initialize logger");
    }
}
