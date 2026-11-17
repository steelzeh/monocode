//! What tweaks may ask the app to do. Each permission is the name of an app
//! action, so the tweak system itself needs to know nothing about any of them.

pub struct TweakPermission {
    pub name: &'static str,
    /// What the user reads in Settings before turning a tweak on.
    pub label: &'static str,
    pub icon: &'static str,
    pub spends_tokens: bool,
    /// Safe without a click, because it changes nothing.
    pub read_only: bool,
    /// The input field naming a chat, which must be a chat the user clicked.
    pub clicked_chat: Option<&'static str>,
    /// False for a permission that is not an action a script can call.
    pub callable: bool,
}

const fn action(name: &'static str, label: &'static str, icon: &'static str) -> TweakPermission {
    TweakPermission {
        name,
        label,
        icon,
        spends_tokens: false,
        read_only: false,
        clicked_chat: None,
        callable: true,
    }
}

/// Lets a tweak send the prompts in its `tweak.json`. Not an action.
pub const SEND_PROMPTS: &str = "send_prompts";

pub const TWEAK_PERMISSIONS: &[TweakPermission] = &[
    TweakPermission {
        read_only: true,
        ..action(
            "sessions.list",
            "Sees this project's chats",
            "message-multiple",
        )
    },
    TweakPermission {
        read_only: true,
        clicked_chat: Some("sessionId"),
        ..action("sessions.read", "Reads chat messages", "message-square")
    },
    TweakPermission {
        clicked_chat: Some("sessionId"),
        ..action("sessions.rename", "Renames chats", "pencil-edit-02")
    },
    TweakPermission {
        clicked_chat: Some("sessionId"),
        ..action("sessions.setWorking", "Marks chats as working", "loader")
    },
    TweakPermission {
        read_only: true,
        ..action("worktrees.list", "Sees worktrees", "git-branch")
    },
    action("worktrees.select", "Switches worktrees", "git-branch"),
    action(
        "composer.insert",
        "Writes in your message box",
        "pencil-edit",
    ),
    TweakPermission {
        spends_tokens: true,
        ..action("models.ask", "Uses your model and tokens", "sparkles")
    },
    TweakPermission {
        spends_tokens: true,
        callable: false,
        ..action(SEND_PROMPTS, "Sends prompts to your agent", "mono-send")
    },
];

pub fn tweak_permission(name: &str) -> Option<&'static TweakPermission> {
    TWEAK_PERMISSIONS.iter().find(|item| item.name == name)
}
