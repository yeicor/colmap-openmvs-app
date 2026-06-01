use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("No file selected")]
    NoFileSelected,
    #[error("No directory selected")]
    NoDirectorySelected,
    #[error("Directory picker not supported on this platform")]
    DirectoryNotSupported,
    #[error("Save dialog not supported on this platform")]
    SaveNotSupported,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Platform error: {0}")]
    Platform(String),
}

pub struct FileFilter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

pub struct PickedFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Pick one or more files. Returns an empty `Vec` if the user cancelled.
pub async fn pick_files(filter: FileFilter, multiple: bool) -> Result<Vec<PickedFile>, Error> {
    #[cfg(not(target_os = "android"))]
    {
        let dialog = rfd::AsyncFileDialog::new().add_filter(filter.name, filter.extensions);
        let handles = if multiple {
            dialog.pick_files().await.unwrap_or_default()
        } else {
            dialog
                .pick_file()
                .await
                .map(|h| vec![h])
                .unwrap_or_default()
        };

        let mut out = Vec::with_capacity(handles.len());
        for h in handles {
            let name = h.file_name();
            let bytes = h.read().await;
            out.push(PickedFile { name, bytes });
        }
        Ok(out)
    }
    #[cfg(target_os = "android")]
    {
        // Android file picking is handled via JNI / Intent; not yet implemented.
        Err(Error::Platform(
            "Android file picker not yet implemented".to_string(),
        ))
    }
}

/// Pick a directory. Returns `Error::DirectoryNotSupported` on Android.
pub async fn pick_directory() -> Result<std::path::PathBuf, Error> {
    #[cfg(not(target_os = "android"))]
    {
        rfd::AsyncFileDialog::new()
            .pick_folder()
            .await
            .map(|h| h.path().to_path_buf())
            .ok_or(Error::NoDirectorySelected)
    }
    #[cfg(target_os = "android")]
    {
        Err(Error::DirectoryNotSupported)
    }
}

/// Open a save-file dialog and write `data` to the chosen path.
pub async fn save_file(default_name: &str, data: Vec<u8>) -> Result<(), Error> {
    #[cfg(not(target_os = "android"))]
    {
        let handle = rfd::AsyncFileDialog::new()
            .set_file_name(default_name)
            .save_file()
            .await
            .ok_or(Error::NoFileSelected)?;
        tokio::fs::write(handle.path(), data)
            .await
            .map_err(Error::Io)
    }
    #[cfg(target_os = "android")]
    {
        Err(Error::SaveNotSupported)
    }
}
