use dioxus::prelude::*;
use tokio::time::{sleep, Duration};
#[get("/projects")]
pub async fn get_projects() -> Result<Vec<String>> {
    sleep(Duration::from_secs(1)).await;
    Err(dioxus::CapturedError::from_display("TODO: implement get_projects (settings first to support changing projects folder location)"))
}
