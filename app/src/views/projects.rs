use crate::mycomponents::{Banner, BannerType, PageHeaderButton, TasksPanel};
use crate::server::{create_project, delete_project, get_projects, rename_project};
use crate::{backend_url, Route};
use crate::{
    components::{
        alert_dialog::{
            AlertDialogAction, AlertDialogActions, AlertDialogCancel, AlertDialogContent,
            AlertDialogRoot, AlertDialogTitle,
        },
        button::{Button, ButtonVariant},
        sidebar::{Sidebar as BaseSidebar, SidebarProvider, SidebarTrigger},
        tooltip::{Tooltip, TooltipContent, TooltipTrigger},
    },
    mycomponents::PageHeader,
};
use dioxus::{document::eval, prelude::*};
use dioxus_free_icons::icons::bs_icons::{
    BsBug, BsCardList, BsCupFill, BsGear, BsGithub, BsHeart, BsPencil, BsPlusCircle, BsTrash,
};
use dioxus_free_icons::Icon;
use dioxus_primitives::ContentSide;
use tracing::{debug, error, info, warn};

#[derive(Clone, Copy, PartialEq)]
enum DialogType {
    Create,
    Rename(usize),
    Delete(usize),
}

#[component]
pub fn ProjectsSidebar() -> Element {
    let cur_route = use_route::<crate::Route>();
    let mut route_state = use_signal(|| cur_route.clone());
    let mut sidebar_open: Signal<Option<bool>> = use_signal(|| None);

    if *route_state.peek() != cur_route {
        route_state.set(cur_route.clone());
    }

    use_effect(move || {
        sidebar_open.set(if matches!(route_state(), Route::Projects {}) {
            Some(false)
        } else {
            None
        });
    });

    let is_projects_route = matches!(cur_route, Route::Projects {});

    rsx! {
        SidebarProvider {
            open: sidebar_open,
            if !is_projects_route {
                SidebarTrigger {}
            }
            BaseSidebar {
                SidebarTrigger {}
                Projects { is_sidebar: true, selected: if let Route::Project { name } = cur_route { Some(name.clone()) } else { None } }
            }
        }
        Outlet::<Route> {}
        TasksPanel {}
    }
}

#[component]
pub fn Projects(
    #[props(default)] is_sidebar: bool,
    #[props(default)] selected: Option<String>,
) -> Element {
    let mut dialog_type = use_signal(|| None::<DialogType>);
    let mut input_value = use_signal(String::new);
    let mut error_message = use_signal(String::new);
    let mut info_message = use_signal(String::new);
    let mut projects = use_signal(Vec::new);
    let mut loading = use_signal(|| true);

    let refresh_projects = move || {
        debug!("Refreshing projects list");
        spawn({
            async move {
                loading.set(true);
                debug!("Fetching projects from server");
                match get_projects().await {
                    Ok(proj_list) => {
                        let count = proj_list.len();
                        info!(project_count = count, "Successfully loaded projects");
                        projects.set(proj_list);
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to load projects");
                        error_message.set(format!("Failed to load projects: {}", e));
                    }
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
                        warn!("User attempted to create project with empty name");
                        error_message.set("Project name cannot be empty".to_string());
                        return;
                    }

                    let result = match dialog {
                        DialogType::Create => {
                            info!(project_name = %name, "Creating new project");
                            create_project(name).await.map(|_| ())
                        }
                        DialogType::Rename(idx) => {
                            let old_name = projects()
                                .get(idx)
                                .expect("Invalid project index")
                                .name
                                .clone();
                            info!(old_project_name = %old_name, new_project_name = %name, "Renaming project");
                            rename_project(old_name, name).await.map(|_| ())
                        }
                        DialogType::Delete(idx) => {
                            let project_name_to_delete = projects()
                                .get(idx)
                                .expect("Invalid project index")
                                .name
                                .clone();
                            info!(project_name = %project_name_to_delete, "Deleting project");
                            delete_project(project_name_to_delete).await
                        }
                    };

                    match result {
                        Ok(_) => {
                            let operation = match dialog {
                                DialogType::Create => "created",
                                DialogType::Rename(_) => "renamed",
                                DialogType::Delete(_) => "deleted",
                            };
                            info!(operation, "Project operation succeeded");
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
                        Err(e) => {
                            let operation = match dialog {
                                DialogType::Create => "create",
                                DialogType::Rename(_) => "rename",
                                DialogType::Delete(_) => "delete",
                            };
                            error!(error = %e, operation, "Project operation failed");
                            error_message.set(e.to_string());
                        }
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
            PageHeader {
                title: "Projects".to_string(),
                icon: Some(rsx! { Icon { icon: BsCardList } }),
                no_left_margin: !is_sidebar,
                PageHeaderButton {
                    icon: rsx! { Icon { icon: BsPlusCircle } },
                    extra: rsx! { "New" },
                    extra_tooltip: rsx! { "Create a new project" },
                    onclick: move |_| {
                        debug!("Opening create new project dialog");
                        input_value.set(String::new());
                        error_message.set(String::new());
                        info_message.set(String::new());
                        dialog_type.set(Some(DialogType::Create));
                    },
                },
                PageHeaderButton {
                    icon: rsx! { Icon { icon: BsGear } },
                    extra: rsx! { "Settings" },
                    extra_tooltip: rsx! { "Configure application settings" },
                    onclick: move |_| {
                        debug!("Navigating to settings view");
                        dioxus::prelude::navigator().push(Route::SettingsView {});
                        eval("if (window.innerWidth <= 768) { document.querySelector('.dx-sidebar-trigger').click(); }");
                    },
                }
            }

            Banner {
                message: error_message(),
                banner_type: BannerType::Error,
                on_close: move |_| error_message.set(String::new()),
            }
            Banner {
                message: info_message(),
                banner_type: BannerType::Info,
                on_close: move |_| info_message.set(String::new()),
            }
            if !backend_url::BACKEND_URL.get().map(|s| s.as_str()).unwrap_or("").is_empty() {
                Banner {
                    message: format!("Using backend: {}", backend_url::BACKEND_URL.get().unwrap()),
                    banner_type: BannerType::Info,
                    on_close: move |_| {},
                }
            }

            if !is_sidebar && cfg!(feature = "demo") {
                Banner {
                    message: "This is a demo build running with mock data and without a real backend connection. Download the full version for your preferred platform to manage your actual projects and tasks.",
                    banner_type: BannerType::Info,
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
                                onclick: move |_| {
                                    if dialog_type().is_none() { // Ignore clicks from the action buttons
                                        if let Some(proj) = projects().get(idx) {
                                            debug!(project_name = %proj.name, "Navigating to project");
                                            dioxus::prelude::navigator().push(Route::Project { name: proj.name.clone() });
                                            eval("if (window.innerWidth <= 768) { document.querySelector('.dx-sidebar-trigger').click(); }");
                                        }
                                    }
                                },
                                class: format!("project-item{}", if Some(project.name.clone()) == selected {" selected" } else { "" }),
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
                                                        debug!(project_name = %proj.name, "Opening rename dialog");
                                                        input_value.set(proj.name.clone());
                                                        error_message.set(String::new());
                                                        info_message.set(String::new());
                                                        dialog_type.set(Some(DialogType::Rename(idx)));
                                                    }
                                                },
                                                Icon { icon: BsPencil }
                                            }
                                        }
                                        TooltipContent {
                                            side: ContentSide::Left,
                                            "Rename"
                                        }
                                    }
                                    Tooltip {
                                        TooltipTrigger {
                                            Button {
                                                variant: ButtonVariant::Destructive,
                                                onclick: move |_| {
                                                    if let Some(proj) = projects().get(idx) {
                                                        debug!(project_name = %proj.name, "Opening delete confirmation dialog");
                                                    }
                                                    error_message.set(String::new());
                                                    info_message.set(String::new());
                                                    dialog_type.set(Some(DialogType::Delete(idx)));
                                                },
                                                Icon { icon: BsTrash }
                                            }
                                        }
                                        TooltipContent {
                                            side: ContentSide::Left,
                                            "Delete"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !is_sidebar {
                div {
                    class: "footer-links",
                    div {
                        class: "footer-section",
                        span { class: "footer-label", "Project" }
                        div {
                            class: "footer-links-row",
                            a {
                                href: "https://github.com/yeicor/colmap-openmvs-app",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                Icon { icon: BsGithub }
                                "Source Code"
                            }
                            span { class: "footer-separator", "·" }
                            a {
                                href: "https://github.com/yeicor/colmap-openmvs-app/issues",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                Icon { icon: BsBug }
                                "Open Issues"
                            }
                        }
                    }
                    div {
                        class: "footer-section",
                        span { class: "footer-label", "Donate" }
                        div {
                            class: "footer-links-row",
                            a {
                                href: "https://github.com/sponsors/yeicor",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                Icon { icon: BsHeart }
                                "GitHub Sponsors"
                            }
                            span { class: "footer-separator", "·" }
                            a {
                                href: "https://patreon.com/Yeicor",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                Icon { icon: BsSupportBlobP }
                                "Patreon"
                            }
                            span { class: "footer-separator", "·" }
                            a {
                                href: "https://buymeacoffee.com/yeicor",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                Icon { icon: BsCupFill }
                                "Buy Me a Coffee"
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BsSupportBlobP;

impl dioxus_free_icons::IconShape for BsSupportBlobP {
    fn view_box(&self) -> &str {
        "0 0 436 476"
    }

    fn xmlns(&self) -> &str {
        "http://www.w3.org/2000/svg"
    }

    fn fill_and_stroke<'a>(&self, user_color: &'a str) -> (&'a str, &'a str, &'a str) {
        (user_color, "none", "0")
    }

    fn stroke_linecap(&self) -> &str {
        "round"
    }

    fn stroke_linejoin(&self) -> &str {
        "round"
    }

    fn child_elements(&self) -> Element {
        rsx! {
            // stylized “P”
            path {
                d: "M436 143c-.084-60.778-47.57-110.591-103.285-128.565C263.528-7.884 172.279-4.649 106.214 26.424 26.142 64.089.988 146.596.051 228.883c-.77 67.653 6.004 245.841 106.83 247.11 74.917.948 86.072-95.279 120.737-141.623 24.662-32.972 56.417-42.285 95.507-51.929C390.309 265.865 436.097 213.011 436 143Z",
            }
        }
    }
}
