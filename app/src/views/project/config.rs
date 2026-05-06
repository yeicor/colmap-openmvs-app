use crate::mycomponents::Help;
use colmap_openmvs_api::{ConfigSchema, EnvVarConfig, EnvVarWithHelp, SavedProjectConfig};
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::BsQuestionCircle;
use dioxus_free_icons::Icon;
use std::collections::HashMap;

type EnvVarValuesSignal = Signal<HashMap<String, String>>;

#[component]
pub fn ConfigTab(project_name: String) -> Element {
    // Clone project_name early so we can use it in multiple closures
    let project_name_effect = project_name.clone();
    let project_name_save = project_name.clone();

    let mut config_schema = use_signal(|| Option::<ConfigSchema>::None);
    let loading = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let env_var_values = use_signal(|| HashMap::<String, String>::new());
    let mut custom_script = use_signal(|| String::new());
    let mut refresh_counter = use_signal(|| 0u32);
    let saving = use_signal(|| false);
    let save_status = use_signal(|| Option::<SaveStatus>::None);
    let mut has_changes = use_signal(|| false);
    let mut help_modal_open = use_signal(|| false);
    let mut help_modal_text = use_signal(|| String::new());

    // Fetch on mount and on manual refresh
    use_effect(move || {
        let _trigger = refresh_counter();
        spawn_fetch_config(
            config_schema,
            loading,
            error,
            env_var_values,
            custom_script,
            project_name_effect.clone(),
        );
    });

    let env_vars = config_schema()
        .as_ref()
        .map(|s| s.environment_variables.clone())
        .unwrap_or_default();

    let on_save = {
        move |_| {
            save_config(
                project_name_save.clone(),
                config_schema().clone(),
                env_var_values(),
                custom_script(),
                saving,
                save_status,
                has_changes,
            );
        }
    };

    rsx! {
        Help {
            help_text: help_modal_text(),
            open: help_modal_open,
        }

        div {
            class: "config-tab",

            // Loading state
            if loading() {
                div {
                    class: "config-loading",
                    div { class: "spinner" }
                    span { "Loading configuration..." }
                }
            }

            // Error state
            if let Some(err) = error() {
                div {
                    class: "config-error",
                    div {
                        strong { "Error: " }
                        span { "{err}" }
                    }
                    button {
                        class: "btn btn-retry",
                        onclick: move |_| {
                            error.set(None);
                            config_schema.set(None);
                            *refresh_counter.write() += 1;
                        },
                        "Retry"
                    }
                }
            }

            // Main content
            if !loading() && error().is_none() {
                if env_vars.is_empty() {
                    div {
                        class: "config-empty",
                        p { "No environment variables found" }
                    }
                } else {
                    div {
                        class: "config-content",
                        div {
                            class: "config-vars",
                            // Environment variables section
                            for env_var in env_vars.iter() {
                                EnvVarRow {
                                    var: env_var.clone(),
                                    env_var_values,
                                    has_changes,
                                    on_help: EventHandler::new(move |help_text: String| {
                                        help_modal_text.set(help_text.lines()
                                                .map(|line| line.trim_start())
                                                .collect::<Vec<_>>()
                                                .join("\n"));
                                        help_modal_open.set(true);
                                    }),
                                }
                            }

                            // Custom script section
                            EnvVarRow {
                                var: EnvVarWithHelp {
                                    name: "Custom Script".to_string(),
                                    help: Some("Write any bash commands here to override or extend the pipeline behavior.\n\nExamples:\n- Call tools directly: colmap feature_extractor --database_path db.db --image_path images\n- Set environment variables: export COLMAP_NUM_THREADS=8\n- Exit early: exit 0 (or non-zero for error)\n- Add debugging: set -x\n\nThis script is appended to the configuration file and runs after environment variables are sourced. Use exit codes to control pipeline flow.".to_string()),
                                },
                                env_var_values: use_signal(|| HashMap::new()),
                                has_changes,
                                on_help: EventHandler::new(move |help_text: String| {
                                    help_modal_text.set(help_text.lines()
                                            .map(|line| line.trim_start())
                                            .collect::<Vec<_>>()
                                            .join("\n"));
                                    help_modal_open.set(true);
                                }),
                                text_area: Some(true),
                            }

                            // Status message (only show when not editing)
                            if !has_changes() {
                                if let Some(status) = save_status() {
                                    div {
                                        class: match &status {
                                            SaveStatus::Success(_) => "config-status config-status-success",
                                            SaveStatus::Error(_) => "config-status config-status-error",
                                        },
                                        match &status {
                                            SaveStatus::Success(msg) => msg.as_str(),
                                            SaveStatus::Error(msg) => msg.as_str(),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Fixed save button at bottom (only visible if there are changes)
            if has_changes() && !loading() && error().is_none() && !env_vars.is_empty() {
                div {
                    class: "config-save-bar",
                    div {
                        class: "config-save-container",
                        button {
                            class: "btn btn-primary",
                            disabled: saving(),
                            onclick: on_save,
                            if saving() {
                                "Saving..."
                            } else {
                                "Save Configuration"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn EnvVarRow(
    var: EnvVarWithHelp,
    mut env_var_values: EnvVarValuesSignal,
    mut has_changes: Signal<bool>,
    on_help: EventHandler<String>,
    #[props(default)] text_area: Option<bool>,
) -> Element {
    let name_clone = var.name.clone();
    let current_value = env_var_values().get(&var.name).cloned().unwrap_or_default();
    let has_help = var.help.is_some();
    let help_text = var.help.clone();

    rsx! {
        div {
            class: "config-var-row",
            div {
                class: "config-var-label-container",
                label {
                    class: "config-var-label",
                    "{var.name}"
                }
                if has_help {
                    if let Some(help) = help_text {
                        button {
                            class: "config-var-help-button",
                            title: "Show help for this variable",
                            onclick: move |_| {
                                on_help.call(help.clone());
                            },
                            Icon { icon: BsQuestionCircle }
                        }
                    }
                }
            }
            if text_area.unwrap_or(false) {
                textarea {
                    class: "config-custom-script-textarea",
                    placeholder: "# Enter custom bash script here (optional)\n# Example: colmap feature_extractor --database_path db.db --image_path images\n# Example: exit 0 to finish early\n# Use this to override pipeline behavior",
                    value: "{current_value}",
                    oninput: move |e| {
                        let mut values = env_var_values();
                        values.insert(name_clone.clone(), e.value());
                        env_var_values.set(values);
                        has_changes.set(true);
                    },
                }
            } else {
                input {
                    class: "config-var-input",
                    r#type: "text",
                    placeholder: "Enter value...",
                    value: "{current_value}",
                    oninput: move |e| {
                        let mut values = env_var_values();
                        values.insert(name_clone.clone(), e.value());
                        env_var_values.set(values);
                        has_changes.set(true);
                    },
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SaveStatus {
    Success(String),
    Error(String),
}

fn save_config(
    project_name: String,
    schema: Option<ConfigSchema>,
    values: HashMap<String, String>,
    custom_script: String,
    mut saving: Signal<bool>,
    mut save_status: Signal<Option<SaveStatus>>,
    mut has_changes: Signal<bool>,
) {
    if let Some(schema) = schema {
        saving.set(true);
        save_status.set(None);

        spawn(async move {
            let env_vars: Vec<EnvVarConfig> = values
                .into_iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .map(|(k, v)| EnvVarConfig { name: k, value: v })
                .collect();

            let config = SavedProjectConfig {
                image_tag: schema.image_tag,
                environment_variables: env_vars,
                custom_script: if custom_script.trim().is_empty() {
                    None
                } else {
                    Some(custom_script)
                },
            };

            match crate::server::save_project_config(project_name, config).await {
                Ok(_) => {
                    save_status.set(Some(SaveStatus::Success(
                        "Configuration saved successfully!".to_string(),
                    )));
                    has_changes.set(false);
                }
                Err(e) => {
                    save_status.set(Some(SaveStatus::Error(format!("Failed to save: {}", e))));
                }
            }

            saving.set(false);
        });
    }
}

fn spawn_fetch_config(
    mut config_schema: Signal<Option<ConfigSchema>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
    mut env_var_values: Signal<HashMap<String, String>>,
    mut custom_script: Signal<String>,
    project_name: String,
) {
    spawn(async move {
        loading.set(true);
        error.set(None);

        let image_tag = match crate::server::get_settings().await {
            Ok(settings) => match settings.default_image_tag {
                Some(tag) if !tag.trim().is_empty() => tag,
                _ => {
                    error.set(Some(
                        "No default image configured. Go to Settings → Images and set one."
                            .to_string(),
                    ));
                    loading.set(false);
                    return;
                }
            },
            Err(e) => {
                error.set(Some(format!("Failed to load settings: {}", e)));
                loading.set(false);
                return;
            }
        };

        // Fetch the configuration schema
        match crate::server::get_image_config(image_tag).await {
            Ok(schema) => {
                config_schema.set(Some(schema));
            }
            Err(e) => {
                let error_msg = e.to_string();
                let display_msg = if error_msg.contains("Image not prepared") {
                    "Container image not prepared. Go to Settings → Images to prepare it."
                        .to_string()
                } else if error_msg.contains("PRoot binary not found") {
                    "PRoot binary not found. Go to Settings → Runtime → PRoot to install it."
                        .to_string()
                } else {
                    format!("Failed to load configuration: {}", error_msg)
                };
                error.set(Some(display_msg));
                loading.set(false);
                return;
            }
        }

        // Try to load previously saved config for this project
        match crate::server::load_project_config(project_name).await {
            Ok(loaded_config) => {
                // Load the environment variables from the saved config
                let mut values = HashMap::new();
                for env_var in loaded_config.environment_variables {
                    values.insert(env_var.name, env_var.value);
                }
                env_var_values.set(values);

                // Load the custom script
                if !loaded_config.custom_script.is_empty() {
                    custom_script.set(loaded_config.custom_script);
                }
            }
            Err(_) => {
                // It's okay if there's no saved config yet (first time opening)
                // Just continue with empty values
            }
        }

        loading.set(false);
    });
}
