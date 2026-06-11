# syntax=docker/dockerfile:1
#
# Take reproducible phone-form-factor screenshots of the web demo.
#
# The Playwright base image provides pinned Chromium + SwiftShader for
# consistent WebGL rendering across any host environment.  Font packages
# are pinned so text metrics do not vary between systems.
#
# The web demo public directory is baked into the image at build time so
# that screenshots are fully deterministic — every run uses the exact same
# WASM binary, assets, and embedded demo data regardless of the host.
#
# Build (from repo root):
#   1. Build the web demo:
#        dx bundle --web --release --verbose --no-default-features --features=demo
#   2. Copy the public dir into the Docker context:
#        cp -r target/dx/colmap-openmvs-app/release/web/public .github/scripts/screenshots/public
#   3. Build:
#        docker build -t colmap-screenshots .github/scripts/screenshots
#
# Run (output directory is mounted so screenshots survive container exit):
#   docker run --rm \
#     -v "$PWD/screenshots:/app/screenshots" \
#     colmap-screenshots
#
# To override the public directory, base path, or output location:
#   docker run --rm \
#     -v /path/to/custom-public:/custom-public:ro \
#     -v "$PWD/output:/output" \
#     colmap-screenshots \
#       /custom-public /output /custom-base-path

ARG PLAYWRIGHT_VERSION=1.60.0

# Official Playwright Docker image — ships pinned Chromium, system deps,
# fonts, and SwiftShader for reliable headless WebGL.
FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble

# Re-declare so the value survives into this build stage.
ARG PLAYWRIGHT_VERSION

WORKDIR /app

# Install the matching Playwright npm package.  The browsers are already
# pre-installed in the base image at /ms-playwright/; this package
# provides the Node.js API to drive them.
RUN npm install playwright@"${PLAYWRIGHT_VERSION}" \
    && npm cache clean --force

# Install additional font packages for consistent text rendering across
# environments.  The base image has basic fonts; these ensure CJK and
# emoji glyphs are available for any locale.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        fonts-dejavu-core \
        fonts-noto \
        fonts-noto-cjk \
        fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/*

# Copy the screenshot script.
COPY screenshots.mjs .

# Copy the web demo public directory (built separately with `dx bundle`).
# This directory must exist at build time — it contains the WASM binary,
# JS bundle, CSS, and embedded demo data used for screenshot capture.
COPY public /app/public

ENTRYPOINT ["node", "/app/screenshots.mjs"]

# Default arguments:  public-dir  output-dir     base-path
CMD ["/app/public", "/app/screenshots", "/colmap-openmvs-app"]
