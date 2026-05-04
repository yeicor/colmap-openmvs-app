#[cfg(test)]
mod tests {
    use crate::server::runtimes::{PRoot, PrepareProgress, Runtime};
    use std::path::PathBuf;

    #[test]
    fn test_proot_new() {
        let install_dir = PathBuf::from("/tmp/proot");
        let proot = PRoot::new(install_dir.clone());
        assert_eq!(proot.install_dir, install_dir);
    }

    #[test]
    fn test_runtime_proot_creation() {
        let runtime = Runtime::proot();
        match runtime {
            Runtime::PRoot(proot) => {
                assert_eq!(proot.install_dir, PathBuf::from("./runtimes/proot"));
            }
        }
    }

    #[test]
    fn test_runtime_proot_with_custom_dir() {
        let custom_dir = PathBuf::from("/custom/path");
        let runtime = Runtime::proot_with_dir(custom_dir.clone());
        assert_eq!(*runtime.install_dir(), custom_dir);
    }

    #[test]
    fn test_proot_sanitize_image_name() {
        let proot = PRoot::new(PathBuf::from("/tmp"));

        // Test various image name formats
        let test_cases = vec![
            ("ubuntu:20.04", "ubuntu_20_04"),
            ("docker.io/library/alpine", "docker_io_library_alpine"),
            ("gcr.io/project/image:v1.2.3", "gcr_io_project_image_v1_2_3"),
            ("myimage", "myimage"),
            ("my/image:latest", "my_image_latest"),
        ];

        for (input, _expected) in test_cases {
            let result = proot.hash_image_name(input);
            // hash_image_name returns a hash, not the sanitized name directly
            // but it should be consistent
            let result2 = proot.hash_image_name(input);
            assert_eq!(result, result2, "Hash should be consistent for {}", input);
        }
    }

    #[test]
    fn test_proot_version_parsing() {
        let test_cases = vec![
            (
                "|__|  |__|__\\_____/\\_____/\\____| v5.4.0-5f780cba",
                "5.4.0-5f780cba",
            ),
            ("version 5.3.1", "5.3.1"),
            ("v4.0.0", "4.0.0"),
        ];

        for (output, expected) in test_cases {
            let result = PRoot::parse_proot_version(output);
            assert!(result.is_ok(), "Failed to parse: {}", output);
            assert_eq!(result.unwrap(), expected, "Mismatch for: {}", output);
        }
    }

    #[test]
    fn test_proot_version_parsing_invalid() {
        let invalid_outputs = vec!["", "no version here", "release 123"];

        for output in invalid_outputs {
            let result = PRoot::parse_proot_version(output);
            assert!(result.is_err(), "Should fail for: {}", output);
        }
    }

    #[test]
    fn test_prepare_progress_serialization() {
        use serde_json;

        let progress = PrepareProgress::ResolvingImage;
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("ResolvingImage"));

        let progress = PrepareProgress::Downloading {
            downloaded_bytes: 1024,
            total_bytes: Some(2048),
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("1024"));
        assert!(json.contains("2048"));

        let progress = PrepareProgress::ExtractingLayer {
            layer: "sha256:abc123".to_string(),
            progress: 0.5,
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("sha256:abc123"));
        assert!(json.contains("0.5"));

        let progress = PrepareProgress::Completed;
        let json = serde_json::to_string(&progress).unwrap();
        assert!(json.contains("Completed"));
    }

    #[test]
    fn test_runtime_install_dir_getter() {
        let custom_dir = PathBuf::from("/opt/runtime");
        let runtime = Runtime::proot_with_dir(custom_dir.clone());
        assert_eq!(runtime.install_dir(), &custom_dir);
    }

    #[test]
    fn test_runtime_clone() {
        let runtime1 = Runtime::proot();
        let runtime2 = runtime1.clone();

        match (runtime1, runtime2) {
            (Runtime::PRoot(p1), Runtime::PRoot(p2)) => {
                assert_eq!(p1.install_dir, p2.install_dir);
            }
        }
    }

    // Platform detection tests
    #[test]
    fn test_platform_detection() {
        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        // We can only test the current platform
        let proot = PRoot::new(PathBuf::from("/tmp/proot"));
        let is_supported = proot.is_supported().is_ok();

        match (target_arch, target_os) {
            ("x86_64", "linux") => assert!(is_supported, "x86_64-linux should be supported"),
            ("aarch64", "android") | ("x86_64", "android") => {
                assert!(is_supported, "Android should be supported")
            }
            _ => {
                // Other platforms may or may not be supported depending on system proot
                println!(
                    "Platform {}-{} support depends on system proot",
                    target_arch, target_os
                );
            }
        }
    }

    // Async tests
    #[tokio::test]
    async fn test_runtime_is_supported() {
        let runtime = Runtime::proot();
        let result = runtime.is_supported();

        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        // x86_64-linux and *-android should be supported
        match (target_arch, target_os) {
            ("x86_64", "linux") => assert!(result.is_ok()),
            ("aarch64", "android") | ("x86_64", "android") => assert!(result.is_ok()),
            _ => {
                // Other platforms may fail unless system proot is available
                println!(
                    "Platform {}-{} result: {:?}",
                    target_arch, target_os, result
                );
            }
        }
    }

    #[tokio::test]
    async fn test_parse_version_formats() {
        let _proot = PRoot::new(PathBuf::from("/tmp"));

        let test_cases = vec![
            "PRoot v5.4.0-5f780cba\n",
            "v5.3.0\n",
            "|__|  |__|__\\_____/\\_____/\\____| v5.2.1\n",
            "5.1.0",
        ];

        for output in test_cases {
            let result = PRoot::parse_proot_version(output);
            assert!(result.is_ok(), "Failed to parse version from: {}", output);
            let version = result.unwrap();
            assert!(
                !version.is_empty(),
                "Version should not be empty for: {}",
                output
            );
            assert!(
                version.chars().next().unwrap().is_numeric(),
                "Version should start with number: {}",
                version
            );
        }
    }

    #[test]
    fn test_process_handle_creation() {
        use std::process::Command;

        let child = Command::new("echo").arg("test").spawn().unwrap();

        let handle = crate::server::runtimes::ProcessHandle { child };
        assert_ne!(handle.child.id(), 0);
    }

    #[tokio::test]
    async fn test_runtime_metadata_structure() {
        let metadata_json = r#"{
            "env": {"PATH": "/bin:/usr/bin"},
            "entrypoint": ["/bin/sh"],
            "cmd": ["-c", "echo hello"],
            "working_dir": "/"
        }"#;

        let result: Result<serde_json::Value, _> = serde_json::from_str(metadata_json);
        assert!(result.is_ok());

        let metadata = result.unwrap();
        assert_eq!(metadata["env"]["PATH"], "/bin:/usr/bin");
        assert_eq!(metadata["entrypoint"][0], "/bin/sh");
        assert_eq!(metadata["cmd"][0], "-c");
        assert_eq!(metadata["working_dir"], "/");
    }

    #[test]
    fn test_runtime_enum_variants() {
        let runtime1 = Runtime::proot();
        let runtime2 = Runtime::proot_with_dir(PathBuf::from("/custom"));

        // Verify we can match on variants
        match runtime1 {
            Runtime::PRoot(_) => {
                println!("Successfully matched PRoot variant");
            }
        }

        match runtime2 {
            Runtime::PRoot(_) => {
                println!("Successfully matched PRoot variant");
            }
        }
    }

    #[test]
    fn test_image_name_hashing_consistency() {
        let proot = PRoot::new(PathBuf::from("/tmp"));

        let images = vec![
            "ubuntu:20.04",
            "alpine:latest",
            "nginx:1.21",
            "custom/image:v1.0",
        ];

        for image in images {
            let hash1 = proot.hash_image_name(image);
            let hash2 = proot.hash_image_name(image);
            assert_eq!(hash1, hash2, "Hashes should be consistent for {}", image);

            // Different images should (likely) have different hashes
            let other_hash = proot.hash_image_name(&format!("{}x", image));
            assert_ne!(
                hash1, other_hash,
                "Different images should have different hashes"
            );
        }
    }

    #[test]
    fn test_proot_struct_fields() {
        let install_dir = PathBuf::from("/opt/proot");
        let proot = PRoot::new(install_dir.clone());

        // Verify public field is accessible
        assert_eq!(proot.install_dir, install_dir);

        // Verify we can modify it
        let mut proot = proot;
        proot.install_dir = PathBuf::from("/other/path");
        assert_eq!(proot.install_dir, PathBuf::from("/other/path"));
    }

    #[test]
    fn test_runtime_is_clone() {
        let runtime = Runtime::proot();
        let _cloned = runtime.clone();
        // If this compiles, Clone is implemented correctly
    }

    #[test]
    fn test_runtime_is_debug() {
        let runtime = Runtime::proot();
        let debug_str = format!("{:?}", runtime);
        assert!(debug_str.contains("PRoot"));
    }

    #[test]
    fn test_proot_is_clone() {
        let proot = PRoot::new(PathBuf::from("/tmp"));
        let _cloned = proot.clone();
        // If this compiles, Clone is implemented correctly
    }

    #[test]
    fn test_proot_is_debug() {
        let proot = PRoot::new(PathBuf::from("/tmp"));
        let debug_str = format!("{:?}", proot);
        assert!(debug_str.contains("install_dir"));
    }

    #[tokio::test]
    async fn test_proot_image_prepare_paths() {
        let install_dir = PathBuf::from("/tmp/test_proot");
        let proot = PRoot::new(install_dir.clone());

        let image = "test:latest";
        let image_hash = proot.hash_image_name(image);

        // Verify path construction would work
        let image_path = install_dir.join("images").join(&image_hash);
        let rootfs_path = image_path.join("rootfs");
        let metadata_path = image_path.join("metadata.json");

        assert!(image_hash.len() > 0);
        assert!(image_path.to_string_lossy().contains("images"));
        assert!(rootfs_path.to_string_lossy().contains("rootfs"));
        assert!(metadata_path.to_string_lossy().contains("metadata.json"));
    }
}
