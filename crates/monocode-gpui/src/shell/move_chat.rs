//! "Move to project": lists a chat under another project. Only the listing
//! and the tab change. The agent, Git and terminal keep the chat's own folder.
use super::*;

impl Shell {
    /// The project a chat is listed under: the one it was moved to, or its
    /// own folder. A move to a project that has since closed no longer counts.
    pub(super) fn session_project<'a>(&'a self, id: i64, cx: &'a App) -> &'a str {
        self.moved_session_project(id)
            .unwrap_or(&self.session_metadata(id, cx).summary.cwd)
    }

    fn moved_session_project(&self, id: i64) -> Option<&str> {
        self.workspace
            .session_projects
            .get(&id)
            .filter(|path| self.workspace.projects.contains(path))
            .map(String::as_str)
    }

    pub(super) fn session_is_moved(&self, id: i64) -> bool {
        self.moved_session_project(self.workspace.session_for_pane(id))
            .is_some()
    }

    /// The chat's real folder, for its tab, when it is listed somewhere else.
    pub(super) fn moved_session_folder(&self, id: i64, cx: &App) -> Option<String> {
        self.session_is_moved(id)
            .then(|| self.project_label(&self.session_metadata(id, cx).summary.cwd))
    }

    pub(super) fn move_to_project_items(&self, id: i64, cx: &App) -> Vec<MenuItem> {
        if !self.session_summaries.contains_key(&id)
            || !self.session_metadata(id, cx).summary.has_messages
        {
            return Vec::new();
        }
        let listed = self.session_project(id, cx);
        let own = &self.session_metadata(id, cx).summary.cwd;
        let targets: Vec<_> = self
            .workspace
            .projects
            .iter()
            .filter(|path| path.as_str() != listed)
            .map(|path| {
                let label = self.project_label(path);
                MenuAction::new(
                    format!("{MOVE_TO_PROJECT}{path}"),
                    if path == own {
                        format!("{label} (chat's folder)")
                    } else {
                        label
                    },
                )
            })
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        vec![
            MenuItem::Separator,
            MenuAction::new("move-to-project", "Move to project")
                .icon("folder")
                .submenu(targets)
                .into(),
        ]
    }

    /// The project under the pointer, on its row or on any chat listed in it.
    /// The chat's current project is not a target, since nothing would move.
    pub(super) fn project_drop_at(
        &self,
        position: Point<Pixels>,
        source: i64,
        cx: &App,
    ) -> Option<session_groups::ListDrop> {
        if !self.project_list_showing()
            || !(self.workspace.sidebar_visible || self.project_rail_showing())
            || !self.session_summaries.contains_key(&source)
        {
            return None;
        }
        let listed = self.session_project(source, cx);
        let on_row = self
            .workspace
            .projects
            .iter()
            .find(|path| self.project_folder_row_contains(path, position))
            .map(String::as_str);
        let on_chat = || {
            if !self.workspace.sidebar_visible {
                return None;
            }
            self.session_row_bounds
                .iter()
                .find(|(_, row)| row.get().is_some_and(|b| b.contains(&position)))
                .filter(|(id, _)| self.session_summaries.contains_key(id))
                .map(|(id, _)| self.session_project(*id, cx))
        };
        on_row
            .or_else(on_chat)
            .filter(|path| *path != listed)
            .map(|path| session_groups::ListDrop::Project(path.to_owned()))
    }

    /// Moves every chat in a tab, so a split tab stays together.
    pub(super) fn move_tab_to_project(
        &mut self,
        tab: i64,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut ids: Vec<i64> = self
            .workspace
            .layout(tab)
            .leaf_ids()
            .into_iter()
            .map(|id| self.workspace.session_for_pane(id))
            .filter(|id| self.session_summaries.contains_key(id))
            .collect();
        ids.dedup();
        self.move_chats_to_project(&ids, path, window, cx);
    }

    /// The sidebar and tab strip follow when the open chat is the one moved.
    pub(super) fn move_chats_to_project(
        &mut self,
        ids: &[i64],
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for id in ids {
            if self.session_metadata(*id, cx).summary.cwd == path {
                self.workspace.session_projects.remove(id);
            } else {
                self.workspace.session_projects.insert(*id, path.to_owned());
            }
            // The tab's saved checkout belonged to the project it left.
            if let Some(tab) = self.workspace.tab_of(*id) {
                self.workspace.tab_workspaces.remove(&tab);
            }
        }
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.close_preview(cx);
        if let Some(active) = self.workspace.active
            && ids.contains(&self.workspace.session_for_pane(active))
        {
            self.activate_session(active, window, cx);
        }
        self.persist(cx);
        self.invalidate_chrome(cx);
        self.tabs_frame.update(cx, |_, cx| cx.notify());
        cx.notify();
    }
}

pub(super) const MOVE_TO_PROJECT: &str = "move-to-project:";

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    const OTHER: &str = "/work/other";

    fn chat(shell: &mut Shell, cwd: &str, title: &str) -> i64 {
        let mut summary = locked(&shell.store).unwrap().create("codex", cwd).unwrap();
        summary.has_messages = true;
        summary.title = title.into();
        let id = summary.id;
        shell.session_summaries.insert(id, summary);
        shell.order.insert(0, id);
        id
    }

    fn setup(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, String, i64) {
        let (window, _, _) = super::super::tests::window(cx);
        let (home, id) = window
            .update(cx, |shell, window, cx| {
                shell.workspace.add_project(OTHER);
                let home = shell.workspace.project.clone();
                let id = chat(shell, &home, "Moves around");
                shell.open(id, window, cx);
                shell.notify_sidebar(cx);
                (home, id)
            })
            .unwrap();
        cx.run_until_parked();
        (window, home, id)
    }

    fn move_action(path: &str) -> String {
        format!("{MOVE_TO_PROJECT}{path}")
    }

    #[gpui::test]
    fn a_moved_chat_changes_project_but_keeps_its_folder_and_tab(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.run_session_menu(id, &move_action(OTHER), window, cx);
                assert_eq!(shell.session_metadata(id, cx).summary.cwd, home);
                assert_eq!(shell.workspace.project, OTHER);
                assert_eq!(shell.workspace.active, Some(id));
                assert!(shell.sidebar_session_ids(cx).contains(&id));
                let tab = shell.workspace.tab_of(id).unwrap();
                assert!(shell.visible_tabs(cx).contains(&TitleTab::Session(tab)));
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], OTHER);
                assert_eq!(restored.session_projects[&id], OTHER);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.git.read(cx).root, home, "Git follows the real folder");
                let tab = shell.workspace.tab_of(id).unwrap();
                let title = &shell.tab_copy(tab, cx).panes[0].title;
                assert!(title.ends_with("Moves around") && title.contains(" · "));
            })
            .unwrap();
    }

    /// Clicks through the real menus, from the row to the submenu entry.
    fn pick_move(
        window: gpui::WindowHandle<Shell>,
        cx: &mut TestAppContext,
        target: impl FnOnce(&Shell) -> MenuTarget,
    ) {
        window
            .update(cx, |shell, w, cx| {
                let target = target(shell);
                shell.open_menu(
                    target,
                    Anchor::Point(gpui::point(px(300.), px(200.))),
                    w,
                    cx,
                );
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual
            .debug_bounds("menu-action-move-to-project")
            .expect("the menu lists Move to project");
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        let entry: &'static str =
            Box::leak(format!("menu-action-{MOVE_TO_PROJECT}{OTHER}").into_boxed_str());
        let entry = visual
            .debug_bounds(entry)
            .expect("the submenu lists the project");
        visual.simulate_click(entry.center(), Default::default());
        visual.run_until_parked();
    }

    #[gpui::test]
    fn the_real_session_menu_moves_a_chat(cx: &mut TestAppContext) {
        let (window, _, id) = setup(cx);
        pick_move(window, cx, |_| MenuTarget::Session(id));
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.session_projects[&id], OTHER);
                assert_eq!(shell.workspace.project, OTHER);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_real_tab_menu_moves_a_tab(cx: &mut TestAppContext) {
        let (window, _, id) = setup(cx);
        pick_move(window, cx, |shell| {
            MenuTarget::Tab(shell.workspace.tab_of(id).unwrap())
        });
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.session_projects[&id], OTHER);
                assert_eq!(shell.workspace.project, OTHER);
            })
            .unwrap();
    }

    fn folders_on(window: gpui::WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.sidebar_project_folders = true;
                shell.layout_changed(cx);
                shell.notify_sidebar(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn bounds(visual: &mut gpui::VisualTestContext, name: String) -> Bounds<Pixels> {
        let name: &'static str = Box::leak(name.into_boxed_str());
        visual.debug_bounds(name).unwrap()
    }

    #[gpui::test]
    fn dragging_a_sidebar_chat_onto_a_folder_moves_it(cx: &mut TestAppContext) {
        let (window, _, id) = setup(cx);
        folders_on(window, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = bounds(visual, format!("sidebar-session-{id}")).center();
        let folder = bounds(visual, format!("project-folder-{OTHER}")).center();
        visual.simulate_mouse_down(row, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(folder, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.sidebar_drop == Some(session_groups::ListDrop::Project(OTHER.into()))
                );
            })
            .unwrap();
        visual.simulate_mouse_up(folder, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.session_projects[&id], OTHER);
                assert_eq!(shell.workspace.project, OTHER);
                assert!(shell.sidebar_drop.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn dragging_a_chat_onto_a_chat_in_another_folder_moves_it(cx: &mut TestAppContext) {
        let (window, _, id) = setup(cx);
        let there = window
            .update(cx, |shell, _, cx| {
                let there = chat(shell, OTHER, "Already there");
                shell.notify_sidebar(cx);
                there
            })
            .unwrap();
        folders_on(window, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = bounds(visual, format!("sidebar-session-{id}")).center();
        let target = bounds(visual, format!("sidebar-session-{there}")).center();
        visual.simulate_mouse_down(row, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.session_projects[&id], OTHER);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_chat_dropped_on_its_own_folder_stays(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        folders_on(window, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = bounds(visual, format!("sidebar-session-{id}")).center();
        let folder = bounds(visual, format!("project-folder-{home}")).center();
        visual.simulate_mouse_down(row, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(folder, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(folder, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.workspace.session_projects.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn dragging_a_tab_onto_a_folder_moves_it_even_when_it_is_alone(cx: &mut TestAppContext) {
        let (window, _, id) = setup(cx);
        folders_on(window, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let folder = bounds(visual, format!("project-folder-{OTHER}")).center();
        let tab = window
            .update(visual, |shell, _, cx| {
                let tab = shell.workspace.tab_of(id).unwrap();
                let event = MouseDownEvent {
                    button: MouseButton::Left,
                    position: gpui::point(px(400.), px(16.)),
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                };
                shell.start_tab_drag(TitleTab::Session(tab), &event, cx);
                assert!(shell.tab_drag.is_some());
                tab
            })
            .unwrap();
        visual.simulate_mouse_move(folder, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.sidebar_drop == Some(session_groups::ListDrop::Project(OTHER.into()))
                );
            })
            .unwrap();
        visual.simulate_mouse_up(folder, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.session_projects[&id], OTHER);
                assert_eq!(shell.workspace.project, OTHER);
                assert!(shell.visible_tabs(cx).contains(&TitleTab::Session(tab)));
            })
            .unwrap();
    }

    #[gpui::test]
    fn moving_back_restores_the_chat_to_its_own_project(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.run_session_menu(id, &move_action(OTHER), window, cx);
                shell.run_session_menu(id, &move_action(&home), window, cx);
                assert!(shell.workspace.session_projects.is_empty());
                assert_eq!(shell.workspace.project, home);
                let tab = shell.workspace.tab_of(id).unwrap();
                assert!(shell.visible_tabs(cx).contains(&TitleTab::Session(tab)));
                assert!(shell.sidebar_session_ids(cx).contains(&id));
            })
            .unwrap();
    }

    #[gpui::test]
    fn moving_a_chat_that_is_not_open_leaves_you_where_you_are(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                let other_chat = chat(shell, &home, "Stays closed");
                shell.run_session_menu(other_chat, &move_action(OTHER), window, cx);
                assert_eq!(shell.workspace.project, home);
                assert_eq!(shell.workspace.active, Some(id));
                assert!(!shell.sidebar_session_ids(cx).contains(&other_chat));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_tab_menu_moves_the_tab_and_offers_only_other_projects(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                let submenu = |shell: &Shell, cx: &App| {
                    shell
                        .move_to_project_items(id, cx)
                        .iter()
                        .find_map(|item| match item {
                            MenuItem::Action(action) => Some(
                                action
                                    .submenu
                                    .iter()
                                    .map(|entry| entry.id.to_string())
                                    .collect::<Vec<_>>(),
                            ),
                            _ => None,
                        })
                        .unwrap()
                };
                let targets = submenu(shell, cx);
                assert!(targets.contains(&move_action(OTHER)));
                assert!(!targets.contains(&move_action(&home)));
                let tab = shell.workspace.tab_of(id).unwrap();
                shell.run_menu(MenuTarget::Tab(tab), &move_action(OTHER), None, window, cx);
                assert_eq!(shell.workspace.project, OTHER);
                assert!(submenu(shell, cx).contains(&move_action(&home)));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_chat_returns_home_when_its_new_project_closes(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.run_session_menu(id, &move_action(OTHER), window, cx);
                shell.workspace.projects.retain(|path| path != OTHER);
                assert_eq!(shell.session_project(id, cx), home);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_project_list_shows_a_moved_chat_under_its_new_project(cx: &mut TestAppContext) {
        let (window, home, id) = setup(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.sidebar_project_folders = true;
                shell.run_session_menu(id, &move_action(OTHER), window, cx);
                let folders = shell.project_folder_sessions(cx);
                let listed = |path: &str| {
                    folders
                        .iter()
                        .find(|(project, _)| project == path)
                        .is_some_and(|(_, ids)| ids.contains(&id))
                };
                assert!(listed(OTHER));
                assert!(!listed(&home));
            })
            .unwrap();
    }
}
