use dioxus::prelude::*;
use dioxus_primitives::ContentSide;

use crate::components::{
    button::{Button, ButtonVariant},
    tooltip::{Tooltip, TooltipContent, TooltipTrigger},
};

#[component]
pub fn PageHeader(
    title: String,
    #[props(default)] no_left_margin: bool,
    #[props(default)] icon: Option<Element>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "page-header",
            h1 {
                class: if no_left_margin { "no-left-margin" } else { "" },
                if let Some(icon_value) = icon {
                    {icon_value}
                }
                "{title}"
            }
            {children}
        }
    }
}

#[component]
pub fn PageHeaderButton(
    #[props] icon: Element,
    #[props] extra: Element,
    #[props] extra_tooltip: Option<Element>,
    onclick: EventHandler<()>,
) -> Element {
    rsx! {
        Tooltip {
            TooltipTrigger {
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
            TooltipContent { side: ContentSide::Bottom, { extra_tooltip.unwrap_or(extra) } }
        }
    }
}

#[component]
pub fn BackButton(onclick: EventHandler<()>) -> Element {
    rsx! {
        PageHeaderButton {
            icon: rsx! { "←" },
            extra: rsx! { "Back" },
            extra_tooltip: rsx! { "Go back to the project list" },
            onclick,
        }
    }
}
