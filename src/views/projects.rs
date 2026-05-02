use crate::server::{create_project, delete_project, get_projects, rename_project};
use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsCamera, BsGear, BsPencil, BsPlusCircle, BsTrash};
use dioxus_free_icons::Icon;
use dioxus_primitives::alert_dialog::{AlertDialogContent, AlertDialogRoot, AlertDialogTitle};

#[derive(Clone, Copy, PartialEq)]
enum DialogType {
    Create,
    Rename(usize),
    Delete(usize),
}

#[component]
pub fn Projects() -> Element {
    let mut dialog_type = use_signal(|| None::<DialogType>);
    let mut input_value = use_signal(|| String::new());
    let mut error_message = use_signal(|| String::new());
    let mut projects = use_signal(|| Vec::new());
    let mut loading = use_signal(|| true);

    use_effect(move || {
        spawn({
            async move {
                loading.set(true);
                error_message.set(String::new());
                match get_projects().await {
                    Ok(proj_list) => {
                        projects.set(proj_list);
                    }
                    Err(e) => {
                        error_message.set(format!("Failed to load projects: {}", e));
                    }
                }
                loading.set(false);
            }
        });
    });

    let refresh_projects = move || {
        spawn({
            async move {
                loading.set(true);
                error_message.set(String::new());
                match get_projects().await {
                    Ok(proj_list) => {
                        projects.set(proj_list);
                    }
                    Err(e) => {
                        error_message.set(format!("Failed to load projects: {}", e));
                    }
                }
                loading.set(false);
            }
        });
    };

    let handle_confirm = move |_| {
        spawn({
            async move {
                match dialog_type() {
                    Some(DialogType::Create) => {
                        let name = input_value().trim().to_string();
                        if name.is_empty() {
                            error_message.set("Project name cannot be empty".to_string());
                            return;
                        }
                        match create_project(name).await {
                            Ok(_) => {
                                dialog_type.set(None);
                                refresh_projects();
                            }
                            Err(e) => error_message.set(e.to_string()),
                        }
                    }
                    Some(DialogType::Rename(idx)) => {
                        if let Some(project) = projects().get(idx) {
                            let new_name = input_value().trim().to_string();
                            if new_name.is_empty() {
                                error_message.set("Project name cannot be empty".to_string());
                                return;
                            }
                            match rename_project(project.name.clone(), new_name).await {
                                Ok(_) => {
                                    dialog_type.set(None);
                                    refresh_projects();
                                }
                                Err(e) => error_message.set(e.to_string()),
                            }
                        }
                    }
                    Some(DialogType::Delete(idx)) => {
                        if let Some(project) = projects().get(idx) {
                            match delete_project(project.name.clone()).await {
                                Ok(_) => {
                                    dialog_type.set(None);
                                    refresh_projects();
                                }
                                Err(e) => error_message.set(e.to_string()),
                            }
                        }
                    }
                    None => {}
                }
            }
        });
    };

    let dialog_title = match dialog_type() {
        Some(DialogType::Create) => "Create New Project".to_string(),
        Some(DialogType::Rename(idx)) => {
            if let Some(project) = projects().get(idx) {
                format!("Rename \"{}\"", project.name)
            } else {
                "Rename Project".to_string()
            }
        }
        Some(DialogType::Delete(idx)) => {
            if let Some(project) = projects().get(idx) {
                format!("Delete \"{}\"?", project.name)
            } else {
                "Delete Project?".to_string()
            }
        }
        None => String::new(),
    };

    let show_input = !matches!(dialog_type(), Some(DialogType::Delete(_)));
    let is_open = dialog_type().is_some();

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/projects.css") }

        if is_open {
            div {
                class: "modal-overlay",
                onclick: move |_| dialog_type.set(None),
            }
        }

        div {
            id: "projects",
            div {
                class: "header",
                h1 { Icon { icon: BsCamera } "Projects" }
                div {
                    class: "header-actions",
                    button {
                        class: "btn-icon btn-primary",
                        title: "Create new project",
                        onclick: move |_| {
                            input_value.set(String::new());
                            error_message.set(String::new());
                            dialog_type.set(Some(DialogType::Create));
                        },
                        Icon { icon: BsPlusCircle }
                        span { class: "btn-text", "New" }
                    }
                    Link {
                        to: Route::Settings {},
                        class: "btn-icon btn-primary",
                        title: "Settings",
                        Icon { icon: BsGear }
                        span { class: "btn-text", "Settings" }
                    }
                }
            }

            if !error_message().is_empty() {
                div {
                    class: "error-banner",
                    "{error_message}"
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
                            div {
                                class: "project-item",
                                Link {
                                    to: Route::ProjectDetail { name: project.name.clone() },
                                    class: "project-name-link",
                                    span {
                                        class: "project-name",
                                        "{project.name}"
                                    }
                                }
                                div {
                                    class: "project-actions",
                                    button {
                                        class: "btn-small",
                                        title: "Rename",
                                        onclick: move |_| {
                                            if let Some(proj) = projects().get(idx) {
                                                input_value.set(proj.name.clone());
                                                error_message.set(String::new());
                                                dialog_type.set(Some(DialogType::Rename(idx)));
                                            }
                                        },
                                        Icon { icon: BsPencil }
                                    }
                                    button {
                                        class: "btn-small btn-danger",
                                        title: "Delete",
                                        onclick: move |_| {
                                            error_message.set(String::new());
                                            dialog_type.set(Some(DialogType::Delete(idx)));
                                        },
                                        Icon { icon: BsTrash }
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
                class: "dialog-content",
                AlertDialogTitle { "{dialog_title}" }
                if show_input {
                    input {
                        class: "dialog-input",
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
                div {
                    class: "dialog-buttons",
                    button {
                        class: "btn-primary",
                        onclick: move |_| handle_confirm(()),
                        if matches!(dialog_type(), Some(DialogType::Delete(_))) {
                            "Delete"
                        } else {
                            "Create"
                        }
                    }
                    button {
                        class: "btn-secondary",
                        onclick: move |_| {
                            dialog_type.set(None);
                        },
                        "Cancel"
                    }
                }
            }
        }
    }
}
