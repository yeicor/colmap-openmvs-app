use crate::server::get_projects;
use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsCamera, BsGear, BsPlusCircle};
use dioxus_free_icons::Icon;
use dioxus_primitives::alert_dialog::{
    AlertDialogAction, AlertDialogActions, AlertDialogContent, AlertDialogRoot, AlertDialogTitle,
};

#[component]
pub fn Projects() -> Element {
    let mut create_project_open = use_signal(|| false);
    let mut create_project_name = use_signal(|| String::new());
    let project_list = use_resource(|| get_projects());
    rsx! {
        link { rel: "stylesheet", href: asset!("/assets/views/projects.css") }
        div {
            id: "projects",
            h1 {
                display: "inline-flex",
                Icon {
                    icon: BsCamera,
                }
                " Projects",
            }
            a {
                onclick: move |_| {
                    create_project_open.set(true);
                },
                Icon {
                    icon: BsPlusCircle,
                }
            }
            AlertDialogRoot {
                open: create_project_open(),
                AlertDialogContent {
                    AlertDialogTitle { "Create New Project" }
                    input {
                        r#type: "text",
                        placeholder: "Project Name",
                        value: "{create_project_name}",
                        oninput: move |evt| {
                            create_project_name.set(evt.value());
                        }
                    }
                    AlertDialogActions {
                        AlertDialogAction {
                            on_click: move |_| {
                                println!("Creating project: {}", create_project_name());
                                create_project_open.set(false);
                            },
                            "Create"
                        }
                        AlertDialogAction {
                            on_click: move |_| {
                                create_project_open.set(false);
                            },
                            "Cancel"
                        }
                    }
                }
            }
            Link {
                to: Route::Settings {},
                Icon {
                    icon: BsGear,
                }
            }
            match &*project_list.read_unchecked() {
                Some(Ok(projects)) => rsx! { ul { for project in projects {
                    li { "{project}" }
                } }},
                Some(Err(e)) => rsx! { p { "{e}" } },
                None =>  rsx! { p { "Loading..." } }
            }
        }
    }
}
