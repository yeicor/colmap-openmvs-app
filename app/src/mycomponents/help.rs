use crate::components::alert_dialog::{
    AlertDialogContent, AlertDialogDescription, AlertDialogRoot,
};
use dioxus::prelude::*;

#[component]
pub fn Help(help_text: String, mut open: Signal<bool>) -> Element {
    rsx! {
        AlertDialogRoot {
            open: open().then_some(true),
            on_open_change: move |is_open: bool| {
                open.set(is_open);
            },
            AlertDialogContent {
                div {
                    class: "help-modal-description-wrapper",
                    button {
                        class: "help-modal-close-x",
                        aria_label: "Close",
                        onclick: move |_| {
                            open.set(false);
                        },
                        "X"
                    }
                    AlertDialogDescription {
                        pre {
                            code {
                                class: "help-modal-code",
                                "{help_text}"
                            }
                        }
                    }
                }
            }
        }
    }
}
