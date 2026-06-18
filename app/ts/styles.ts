/**
 * CSS styles for the 3D viewer — injected once into the document.
 */
const VIEWER_STYLE_ID = "v3d-style";

export function injectViewerStyles(): void {
  if (document.getElementById(VIEWER_STYLE_ID)) return;

  const css = document.createElement("style");
  css.id = VIEWER_STYLE_ID;
  css.textContent = `
.v3d-toolbar {
  position:absolute;top:14px;left:14px;display:flex;gap:10px;z-index:50;
  flex-wrap:wrap;max-width:calc(100% - 28px);pointer-events:none;
}
.v3d-toolbar-section {
  display:flex;gap:6px;padding:6px;border-radius:14px;
  background:rgba(18,20,26,0.5);backdrop-filter:blur(2px);
  border:1px solid rgba(255,255,255,0.08);pointer-events:auto;
  box-shadow:0 10px 30px rgba(0,0,0,0.24);
}
.v3d-btn {
  border:none;background:rgba(255,255,255,0.07);color:#f3f4f7;
  display:flex;align-items:center;gap:8px;border-radius:10px;
  padding:9px 12px;cursor:pointer;font-size:13px;font-weight:500;
  transition:background 120ms,transform 120ms,opacity 120ms;
}
.v3d-btn:hover { background:rgba(255,255,255,0.16); }
.v3d-btn:active { transform:scale(0.98); }
.v3d-btn.active { background:rgba(78,132,255,0.28);color:#a8c4ff; }
.v3d-btn-icon { font-size:15px; }
.v3d-panel {
  position:fixed;min-width:260px;max-width:340px;border-radius:16px;
  background:rgba(16,18,24,0.5);color:#eef2ff;
  border:1px solid rgba(255,255,255,0.08);box-shadow:0 16px 50px rgba(0,0,0,0.4);
  z-index:10000;overflow:hidden;backdrop-filter:blur(2px);
}
.v3d-panel.hidden { display:none; }
.v3d-panel-header {
  display:flex;align-items:center;justify-content:space-between;
  padding:14px 16px;border-bottom:1px solid rgba(255,255,255,0.06);
}
.v3d-panel-title { font-weight:700;font-size:14px; }
.v3d-close { border:none;background:transparent;color:inherit;cursor:pointer;font-size:18px; }
.v3d-panel-content { padding:14px 16px;display:flex;flex-direction:column;gap:12px; }
.v3d-stat-row,.v3d-help-item { display:flex;justify-content:space-between;gap:20px;font-size:13px; }
.v3d-stat-row span:last-child { font-weight:600; }
.v3d-toast-stack {
  position:absolute;right:16px;bottom:16px;display:flex;flex-direction:column;
  gap:10px;z-index:40;pointer-events:none;
}
.v3d-toast {
  min-width:240px;max-width:360px;border-radius:14px;
  background:rgba(18,20,26,0.92);border:1px solid rgba(255,255,255,0.08);
  color:#f1f4ff;overflow:hidden;pointer-events:auto;
  box-shadow:0 14px 34px rgba(0,0,0,0.35);backdrop-filter:blur(16px);
}
.v3d-toast-header {
  display:flex;justify-content:space-between;align-items:center;
  padding:10px 12px;border-bottom:1px solid rgba(255,255,255,0.06);
  font-size:13px;font-weight:600;
}
.v3d-toast-header button {
  border:none;background:transparent;color:inherit;cursor:pointer;font-size:16px;
}
.v3d-toast-content { padding:12px;font-size:13px;line-height:1.45; }
.v3d-measure-line { color:#9cc2ff; }
canvas { touch-action:none; }
/* Label visibility is now handled by JS (_updateToolbarLabels) so it
   accounts for CSS zoom (e.g. from Playwright screenshots). */
.v3d-compact .v3d-toolbar { gap:8px; }
.v3d-compact .v3d-btn-label { display:none; }
.v3d-compact .v3d-toolbar-section { padding:4px; }
.v3d-compact .v3d-btn { padding:10px; }
.v3d-compact .v3d-panel { width:calc(100vw - 24px);left:12px !important;right:12px;max-width:none; }`;
  document.head.appendChild(css);
}
