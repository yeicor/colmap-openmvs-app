//! Take desktop-viewport screenshots of the web demo for the README.
//!
//! Usage (Docker - recommended):
//!   docker build -t colmap-screenshots .github/scripts/screenshots
//!   docker run --rm \
//!     -v /path/to/public-dir:/public:ro \
//!     -v /path/to/output-dir:/output \
//!     colmap-screenshots \
//!       /public /output /colmap-openmvs-app
//!
//! Usage (direct, requires playwright installed):
//!   node .github/scripts/screenshots/screenshots.mjs <public-dir> <output-dir> [base-path]
//!
//!   <public-dir>   Path to the built web demo root (default: ./public)
//!   <output-dir>   Directory to write screenshots (default: ./screenshots)
//!   <base-path>    Base path prefix used in asset URLs (default: /colmap-openmvs-app)
//!                  Pass "/" (or empty) for local builds without a base_path set.
//!
//! Requires: playwright (install with: npx playwright install chromium)

import { chromium } from "playwright";
import http from "http";
import fs from "fs";
import path from "path";

const [, , publicDir = "./public", outputDir = "./screenshots", basePathArg = "/colmap-openmvs-app"] = process.argv;

// Normalise base path: "/" and "" both mean "serve at root, no stripping".
const basePath = basePathArg === "/" || basePathArg === "" ? "" : basePathArg;

const VIEWPORT_W = 390;
const VIEWPORT_H = 844;

const PORT = 9876;

// ── MIME types ────────────────────────────────────────────────────────────
const MIME_TYPES = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".wasm": "application/wasm",
  ".css": "text/css",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ico": "image/x-icon",
};

// ── Static HTTP server ──────────────────────────────────────────────────
const serverRoot = path.resolve(publicDir);

function serveFile(res, filePath) {
  const ext = path.extname(filePath);
  const contentType = MIME_TYPES[ext] || "application/octet-stream";
  try {
    const content = fs.readFileSync(filePath);
    res.writeHead(200, { "Content-Type": contentType });
    res.end(content);
  } catch {
    res.writeHead(404);
    res.end("Not Found");
  }
}

const server = http.createServer((req, res) => {
  const parsedUrl = new URL(req.url, `http://localhost:${PORT}`);
  let urlPath = parsedUrl.pathname;

  if (basePath && urlPath.startsWith(basePath)) {
    urlPath = urlPath.slice(basePath.length);
  }

  if (urlPath === "" || urlPath === "/") {
    urlPath = "/index.html";
  }

  const filePath = path.join(serverRoot, urlPath);

  if (!filePath.startsWith(serverRoot)) {
    res.writeHead(403);
    res.end("Forbidden");
    return;
  }

  if (fs.existsSync(filePath) && fs.statSync(filePath).isFile()) {
    serveFile(res, filePath);
  } else {
    const indexPath = path.join(serverRoot, "index.html");
    if (fs.existsSync(indexPath)) {
      serveFile(res, indexPath);
    } else {
      res.writeHead(404);
      res.end("Not Found");
    }
  }
});

// ── Routes ──────────────────────────────────────────────────────────────
// Each route has a hash URL path and a screenshot filename (without extension).
// Viewer page URLs embed the render config (base64-encoded JSON) after the model
// path — no extra route properties needed.
const routes = [
  // App pages
  { url: "/", name: "projects-page" },
  { url: "/settings", name: "settings-general" },
  { url: "/settings/runtime", name: "settings-runtime" },
  { url: "/project/demo/images", name: "project-demo-images" },
  { url: "/project/demo/config", name: "project-demo-config" },
  { url: "/project/demo/logs", name: "project-demo-logs" },
  { url: "/project/demo/outputs", name: "project-demo-outputs" },
  {
    url: "/viewer/demo/openmvs|scene_dense.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjYzNTcsLTAuMzgzNSwtMi4xMzhdLCJ0YXJnZXQiOlswLjAwODMsLTIuNDcxNCw0LjMzNzddLCJ1cCI6Wy0wLjA5NDUsLTAuOTQ0OCwtMC4zMTM4XSwibmVhciI6MC4wMDY4LCJmYXIiOjEwMDB9LCJjb25maWciOnsidGV4dHVyZXMiOnRydWUsIndpcmVmcmFtZSI6ZmFsc2UsImJhY2tmYWNlcyI6ZmFsc2UsImxpZ2h0aW5nIjpmYWxzZSwibGlnaHRBemltdXRoIjowLCJsaWdodEVsZXZhdGlvbiI6MzAsInBvaW50c1NpemUiOjEuNSwidG9uZU1hcHBpbmciOnRydWUsImV4cG9zdXJlIjowLjd9fQ==",
    name: "viewer-pointcloud",
  },
  {
    url: "/viewer/demo/openmvs|scene_dense_mesh_refined_textured.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjYzNTcsLTAuMzgzNSwtMi4xMzhdLCJ0YXJnZXQiOlswLjAwODMsLTIuNDcxNCw0LjMzNzddLCJ1cCI6Wy0wLjA5NDUsLTAuOTQ0OCwtMC4zMTM4XSwibmVhciI6MC4wMDY4LCJmYXIiOjEwMDB9LCJjb25maWciOnsidGV4dHVyZXMiOnRydWUsIndpcmVmcmFtZSI6dHJ1ZSwiYmFja2ZhY2VzIjpmYWxzZSwibGlnaHRpbmciOnRydWUsImxpZ2h0QXppbXV0aCI6MCwibGlnaHRFbGV2YXRpb24iOjMwLCJwb2ludHNTaXplIjoyLjIsInRvbmVNYXBwaW5nIjp0cnVlLCJleHBvc3VyZSI6MX19",
    name: "viewer-textured-wireframe",
  },
  {
    url: "/viewer/demo/openmvs|scene_dense_mesh_refined_textured.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjYzNTcsLTAuMzgzNSwtMi4xMzhdLCJ0YXJnZXQiOlswLjAwODMsLTIuNDcxNCw0LjMzNzddLCJ1cCI6Wy0wLjA5NDUsLTAuOTQ0OCwtMC4zMTM4XSwibmVhciI6MC4wMDY4LCJmYXIiOjEwMDB9LCJjb25maWciOnsidGV4dHVyZXMiOnRydWUsIndpcmVmcmFtZSI6ZmFsc2UsImJhY2tmYWNlcyI6ZmFsc2UsImxpZ2h0aW5nIjp0cnVlLCJsaWdodEF6aW11dGgiOjAsImxpZ2h0RWxldmF0aW9uIjozMCwicG9pbnRzU2l6ZSI6Mi4yLCJ0b25lTWFwcGluZyI6dHJ1ZSwiZXhwb3N1cmUiOjF9fQ==",
    name: "viewer-textured-mesh",
  },
];

// ── Wait helpers ─────────────────────────────────────────────────────────

async function waitForAppReady(page, timeout = 25000) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    const markers = ["#dx-toast-template", "[data-dioxus-id]", "nav", "aside", "main", "button"];
    for (const sel of markers) {
      if ((await page.locator(sel).count()) > 0) {
        await page.waitForTimeout(800);
        return true;
      }
    }
    const title = await page.title();
    if (title !== "dioxus | ⛺" && !title.includes("dioxus")) {
      await page.waitForTimeout(800);
      return true;
    }
    await page.waitForTimeout(300);
  }
  console.warn(`  ⚠  App didn't fully render within ${timeout}ms — capturing anyway`);
  return false;
}

async function waitForViewerModel(page, timeout = 60000) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    const ready = await page.evaluate(() => {
      try {
        const v = window.__viewer3d_instance;
        // Check that modelRoot exists AND has at least one child with
        // geometry (vertices > 0).  This avoids false-positives where the
        // viewer object exists but the model data never arrived or rendered.
        if (!v || !v.modelRoot || !v.controls) return false;
        const stats = v.stats || {};
        if (stats.vertices === undefined || stats.vertices === 0) return false;
        return true;
      } catch {
        return false;
      }
    });
    if (ready) {
      // Give the renderer time to composite after the forced render + at
      // least one animation frame.
      await page.waitForTimeout(2000);
      return true;
    }
    await page.waitForTimeout(500);
  }
  console.warn(`  \u26a0  3D viewer didn't load within ${timeout}ms \u2014 capturing anyway`);
  return false;
}

// ── Main ─────────────────────────────────────────────────────────────────

async function main() {
  fs.mkdirSync(outputDir, { recursive: true });

  await new Promise((resolve, reject) => {
    server.listen(PORT, () => {
      console.log(`🌐 Server at http://localhost:${PORT}${basePath}/`);
      console.log(`📂 Serving: ${serverRoot}`);
      resolve();
    });
    server.on("error", reject);
  });

  const browser = await chromium.launch({
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-setuid-sandbox",
      "--use-gl=angle",
      "--use-angle=swiftshader",
      "--ignore-gpu-blocklist",
      "--enable-unsafe-swiftshader",
    ],
  });

  const context = await browser.newContext({
    viewport: { width: VIEWPORT_W, height: VIEWPORT_H },
  });

  const page = await context.newPage();

  // Relay ALL browser console messages for debuggability, especially
  // useful for diagnosing 3D viewer / WebGL failures on headless CI.
  page.on("console", (msg) => {
    console.log(`  [browser:${msg.type()}] ${msg.text()}`);
  });
  page.on("pageerror", (err) => {
    console.log(`  [browser:uncaught] ${err.message}`);
  });

  const results = [];

  for (const route of routes) {
    console.log(`\n📸 ${route.name} …`);

    const fullUrl = `http://localhost:${PORT}${basePath}/index.html#${route.url}`;

    try {
      if (route.url.startsWith("/viewer/")) {
        // Viewer pages: navigate from about:blank so the browser performs a
        // full page load (hash-only navigation is ignored by the 3D viewer,
        // leaving WebGL/WASM state stale between routes).
        await page.goto("about:blank");
        await page.goto(fullUrl, { waitUntil: "domcontentloaded", timeout: 20000 });

        // The demo manifest has dark_mode:null, so the app does not set
        // data-theme on its own and leaves it to the browser's
        // prefers-color-scheme media query (which defaults to dark in some
        // headless environments).  Force light mode so the viewer page CSS
        // (html[data-theme="light"] .viewer-page) takes effect.
        await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));

        const viewerReady = await waitForViewerModel(page);
        console.log(viewerReady ? "  🎯 Model loaded" : "  ⚠ Model not loaded");

        // Log the actual camera state after restoration for cross-environment
        // comparison (helps debug CI-vs-local camera transform differences).
        const camState = await page.evaluate(() => {
          const v = window.__viewer3d_instance;
          if (!v) return null;
          const s = v._getCameraState();
          return { position: s.position, target: s.target, up: s.up };
        });
        if (camState) {
          console.log(
            `  camera: pos=[${camState.position.map((v) => v.toFixed(4)).join(", ")}] target=[${camState.target.map((v) => v.toFixed(4)).join(", ")}] up=[${camState.up.map((v) => v.toFixed(4)).join(", ")}]`,
          );
        }
      } else {
        await page.goto(fullUrl, { waitUntil: "domcontentloaded", timeout: 20000 });

        // The demo manifest has dark_mode:null, so the app does not set
        // data-theme on its own and leaves it to the browser's
        // prefers-color-scheme media query (which defaults to dark in some
        // headless environments).  Force light mode for consistent screenshots.
        await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));

        await waitForAppReady(page);
      }

      const outputPath = path.join(outputDir, `${route.name}.png`);

      if (route.url.startsWith("/viewer/")) {
        // Playwright's page.screenshot() does not reliably capture WebGL
        // canvas content rendered with SwiftShader on headless CI runners.
        // The GPU compositing pipeline may skip the canvas, producing a
        // near-empty image (10-20 KB) instead of the actual 3D content
        // (>=50 KB).
        //
        // We work around this by taking a Playwright element screenshot of
        // the <canvas> itself — this reads from the already-composited
        // layer at the GPU level rather than through the page compositor.
        const canvas = page.locator(".viewer-canvas-container canvas, #viewer3d-container canvas").first();
        const canvasCount = await canvas.count();
        if (canvasCount > 0) {
          await canvas.screenshot({ path: outputPath });
          const stats = fs.statSync(outputPath);
          console.log(`  \u2705 ${route.name}.png  (element-screenshot, ${(stats.size / 1024).toFixed(1)} KB)`);
        } else {
          // No dedicated canvas found — grab the full page as a fallback.
          await page.screenshot({ path: outputPath });
          const stats = fs.statSync(outputPath);
          console.log(`  \u2705 ${route.name}.png  (full-page-fallback, ${(stats.size / 1024).toFixed(1)} KB)`);
        }
      } else {
        await page.screenshot({ path: outputPath });
        const stats = fs.statSync(outputPath);
        console.log(`  \u2705 ${route.name}.png  (${(stats.size / 1024).toFixed(1)} KB)`);
      }
      results.push({ ...route, success: true });
    } catch (err) {
      console.error(`  ❌ ${err.message}`);
      try {
        const outputPath = path.join(outputDir, `${route.name}.png`);
        await page.screenshot({ path: outputPath });
        console.log(`  📸 Debug screenshot saved`);
      } catch {
        /* ignore */
      }
      results.push({ ...route, success: false, error: err.message });
    }
  }

  await browser.close();
  await new Promise((resolve) => server.close(resolve));

  const ok = results.filter((r) => r.success).length;
  const fail = results.filter((r) => !r.success).length;
  console.log(`\n${"─".repeat(40)}`);
  console.log(`📊 Screenshots: ${ok} succeeded, ${fail} failed`);

  if (fail > 0) {
    for (const r of results) {
      if (!r.success) console.log(`  ❌ ${r.name} — ${r.error}`);
    }
    process.exit(1);
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
