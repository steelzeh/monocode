//! A chat dragged from the sidebar onto a message box is mentioned there,
//! which links the two chats when the message is sent.
use super::*;

impl Shell {
    /// The chat whose message box is under a chat dragged from the sidebar.
    pub(super) fn chat_mention_drop_at(
        &self,
        position: Point<Pixels>,
        source: i64,
        cx: &App,
    ) -> Option<i64> {
        if !self
            .session_summaries
            .get(&source)
            .is_some_and(|summary| !summary.cwd.starts_with("remote://"))
        {
            return None;
        }
        let (id, _) = self.pane_hit(position, cx)?;
        (id != source
            && self
                .sessions
                .get(&id)?
                .read(cx)
                .accepts_chat_mention(position, cx))
        .then_some(id)
    }
    pub(super) fn set_chat_mention_drop(&mut self, target: Option<i64>, cx: &mut Context<Self>) {
        if self.chat_mention_drop == target {
            return;
        }
        for (id, over) in [(self.chat_mention_drop, false), (target, true)] {
            if let Some(view) = id.and_then(|id| self.sessions.get(&id)) {
                view.update(cx, |chat, cx| chat.set_chat_drop(over, cx));
            }
        }
        self.chat_mention_drop = target;
    }
    /// Writes the mention of the dragged chat. True when the drop was used.
    pub(super) fn finish_chat_mention_drop(
        &mut self,
        source: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.chat_mention_drop.take() else {
            return false;
        };
        let summary = self.session_metadata(source, cx).summary;
        let title = monocode_core::session_title::display_title(&summary.title, &summary.provider)
            .to_owned();
        if let Some(view) = self.sessions.get(&target).cloned() {
            view.update(cx, |chat, cx| chat.mention_chat(source, &title, window, cx));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn a_chat_dropped_on_a_message_box_is_mentioned_there(cx: &mut TestAppContext) {
        let (window, other, chat) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                locked(&shell.store)
                    .unwrap()
                    .title(other, "Run the tests")
                    .unwrap();
                shell.session_summaries.get_mut(&other).unwrap().title = "Run the tests".into();
                if let Some(view) = shell.sessions.get(&other) {
                    view.update(cx, |view, _| view.summary.title = "Run the tests".into());
                }
                shell.sessions[&chat].update(cx, |view, cx| {
                    view.insert_composer_text("Ask", cx);
                });
                // Nothing is under the pointer: the drop is not used.
                assert!(!shell.finish_chat_mention_drop(other, window, cx));
                shell.set_chat_mention_drop(Some(chat), cx);
                assert!(shell.finish_chat_mention_drop(other, window, cx));
                assert!(shell.chat_mention_drop.is_none());
                assert_eq!(
                    shell.sessions[&chat].read(cx).composer_text_for_test(cx),
                    format!("Ask @chat/run-the-tests-{other} ")
                );
            })
            .unwrap();
    }
    #[gpui::test]
    fn a_sidebar_chat_dragged_onto_the_message_box_is_mentioned_and_splits_nothing(
        cx: &mut TestAppContext,
    ) {
        use gpui::{Modifiers, MouseButton, point, px};
        let (window, a, b) = crate::shell::tests::window(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        window
            .update(cx, |shell, w, cx| {
                for id in [a, b] {
                    shell.sessions[&id].update(cx, |chat, _| chat.summary.has_messages = true);
                }
                shell.sessions[&a].update(cx, |chat, _| {
                    chat.summary.cwd = "/work/monocode".into();
                });
                shell.open(b, w, cx);
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        let row = cx
            .debug_bounds(Box::leak(format!("sidebar-session-{a}").into_boxed_str()))
            .unwrap()
            .center();
        let message_box = cx.debug_bounds("composer-box").unwrap().center();
        cx.simulate_mouse_down(row, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            row + point(px(20.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(message_box, MouseButton::Left, Modifiers::default());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.chat_mention_drop, Some(b));
                // No split is offered while the chat is over the message box.
                assert_eq!(shell.pane_drag.as_ref().unwrap().over, None);
            })
            .unwrap();
        cx.simulate_mouse_up(message_box, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let text = shell.sessions[&b].read(cx).composer_text_for_test(cx);
                assert!(text.starts_with("@chat/"), "{text}");
                assert!(text.ends_with(&format!("-{a} ")), "{text}");
                assert_eq!(shell.workspace.layout(b).leaf_ids(), [b]);
                assert!(shell.pane_drag.is_none() && shell.chat_mention_drop.is_none());
            })
            .unwrap();
    }
}
