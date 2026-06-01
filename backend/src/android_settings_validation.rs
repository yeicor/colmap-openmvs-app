//! Android settings validation and repair system.
//!
//! This module detects when app reinstalls have left invalid paths in settings
//! and automatically repairs them by resetting to appropriate defaults.

/// Validates and repairs Android settings paths.
pub struct AndroidSettingsValidation;

impl AndroidSettingsValidation {
    /// Check if the configured paths are accessible and valid on Android.
    ///
    /// Returns `true` if invalid paths were detected.
    pub async fn detect_invalid_paths() -> anyhow::Result<bool> {
        #[cfg(not(target_os = "android"))]
        {
            // On non-Android platforms, paths are always valid
            Ok(false)
        }

        #[cfg(target_os = "android")]
        {
            use tracing::{debug, warn};
            let settings = crate::settings::get_settings()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;

            let binary_dir = std::path::Path::new(&settings.proot_binary_dir);
            let images_dir = std::path::Path::new(&settings.proot_images_dir);

            // Check if paths are accessible
            let binary_accessible = binary_dir.exists();
            let images_accessible = images_dir.exists();

            debug!(
                binary_dir = %settings.proot_binary_dir,
                binary_accessible,
                images_dir = %settings.proot_images_dir,
                images_accessible,
                "Android settings validation: path accessibility check"
            );

            if !binary_accessible || !images_accessible {
                warn!(
                    binary_accessible,
                    images_accessible, "Android settings validation: detected invalid paths"
                );
                return Ok(true);
            }

            Ok(false)
        }
    }

    /// Repair invalid settings by resetting paths to defaults.
    ///
    /// Returns `Ok(true)` if changes were made, `Ok(false)` if no changes needed.
    pub async fn repair_paths() -> anyhow::Result<bool> {
        #[cfg(not(target_os = "android"))]
        {
            // On non-Android platforms, nothing to repair
            Ok(false)
        }

        #[cfg(target_os = "android")]
        {
            use tracing::{debug, info};

            // Check if repair is needed
            let needs_repair = Self::detect_invalid_paths()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to detect invalid paths: {}", e))?;

            if !needs_repair {
                debug!("Android settings validation: no repairs needed");
                return Ok(false);
            }

            info!("Android settings validation: repairing invalid paths");

            // Load current settings
            let mut settings = crate::settings::get_settings()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;

            // Reset paths to defaults
            let old_binary_dir = settings.proot_binary_dir.clone();
            let old_images_dir = settings.proot_images_dir.clone();

            settings.proot_binary_dir = crate::settings::default_proot_binary_dir();
            settings.proot_images_dir = crate::settings::default_proot_images_dir();

            info!(
                old_binary_dir = %old_binary_dir,
                new_binary_dir = %settings.proot_binary_dir,
                old_images_dir = %old_images_dir,
                new_images_dir = %settings.proot_images_dir,
                "Android settings validation: reset paths to defaults"
            );

            // Persist the repaired settings
            crate::settings::update_settings(settings.clone())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to update settings: {}", e))?;

            info!("Android settings validation: settings persisted after repair");

            // Re-trigger Android runtime setup with the new paths
            info!("Android settings validation: re-triggering runtime setup");
            crate::android_startup::setup_android_runtime()
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to setup Android runtime after repair: {}", e)
                })?;

            info!("Android settings validation: repair completed successfully");
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    #[cfg(target_os = "android")]
    async fn test_detect_invalid_paths() {
        // This test would require a real Android environment
        // For now, it's a placeholder
        let _ = AndroidSettingsValidation::detect_invalid_paths().await;
    }
}
