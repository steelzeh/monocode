//! Chat titles set from outside the rename field, and marking a chat as busy.
use super::*;

impl Shell {
    /// Saves an exact title, as the rename field does. Later automatic naming
    /// keeps it, because the session no longer has its placeholder title.
    pub(super) fn set_session_title(&mut self, id: i64, title: String, cx: &mut Context<Self>) {
        if !self.session_summaries.contains_key(&id)
            || self.session_metadata(id, cx).summary.title == title
        {
            return;
        }
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.title = title.clone();
        }
        if let Some(session) = self.sessions.get(&id) {
            session.update(cx, |s, cx| s.rename(title.clone(), cx));
        }
        self.store_write("rename the session", cx, move |s| s.title(id, &title));
        self.invalidate_chrome(cx);
        cx.notify();
    }

    pub(super) fn tweak_working_on(&self, chat: i64, owner: &str) -> bool {
        self.working_titles
            .get(&chat)
            .is_some_and(|(mark, _)| mark == owner)
    }

    pub(super) fn set_title_working(&mut self, chat: i64, owner: &str, working: bool) {
        if working {
            self.working_titles
                .insert(chat, (owner.to_owned(), std::time::Instant::now()));
        } else {
            self.working_titles.remove(&chat);
        }
    }

    pub(super) fn release_working_titles(&mut self, owner: &str) {
        self.working_titles.retain(|_, (mark, _)| mark != owner);
    }

    /// A sidebar row's title. It shimmers while a chat is marked as working.
    pub(super) fn session_title_text(
        &self,
        id: i64,
        title: String,
        line_height: f32,
    ) -> AnyElement {
        let Some((_, started)) = self.working_titles.get(&id) else {
            return crate::particle_text::particle_text(title).into_any_element();
        };
        div()
            .debug_selector(move || format!("session-title-shimmer-{id}"))
            .w_full()
            .line_height(px(line_height))
            .truncate()
            .child(crate::activity::shimmer(
                title,
                started.elapsed().as_secs_f32(),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn chat(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, i64) {
        let (window, _, _) = super::super::tests::window(cx);
        let id = window
            .update(cx, |shell, _, cx| {
                let cwd = shell.workspace.project.clone();
                let mut summary = locked(&shell.store).unwrap().create("codex", &cwd).unwrap();
                summary.has_messages = true;
                summary.title = "Fork: Old title".into();
                let id = summary.id;
                shell.session_summaries.insert(id, summary);
                shell.order.insert(0, id);
                shell.notify_sidebar(cx);
                id
            })
            .unwrap();
        (window, id)
    }

    #[gpui::test]
    fn the_row_title_shimmers_only_while_a_tweak_works_on_it(cx: &mut TestAppContext) {
        let (window, id) = chat(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let shimmer: &'static str =
            Box::leak(format!("session-title-shimmer-{id}").into_boxed_str());
        let row: &'static str = Box::leak(format!("sidebar-session-{id}").into_boxed_str());
        assert!(visual.debug_bounds(shimmer).is_none());
        window
            .update(visual, |shell, _, cx| {
                shell
                    .working_titles
                    .insert(id, ("t".into(), std::time::Instant::now()));
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        visual.run_until_parked();
        let title = visual.debug_bounds(shimmer).unwrap();
        let row = visual.debug_bounds(row).unwrap();
        assert!(title.top() >= row.top() && title.bottom() <= row.bottom());
        assert!(title.right() <= row.right());
        window
            .update(visual, |shell, _, cx| {
                shell.working_titles.remove(&id);
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds(shimmer).is_none());
    }

    #[gpui::test]
    fn set_session_title_saves_for_unopened_chats(cx: &mut TestAppContext) {
        let (window, id) = chat(cx);
        window
            .update(cx, |shell, _, cx| {
                assert!(!shell.sessions.contains_key(&id));
                shell.set_session_title(id, "Project folders sidebar".into(), cx);
                assert_eq!(
                    shell.session_metadata(id, cx).summary.title,
                    "Project folders sidebar"
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let saved = locked(&shell.store).unwrap().sessions().unwrap();
                assert!(
                    saved
                        .iter()
                        .any(|s| s.id == id && s.title == "Project folders sidebar")
                );
            })
            .unwrap();
    }
}
