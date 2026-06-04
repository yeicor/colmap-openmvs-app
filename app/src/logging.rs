/// Initializes the logger for the application.
///
/// - On native targets (not wasm32), registers a `tracing_subscriber` with
///   the `RUST_LOG` filter (defaulting to `info` if not already set).
/// - On wasm32 (WebAssembly), determines the log level from the URL query
///   parameters or hash fragment (`log` or `level`), or falls back to
///   `DEBUG` in debug builds and `INFO` otherwise.
///
/// **Important**: must be called before `dioxus::launch` because Dioxus does
/// not register a subscriber — it only sets up a logger bridge.  If you
/// `tracing::info!` in `main()` before `dioxus::launch` without a subscriber,
/// the message is silently dropped.
pub fn init() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use tracing_subscriber::EnvFilter;

        // Default filter if none is set by the user / environment.
        let filter = if std::env::var("RUST_LOG").is_ok() {
            EnvFilter::from_default_env()
        } else {
            if cfg!(debug_assertions) {
                EnvFilter::new("info,colmap_openmvs_app=debug")
            } else {
                EnvFilter::new("info")
            }
        };

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .pretty()
            .init();
    }

    #[cfg(target_arch = "wasm32")]
    {
        use tracing::Level;
        // Default log level: DEBUG in debug builds, INFO otherwise
        let level = if cfg!(debug_assertions) {
            Level::DEBUG
        } else {
            Level::INFO
        };
        // TODO: Override with URL parameters for easier debugging in production builds.
        // TODO: Override per logger, like the RUST_LOG env var on native.
        dioxus::logger::init(level).expect("Failed to initialize logger");
    }
}
