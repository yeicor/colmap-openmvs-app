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

pub mod runtimes;
