use backend::task_registry::TASK_REGISTRY;
use colmap_openmvs_api::{Settings, TaskEvent, TaskState};
use colmap_openmvs_backend as backend;
use colmap_openmvs_backend::download_runtime_version;
use colmap_openmvs_backend::get_available_runtime_versions;
use futures::StreamExt;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::time::{sleep, Duration};

/// Initialize tracing for the test, respecting `RUST_LOG` if set.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

#[tokio::test]
async fn test_generate_demo_data() {
    init_tracing();
    let out_dir = env::var("DEMO_ASSETS_DIR").unwrap_or_else(|_| {
        let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
        format!("{}/../app/assets/demo", manifest)
    });
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
        theme_override: None,
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
            if !info.installed {
                println!("PRoot not installed but supported, will attempt to download and use PRoot runtime");
                download_runtime_version(
                    get_available_runtime_versions()
                        .await
                        .expect("Failed to get available runtime versions")
                        .first()
                        .unwrap()
                        .to_string(),
                )
                .await
                .expect("Failed to download PRoot runtime");
            }
            info
        }
        _ => {
            println!("PRoot not installed, falling back to Docker...");
            let mut docker_info = backend::get_docker_runtime_info()
                .await
                .expect("Docker check failed");
            // Avoid using on Windows GitHub CI (can only run windows images in this case)
            if cfg!(windows) && docker_info.supported && std::env::var("GITHUB_ACTIONS").is_ok() {
                docker_info.supported = false;
                docker_info.unsupported_reason =
                    Some("Docker is not supported on Windows GitHub Actions runners".to_string());
            }
            if !docker_info.supported {
                let reason = docker_info
                    .unsupported_reason
                    .as_deref()
                    .unwrap_or("unknown");
                println!(
                    "Skipping test: neither PRoot nor Docker are supported on this platform.\n  Docker unsupported reason: {reason}"
                );
                return;
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
    let settings_tag = backend::get_settings()
        .await
        .unwrap()
        .default_image_tag
        .unwrap();
    let (runtime, image_tag) = settings_tag
        .split_once(':')
        .expect("Image tag must have runtime prefix");
    let prep_task_id = match runtime {
        "docker" => backend::prepare_docker_image(image_tag.to_string())
            .await
            .expect("Failed to prepare docker image"),
        "proot" => backend::prepare_runtime_image(image_tag.to_string())
            .await
            .expect("Failed to prepare proot image"),
        other => panic!("Unknown runtime prefix: {other}"),
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
    let project_images = backend::get_project_images("demo".to_string())
        .await
        .unwrap();
    let images = project_images.images;
    let outputs = backend::list_project_outputs("demo".to_string())
        .await
        .unwrap();
    let run_status = backend::get_project_run_status("demo".to_string())
        .await
        .unwrap();

    let config_schema = backend::get_image_config(image_tag.to_string())
        .await
        .unwrap();
    // Save a default project config so load_project_config succeeds
    let settings_tag = backend::get_settings()
        .await
        .unwrap()
        .default_image_tag
        .unwrap();
    let (_runtime, raw_image_tag) = settings_tag.split_once(':').unwrap();
    backend::save_project_config(
        "demo".to_string(),
        colmap_openmvs_api::SavedProjectConfig {
            image_tag: raw_image_tag.to_string(),
            environment_variables: vec![],
            custom_script: None,
        },
    )
    .await
    .unwrap();
    let project_config = backend::load_project_config("demo".to_string())
        .await
        .unwrap();

    // 7. Generate GLB files for viewable outputs.
    //    Store them alongside the raw files so the demo backend can serve
    //    them via demo_output_bytes(), but do NOT add them to the manifest —
    //    the demo emulates the real back-end which lists only raw files with
    //    glb_available=true.
    println!("Generating GLB files...");
    let project_path = Path::new(&project.path);
    let mut glb_paths: Vec<String> = Vec::new();
    for output in &outputs {
        if output.is_viewable {
            let glb_path = raw_to_glb_path(&output.relative_path);
            match backend::generate_glb(&output.relative_path, project_path) {
                Ok(glb_bytes) => {
                    // Write .glb alongside the original in the project work directory
                    // (so the real back-end would also see it on a re-list, but
                    // we use the original output list for the manifest).
                    if let Some(parent) = Path::new(&glb_path).parent() {
                        let _ = fs::create_dir_all(project_path.join(parent));
                    }
                    if let Err(e) = fs::write(project_path.join(&glb_path), &glb_bytes) {
                        println!("WARNING: Failed to write GLB to project: {e}");
                    }
                    // Write the GLB directly to the demo outputs directory
                    let glb_rel = Path::new(&glb_path);
                    if let Some(parent) = glb_rel.parent() {
                        let _ = fs::create_dir_all(outputs_dir.join(parent));
                    }
                    if let Err(e) = fs::write(outputs_dir.join(&glb_path), &glb_bytes) {
                        println!("WARNING: Failed to write GLB to demo outputs: {e}");
                    }
                    glb_paths.push(glb_path);
                }
                Err(e) => {
                    println!(
                        "WARNING: Failed to generate GLB for {}: {e}",
                        output.relative_path
                    );
                }
            }
        }
    }

    // 8. Set glb_available on viewable outputs and write manifest.
    //    Only raw files appear — GLB files are NOT listed as separate entries.
    let outputs_for_manifest: Vec<colmap_openmvs_api::OutputFile> = outputs
        .iter()
        .cloned()
        .map(|mut o| {
            if o.is_viewable {
                o.glb_available = true;
            }
            o
        })
        .collect();

    let manifest = json!({
        "projects": [project.clone()],
        "settings": settings,
        "dark_mode": null,
        "project": {
            "images": images,
            "config_schema": config_schema,
            "project_config": project_config,
            "outputs": outputs_for_manifest,
            "run_status": run_status,
        },
        "runtime_info": runtime_info,
    });

    fs::write(
        out_path.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // 9. Copy generated files to assets/demo
    for img in &images {
        let stream = backend::get_project_image_bytes("demo".to_string(), img.clone())
            .await
            .unwrap();
        // ImageData has a `.stream` field carrying the byte stream.
        let bytes: Vec<u8> = stream
            .stream
            .filter_map(|r| async move { r.ok() })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flat_map(|b| b.to_vec())
            .collect();
        fs::write(images_dir.join(img), bytes).unwrap();
    }

    // Copy raw viewable/png outputs to demo outputs directory
    for output in &outputs {
        if output.is_viewable || output.relative_path.ends_with(".png") {
            if let Ok(stream) =
                backend::get_project_output_bytes("demo".to_string(), output.relative_path.clone())
                    .await
            {
                let rel_path = Path::new(&output.relative_path);
                if let Some(parent) = rel_path.parent() {
                    fs::create_dir_all(outputs_dir.join(parent)).unwrap();
                }
                let bytes: Vec<u8> = stream
                    .into_inner()
                    .filter_map(|r| async move { r.ok() })
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .flat_map(|b| b.to_vec())
                    .collect();
                fs::write(outputs_dir.join(rel_path), bytes).unwrap();
            }
        }
    }

    // The pre-generated GLB files were already written to outputs_dir in step 7.
}

/// Map a raw output path (e.g. "openmvs/scene_mesh.ply" or
/// "colmap/dense/sparse/points3D.bin") to its GLB companion path.
fn raw_to_glb_path(relative_path: &str) -> String {
    let lower = relative_path.to_lowercase();
    if lower.ends_with(".ply") {
        let without_ext = relative_path.strip_suffix(".ply").unwrap_or(relative_path);
        format!("{}.glb", without_ext)
    } else if lower.ends_with("points3d.bin") {
        // Strip the ".bin" suffix (4 chars) and append ".glb"
        let without_ext = &relative_path[..relative_path.len() - 4];
        format!("{}.glb", without_ext)
    } else {
        format!("{}.glb", relative_path)
    }
}

async fn poll_task_until_done(task_id: &str) -> Vec<TaskEvent> {
    let mut cursor = 0;
    let mut collected = Vec::new();
    loop {
        if let Some(batch) = TASK_REGISTRY.poll_events(task_id, cursor, None) {
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
