# Photos to 3D Model Offline

**One-click photogrammetry.** Turn photos into 3D models using the latest COLMAP + OpenMVS — no command line needed.


## Features

* **🚀 One-click photogrammetry**: Turn photos into 3D models with a single click. Feature extraction, matching, sparse reconstruction, dense reconstruction, meshing, and texturing are fully automated using the latest COLMAP and OpenMVS.
* **📦 Zero-install, fully offline**: COLMAP and OpenMVS come prepackaged in a container image, with no manual compilation or dependency setup required. Works entirely offline after the initial image download.
* **🖥️ Runs anywhere**: Desktop (Windows, macOS, Linux), Web, and Android — the same app, UI, and reconstruction pipeline across all platforms.
* **🐳 Flexible container runtimes**: Supports both Docker and PRoot. Docker provides the best performance on desktops and servers, while PRoot runs without special privileges and can be downloaded and managed automatically by the app.
* **🖱️ Interactive 3D viewer**: Inspect point clouds, meshes, and textured models directly in your browser. Includes orbit controls, measurement tools (distance and area), wireframe mode, and adjustable lighting.
* **⏱️ Real-time progress tracking**: Follow every reconstruction stage with live logs and progress bars. Tasks continue running even if you navigate away from the page.
* **📂 Built-in output browser**: Explore and export reconstruction results with a tree-based file browser and 3D previews.
* **⚙️ Fully configurable pipeline**: Adjust COLMAP and OpenMVS parameters through a UI generated from each tool’s `--help` output, or inject custom Bash scripts for advanced workflows.
* **🖼️ Image management tools**: Upload photos from any device, batch-resize datasets before processing, and download sample datasets.
* **🔄 Always up to date**: Update to the latest COLMAP and OpenMVS builds with a single click. Nightly container images are published automatically.
* **🐳 Docker-in-Docker support**: Run the app inside a container while transparently orchestrating reconstruction pipelines on the host system.
* **🔒 Privacy-first**: All processing happens locally. Your photos and models never leave your device. The **[web demo](https://yeicor.github.io/colmap-openmvs-app/)** uses pre-reconstructed data and requires no uploads.
* **🆓 Open source**: Released under the MIT License. Contributions are welcome.
* **📄 Precompiled binaries for all platforms**: Just download the [latest release](https://github.com/yeicor/colmap-openmvs-app/releases/latest) or the [nightly builds](https://github.com/yeicor/colmap-openmvs-app/actions/workflows/ci.yml).


## Screenshots

Project management:

<p align="center">
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/projects-page.png" width="200" alt="Projects page" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-images.png" width="200" alt="Project images" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-logs.png" width="200" alt="Real-time logs" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-outputs.png" width="200" alt="Project outputs" />
</p>

3D viewer (dark theme):

<p align="center">
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-pointcloud.png" width="200" alt="Point cloud" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-wireframe.png" width="200" alt="Textured wireframe" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/viewer-textured-mesh.png" width="200" alt="Textured mesh" />
</p>

Pipeline configuration and settings:

<p align="center">
    <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/project-demo-config.png" width="200" alt="Pipeline config" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-general.png" width="200" alt="Settings" />
  <img src="https://yeicor.github.io/colmap-openmvs-app/screenshots/settings-runtime.png" width="200" alt="Runtime settings" />
</p>

Screenshots are automatically captured from the latest [web demo](https://yeicor.github.io/colmap-openmvs-app/), which is rebuilt on every push to `main`.
