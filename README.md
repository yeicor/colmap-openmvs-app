# Photos to 3D Model Offline


## Features

### 🚀 Photogrammetry Pipeline

- **One-click reconstruction** — Automate the full pipeline: feature extraction, matching, sparse reconstruction, dense reconstruction, meshing, and texturing. Powered by **COLMAP** (state-of-the-art SfM) and **OpenMVS** (multi-view stereo).
- **Live progress tracking** — Follow every stage with real-time logs and progress bars. Pipelines keep running even if you navigate away, and you can cancel at any time.
- **Smart caching** — Completed pipeline stages are automatically cached. Re-run with confidence — only the changed steps are recomputed.
- **Dry-run & log recovery** — Replay logs from a previous run without re-executing, useful for debugging or sharing results.

### 🖱️ 3D Viewer

- **In-browser model inspection** — View point clouds, meshes, and textured models directly without external tools.
- **Full interaction** — Orbit, pan, zoom controls, distance and area measurement tools, wireframe overlay, and adjustable lighting.
- **Auto-conversion** — PLY point clouds and meshes are automatically converted to glTF/GLB for seamless browser rendering.
- **Deep linking** — Link directly to a specific model with a viewport camera preset.

### ⚙️ Pipeline Configuration

- **Visual parameter editor** — A UI generated automatically from each tool's `--help` output lets you tweak every COLMAP and OpenMVS parameter without touching the command line.
- **Custom scripts** — Inject arbitrary Bash code before or instead of the pipeline for advanced workflows.
- **Low-resource presets** — Android devices get conservative defaults tuned for stability on mobile hardware.

### 🖼️ Image Management

- **Upload from anywhere** — Add photos from any device via the browser's file picker.
- **Batch resize** — Resize all images in a project to a max dimension in one click, keeping file sizes manageable.
- **Sample datasets** — Download built-in demo datasets (ET, Kermit) to test the pipeline immediately.
- **Thumbnail gallery** — Browse project images with cached thumbnail previews.

### 📁 Output Browser & Export

- **Tree-based file explorer** — Navigate COLMAP's directory structure (sparse models, dense point clouds, meshes, textures) with a familiar tree view.
- **3D preview** — Click any viewable file (`.ply`, `points3D.bin`) to inspect it instantly in the 3D viewer.
- **ZIP backup & restore** — Download any output folder as a ZIP archive, or upload one to restore.
- **Selective cleanup** — Delete individual files or clear all outputs (preserving images and config) with a single click.

### 🐳 Deployment & Runtime

- **Zero-install COLMAP/OpenMVS** — Both tools come pre-packaged in a container image. No compilation or dependency setup needed.
  - **Automated updates** — Powered by [colmap-openmvs](https://github.com/yeicor-docker/colmap-openmvs), which automatically rebuilds container images whenever COLMAP or OpenMVS update.
- **Container runtimes:**
  - **Docker** — Best performance on desktops and servers, but requires the Docker daemon to be installed and running.
    - **Docker-in-Docker** — Run the app inside a container while it transparently orchestrates pipelines on the host.
  - **PRoot** — No special privileges required. Auto-downloaded and managed by the app; ideal for Android and restricted environments.
- **Hardware support:**
  - **CPU** — Runs the full pipeline end-to-end on any system.
  - **CUDA** — Mount host GPU drivers for hardware-accelerated reconstruction.

### 🖥️ Cross-Platform

- **Desktop** — Native bundles for Windows (`.exe`), macOS (`.dmg`), and Linux (`.AppImage`).
- **Web** — Full-stack web app, deployable behind any reverse proxy.
- **Android** — APK builds with automatic PRoot runtime setup.
- **Same code, same UI** — The exact same application runs everywhere.

### 🔧 System Tools

- **Background task manager** — A persistent side panel shows all running, completed, and failed tasks with expandable logs and progress bars.
- **Settings UI** — Configure projects folder, runtime paths, default container image, custom mounts, and theme override from an in-app settings page.
- **System startup** — Platform-specific initialization (Android runtime setup, path validation) runs automatically on boot.
- **Dark/light theme** — Automatic theme detection with manual override.
- **Separable back-end** — The frontend can connect to a remote server by configuring the backend URL.

### 🔒 Privacy & Licensing

- **100% offline** — After the initial container image download, all processing is local. Your photos and models never leave your device.
- **Web demo** — The live demo uses pre-reconstructed data and requires no uploads. See [screenshots](#screenshots) below or [simply try it out](https://yeicor.github.io/colmap-openmvs-app/).
- **MIT License** — Free to use, modify, and distribute. Contributions welcome.


## Screenshots

Project management:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/projects-page-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/projects-page-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/projects-page-light.png" width="200" alt="Projects page" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-images-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-images-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-images-light.png" width="200" alt="Project images" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-logs-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-logs-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-logs-light.png" width="200" alt="Real-time logs" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-outputs-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-outputs-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-outputs-light.png" width="200" alt="Project outputs" />
  </picture>
</p>

3D viewer:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-pointcloud-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-pointcloud-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-pointcloud-light.png" width="200" alt="Point cloud" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-wireframe-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-wireframe-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-wireframe-light.png" width="200" alt="Textured wireframe" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-mesh-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-mesh-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-mesh-light.png" width="200" alt="Textured mesh" />
  </picture>
</p>

Pipeline configuration and settings:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-config-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-config-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-config-light.png" width="200" alt="Pipeline config" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-general-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-general-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-general-light.png" width="200" alt="Settings" />
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-runtime-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-runtime-light.png" />
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-runtime-light.png" width="200" alt="Runtime settings" />
  </picture>
</p>

Screenshots are automatically captured from the latest [web demo](https://yeicor.github.io/colmap-openmvs-app/), which is rebuilt on every push to `main`.
