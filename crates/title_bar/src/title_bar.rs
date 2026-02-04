mod application_menu;
mod onboarding_banner;
pub mod platform_title_bar;
mod platforms;
mod system_window_tabs;
mod title_bar_settings;

#[cfg(feature = "stories")]
mod stories;

use crate::{
    application_menu::{ApplicationMenu, show_menus},
    platform_title_bar::PlatformTitleBar,
    system_window_tabs::SystemWindowTabs,
};

#[cfg(not(target_os = "macos"))]
use crate::application_menu::{
    ActivateDirection, ActivateMenuLeft, ActivateMenuRight, OpenApplicationMenu,
};

use client::Client;
use gpui::{
    Action, AnyElement, App, Context, Corner, Element, Entity, Focusable, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, actions, div,
};
use onboarding_banner::OnboardingBanner;
use project::{
    Project, WorktreeSettings, git_store::GitStoreEvent, trusted_worktrees::TrustedWorktrees,
};
use settings::{Settings, SettingsLocation};
use std::sync::Arc;
use title_bar_settings::TitleBarSettings;
use ui::{ContextMenu, PopoverMenu, TintColor, Tooltip, prelude::*};
use util::{ResultExt, rel_path::RelPath};
use workspace::{ToggleWorktreeSecurity, Workspace};

pub use onboarding_banner::restore_banner;

#[cfg(feature = "stories")]
pub use stories::*;

const MAX_PROJECT_NAME_LENGTH: usize = 40;
const MAX_BRANCH_NAME_LENGTH: usize = 40;
const MAX_SHORT_SHA_LENGTH: usize = 8;

actions!(
    collab,
    [
        /// Toggles the user menu dropdown.
        ToggleUserMenu,
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch
    ]
);

pub fn init(cx: &mut App) {
    SystemWindowTabs::init(cx);

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let item = cx.new(|cx| TitleBar::new("title-bar", workspace, window, cx));
        workspace.set_titlebar_item(item.into(), window, cx);

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, action: &OpenApplicationMenu, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| menu.open_menu(action, window, cx));
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuRight, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Right, window, cx)
                        });
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuLeft, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Left, window, cx)
                        });
                    }
                });
            }
        });
    })
    .detach();
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    client: Arc<Client>,
    workspace: WeakEntity<Workspace>,
    application_menu: Option<Entity<ApplicationMenu>>,
    _subscriptions: Vec<Subscription>,
    banner: Entity<OnboardingBanner>,
}

impl Render for TitleBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar_settings = *TitleBarSettings::get_global(cx);

        let show_menus = show_menus();

        let mut children = Vec::new();

        let has_restricted_worktrees = TrustedWorktrees::try_get_global(cx)
            .map(|trusted_worktrees| {
                trusted_worktrees
                    .read(cx)
                    .has_restricted_worktrees(&self.project.read(cx).worktree_store(), cx)
            })
            .unwrap_or(false);

        children.push(
            h_flex()
                .gap_1()
                .map(|title_bar| {
                    let mut render_project_items = title_bar_settings.show_branch_name
                        || title_bar_settings.show_project_items;
                    title_bar
                        .when_some(
                            self.application_menu.clone().filter(|_| !show_menus),
                            |title_bar, menu| {
                                render_project_items &=
                                    !menu.update(cx, |menu, _| menu.all_menus_shown());
                                title_bar.child(menu)
                            },
                        )
                        .when(has_restricted_worktrees, |this| {
                            this.child(self.render_restricted_mode(cx))
                        })
                        .when(render_project_items, |title_bar| {
                            title_bar
                                .when(title_bar_settings.show_project_items, |title_bar| {
                                    title_bar.child(self.render_project_name(cx))
                                })
                                .when(title_bar_settings.show_branch_name, |title_bar| {
                                    title_bar.children(self.render_project_repo(cx))
                                })
                        })
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
        );

        if title_bar_settings.show_onboarding_banner {
            children.push(self.banner.clone().into_any_element())
        }

        let status = self.client.status();
        let status = &*status.borrow();

        children.push(
            h_flex()
                .pr_1()
                .gap_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(self.render_connection_status(status, cx))
                .when(TitleBarSettings::get_global(cx).show_user_menu, |this| {
                    this.child(self.render_user_menu_button(cx))
                })
                .into_any_element(),
        );

        if show_menus {
            let restricted_mode_button = self.render_restricted_mode(cx);
            self.platform_titlebar.update(cx, |this, _| {
                this.set_children(
                    self.application_menu
                        .clone()
                        .map(|menu| {
                            h_flex()    
                                .gap_1()
                                .child(menu.into_any_element())
                                .when(has_restricted_worktrees, |this|
                                    this.child(restricted_mode_button)
                                )
                                .into_any_element()
                        }),
                );
            });

            v_flex()
                .w_full()
                .child(self.platform_titlebar.clone().into_any_element())
                .into_any_element()
        } else {
            self.platform_titlebar.update(cx, |this, _| {
                this.set_children(children);
            });
            self.platform_titlebar.clone().into_any_element()
        }
    }
}

impl TitleBar {
    pub fn new(
        id: impl Into<ElementId>,
        workspace: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let user_store = workspace.app_state().user_store.clone();
        let client = workspace.app_state().client.clone();

        let platform_style = PlatformStyle::platform();
        let application_menu = match platform_style {
            PlatformStyle::Mac => {
                if option_env!("ZED_USE_CROSS_PLATFORM_MENU").is_some() {
                    Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
                } else {
                    None
                }
            }
            PlatformStyle::Linux | PlatformStyle::Windows => {
                Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
            }
        };

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.observe(&workspace.weak_handle().upgrade().unwrap(), |_, _, cx| {
                cx.notify()
            }),
        );
        subscriptions.push(cx.subscribe(&project, |_, _, _: &project::Event, cx| cx.notify()));
        subscriptions.push(
            cx.subscribe(&git_store, move |_, _, event, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_)
                | GitStoreEvent::RepositoryUpdated(_, _, true) => {
                    cx.notify();
                }
                _ => {}
            }),
        );
        subscriptions.push(cx.observe(&user_store, |_a, _, cx| cx.notify()));
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            subscriptions.push(cx.subscribe(&trusted_worktrees, |_, _, _, cx| {
                cx.notify();
            }));
        }

        let banner = cx.new(|cx| {
            OnboardingBanner::new(
                "ACP Claude Code Onboarding",
                IconName::AiClaude,
                "Claude Code",
                Some("Introducing:".into()),
                zed_actions::agent::OpenClaudeCodeOnboardingModal.boxed_clone(),
                cx,
            )
            // When updating this to a non-AI feature release, remove this line.
            .visible_when(|cx| !project::DisableAiSettings::get_global(cx).disable_ai)
        });

        let platform_titlebar = cx.new(|cx| PlatformTitleBar::new(id, cx));

        Self {
            platform_titlebar,
            application_menu,
            workspace: workspace.weak_handle(),
            project,
            client,
            _subscriptions: subscriptions,
            banner,
        }
    }

    fn project_name(&self, cx: &Context<Self>) -> Option<SharedString> {
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| {
                let worktree = worktree.read(cx);
                let settings_location = SettingsLocation {
                    worktree_id: worktree.id(),
                    path: RelPath::empty(),
                };

                let settings = WorktreeSettings::get(Some(settings_location), cx);
                let name = match &settings.project_name {
                    Some(name) => name.as_str(),
                    None => worktree.root_name_str(),
                };
                SharedString::new(name)
            })
            .next()
    }

    pub fn render_restricted_mode(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("restricted_mode_trigger", "Restricted Mode")
            .style(ButtonStyle::Tinted(TintColor::Warning))
            .label_size(LabelSize::Small)
            .color(Color::Warning)
            .icon(IconName::Warning)
            .icon_color(Color::Warning)
            .icon_size(IconSize::Small)
            .icon_position(IconPosition::Start)
            .tooltip(|_, cx| {
                Tooltip::with_meta(
                    "You're in Restricted Mode",
                    Some(&ToggleWorktreeSecurity),
                    "Mark this project as trusted and unlock all features",
                    cx,
                )
            })
            .on_click({
                cx.listener(move |this, _, window, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| {
                            workspace.show_worktree_trust_security_modal(true, window, cx)
                        })
                        .log_err();
                })
            });

        if cfg!(macos_sdk_26) {
            // Make up for Tahoe's traffic light buttons having less spacing around them
            div().child(button).ml_0p5().into_any_element()
        } else {
            button.into_any_element()
        }
    }

    pub fn render_project_name(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let name = self.project_name(cx);
        let is_project_selected = name.is_some();
        let name = if let Some(name) = name {
            util::truncate_and_trailoff(&name, MAX_PROJECT_NAME_LENGTH)
        } else {
            "Open Recent Project".to_string()
        };

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        PopoverMenu::new("recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    false,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", name)
                    .label_size(LabelSize::Small)
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .when(!is_project_selected, |s| s.color(Color::Muted)),
                move |_window, cx| {
                    Tooltip::for_action(
                        "Recent Projects",
                        &zed_actions::OpenRecent {
                            create_new_window: false,
                        },
                        cx,
                    )
                },
            )
            .anchor(gpui::Corner::TopLeft)
    }

    pub fn render_project_repo(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let repository = self.project.read(cx).active_repository(cx)?;
        let repository_count = self.project.read(cx).repositories(cx).len();
        let workspace = self.workspace.upgrade()?;

        let (branch_name, icon_info) = {
            let repo = repository.read(cx);
            let branch_name = repo
                .branch
                .as_ref()
                .map(|branch| branch.name())
                .map(|name| util::truncate_and_trailoff(name, MAX_BRANCH_NAME_LENGTH))
                .or_else(|| {
                    repo.head_commit.as_ref().map(|commit| {
                        commit
                            .sha
                            .chars()
                            .take(MAX_SHORT_SHA_LENGTH)
                            .collect::<String>()
                    })
                });

            let branch_name = branch_name?;

            let project_name = self.project_name(cx);
            let repo_name = repo
                .work_directory_abs_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(SharedString::new);
            let show_repo_name =
                repository_count > 1 && repo.branch.is_some() && repo_name != project_name;
            let branch_name = if let Some(repo_name) = repo_name.filter(|_| show_repo_name) {
                format!("{repo_name}/{branch_name}")
            } else {
                branch_name
            };

            let status = repo.status_summary();
            let tracked = status.index + status.worktree;
            let icon_info = if status.conflict > 0 {
                (IconName::Warning, Color::VersionControlConflict)
            } else if tracked.modified > 0 {
                (IconName::SquareDot, Color::VersionControlModified)
            } else if tracked.added > 0 || status.untracked > 0 {
                (IconName::SquarePlus, Color::VersionControlAdded)
            } else if tracked.deleted > 0 {
                (IconName::SquareMinus, Color::VersionControlDeleted)
            } else {
                (IconName::GitBranch, Color::Muted)
            };

            (branch_name, icon_info)
        };

        let settings = TitleBarSettings::get_global(cx);
        let project = self.project.clone();

        Some(
            PopoverMenu::new("branch-menu")
                .menu(move |window, cx| {
                    let repository = project.read(cx).active_repository(cx);
                    Some(git_ui::branch_picker::popover(
                        workspace.downgrade(),
                        true,
                        repository,
                        window,
                        cx,
                    ))
                })
                .trigger_with_tooltip(
                    Button::new("project_branch_trigger", branch_name)
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .label_size(LabelSize::Small)
                        .color(Color::Muted)
                        .when(settings.show_branch_icon, |branch_button| {
                            let (icon, icon_color) = icon_info;
                            branch_button
                                .icon(icon)
                                .icon_position(IconPosition::Start)
                                .icon_color(icon_color)
                                .icon_size(IconSize::Indicator)
                        }),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            "Recent Branches",
                            Some(&zed_actions::git::Branch),
                            "Local branches only",
                            cx,
                        )
                    },
                )
                .anchor(gpui::Corner::TopLeft),
        )
    }

    fn render_connection_status(
        &self,
        status: &client::Status,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match status {
            client::Status::ConnectionError
            | client::Status::ConnectionLost
            | client::Status::Reauthenticating
            | client::Status::Reconnecting
            | client::Status::ReconnectionError { .. } => Some(
                div()
                    .id("disconnected")
                    .child(Icon::new(IconName::Disconnected).size(IconSize::Small))
                    .tooltip(Tooltip::text("Disconnected"))
                    .into_any_element(),
            ),
            client::Status::UpgradeRequired | _ => None,
        }
    }

    pub fn render_user_menu_button(&mut self, _cx: &mut Context<Self>) -> impl Element {
        PopoverMenu::new("user-menu")
            .anchor(Corner::TopRight)
            .menu(move |window, cx| {
                ContextMenu::build(window, cx, |menu, _, _cx| {
                    menu.action("Settings", zed_actions::OpenSettings.boxed_clone())
                        .action("Keymap", Box::new(zed_actions::OpenKeymap))
                        .action(
                            "Themes…",
                            zed_actions::theme_selector::Toggle::default().boxed_clone(),
                        )
                        .action(
                            "Icon Themes…",
                            zed_actions::icon_theme_selector::Toggle::default().boxed_clone(),
                        )
                        .action(
                            "Extensions",
                            zed_actions::Extensions::default().boxed_clone(),
                        )
                })
                .into()
            })
            .map(|this| {
                this.trigger_with_tooltip(
                    IconButton::new("user-menu", IconName::ChevronDown).icon_size(IconSize::Small),
                    Tooltip::text("Toggle User Menu"),
                )
            })
            .anchor(gpui::Corner::TopRight)
    }
}
