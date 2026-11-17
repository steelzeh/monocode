//! Linked chats: `@chat/…` in a message links this chat with another one, and
//! the two agents can then send each other messages.
use super::*;
use monocode_core::chat_links::{self, Chat};

/// The app CLI as an agent runs it from a shell.
fn app_cli() -> String {
    crate::runtime::shlex_quote(
        &std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy(),
    )
}

impl Conversation {
    /// Links this chat with the chats `text` mentions, and returns what the
    /// agent must know to reach them. Only the user links chats: a message
    /// from another chat links nothing.
    fn link_mentioned_chats(&self, text: &str) -> Option<String> {
        if self.is_remote() || self.settings.mono.is_some() || chat_links::is_received(text) {
            return None;
        }
        let mentioned = chat_links::mentioned(text);
        if mentioned.is_empty() {
            return None;
        }
        let store = locked(&self.store).ok()?;
        let sessions = store.sessions().ok()?;
        let chats: Vec<Chat> = mentioned
            .into_iter()
            .filter(|id| *id != self.summary.id)
            .filter_map(|id| sessions.iter().find(|session| session.id == id))
            .filter(|session| !session.cwd.starts_with("remote://"))
            .filter(|session| store.link_chats(self.summary.id, session.id).is_ok())
            .map(|session| Chat {
                id: session.id,
                title: monocode_core::session_title::display_title(
                    &session.title,
                    &session.provider,
                )
                .to_owned(),
                cwd: session.cwd.clone(),
            })
            .collect();
        (!chats.is_empty()).then(|| chat_links::sender_note(&app_cli(), &chats))
    }
    /// `prepared` with the note about the chats that `text` mentions.
    pub(super) fn with_linked_chats(&self, text: &str, prepared: String) -> String {
        match self.link_mentioned_chats(text) {
            // A slash command must stay first, or the agent reads it as text.
            Some(note) if prepared.trim_start().starts_with('/') => {
                format!("{prepared}\n\n{note}")
            }
            Some(note) => format!("{note}\n\n{prepared}"),
            None => prepared,
        }
    }
    /// The agent needs the app while this chat has a partner.
    pub(super) fn has_chat_links(&self) -> bool {
        locked(&self.store)
            .ok()
            .and_then(|store| store.chat_links(self.summary.id).ok())
            .is_some_and(|links| !links.is_empty())
    }
    /// A message from the agent of a linked chat. It is a queued message of
    /// its own: it never takes the text or the files the user has not sent,
    /// it waits while a turn runs, and it survives a restart.
    pub(crate) fn receive_chat_message(
        &mut self,
        from: Chat,
        body: String,
        cx: &mut Context<Self>,
    ) {
        if !self.history_ready() {
            self.defer_history_action(
                move |this, cx| this.receive_chat_message(from, body, cx),
                cx,
            );
            return;
        }
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let message = monocode_core::follow_up::Message {
            id: format!(
                "chat-{}-{}-{}-{}",
                from.id,
                self.summary.id,
                monocode_core::time::now_ms(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ),
            text: chat_links::received_text(&from.title, &body),
            prepared_prompt: Some(chat_links::receiver_prompt(&app_cli(), &from, &body)),
            // It waits for the turn to end. Only a turn that background work
            // keeps open, such as a dev server, gets it now.
            steer: !self.background.tasks.is_empty()
                && Kind::from_id(&self.summary.provider).can_steer(),
            ..Default::default()
        };
        // A queue the user paused with Stop must not hold a coworker's
        // message back. The older queued messages stay paused.
        if !self.busy || self.follow_ups.state.paused {
            self.follow_ups.continuations.push_back(message.id.clone());
            if self.follow_ups.requested.is_none() {
                self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
            }
        }
        self.follow_ups.motion.insert(
            &message.id,
            self.follow_ups.state.messages.is_empty(),
            !cx.reduce_motion(),
        );
        self.follow_ups.state.messages.push(message);
        self.save_follow_ups(cx);
        cx.emit(SessionChanged);
    }
    /// Whether a chat dragged from the sidebar to `position` is on the message
    /// box of a chat that can mention it. Only the box counts: a drop on the
    /// rest of the pane still splits it.
    pub(crate) fn accepts_chat_mention(
        &self,
        position: gpui::Point<gpui::Pixels>,
        cx: &gpui::App,
    ) -> bool {
        !self.is_remote()
            && self.settings.mono.is_none()
            && self
                .completion
                .read(cx)
                .composer_bounds()
                .is_some_and(|bounds| bounds.contains(&position))
    }
    pub(crate) fn set_chat_drop(&mut self, over: bool, cx: &mut Context<Self>) {
        if self.chat_drop != over {
            self.chat_drop = over;
            cx.notify();
        }
    }
    /// Writes the mention of a chat at the end of the message box.
    pub(crate) fn mention_chat(
        &mut self,
        id: i64,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat_drop = false;
        if id == self.summary.id || self.input.read(cx).is_composing() {
            cx.notify();
            return;
        }
        let mention = chat_links::mention(id, title);
        let current = self.input.read(cx).content.to_string();
        let gap = if current.is_empty() || current.ends_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        self.completion
            .update(cx, |completion, _| completion.know_token(mention.clone()));
        self.input.update(cx, |input, cx| {
            input.set_text(format!("{current}{gap}{mention} "), cx)
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    pub(super) fn chat_drop_overlay(&self, radius: f32) -> Option<AnyElement> {
        self.chat_drop.then(|| {
            div()
                .absolute()
                .inset_0()
                .rounded(px(radius))
                .bg(accent_color().opacity(0.08))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.7))
                .child("Drop to mention this chat")
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::store::Store;
    use std::sync::Mutex;

    /// A chat that fails after it saved the turn, so no real agent starts,
    /// and the id of a second chat in another folder.
    fn chat(cx: &mut TestAppContext) -> (WindowHandle<Conversation>, i64) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let mut summary = store.create("codex", "/missing/billing").unwrap();
        summary.provider = "fixture".into();
        let other = store.create("codex", "/missing/api").unwrap();
        store.title(other.id, "Run the tests").unwrap();
        let window = cx.update(|cx| {
            input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view
                })
            })
            .unwrap()
        });
        (window, other.id)
    }
    fn sender(id: i64) -> Chat {
        Chat {
            id,
            title: "Billing fix".into(),
            cwd: "/missing/billing".into(),
        }
    }

    #[gpui::test]
    fn a_mention_links_the_chats_and_tells_the_agent_how_to_reach_the_other(
        cx: &mut TestAppContext,
    ) {
        let (window, other) = chat(cx);
        window
            .update(cx, |view, _, _| {
                let own = view.summary.id;
                let mention = chat_links::mention(other, "Run the tests");
                assert!(!view.has_chat_links());
                // A chat that does not exist, and this chat itself, link nothing.
                let plain = view.with_linked_chats(
                    &format!("Ask @chat/gone-999 and @chat/me-{own}"),
                    "prepared".into(),
                );
                assert_eq!(plain, "prepared");
                assert!(!view.has_chat_links());
                // Neither does a message that came from another chat.
                let received = chat_links::received_text("Billing fix", &mention);
                assert_eq!(
                    view.with_linked_chats(&received, "prepared".into()),
                    "prepared"
                );
                assert!(!view.has_chat_links());

                let prepared =
                    view.with_linked_chats(&format!("Ask {mention} to run"), "prepared".into());
                assert!(prepared.starts_with("<linked_chats>"), "{prepared}");
                assert!(prepared.ends_with("</linked_chats>\n\nprepared"));
                assert!(prepared.contains("Chat \"Run the tests\""));
                assert!(prepared.contains(&format!(
                    "app sessions.send --json '{{\"sessionId\":{other},"
                )));
                assert!(view.has_chat_links());
                let store = locked(&view.store).unwrap();
                assert!(store.chats_linked(own, other).unwrap());
                assert!(store.chats_linked(other, own).unwrap());
                drop(store);
                // A slash command stays first, so the agent still runs it.
                let command = view.with_linked_chats(&mention, "/review now".into());
                assert!(command.starts_with("/review now\n\n<linked_chats>"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_message_from_a_linked_chat_starts_a_turn_and_keeps_the_unsent_draft(
        cx: &mut TestAppContext,
    ) {
        let (window, other) = chat(cx);
        window
            .update(cx, |view, _, cx| {
                view.input
                    .update(cx, |input, cx| input.set_text("my unsent draft".into(), cx));
                view.attachments.push("/missing/unsent.png".into());
                // Stop left the queue paused. The message must still go out.
                view.follow_ups.state.paused = true;
                view.receive_chat_message(sender(other), " run the tests ".into(), cx);
                let message = &view.follow_ups.state.messages[0];
                assert_eq!(
                    message.text,
                    "Message from chat \"Billing fix\":\n\nrun the tests"
                );
                let prepared = message.prepared_prompt.as_deref().unwrap();
                assert!(prepared.starts_with(&format!(
                    "<chat_message from_session=\"{other}\" from_title=\"Billing fix\">\nrun the tests\n"
                )));
                assert!(message.attachments.is_empty());
                assert_eq!(view.follow_ups.requested.as_ref(), Some(&message.id));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(rows[0].role, "user");
                assert_eq!(
                    rows[0].text,
                    "Message from chat \"Billing fix\":\n\nrun the tests"
                );
                assert_eq!(view.input.read(cx).content, "my unsent draft");
                assert_eq!(view.attachments.len(), 1);
                // The agent's own text links nothing.
                assert!(!view.has_chat_links());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_message_to_a_busy_chat_waits_for_the_turn_to_end(cx: &mut TestAppContext) {
        let (window, other) = chat(cx);
        window
            .update(cx, |view, _, cx| {
                view.busy = true;
                view.receive_chat_message(sender(other), "first".into(), cx);
                view.receive_chat_message(sender(other), "second".into(), cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let queued: Vec<_> = view
                    .follow_ups
                    .state
                    .messages
                    .iter()
                    .map(|message| (message.text.rsplit('\n').next().unwrap(), message.steer))
                    .collect();
                assert_eq!(queued, vec![("first", false), ("second", false)]);
                assert!(view.follow_ups.requested.is_none());
                assert!(view.transcript.read(cx).source_rows().is_empty());
                assert_ne!(
                    view.follow_ups.state.messages[0].id,
                    view.follow_ups.state.messages[1].id
                );
            })
            .unwrap();
    }
}
