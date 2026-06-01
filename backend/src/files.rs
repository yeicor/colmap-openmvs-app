use crate::projects::get_projects;
use anyhow::anyhow;
use dioxus::Result as DioxusResult;
use rrfd::{pick_directory, pick_files, save_file, FileFilter};
use tracing::debug;

/// Open a native file-picker on the server and import the chosen images into
/// the project's `images/` directory.  Returns the names of the imported files.
/// Returns an empty Vec if the user cancelled without selecting a file.
pub async fn pick_and_import_images(project_name: String) -> DioxusResult<Vec<String>> {
    let files = pick_files(
        FileFilter {
            name: "Images",
            extensions: &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "heic"],
        },
        true,
    )
    .await
    .map_err(|e| anyhow!("File picker: {e}"))?;

    if files.is_empty() {
        return Ok(Vec::new());
    }

    let project_path = get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .map(|p| p.path)
        .ok_or_else(|| anyhow!("Project not found: {}", project_name))?;

    let images_dir = std::path::Path::new(&project_path).join("images");
    tokio::fs::create_dir_all(&images_dir)
        .await
        .map_err(|e| anyhow!("create images dir: {e}"))?;

    let mut imported = Vec::with_capacity(files.len());
    for file in files {
        let dest = images_dir.join(&file.name);
        tokio::fs::write(&dest, &file.bytes)
            .await
            .map_err(|e| anyhow!("write {}: {e}", file.name))?;
        debug!(name = %file.name, "imported image via file picker");
        imported.push(file.name);
    }
    Ok(imported)
}

/// Open a native folder-picker on the server and return the chosen path string.
/// Not available on Android (returns an error the caller should surface to the user).
pub async fn pick_projects_folder() -> DioxusResult<String> {
    pick_directory()
        .await
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| anyhow!("Folder picker: {e}").into())
}

/// Open a native file-picker for a JSON settings file and return the chosen path.
/// Not available on Android (returns an error).
pub async fn pick_settings_file() -> DioxusResult<String> {
    #[cfg(not(target_os = "android"))]
    {
        let handle = rfd::AsyncFileDialog::new()
            .add_filter("JSON settings", &["json"])
            .pick_file()
            .await
            .ok_or_else(|| anyhow!("No file selected"))?;
        Ok(handle.path().to_string_lossy().into_owned())
    }
    #[cfg(target_os = "android")]
    {
        Err(anyhow!("Settings file picker not supported on Android").into())
    }
}

/// Open a native save-file dialog on the server and write the output file to
/// the user-chosen location.
pub async fn save_output_as(project_name: String, relative_path: String) -> DioxusResult<()> {
    let project_path = get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .map(|p| p.path)
        .ok_or_else(|| anyhow!("Project not found: {}", project_name))?;

    let sanitized = relative_path.trim_start_matches('/');
    for component in std::path::Path::new(sanitized).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow!("Path traversal not allowed").into());
        }
    }
    let full_path = std::path::Path::new(&project_path).join(sanitized);
    let file_name = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|e| anyhow!("read output file: {e}"))?;
    save_file(&file_name, bytes)
        .await
        .map_err(|e| anyhow!("save dialog: {e}").into())
}
