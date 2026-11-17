//! Decides which app calls a tweak may make. Everything here is enforced
//! outside the sandbox, whatever the script asked for.
use super::*;
use crate::app_access::tweak_permission;

const MAX_AT_ONCE: usize = 8;
const MAX_PER_CLICK: usize = 16;
const MAX_TOKEN_CALLS_PER_CLICK: usize = 4;

/// What started a chain of calls. An answer continues the chain it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    id: u64,
    clicked: bool,
}

#[derive(Debug, Default)]
struct Spent {
    calls: usize,
    token_calls: usize,
}

#[derive(Debug, Default)]
pub struct Guard {
    permissions: Vec<String>,
    chats: HashSet<String>,
    next_origin: u64,
    waiting: HashMap<u64, Origin>,
    spent: HashMap<u64, Spent>,
}

impl Guard {
    pub fn new(permissions: &[String]) -> Self {
        Self {
            permissions: permissions.to_vec(),
            ..Self::default()
        }
    }
    fn origin(&mut self, clicked: bool) -> Origin {
        self.next_origin += 1;
        Origin {
            id: self.next_origin,
            clicked,
        }
    }
    /// The user clicked one of the tweak's controls or menu items.
    pub fn click(&mut self) -> Origin {
        self.origin(true)
    }
    /// The app asked a panel to load its data.
    pub fn refresh(&mut self) -> Origin {
        self.origin(false)
    }
    /// The user clicked the tweak on this chat, so it may act on it.
    pub fn allow_chat(&mut self, id: &str) {
        self.chats.insert(id.to_owned());
    }
    /// The chain an answer belongs to, once. None for a call never made.
    pub fn answered(&mut self, request: u64) -> Option<Origin> {
        self.waiting.remove(&request)
    }
    /// The calls to run, or why the tweak must be stopped.
    pub fn admit(&mut self, origin: Origin, calls: Vec<AppCall>) -> Result<Vec<AppCall>, String> {
        let mut seen = HashSet::new();
        let mut spent = 0;
        let mut token_calls = 0;
        for call in &calls {
            let Some(permission) = tweak_permission(&call.action).filter(|p| p.callable) else {
                return Err(format!("called the unknown action {}", call.action));
            };
            if !self.permissions.contains(&call.action) {
                return Err(format!("called {} without permission", call.action));
            }
            if !origin.clicked && !permission.read_only {
                return Err(format!("called {} without a click", call.action));
            }
            if let Some(field) = permission.clicked_chat {
                let clicked = call.input[field]
                    .as_str()
                    .is_some_and(|chat| self.chats.contains(chat));
                if !clicked {
                    return Err(format!(
                        "called {} on a chat you did not click",
                        call.action
                    ));
                }
            }
            if !seen.insert(call.request) || self.waiting.contains_key(&call.request) {
                return Err("reused a request number".into());
            }
            spent += 1;
            token_calls += usize::from(permission.spends_tokens);
        }
        let so_far = self.spent.entry(origin.id).or_default();
        so_far.calls += spent;
        so_far.token_calls += token_calls;
        if self.waiting.len() + calls.len() > MAX_AT_ONCE
            || so_far.calls > MAX_PER_CLICK
            || so_far.token_calls > MAX_TOKEN_CALLS_PER_CLICK
        {
            return Err("called the app too many times".into());
        }
        self.waiting
            .extend(calls.iter().map(|call| (call.request, origin)));
        let waiting = &self.waiting;
        self.spent
            .retain(|id, _| *id == origin.id || waiting.values().any(|o| o.id == *id));
        Ok(calls)
    }
}

#[cfg(test)]
mod tests;
