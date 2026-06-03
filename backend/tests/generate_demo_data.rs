use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::json;
use image::{ImageBuffer, RgbImage, Rgb};

#[tokio::test]
#[ignore]
async fn test_generate_demo_data() {
    let out_dir = env::var("DEMO_ASSETS_DIR").unwrap_or_else(|_| "../app/assets/demo".to_string());
    let out_path = PathBuf::from(out_dir);

    // Create directories
    let images_dir = out_path.join("images");
    let outputs_dir = out_path.join("outputs");
    fs::create_dir_all(&images_dir).expect("Failed to create images dir");
    fs::create_dir_all(&outputs_dir).expect("Failed to create outputs dir");

    // 1. Generate synthetic images
    let image_names = ["demo_01.jpg", "demo_02.jpg", "demo_03.jpg"];
    for (i, name) in image_names.iter().enumerate() {
        let mut img: RgbImage = ImageBuffer::new(100, 100);
        let color = match i {
            0 => Rgb([255, 100, 100]),
            1 => Rgb([100, 255, 100]),
            _ => Rgb([100, 100, 255]),
        };
        for x in 0..100 {
            for y in 0..100 {
                let r = (color[0] as f32 * (x as f32 / 100.0)) as u8;
                let g = (color[1] as f32 * (y as f32 / 100.0)) as u8;
                let b = (color[2] as f32 * ((100 - x) as f32 / 100.0)) as u8;
                img.put_pixel(x, y, Rgb([r, g, b]));
            }
        }
        img.save(images_dir.join(name)).expect("Failed to save image");
    }

    // 2. Generate a minimal PLY point cloud
    let ply_content = "\
ply
format ascii 1.0
element vertex 3
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
end_header
0.0 1.0 0.0 255 0 0
-1.0 -1.0 0.0 0 255 0
1.0 -1.0 0.0 0 0 255
";
    fs::write(outputs_dir.join("reconstruction.ply"), ply_content).expect("Failed to save ply");

    // 3. Generate manifest.json
    let manifest = json!({
        "projects": [
            { "name": "demo", "path": "/demo" }
        ],
        "settings": {
            "projects_folder": "/demo",
            "proot_binary_dir": "/demo/proot",
            "proot_images_dir": "/demo/images",
            "default_image_tag": "proot:demo:latest",
            "custom_mounts": [],
            "settings_file_path": null
        },
        "dark_mode": null,
        "project": {
            "images": image_names,
            "config_schema": {
                "image_tag": "demo:latest",
                "build_date": "2024-01-01T00:00:00Z",
                "tools": [],
                "environment_variables": [
                    { "name": "COLMAP_NUM_THREADS", "help": "Number of threads" }
                ]
            },
            "project_config": {
                "image_tag": "demo:latest",
                "environment_variables": [
                    { "name": "COLMAP_NUM_THREADS", "value": "4" }
                ],
                "custom_script": ""
            },
            "outputs": [
                {
                    "relative_path": "reconstruction.ply",
                    "name": "reconstruction.ply",
                    "size": ply_content.len(),
                    "is_viewable": true,
                    "modified_at": 0
                }
            ],
            "run_status": {
                "is_running": false,
                "is_dry_run": false,
                "progress": null,
                "task_id": ""
            }
        },
        "runtime_info": {
            "name": "Demo Runtime",
            "supported": false,
            "unsupported_reason": "Demo mode does not support runtimes",
            "installed": false,
            "version": null
        }
    });

    let manifest_str = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");
    fs::write(out_path.join("manifest.json"), manifest_str).expect("Failed to save manifest");
}
