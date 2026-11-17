//! Forking a chat: a new chat starts with a copy of another chat's history.
//! The provider copies its own context on the new chat's first turn, from the
//! source recorded here, so the agent remembers the whole conversation.
use super::*;
use crate::tool::{ToolCall, ToolState};
use serde_json::Value;

fn fork_key(id: i64) -> String {
    format!("fork-source:{id}")
}

impl Store {
    /// Creates a fork of `from` named `title`. The source chat is not changed.
    pub fn fork_session(&mut self, from: i64, title: &str) -> Result<Session, String> {
        if from < 0 || self.remote_session(from)?.is_some() {
            return Err("Chats on another machine cannot be forked yet".into());
        }
        let source = self
            .sessions()?
            .into_iter()
            .find(|session| session.id == from)
            .ok_or("This chat was removed")?;
        if !crate::provider::supports_fork(crate::provider::Kind::from_id(&source.provider)) {
            return Err("This agent cannot fork a chat".into());
        }
        let provider_id = source
            .provider_id
            .clone()
            .filter(|id| !id.trim().is_empty())
            .ok_or("Send a message in this chat before you fork it")?;
        let mut settings = self.agent_settings(from)?.unwrap_or_default();
        settings.handoff = None;
        settings.pending_provider_switch = None;
        settings.usage_limit = None;
        let mut session = self.create(&source.provider, &source.cwd)?;
        if self.shared.is_some() {
            // Turn receipts live inside the blocks, so they come with the copy.
            let blocks: Vec<Value> = self
                .shared_all_blocks(from)?
                .into_iter()
                .filter(|block| block["draft"] != true)
                .collect();
            self.shared_store_blocks(session.id, &blocks)?;
            settings.turns.clear();
        } else {
            let rows = self.load(from)?.rows;
            self.write_rows(session.id, 0, &rows)?;
        }
        let copied = self.load(session.id)?.rows.len();
        let now = crate::time::now_ms();
        let mut marker = ToolCall::new("chat-fork", crate::tool::FORK);
        marker.input = serde_json::json!({"from": source.title, "at": now}).to_string();
        marker.state = ToolState::Completed;
        self.write_rows(
            session.id,
            copied,
            &[Row {
                role: "tool".into(),
                text: serde_json::to_string(&marker).map_err(|e| e.to_string())?,
                first: true,
                sent_at: Some(now),
            }],
        )?;
        self.configure_agent(session.id, &source.provider, &settings)?;
        if let Some(usage) = self.context_usage(from)? {
            self.save_context_usage(session.id, &usage)?;
        }
        self.save_setting(
            &fork_key(session.id),
            &format!("{}:{provider_id}", source.provider),
        )?;
        self.title(session.id, title)?;
        session.title = title.into();
        session.has_messages = true;
        Ok(session)
    }
    /// The `provider` session a fork still has to copy, until its first turn
    /// binds it. None once the chat moved to another provider.
    pub fn fork_source(&self, id: i64, provider: &str) -> Result<Option<String>, String> {
        Ok(self.setting(&fork_key(id))?.and_then(|value| {
            value
                .strip_prefix(provider)
                .and_then(|rest| rest.strip_prefix(':'))
                .filter(|source| !source.is_empty())
                .map(str::to_owned)
        }))
    }
    pub fn clear_fork_source(&self, id: i64) -> Result<(), String> {
        if self
            .setting(&fork_key(id))?
            .is_some_and(|value| !value.is_empty())
        {
            self.save_setting(&fork_key(id), "")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Row;
    use std::path::Path;

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }

    fn forks_with_history_settings_and_source(mut store: Store) {
        let source = store.create("claude", "/project").unwrap();
        let settings = crate::agent::Settings {
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        store
            .configure_agent(source.id, "claude", &settings)
            .unwrap();
        // A chat without a provider session has nothing for the agent to copy.
        assert!(store.fork_session(source.id, "Fork: Plan").is_err());
        store
            .write_rows(
                source.id,
                0,
                &[row("user", "Remember PELICAN"), row("assistant", "OK")],
            )
            .unwrap();
        store.provider_id(source.id, "claude-source").unwrap();

        let fork = store.fork_session(source.id, "Fork: Plan").unwrap();
        assert_ne!(fork.id, source.id);
        assert_eq!(fork.title, "Fork: Plan");
        assert_eq!(fork.provider, "claude");
        assert_eq!(fork.cwd, "/project");
        assert!(fork.provider_id.is_none());
        let rows = store.load(fork.id).unwrap().rows;
        assert_eq!(
            rows[..2]
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>(),
            ["Remember PELICAN", "OK"]
        );
        // The copied history ends with a line that names where it came from.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].role, "tool");
        let marker: ToolCall = serde_json::from_str(&rows[2].text).unwrap();
        assert!(marker.is_milestone());
        assert_eq!(marker.state, ToolState::Completed);
        assert_eq!(marker.fork_origin().unwrap().0, source.title);
        assert!(marker
            .summary()
            .0
            .starts_with(&format!("Forked from {} · ", source.title)));
        let saved = store.agent_settings(fork.id).unwrap().unwrap();
        assert_eq!(saved.model.as_deref(), Some("opus"));
        assert_eq!(saved.effort.as_deref(), Some("high"));
        let listed = store.sessions().unwrap();
        let listed = listed.iter().find(|s| s.id == fork.id).unwrap();
        assert_eq!(listed.title, "Fork: Plan");

        // The source is only offered to the provider it belongs to.
        assert_eq!(
            store.fork_source(fork.id, "claude").unwrap().as_deref(),
            Some("claude-source")
        );
        assert_eq!(store.fork_source(fork.id, "codex").unwrap(), None);
        assert_eq!(store.fork_source(source.id, "claude").unwrap(), None);
        store.clear_fork_source(fork.id).unwrap();
        assert_eq!(store.fork_source(fork.id, "claude").unwrap(), None);

        // The source chat is unchanged.
        assert_eq!(store.load(source.id).unwrap().rows.len(), 2);
        assert_eq!(
            store
                .sessions()
                .unwrap()
                .iter()
                .find(|s| s.id == source.id)
                .unwrap()
                .provider_id
                .as_deref(),
            Some("claude-source")
        );
    }

    #[test]
    fn a_fork_copies_the_chat_in_the_native_store() {
        forks_with_history_settings_and_source(Store::open(Path::new(":memory:")).unwrap());
    }

    #[test]
    fn a_fork_copies_the_chat_in_the_shared_store() {
        forks_with_history_settings_and_source(
            Store::open_monocode(Path::new(":memory:")).unwrap(),
        );
    }

    #[test]
    fn agents_without_a_fork_command_are_refused() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let source = store.create("pi", "/project").unwrap();
        store.provider_id(source.id, "pi-session").unwrap();
        assert!(store.fork_session(source.id, "Fork").is_err());
    }
}
