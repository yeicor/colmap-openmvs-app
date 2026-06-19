//! Take desktop-viewport screenshots of the web demo for the README.
//!
//! Screenshots are captured at 1080x1920 with CSS zoom applied so content
//! fills the frame.  Dark/light variants are written into dark/ and light/
//! subdirectories.  All routes share a single capture path — only the wait
//! method differs (app-ready vs viewer-model).
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

const VIEWPORT_W = 1080;
const VIEWPORT_H = 1920;

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
    url: "/viewer/demo/openmvs|scene_dense.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjkxMzIsMS4xNDA2LDAuOTg5Nl0sInRhcmdldCI6WzIuNjE4NiwyLjk5NTUsLTMuNjY4XSwidXAiOlstMC4xMjA5LDAuNzk1NywwLjU5MzVdLCJuZWFyIjowLjAwNzIsImZhciI6MTAwMH0sImNvbmZpZyI6eyJ0ZXh0dXJlcyI6dHJ1ZSwid2lyZWZyYW1lIjp0cnVlLCJiYWNrZmFjZXMiOmZhbHNlLCJsaWdodGluZyI6dHJ1ZSwibGlnaHRBemltdXRoIjowLCJsaWdodEVsZXZhdGlvbiI6MCwicG9pbnRzU2l6ZSI6MS41LCJ0b25lTWFwcGluZyI6dHJ1ZSwiZXhwb3N1cmUiOjF9fQ==",
    name: "viewer-pointcloud",
  },
  {
    url: "/viewer/demo/openmvs|scene_dense_mesh_refined_textured.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjkxMzIsMS4xNDA2LDAuOTg5Nl0sInRhcmdldCI6WzIuNjE4NiwyLjk5NTUsLTMuNjY4XSwidXAiOlstMC4xMjA5LDAuNzk1NywwLjU5MzVdLCJuZWFyIjowLjAwNzIsImZhciI6MTAwMH0sImNvbmZpZyI6eyJ0ZXh0dXJlcyI6dHJ1ZSwid2lyZWZyYW1lIjp0cnVlLCJiYWNrZmFjZXMiOmZhbHNlLCJsaWdodGluZyI6dHJ1ZSwibGlnaHRBemltdXRoIjowLCJsaWdodEVsZXZhdGlvbiI6MCwicG9pbnRzU2l6ZSI6MS41LCJ0b25lTWFwcGluZyI6dHJ1ZSwiZXhwb3N1cmUiOjF9fQ==",
    name: "viewer-textured-wireframe",
  },
  {
    url: "/viewer/demo/openmvs|scene_dense_mesh_refined_textured.ply/eyJjYW0iOnsicG9zaXRpb24iOlswLjkxMzIsMS4xNDA2LDAuOTg5Nl0sInRhcmdldCI6WzIuNjE4NiwyLjk5NTUsLTMuNjY4XSwidXAiOlstMC4xMjA5LDAuNzk1NywwLjU5MzVdLCJuZWFyIjowLjAwNzIsImZhciI6MTAwMH0sImNvbmZpZyI6eyJ0ZXh0dXJlcyI6dHJ1ZSwid2lyZWZyYW1lIjpmYWxzZSwiYmFja2ZhY2VzIjpmYWxzZSwibGlnaHRpbmciOnRydWUsImxpZ2h0QXppbXV0aCI6MCwibGlnaHRFbGV2YXRpb24iOjAsInBvaW50c1NpemUiOjEuNSwidG9uZU1hcHBpbmciOnRydWUsImV4cG9zdXJlIjoxfX0=",
    name: "viewer-textured-mesh",
  },
];

// ── Wait helpers ─────────────────────────────────────────────────────────

async function forceTheme(page, theme) {
  const themeValue = theme === "dark" ? "dark" : "light";
  await page.evaluate((t) => {
    document.documentElement.setAttribute("data-theme", t);
    // Also override prefers-color-scheme for any media-query-based logic.
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    Object.defineProperty(mq, "matches", { get: () => t === "dark" });
  }, themeValue);
}

// Increased timeouts for slow-CPU browser (e.g. CI / throttled environments)
async function waitForAppReady(page, timeout = 60000) {
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

async function waitForViewerModel(page, timeout = 120000) {
  const started = Date.now();

  // ── Phase 1: wait until the viewer object and model data exist ────
  while (Date.now() - started < timeout) {
    const ready = await page.evaluate(() => {
      try {
        const v = window.__viewer3d_instance;
        if (!v || !v.modelRoot || !v.controls) return false;
        const stats = v.stats || {};
        if (stats.vertices === undefined || stats.vertices === 0) return false;
        // _initialized is set at the end of setModel(), after the first
        // render has been submitted.
        if (!v._initialized) return false;
        // Container readiness is set independently by both the viewer
        // class and the Rust-mounted JS.
        if (v.container && v.container.dataset.ready !== "true") return false;
        return true;
      } catch {
        return false;
      }
    });
    if (ready) break;
    await page.waitForTimeout(500);
  }

  if (Date.now() - started >= timeout) {
    console.warn(`  ⚠  3D viewer didn't load within ${timeout}ms — capturing anyway`);
    return false;
  }

  // ── Phase 2: guarantee at least one fully-composited frame ────────
  // requestAnimationFrame only fires after the browser has composited
  // the *previous* frame.  So waiting for two consecutive rAFs proves
  // that the forced render from setModel() and at least one subsequent
  // rAF-driven render have been fully composited on the GPU.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

  // "Small" safety margin for any in-flight presentation work (needed on CI only).
  await page.waitForTimeout(30000);
  return true;
}

// ── Screenshot capture ──────────────────────────────────────────────────
// All routes share the same capture path: navigate from about:blank (to
// force a fresh page load for hash-routed SPA pages), force the theme,
// wait for content, scale up via CSS zoom, then take a full-page screenshot.
// Only the wait method differs between viewer and non-viewer routes.

async function captureForTheme(page, route, theme) {
  const themeDir = theme === "dark" ? "dark" : "light";
  const outputFilename = `${route.name}.jpg`;
  const outputPath = path.join(outputDir, themeDir, outputFilename);
  const fullUrl = `http://localhost:${PORT}${basePath}/index.html#${route.url}`;

  console.log(`  🎨 ${theme} …`);

  try {
    // Navigate from about:blank so hash-only route changes always perform a
    // full page load — this avoids stale WASM/WebGL state between routes.
    await page.goto("about:blank");
    await page.goto(fullUrl, { waitUntil: "domcontentloaded", timeout: 60000 });
    await forceTheme(page, theme);

    // Scale content up to fill the larger viewport so screenshots
    // are not mostly empty space.
    await page.evaluate(() => {
      const scale = Math.min(window.innerWidth / 390, window.innerHeight / 844);
      document.documentElement.style.zoom = scale;
    });

    if (route.url.startsWith("/viewer/")) {
      const viewerReady = await waitForViewerModel(page);
      console.log(viewerReady ? `  🎯 Model loaded` : `  ⚠ Model not loaded`);
    } else {
      await waitForAppReady(page);
    }

    await page.screenshot({ path: outputPath, type: "jpeg", quality: 90 });
    const stats = fs.statSync(outputPath);
    console.log(`  ✅ ${outputFilename}  (${(stats.size / 1024).toFixed(1)} KB)`);
  } catch (err) {
    // Save a debug screenshot on failure to help diagnose the issue.
    try {
      const debugPath = path.join(outputDir, themeDir, `${route.name}-debug.png`);
      await page.screenshot({ path: debugPath, type: "png" });
      console.log(`  📸 Debug screenshot saved as ${path.basename(debugPath)}`);
    } catch {
      /* ignore */
    }
    throw err;
  }
}

// ── Main ─────────────────────────────────────────────────────────────────

async function main() {
  fs.mkdirSync(path.join(outputDir, "dark"), { recursive: true });
  fs.mkdirSync(path.join(outputDir, "light"), { recursive: true });

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
    // Override the browser-level prefers-color-scheme so initial renders
    // match the theme we'll set via data-theme. This avoids a flash of
    // wrong-themed content before the JS-based forceTheme() runs.
    colorScheme: "light",
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

    try {
      await captureForTheme(page, route, "light");
      await captureForTheme(page, route, "dark");
      results.push({ ...route, success: true });
    } catch (err) {
      console.error(`  ❌ ${err.message}`);
      results.push({ ...route, success: false, error: err.message });
    }
  }

  await browser.close();
  await new Promise((resolve) => server.close(resolve));

  const ok = results.filter((r) => r.success).length;
  const fail = results.filter((r) => !r.success).length;
  console.log(`\n${`─`.repeat(40)}`);
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
