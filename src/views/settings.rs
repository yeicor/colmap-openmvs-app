use crate::server::{get_settings, update_settings};
use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsArrowLeft, BsGear};
use dioxus_free_icons::Icon;

#[component]
pub fn Settings() -> Element {
    let mut projects_folder = use_signal(|| String::new());
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| String::new());
    let mut success = use_signal(|| String::new());
    let mut has_changed = use_signal(|| false);

    use_effect(move || {
        spawn({
            async move {
                loading.set(true);
                error.set(String::new());
                match get_settings().await {
                    Ok(s) => {
                        projects_folder.set(s.projects_folder);
                    }
                    Err(e) => {
                        error.set(format!("Failed to load settings: {}", e));
                    }
                }
                loading.set(false);
            }
        });
    });

    let handle_save = move |_| {
        spawn({
            async move {
                error.set(String::new());
                success.set(String::new());

                let folder = projects_folder().trim().to_string();
                if folder.is_empty() {
                    error.set("Projects folder path cannot be empty".to_string());
                    return;
                }

                let new_settings = crate::server::Settings {
                    projects_folder: folder,
                };

                match update_settings(new_settings).await {
                    Ok(_) => {
                        success.set("Settings saved successfully!".to_string());
                        has_changed.set(false);
                    }
                    Err(e) => {
                        error.set(format!("Failed to save settings: {}", e));
                    }
                }
            }
        });
    };

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/settings.css") }

        div {
            id: "settings",
            div {
                class: "header",
                Link {
                    to: Route::Projects {},
                    class: "btn-icon btn-primary",
                    title: "Back to projects",
                    Icon { icon: BsArrowLeft }
                }
                h1 {
                    Icon { icon: BsGear }
                    "Settings"
                }
            }

            if !error().is_empty() {
                div {
                    class: "error-banner",
                    "{error}"
                }
            }

            if !success().is_empty() {
                div {
                    class: "success-banner",
                    "{success}"
                }
            }

            if loading() {
                p { class: "loading", "Loading settings..." }
            } else {
                div {
                    class: "settings-container",
                    div {
                        class: "settings-item",
                        label { "Projects Folder" }
                        div {
                            class: "input-group",
                            input {
                                r#type: "text",
                                value: "{projects_folder}",
                                placeholder: "./projects",
                                oninput: move |evt| {
                                    projects_folder.set(evt.value());
                                    has_changed.set(true);
                                    error.set(String::new());
                                    success.set(String::new());
                                },
                            }
                        }
                        p {
                            class: "help-text",
                            "Path to the directory containing your projects"
                        }
                    }

                    if has_changed() {
                        div {
                            class: "button-group",
                            button {
                                class: "btn-primary btn-save",
                                onclick: handle_save,
                                "Save"
                            }
                            button {
                                class: "btn-secondary btn-cancel",
                                onclick: move |_| {
                                    spawn({
                                        async move {
                                            match get_settings().await {
                                                Ok(s) => {
                                                    projects_folder.set(s.projects_folder);
                                                    has_changed.set(false);
                                                    error.set(String::new());
                                                }
                                                Err(e) => {
                                                    error.set(format!("Failed to reload settings: {}", e));
                                                }
                                            }
                                        }
                                    });
                                },
                                "Cancel"
                            }
                        }
                    }
                }
            }
        }
    }
}
