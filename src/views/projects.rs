use crate::components::{
    alert_dialog::{
        AlertDialogAction, AlertDialogActions, AlertDialogCancel, AlertDialogContent,
        AlertDialogRoot, AlertDialogTitle,
    },
    button::{Button, ButtonVariant},
    tooltip::{Tooltip, TooltipContent, TooltipTrigger},
};
use crate::server::{create_project, delete_project, get_projects, rename_project};
use crate::Route;
use dioxus::{document::eval, prelude::*};
use dioxus_free_icons::icons::bs_icons::{BsGear, BsPencil, BsPlusCircle, BsTrash};
use dioxus_free_icons::Icon;
use dioxus_primitives::ContentSide;

#[derive(Clone, Copy, PartialEq)]
enum DialogType {
    Create,
    Rename(usize),
    Delete(usize),
}
use crate::components::sidebar::{Sidebar as BaseSidebar, SidebarProvider, SidebarTrigger};

#[component]
pub fn Sidebar() -> Element {
    rsx! {
        SidebarProvider {
            SidebarTrigger {}
            BaseSidebar {
                SidebarTrigger {}
                Projects {}
            }
        }
        Outlet::<Route> {}
    }
}

#[component]
pub fn Projects() -> Element {
    let mut dialog_type = use_signal(|| None::<DialogType>);
    let mut input_value = use_signal(|| String::new());
    let mut error_message = use_signal(|| String::new());
    let mut info_message = use_signal(|| String::new());
    let mut projects = use_signal(|| Vec::new());
    let mut loading = use_signal(|| true);

    let refresh_projects = move || {
        spawn({
            async move {
                loading.set(true);
                match get_projects().await {
                    Ok(proj_list) => projects.set(proj_list),
                    Err(e) => error_message.set(format!("Failed to load projects: {}", e)),
                }
                loading.set(false);
            }
        });
    };

    use_effect(refresh_projects);

    let handle_confirm = move |_| {
        spawn({
            async move {
                if let Some(dialog) = dialog_type() {
                    let name = input_value().trim().to_string();
                    if name.is_empty() && !matches!(dialog, DialogType::Delete(_)) {
                        error_message.set("Project name cannot be empty".to_string());
                        return;
                    }

                    let result = match dialog {
                        DialogType::Create => create_project(name).await.and_then(|_| Ok(())),
                        DialogType::Rename(idx) => rename_project(
                            projects()
                                .get(idx)
                                .expect("Invalid project index")
                                .name
                                .clone(),
                            name,
                        )
                        .await
                        .and_then(|_| Ok(())),
                        DialogType::Delete(idx) => {
                            delete_project(
                                projects()
                                    .get(idx)
                                    .expect("Invalid project index")
                                    .name
                                    .clone(),
                            )
                            .await
                        }
                    };

                    match result {
                        Ok(_) => {
                            dialog_type.set(None);
                            info_message.set(
                                match dialog {
                                    DialogType::Create => "Project created successfully",
                                    DialogType::Rename(_) => "Project renamed successfully",
                                    DialogType::Delete(_) => "Project deleted successfully",
                                }
                                .to_string(),
                            );
                            refresh_projects();
                        }
                        Err(e) => error_message.set(e.to_string()),
                    }
                }
            }
        });
    };

    let dialog_title = match dialog_type() {
        Some(DialogType::Create) => "Create New Project".to_string(),
        Some(DialogType::Rename(idx)) => projects()
            .get(idx)
            .map_or("Rename Project".to_string(), |project| {
                format!("Rename \"{}\"", project.name)
            }),
        Some(DialogType::Delete(idx)) => projects()
            .get(idx)
            .map_or("Delete Project?".to_string(), |project| {
                format!("Delete \"{}\"?", project.name)
            }),
        None => String::new(),
    };

    let show_input = !matches!(dialog_type(), Some(DialogType::Delete(_)));
    let is_open = dialog_type().is_some();

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/projects.css") }

        div {
            id: "projects",
            div {
                class: "header",
                h1 { "Projects" }
                div {
                    class: "header-actions",
                    Tooltip {
                        TooltipTrigger {
                            Button {
                                variant: ButtonVariant::Ghost,
                                onclick: move |_| {
                                    input_value.set(String::new());
                                    error_message.set(String::new());
                                    info_message.set(String::new());
                                    dialog_type.set(Some(DialogType::Create));
                                },
                                Icon { icon: BsPlusCircle }
                                span { class: "btn-text", "New" }
                            }
                        }
                        TooltipContent { side: ContentSide::Bottom, "Create a new project" }
                    }
                    Tooltip {
                        TooltipTrigger {
                            Link {
                                to: Route::Settings {},
                                Button {
                                    onclick: move |_| { eval("document.querySelector('.dx-sidebar-trigger').click()"); },
                                    variant: ButtonVariant::Ghost,
                                    Icon { icon: BsGear }
                                    span { class: "btn-text", "Settings" }
                                }
                            }
                        }
                        TooltipContent { side: ContentSide::Bottom, "Settings" }
                    }
                }
            }

            if !error_message().is_empty() {
                div {
                    class: "error-banner",
                    "{error_message}"
                    Button {
                        variant: ButtonVariant::Ghost,
                        onclick: move |_| error_message.set(String::new()),
                        "×"
                    }
                }
            } else if !info_message().is_empty() {
                div {
                    class: "info-banner",
                    "{info_message}"
                    Button {
                        variant: ButtonVariant::Ghost,
                        onclick: move |_| info_message.set(String::new()),
                        "×"
                    }
                }
            }

            if loading() {
                p { class: "loading", "Loading projects..." }
            } else if projects().is_empty() {
                p { class: "empty", "No projects yet. Click + to create one." }
            } else {
                ul {
                    class: "project-list",
                    for (idx, project) in projects().iter().enumerate() {
                        li {
                            key: "{project.name}",
                            Link {
                                to: Route::Project { name: project.name.clone() },
                                class: "project-name-link",
                                div {
                                    onclick: move |_| { eval("document.querySelector('.dx-sidebar-trigger').click()"); },
                                    class: "project-item",
                                    span {
                                        class: "project-name",
                                        "{project.name}"
                                    }
                                    div {
                                        class: "project-actions",
                                        Tooltip {
                                            TooltipTrigger {
                                                Button {
                                                    variant: ButtonVariant::Secondary,
                                                    onclick: move |_| {
                                                        if let Some(proj) = projects().get(idx) {
                                                            input_value.set(proj.name.clone());
                                                            error_message.set(String::new());
                                                            info_message.set(String::new());
                                                            dialog_type.set(Some(DialogType::Rename(idx)));
                                                        }
                                                    },
                                                    Icon { icon: BsPencil }
                                                }
                                            }
                                            TooltipContent { "Rename" }
                                        }
                                        Tooltip {
                                            TooltipTrigger {
                                                Button {
                                                    variant: ButtonVariant::Destructive,
                                                    onclick: move |_| {
                                                        error_message.set(String::new());
                                                        info_message.set(String::new());
                                                        dialog_type.set(Some(DialogType::Delete(idx)));
                                                    },
                                                    Icon { icon: BsTrash }
                                                }
                                            }
                                            TooltipContent { "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        AlertDialogRoot {
            open: is_open,
            AlertDialogContent {
                AlertDialogTitle { "{dialog_title}" }
                if show_input {
                    input {
                        r#type: "text",
                        placeholder: "Project name",
                        value: "{input_value}",
                        autofocus: true,
                        oninput: move |evt| input_value.set(evt.value()),
                        onkeydown: move |evt| {
                            if evt.key() == Key::Enter {
                                handle_confirm(());
                            }
                        }
                    }
                }
                if !error_message().is_empty() {
                    p { class: "dialog-error", "{error_message}" }
                }
                AlertDialogActions {
                    if matches!(dialog_type(), Some(DialogType::Delete(_))) {
                        AlertDialogAction {
                            on_click: move |_| handle_confirm(()),
                            "Delete"
                        }
                    } else {
                        AlertDialogAction {
                            on_click: move |_| handle_confirm(()),
                            if matches!(dialog_type(), Some(DialogType::Rename(_))) { "Rename" } else { "Create" }
                        }
                    }
                    AlertDialogCancel {
                        on_click: move |_| dialog_type.set(None),
                        "Cancel"
                    }
                }
            }
        }
    }
}
