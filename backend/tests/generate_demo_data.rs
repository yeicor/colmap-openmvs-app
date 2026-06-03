use backend::task_registry::TASK_REGISTRY;
use colmap_openmvs_api::{Settings, TaskEvent, TaskState};
use colmap_openmvs_backend as backend;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_generate_demo_data() {
    let out_dir = env::var("DEMO_ASSETS_DIR").unwrap_or_else(|_| "../app/assets/demo".to_string());
    let out_path = PathBuf::from(&out_dir);

    // Create directories
    let images_dir = out_path.join("images");
    let outputs_dir = out_path.join("outputs");
    if out_path.exists() {
        fs::remove_dir_all(&out_path).ok();
    }
    fs::create_dir_all(&images_dir).expect("Failed to create images dir");
    fs::create_dir_all(&outputs_dir).expect("Failed to create outputs dir");

    // 1. Setup temporary workspace
    let temp_workspace = tempfile::tempdir().expect("Failed to create temp workspace");
    let projects_folder = temp_workspace.path().join("projects");
    let proot_binary_dir = temp_workspace.path().join("proot");
    let proot_images_dir = temp_workspace.path().join("proot_images");

    fs::create_dir_all(&projects_folder).unwrap();
    fs::create_dir_all(&proot_binary_dir).unwrap();
    fs::create_dir_all(&proot_images_dir).unwrap();

    let settings = Settings {
        projects_folder: projects_folder.to_string_lossy().into_owned(),
        proot_binary_dir: proot_binary_dir.to_string_lossy().into_owned(),
        proot_images_dir: proot_images_dir.to_string_lossy().into_owned(),
        default_image_tag: Some("proot:mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest".to_string()),
        custom_mounts: vec![],
        settings_file_path: Some(
            temp_workspace
                .path()
                .join("settings.json")
                .to_string_lossy()
                .into_owned(),
        ),
    };

    backend::update_settings(settings.clone())
        .await
        .expect("Failed to update settings");

    // Try PRoot or Docker
    let runtime_info = match backend::get_runtime_info().await {
        Ok(info) if info.installed || info.supported => {
            if !info.installed && info.supported {
                println!("Installing PRoot...");
                backend::download_runtime_version("latest".to_string())
                    .await
                    .expect("Failed to download PRoot");
            }
            backend::get_runtime_info().await.unwrap()
        }
        _ => {
            println!("Falling back to Docker...");
            let docker_info = backend::get_docker_runtime_info()
                .await
                .expect("Docker not supported either!");
            if !docker_info.supported {
                panic!("Neither PRoot nor Docker are supported on this platform!");
            }

            // Switch to docker
            let mut s = settings.clone();
            s.default_image_tag =
                Some("docker:mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest".to_string());
            backend::update_settings(s.clone()).await.unwrap();
            docker_info
        }
    };

    // 2. Prepare runtime image
    println!("Preparing runtime image...");
    let image_tag = backend::get_settings()
        .await
        .unwrap()
        .default_image_tag
        .unwrap();
    let prep_task_id = if image_tag.starts_with("docker:") {
        backend::prepare_docker_image(image_tag.clone())
            .await
            .expect("Failed to prepare docker image")
    } else {
        backend::prepare_runtime_image(image_tag.clone())
            .await
            .expect("Failed to prepare proot image")
    };
    poll_task_until_done(&prep_task_id).await;

    // 3. Create Project
    println!("Creating project...");
    backend::create_project("demo".to_string())
        .await
        .expect("Failed to create project");

    // 4. Download Kermit dataset
    println!("Downloading dataset...");
    let download_task = backend::download_demo_images("demo".to_string(), "kermit".to_string())
        .await
        .expect("Failed to start download");
    let download_events = poll_task_until_done(&download_task).await;
    fs::write(
        out_path.join("download_events.json"),
        serde_json::to_string_pretty(&download_events).unwrap(),
    )
    .unwrap();

    // 5. Run Pipeline
    println!("Running pipeline...");
    let pipeline_task = backend::run_pipeline("demo".to_string(), false)
        .await
        .expect("Failed to start pipeline");
    let pipeline_events = poll_task_until_done(&pipeline_task).await;
    fs::write(
        out_path.join("pipeline_events.json"),
        serde_json::to_string_pretty(&pipeline_events).unwrap(),
    )
    .unwrap();

    // 6. Gather all realistic data
    let projects = backend::get_projects().await.unwrap();
    let project = projects.into_iter().find(|p| p.name == "demo").unwrap();
    let images = backend::get_project_images("demo".to_string())
        .await
        .unwrap();
    let outputs = backend::list_project_outputs("demo".to_string())
        .await
        .unwrap();
    let run_status = backend::get_project_run_status("demo".to_string())
        .await
        .unwrap();

    let config_schema = backend::get_image_config(image_tag.clone()).await.unwrap();
    let project_config = backend::load_project_config("demo".to_string())
        .await
        .unwrap();

    let manifest = json!({
        "projects": [project.clone()],
        "settings": settings,
        "dark_mode": null,
        "project": {
            "images": images,
            "config_schema": config_schema,
            "project_config": project_config,
            "outputs": outputs,
            "run_status": run_status,
        },
        "runtime_info": runtime_info,
    });

    fs::write(
        out_path.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // 7. Copy generated files to assets/demo
    for img in &images {
        let bytes = backend::get_project_image_bytes("demo".to_string(), img.clone())
            .await
            .unwrap();
        fs::write(images_dir.join(img), bytes).unwrap();
    }

    for output in &outputs {
        if output.is_viewable {
            if let Ok(bytes) =
                backend::get_project_output_bytes("demo".to_string(), output.relative_path.clone())
                    .await
            {
                // Keep the relative directory structure
                let rel_path = Path::new(&output.relative_path);
                if let Some(parent) = rel_path.parent() {
                    fs::create_dir_all(outputs_dir.join(parent)).unwrap();
                }
                fs::write(outputs_dir.join(rel_path), bytes).unwrap();
            }
        }
    }
}

async fn poll_task_until_done(task_id: &str) -> Vec<TaskEvent> {
    let mut cursor = 0;
    let mut collected = Vec::new();
    loop {
        if let Some(batch) = TASK_REGISTRY.poll_events(task_id, cursor) {
            for e in &batch.events {
                collected.push(e.clone());
            }
            cursor = batch.cursor;
            if batch.is_terminal {
                break;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }

    // Check if it failed
    let info = TASK_REGISTRY.get_task_info(task_id).unwrap();
    if let TaskState::Failed(err) = info.state {
        panic!("Task {} failed: {}", task_id, err);
    }

    collected
}
