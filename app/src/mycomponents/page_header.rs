use dioxus::prelude::*;

use crate::components::button::{Button, ButtonVariant};

#[component]
pub fn PageHeader(
    title: String,
    #[props(default)] no_left_margin: bool,
    #[props(default)] subtitle: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "page-header",
            div { class: "page-header-title-group",
                h1 {
                    class: if no_left_margin { "no-left-margin" } else { "" },
                    "{title}"
                }
                if let Some(ref sub) = subtitle {
                    span { class: "page-header-subtitle", "{sub}" }
                }
            }
            {children}
        }
    }
}

#[component]
pub fn PageHeaderButton(
    #[props] icon: Element,
    #[props] extra: Element,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        Button {
            variant: ButtonVariant::Ghost,
            onclick: move |_| onclick.call(()),
            {icon}
            span {
                class: "btn-label",
                {extra.clone()}
            }
        }
    }
}

#[component]
pub fn BackButton(onclick: EventHandler<()>) -> Element {
    rsx! {
        PageHeaderButton {
            icon: rsx! { "←" },
            extra: rsx! { "Back" },
            onclick,
        }
    }
}
