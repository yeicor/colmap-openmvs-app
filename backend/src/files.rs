use anyhow::anyhow;
use dioxus::Result as DioxusResult;
use rfd::AsyncFileDialog;
use tracing::debug;

pub async fn init() -> DioxusResult<()> {
    Ok(())
}

/// Open a native file-picker on the server and import the chosen images into
/// the project's `images/` directory.  Returns the names of the imported files.
/// Returns an empty Vec if the user cancelled without selecting a file.
pub async fn pick_and_import_images(project_name: String) -> DioxusResult<Vec<String>> {
    let files = AsyncFileDialog::new()
        .pick_files()
        .await
        .ok_or_else(|| anyhow!("No files selected"))?;

    let project_path = crate::project::resolve_project_path(&project_name).await?;
    let images_dir = project_path.join("images");
    tokio::fs::create_dir_all(&images_dir)
        .await
        .map_err(|e| anyhow!("create images dir: {e}"))?;

    let mut imported = Vec::new();
    for file in files {
        let file_name = file.file_name();
        let dest = images_dir.join(&file_name);
        let bytes = file.read().await;
        tokio::fs::write(&dest, &bytes)
            .await
            .map_err(|e| anyhow!("write {}: {e}", file_name))?;
        debug!(name = %file_name, "imported image via file picker");
        imported.push(file_name);
    }

    debug!(count = imported.len(), "imported images via file picker");
    Ok(imported)
}

/// Open a native folder-picker on the server and return the chosen path string.
/// Not available on Android (returns an error the caller should surface to the user).
pub async fn pick_projects_folder() -> DioxusResult<String> {
    let dir = AsyncFileDialog::new()
        .pick_folder()
        .await
        .ok_or_else(|| anyhow!("No folder selected"))?;
    Ok(dir.path().to_string_lossy().into_owned())
}

/// Open a native file-picker for a JSON settings file and return the chosen path.
/// Not available on Android (returns an error).
pub async fn pick_settings_file() -> DioxusResult<String> {
    let file = AsyncFileDialog::new()
        .add_filter("JSON settings", &["json"])
        .pick_file()
        .await
        .ok_or_else(|| anyhow!("No file selected"))?;
    Ok(file.path().to_string_lossy().into_owned())
}

/// Save an output file to a user-chosen location.
///
/// On Android the file is written directly to `/sdcard/Download/` (the rfd
/// save-dialog equivalents on that platform are not suitable for quick
/// exports).  On all other platforms a native save-dialog is shown.
/// Returns a human-readable success message.
pub async fn save_output_as(project_name: String, relative_path: String) -> DioxusResult<String> {
    let project_path = crate::project::resolve_project_path(&project_name).await?;
    let full_path = crate::project::resolve_project_relative_path(&project_path, &relative_path)?;
    let file_name = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|e| anyhow!("read output file: {e}"))?;

    #[cfg(target_os = "android")]
    {
        let dest = std::path::Path::new("/sdcard/Download").join(&file_name);
        tokio::fs::write(&dest, &bytes).await.map_err(|e| {
            let detail = if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Go to App Settings → Permissions and allow \"Files and media\" access.".to_string()
            } else {
                format!("{e}")
            };
            anyhow!("Cannot write to /sdcard/Download/: {detail}")
        })?;
        Ok(format!("Saved to /sdcard/Download/{file_name}"))
    }

    #[cfg(not(target_os = "android"))]
    {
        let dest = AsyncFileDialog::new()
            .set_file_name(&file_name)
            .save_file()
            .await
            .ok_or_else(|| anyhow!("Save cancelled"))?;

        dest.write(&bytes)
            .await
            .map_err(|e| anyhow!("write output file: {e}"))?;

        Ok(format!("Saved as {}", dest.path().display()))
    }
}
