//! Unread agent work per project: files an agent changed that nobody kept,
//! undid or committed yet. The sidebar shows the count; it opens the
//! project's Agent Changes review.
use super::*;

impl Shell {
    /// Recount every open project in the background. A request while one
    /// runs is remembered and runs right after it.
    pub(super) fn refresh_agent_changes(&mut self, cx: &mut Context<Self>) {
        if self.agent_changes_task.is_some() {
            self.agent_changes_dirty = true;
            return;
        }
        let Some(store) = monocode_core::checkpoint::store() else {
            return;
        };
        let mut projects = self.workspace.projects.clone();
        let pane_projects = self.reviews.keys().map(|id| self.pane_project(*id, cx));
        for project in std::iter::once(self.workspace.project.clone())
            .chain(pane_projects)
            .collect::<Vec<_>>()
        {
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
        let work = cx.background_executor().spawn(async move {
            projects
                .into_iter()
                .filter(|project| !project.is_empty() && project != "~")
                .filter_map(|project| {
                    let files = store.exclusive(|s| s.project_status(&project)).ok()?;
                    (!files.is_empty()).then_some((project, files))
                })
                .collect::<HashMap<_, _>>()
        });
        self.agent_changes_task = Some(cx.spawn(async move |this, cx| {
            let counts = work.await;
            // Files change many times a second while an agent works. Each
            // recount runs git per chat, so they start at most every 2 seconds.
            let pause = cx
                .background_executor()
                .timer(std::time::Duration::from_secs(2));
            let _ = this.update(cx, |this, cx| {
                if this.agent_changes != counts {
                    this.agent_changes = counts;
                    this.sync_agent_files(cx);
                    this.notify_sidebar(cx);
                    cx.notify();
                }
            });
            pause.await;
            let _ = this.update(cx, |this, cx| {
                this.agent_changes_task = None;
                // Edits in worktrees and submodules raise no Git event here,
                // so the count and the open review follow a working agent.
                let working = this.sessions.values().any(|view| view.read(cx).busy);
                if working {
                    for pane in this.reviews.values() {
                        pane.update(cx, |pane, cx| pane.refresh_agent(cx));
                    }
                }
                if std::mem::take(&mut this.agent_changes_dirty) || working {
                    this.refresh_agent_changes(cx);
                }
            });
        }));
    }
    /// Files with unread agent work in `project`.
    pub(super) fn agent_change_count(&self, project: &str) -> usize {
        self.agent_changes.get(project).map_or(0, Vec::len)
    }
    /// The project a file pane's chat works in, where its edits are recorded.
    fn pane_project(&self, pane: i64, cx: &App) -> String {
        self.sessions
            .get(&self.workspace.session_for_pane(pane))
            .map_or_else(
                || self.workspace.project.clone(),
                |view| view.read(cx).summary.cwd.clone(),
            )
    }
    /// Give each file pane its project's changed files, for the floating bar.
    pub(super) fn sync_agent_files(&self, cx: &mut Context<Self>) {
        for (id, pane) in &self.reviews {
            let project = self.pane_project(*id, cx);
            let files = self
                .agent_changes
                .get(&project)
                .cloned()
                .unwrap_or_default();
            pane.update(cx, |pane, cx| pane.set_agent_files(project, files, cx));
        }
    }
    pub(super) fn open_agent_changes(
        &mut self,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_review(
            crate::review::ReviewTab::Agent {
                cwd: project,
                path: None,
            },
            window,
            cx,
        );
    }
    /// The sidebar badge: how many files wait for review in `project`.
    pub(super) fn agent_changes_badge(
        &self,
        project: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let count = self.agent_change_count(project);
        if count == 0 {
            return None;
        }
        let project = project.to_owned();
        let green: gpui::Hsla = rgb(crate::right_panel::DIFF_ADD).into();
        Some(
            div()
                .id(SharedString::from(format!("agent-changes-{project}")))
                .role(Role::Button)
                .aria_label(SharedString::from(format!(
                    "Review agent changes in {count} {}",
                    if count == 1 { "file" } else { "files" }
                )))
                .flex_shrink_0()
                .h(px(18.))
                .min_w(px(18.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .rounded_full()
                .bg(green.opacity(0.18))
                .text_color(green)
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(move |s| s.bg(green.opacity(0.3)))
                .child(format!("{count}"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_agent_changes(project.clone(), window, cx)
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    #[gpui::test]
    fn the_badge_shows_only_for_projects_with_unread_agent_work(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.agent_changes_badge("/work/monocode", cx).is_none());
                let file = monocode_core::checkpoint::ProjectFile {
                    path: "/work/monocode/a".into(),
                    relative: "a".into(),
                    status: "modified".into(),
                    additions: 1,
                    deletions: 0,
                    sessions: Vec::new(),
                };
                shell
                    .agent_changes
                    .insert("/work/monocode".into(), vec![file; 3]);
                assert_eq!(shell.agent_change_count("/work/monocode"), 3);
                assert!(shell.agent_changes_badge("/work/monocode", cx).is_some());
                assert!(
                    shell
                        .agent_changes_badge("/work/another-project", cx)
                        .is_none()
                );
            })
            .unwrap();
    }
}
