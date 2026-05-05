//! Backend library for colmap-openmvs-app
//! Contains all implementations for server functions with access to heavy native dependencies

mod project;
pub use project::{
    add_project_image, batch_resize_images, clear_project_images, delete_project_image,
    download_demo_images, get_project_image, get_project_images,
};

mod projects;
pub use projects::{create_project, delete_project, get_projects, rename_project};

mod settings;
pub use settings::{get_settings, update_settings};

mod runtimes_api;
pub use runtimes_api::{
    download_runtime_version, get_available_runtime_versions, get_runtime_info,
    list_available_image_tags, list_runtime_images, prepare_runtime_image, remove_runtime_image,
};

pub mod runtimes;
