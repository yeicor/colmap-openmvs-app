# COLMAP OpenMVS App

A cross-platform desktop/web/mobile application for running [COLMAP](https://colmap.github.io/) and [OpenMVS](https://github.com/cdcseacave/openMVS) photogrammetry pipelines through an intuitive graphical interface. Built with [Dioxus](https://dioxuslabs.com/) Fullstack.

## Features

- **Pipeline Automation** — Run the full COLMAP + OpenMVS reconstruction pipeline (feature extraction, matching, sparse/dense reconstruction, meshing, texturing) with a single click.
- **Containerized Execution** — The pipeline runs inside a container (Docker on desktop, or PRoot on Android/restricted environments) using a prebuilt image containing COLMAP, OpenMVS, and all dependencies.
- **Image Management** — Upload images from your device or download demo datasets (ET, Kermit). Batch resize images before processing.
- **Interactive 3D Viewer** — View reconstruction outputs (point clouds, meshes, textured models) directly in the browser via an embedded Three.js GLB viewer with orbit controls, wireframe mode, and measurement tools.
- **Real-time Progress** — Live pipeline logs with per-stage progress bars, auto-scroll, and expandable stage details. Background tasks persist across page navigation.
- **Configurable Parameters** — Edit COLMAP and OpenMVS environment variables through a generated schema UI. Add custom bash scripts to customize the pipeline.
- **Output Browser** — Tree-based file browser for reconstruction outputs. Download individual files or view 3D models inline.
- **Multiple Runtimes** — Supports Docker and PRoot runtimes. Can be easily kept up to date with the latest COLMAP/OpenMVS with a single click.
- **Cross-Platform** — Desktop (Windows, macOS, Linux), Web (via Docker deployment), and Android (with PRoot-based container support).

## Prerequisites

- **Rust** (edition 2021) with the `wasm32-unknown-unknown` target
- **Node.js** and **npm** (for building the 3D viewer and debug console)
- **Dioxus CLI** (`dx`) — install via `curl -sSL https://dioxus.dev/install.sh | sh`
- **Docker** (optional, for Docker runtime) or **PRoot** binary (auto-downloaded for PRoot runtime)

## Project Structure

```
colmap-openmvs-app/
├── api/                          # Shared types crate (serde-only deps)
│   └── src/
│       └── types.rs              # All shared data types
├── backend/                      # Server-side logic crate
│   └── src/
│       ├── config.rs             # Config schema parsing from container --help
│       ├── line_reader.rs        # Line reader for \r/\n from stdout
│       ├── pipeline.rs           # Pipeline execution with progress markers
│       ├── process.rs            # Cross-platform process tree killing
│       ├── projects.rs           # Project CRUD operations
│       ├── settings.rs           # Settings persistence
│       ├── task_registry.rs      # Global task registry with event log
│       ├── runtimes_api.rs       # Runtime management API
│       ├── runtimes/             # Container runtime implementations
│       │   ├── docker.rs         # Docker runtime
│       │   ├── proot.rs          # PRoot runtime
│       │   ├── registry.rs       # OCI registry client
│       │   └── image_manager.rs  # OCI image puller
│       ├── output_viewer.rs      # Output file viewer with GLB conversion
│       ├── ply_to_glb.rs         # PLY → glTF 2.0/GLB converter
│       ├── android_startup.rs    # Android symlink setup
│       ├── android_settings_validation.rs  # Settings repair for Android
│       ├── theme.rs              # Dark mode detection (Android JNI)
│       └── project/
│           └── images.rs         # Image upload/download/resize
├── app/                          # UI crate (compiles to WASM + native server)
│   ├── src/
│   │   ├── main.rs               # Entrypoint, routing, app component
│   │   ├── server.rs             # Server function wrappers (RPC layer)
│   │   ├── task_manager.rs       # Background task management
│   │   ├── logging.rs            # Log initialization
│   │   ├── views/                # Page-level components
│   │   │   ├── projects.rs       # Project list view
│   │   │   ├── settings.rs       # Settings view (general + runtime tabs)
│   │   │   ├── startup_tasks.rs  # Android settings repair view
│   │   │   └── project/          # Project detail sub-views
│   │   │       ├── mod.rs        # Project layout with tabs
│   │   │       ├── images.rs     # Image gallery & upload
│   │   │       ├── config.rs     # Config editor
│   │   │       ├── logs.rs       # Pipeline log viewer
│   │   │       └── outputs.rs    # Output file browser & 3D viewer
│   │   ├── components/           # Reusable UI primitives
│   │   │   ├── button/
│   │   │   ├── tabs/
│   │   │   ├── sidebar/
│   │   │   ├── sheet/
│   │   │   ├── progress/
│   │   │   ├── tooltip/
│   │   │   ├── separator/
│   │   │   └── alert_dialog/
│   │   └── mycomponents/         # App-specific components
│   │       ├── page_header.rs
│   │       ├── banner.rs
│   │       ├── help.rs
│   │       └── tasks_panel.rs
│   ├── js/                       # JavaScript source files
│   │   └── viewer3d.js           # Three.js GLB viewer
│   ├── assets/                   # CSS, icons, bundled JS
│   └── build.rs                  # npm install + esbuild bundling
├── devstorage/                   # Runtime data (gitignored)
├── .github/workflows/
│   └── ci.yml                    # Build + test matrix, Docker, GitHub Release
├── Dockerfile                    # Multi-stage Docker build
├── Dioxus.toml                   # Dioxus configuration
├── build_android.sh              # Android APK build script
└── package.json                  # JavaScript dependencies
```

## Quick Start

### Development

```bash
# Serve with hot-reload (desktop)
dx serve --desktop

# Serve as web app
dx serve --web
```

### Production Build

```bash
# Desktop bundle
dx bundle --desktop --release --features server

# Web bundle
dx bundle --web --release --features server

# Docker image (web bundle)
docker build --build-arg BUILD_TYPE=release -t colmap-openmvs-app .
docker run -p 8080:8080 colmap-openmvs-app
```

### Android Build

Use the provided build script:

```bash
./build_android.sh
```

See the script for available options.

## Architecture

### Client-Server Model

The app compiles to two targets:
- **WebAssembly (client)** — Runs in the browser. Handles UI rendering and user interaction via Dioxus.
- **Native binary (server)** — Runs on the backend. Handles file I/O, container orchestration, image processing, and 3D model conversion. Communication happens through Dioxus server functions (`#[get]`/`#[post]`).

### Pipeline Execution

1. Images are uploaded to a project directory.
2. When the user clicks "Run", the app launches a container (Docker or PRoot) with the project directory mounted.
3. The pipeline script runs COLMAP commands (feature extraction, matching, sparse reconstruction, image undistortion) followed by OpenMVS (dense reconstruction, meshing, texturing).
4. Progress is reported via special stdout markers (`::group`, `::percent`, `::remaining_groups`), parsed by the backend for real-time UI updates.
5. Outputs (PLY point clouds/meshes, PNG textures, GLB models) are served through a file browser with dynamic PLY-to-GLB conversion for in-browser 3D viewing.

### Runtimes

Two container runtimes are supported:

- **Docker** — Classic Docker CLI integration. Best on desktop.
- **PRoot** — A user-space chroot-like tool that runs without root privileges. Used for Android where Docker is unavailable. The app downloads OCI container images, extracts them to a rootfs directory, and launches pipeline commands via PRoot with bind mounts.

### 3D Viewer

The embedded 3D viewer (`app/js/viewer3d.js`) is a Three.js-based application that:
- Loads GLB files generated by the backend's PLY-to-GLB converter
- Supports orbit, pan, and zoom controls
- Toggles between point cloud, mesh, and wireframe rendering
- Includes FXAA antialiasing and GPU clipping
- Provides measurement tools for distance, area, and angle

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | Server port (Docker) | `8080` |
| `IP` | Server bind address (Docker) | `0.0.0.0` |
| `COLMAP_SETTINGS_PATH` | Custom settings file path | Platform-specific |

### Settings

Settings are persisted to a JSON file at platform-specific locations:
- **Linux**: `~/.local/share/colmap_openmvs/settings.json`
- **macOS**: `~/Library/Application Support/colmap_openmvs/settings.json`
- **Windows**: `%APPDATA%/colmap_openmvs/settings.json`
- **Android**: `/data/data/com.github.yeicor.colmap_openmvs_app/files/settings.json`
- **Debug**: `./devstorage/settings.json`

Key settings include projects folder path, PRoot binary directory, image storage directory, default container image tag, and custom mount points.

## Technologies

- **Rust** — Core language (all crates)
- **[Dioxus](https://dioxuslabs.com/)** — UI framework (fullstack, router)
- **[Three.js](https://threejs.org/)** — 3D model viewer
- **[COLMAP](https://colmap.github.io/)** — Structure-from-Motion and Multi-View Stereo
- **[OpenMVS](https://github.com/cdcseacave/openMVS)** — Multi-View Stereo reconstruction
- **[PRoot](https://proot-me.github.io/)** — User-space chroot for Android
- **[Eruda](https://github.com/liriliri/eruda)** — Debug console for mobile
- **[Docker](https://www.docker.com/)** — Container runtime on desktop
