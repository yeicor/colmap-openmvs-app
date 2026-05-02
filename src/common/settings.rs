use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// The path to the folder containing all the projects
    pub projects_folder: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            projects_folder: "./projects".to_string(),
        }
    }
}
