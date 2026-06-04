//! Global floating toast system.
//!
//! All components use the toast context to show transient notifications
//! (errors, info messages, progress).  A single [`ToastContainer`] mounted
//! at the app root renders every active toast as a sibling, ensuring they
//! never overlap.

use crate::components::button::{Button, ButtonVariant};
use dioxus::document::eval;
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ToastType {
    Error,
    Info,
}

#[derive(Clone, PartialEq)]
pub struct ToastEntry {
    pub id: String,
    pub message: String,
    pub toast_type: ToastType,
    pub progress: Option<(usize, usize)>,
}

/// Global toast list.  Provide once at the app root.
pub type ToastCtx = Signal<Vec<ToastEntry>>;

// ---------------------------------------------------------------------------
// Context helpers
// ---------------------------------------------------------------------------

/// Provide the toast context (call once at `App` level).
pub fn use_toast_provider() -> ToastCtx {
    use_context_provider(|| Signal::new(Vec::<ToastEntry>::new()))
}

/// Retrieve the toast context from the current scope.
pub fn use_toast_ctx() -> ToastCtx {
    use_context::<ToastCtx>()
}

static NEXT_TOAST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Add a toast notification.  Returns the toast ID so the caller can dismiss
/// it early via [`remove_toast`].
pub fn add_toast(
    ctx: &mut ToastCtx,
    message: String,
    toast_type: ToastType,
    progress: Option<(usize, usize)>,
) -> String {
    let id = NEXT_TOAST_ID
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_string();
    ctx.write().push(ToastEntry {
        id: id.clone(),
        message,
        toast_type,
        progress,
    });
    id
}

/// Dismiss a toast by its ID.
pub fn remove_toast(ctx: &mut ToastCtx, id: &str) {
    ctx.write().retain(|t| t.id != id);
}

/// Update an existing toast's message and/or progress.
/// Pass `None` for a field to leave it unchanged.
pub fn update_toast(
    ctx: &mut ToastCtx,
    id: &str,
    message: Option<String>,
    progress: Option<Option<(usize, usize)>>,
) {
    if let Some(entry) = ctx.write().iter_mut().find(|t| t.id == id) {
        if let Some(msg) = message {
            entry.message = msg;
        }
        if let Some(prog) = progress {
            entry.progress = prog;
        }
    }
}

// ---------------------------------------------------------------------------
// Container component (rendered once at the app root)
// ---------------------------------------------------------------------------

/// Renders all active toasts inside a single fixed-position container so they
/// stack without overlap.  Info/error toasts auto-dismiss after 6 seconds;
/// progress toasts stay until explicitly dismissed.
#[component]
pub fn ToastContainer() -> Element {
    let ctx = use_toast_ctx();
    let toasts = ctx();

    if toasts.is_empty() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "toast-container",
            for entry in toasts.iter() {
                ToastInstance {
                    key: "{entry.id}",
                    entry: entry.clone(),
                    on_close: {
                        let mut ctx = ctx;
                        let id = entry.id.clone();
                        move |_| remove_toast(&mut ctx, &id)
                    },
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: renders a single toast instance with auto-dismiss
// ---------------------------------------------------------------------------

#[component]
fn ToastInstance(entry: ToastEntry, on_close: EventHandler<()>) -> Element {
    // Auto-dismiss info/error toasts after 6 s (but not progress toasts).
    if entry.progress.is_none() {
        let handler = on_close.clone();
        let msg = entry.message.clone();
        use_effect(move || {
            if !msg.is_empty() {
                let handler = handler.clone();
                spawn(async move {
                    let _ = eval("await new Promise(r => setTimeout(r, 6000))").await;
                    handler.call(());
                });
            }
        });
    }

    let class = match entry.toast_type {
        ToastType::Error => "error-banner toast-float",
        ToastType::Info => "info-banner toast-float",
    };

    rsx! {
        div {
            class: "{class}",
            div { class: "toast-body",
                span { class: "toast-text", "{entry.message}" }
                if let Some((done, total)) = entry.progress {
                    span { class: "toast-progress-label", "{done}/{total}" }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    onclick: move |_| on_close.call(()),
                    "×"
                }
            }
            if let Some((done, total)) = entry.progress {
                div {
                    class: "toast-progress-track",
                    div {
                        class: "toast-progress-fill",
                        style: if total > 0 {
                            format!("width: {}%", done as f64 / total as f64 * 100.0)
                        } else {
                            "width: 0%".to_string()
                        },
                    }
                }
            }
        }
    }
}
