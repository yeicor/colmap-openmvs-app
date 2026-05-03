use crate::components::button::{Button, ButtonVariant};
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum BannerType {
    Error,
    Info,
}

#[component]
pub fn Banner(
    message: String,
    #[props(default = BannerType::Info)] banner_type: BannerType,
    #[props(default)] on_close: Option<EventHandler<()>>,
) -> Element {
    if message.is_empty() {
        return rsx! {};
    }

    let class = match banner_type {
        BannerType::Error => "error-banner",
        BannerType::Info => "info-banner",
    };

    rsx! {
        div {
            class: "{class}",
            "{message}",
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
    }
}
