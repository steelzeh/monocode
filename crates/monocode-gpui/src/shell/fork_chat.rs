//! "Fork chat": a new chat with the whole history of another one. The agent
//! copies its own session on the fork's first turn, so it remembers everything.
use super::*;

/// "Fork: <title>", without stacking prefixes when a fork is forked again.
pub(super) fn fork_title(title: &str) -> String {
    format!("Fork: {}", title.strip_prefix("Fork: ").unwrap_or(title))
}

impl Shell {
    /// None hides "Fork chat": remote chats, agents without a fork command, and
    /// chats the agent has no session for yet. Some(false) while a turn runs.
    pub(super) fn can_fork(&self, id: i64, cx: &App) -> Option<bool> {
        let meta = self.session_metadata(id, cx);
        let kind = Kind::from_id(&meta.summary.provider);
        if id < 0
            || !monocode_core::provider::supports_fork(kind)
            || meta.summary.provider_id.is_none()
        {
            return None;
        }
        Some(!meta.busy)
    }
    pub(super) fn fork_chat(&mut self, from: i64, cx: &mut Context<Self>) {
        if self.can_fork(from, cx) != Some(true) {
            return;
        }
        let title = fork_title(&self.session_metadata(from, cx).summary.title);
        let worktree = self.session_worktrees.get(&from).cloned();
        let navigation_revision = self.navigation_revision;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.fork_session(from, &title) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(summary) => {
                    let id = summary.id;
                    if let Some(worktree) = worktree {
                        this.session_worktrees.insert(id, worktree);
                    }
                    this.add(summary, cx);
                    this.order.retain(|s| *s != id);
                    this.order.insert(0, id);
                    if this.navigation_revision == navigation_revision {
                        this.activate_session(id, window, cx);
                    }
                    this.notify_sidebar(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};

    #[test]
    fn fork_titles_do_not_stack() {
        assert_eq!(fork_title("Plan"), "Fork: Plan");
        assert_eq!(fork_title("Fork: Plan"), "Fork: Plan");
    }

    #[gpui::test]
    fn fork_chat_opens_a_copy_that_forks_the_agent_session(cx: &mut TestAppContext) {
        let (window, _, claude) = crate::shell::tests::window(cx);
        let menu_has_fork = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| {
                    shell
                        .session_menu_items(claude, cx)
                        .iter()
                        .any(|item| matches!(item, MenuItem::Action(action) if action.id == "fork"))
                })
                .unwrap()
        };
        // Nothing to fork before the agent has a session.
        assert!(!menu_has_fork(&window, cx));
        window
            .update(cx, |shell, _, cx| {
                let store = locked(&shell.store).unwrap();
                store.provider_id(claude, "claude-source").unwrap();
                store.title(claude, "Plan the sidebar").unwrap();
                drop(store);
                shell.sessions[&claude].update(cx, |chat, _| {
                    chat.summary.provider_id = Some("claude-source".into());
                    chat.summary.title = "Plan the sidebar".into();
                    chat.summary.has_messages = true;
                });
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                locked(&shell.store)
                    .unwrap()
                    .write_rows(
                        claude,
                        0,
                        &[monocode_core::transcript::Row {
                            role: "user".into(),
                            text: "Remember PELICAN".into(),
                            first: true,
                            sent_at: None,
                        }],
                    )
                    .unwrap();
                let _ = cx;
            })
            .unwrap();
        assert!(menu_has_fork(&window, cx));
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Session(claude), "fork", None, w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let fork = shell.workspace.active.unwrap();
                assert_ne!(fork, claude);
                let summary = &shell.sessions[&fork].read(cx).summary;
                assert_eq!(summary.title, "Fork: Plan the sidebar");
                assert_eq!(summary.provider, "claude");
                assert!(summary.provider_id.is_none());
                let store = locked(&shell.store).unwrap();
                assert_eq!(store.load(fork).unwrap().rows[0].text, "Remember PELICAN");
                assert_eq!(
                    store.fork_source(fork, "claude").unwrap().as_deref(),
                    Some("claude-source")
                );
                assert_eq!(shell.order.first(), Some(&fork));
            })
            .unwrap();
        // The line "Forked from" is drawn after the one copied message.
        cx.refresh().unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("tool-1").is_some());
    }
}
