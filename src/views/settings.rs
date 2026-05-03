use crate::components::button::{Button, ButtonVariant};
use crate::server::{get_settings, update_settings};
use crate::Route;
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsFolder, BsGear};
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
                    Ok(s) => projects_folder.set(s.projects_folder),
                    Err(e) => error.set(format!("Failed to load settings: {}", e)),
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
                    Err(e) => error.set(format!("Failed to save settings: {}", e)),
                }
            }
        });
    };

    let handle_cancel = move |_| {
        spawn({
            async move {
                match get_settings().await {
                    Ok(s) => {
                        projects_folder.set(s.projects_folder);
                        has_changed.set(false);
                        error.set(String::new());
                    }
                    Err(e) => error.set(format!("Failed to reload settings: {}", e)),
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
                h1 {
                    Icon { icon: BsGear }
                    "Settings"
                }
                Link {
                    to: Route::Projects {},
                    Button {
                        variant: ButtonVariant::Ghost,
                        "← Back"
                    }
                }
            }

            if !error().is_empty() {
                div {
                    class: "error-banner",
                    "{error}"
                    Button {
                        variant: ButtonVariant::Ghost,
                        onclick: move |_| error.set(String::new()),
                        "×"
                    }
                }
            }

            if !success().is_empty() {
                div {
                    class: "info-banner",
                    "{success}"
                    Button {
                        variant: ButtonVariant::Ghost,
                        onclick: move |_| success.set(String::new()),
                        "×"
                    }
                }
            }

            if loading() {
                p { class: "loading", "Loading settings..." }
            } else {
                div {
                    class: "settings-form flex-responsive",
                    div {
                        class: "form-group grow",
                        label { "Projects Folder" }
                        div {
                            class: "folder-row",
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
                                class: "folder-input",
                            }
                            input {
                                r#type: "file",
                                directory: true,
                                style: "display: none;",
                                onchange: move |evt| {
                                    for file in evt.files() {
                                        projects_folder.set(file.path().to_str().expect("Invalid path").to_string());
                                        has_changed.set(true);
                                        error.set(String::new());
                                        success.set(String::new());
                                        break;
                                    }
                                }
                            }
                            Button {
                                variant: ButtonVariant::Secondary,
                                onclick: move |_| {
                                    eval("document.querySelector('#settings input[type=file]').click()");
                                },
                                Icon { icon: BsFolder },
                            }
                        }
                    }

                    if has_changed() {
                        div {
                            class: "form-actions",
                            Button {
                                variant: ButtonVariant::Primary,
                                onclick: handle_save,
                                "Save"
                            }
                            Button {
                                variant: ButtonVariant::Secondary,
                                onclick: handle_cancel,
                                "Cancel"
                            }
                        }
                    }
                }
            }
        }
    }
}
