use crate::components::button::{Button, ButtonVariant};
use dioxus::document::eval;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum BannerType {
    Error,
    Info,
}

/// A floating notification toast.
///
/// * `message` – display text (empty = hidden).
/// * `banner_type` – `Error` (red) or `Info` (blue).
/// * `on_close` – if provided, a dismiss button is shown and the toast
///   auto-dismisses after 6 seconds by calling this handler.
/// * `progress` – optional `(done, total)` pair shown as a progress indicator.
///   When set the toast does NOT auto-dismiss (stays until explicitly closed
///   or the caller updates the message to empty).
#[component]
pub fn Banner(
    message: String,
    #[props(default = BannerType::Info)] banner_type: BannerType,
    #[props(default)] on_close: Option<EventHandler<()>>,
    #[props(default)] progress: Option<(usize, usize)>,
) -> Element {
    if message.is_empty() {
        return rsx! {};
    }

    // Auto-dismiss info/error toasts after 6 s (but not progress toasts).
    if on_close.is_some() && progress.is_none() {
        let handler = on_close.clone();
        let msg = message.clone();
        use_effect(move || {
            if !msg.is_empty() {
                let handler = handler.clone();
                spawn(async move {
                    let _ = eval("await new Promise(r => setTimeout(r, 6000))").await;
                    if let Some(h) = handler {
                        h.call(());
                    }
                });
            }
        });
    }

    let class = match banner_type {
        BannerType::Error => "error-banner toast-float",
        BannerType::Info => "info-banner toast-float",
    };

    rsx! {
        div {
            class: "{class}",
            div { class: "toast-body",
                span { class: "toast-text", "{message}" }
                if let Some((done, total)) = progress {
                    span { class: "toast-progress-label", "{done}/{total}" }
                }
                if on_close.is_some() {
                    Button {
                        variant: ButtonVariant::Ghost,
                        onclick: move |_| {
                            if let Some(handler) = on_close {
                                handler.call(());
                            }
                        },
                        "×"
                    }
                }
            }
            if let Some((done, total)) = progress {
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
