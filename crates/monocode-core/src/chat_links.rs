//! Linked chats. The user links two chats with `@chat/…` in a message. Their
//! agents can then read each other and exchange messages, in both directions.
use serde_json::Value;

/// A chat as the other chat's agent needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub cwd: String,
}

/// `@chat/<title>-<id>`. The title makes it readable, the id finds the chat.
pub fn mention(id: i64, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("@chat/{id}")
    } else {
        format!("@chat/{slug}-{id}")
    }
}

/// The chats a message mentions, in order and once each. Quoted lines do not
/// count, as for file mentions.
pub fn mentioned(text: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    for line in text
        .lines()
        .filter(|line| !line.trim_start().starts_with('>'))
    {
        for word in line.split_whitespace() {
            let word = word
                .trim_start_matches(['(', '[', '{'])
                .trim_end_matches([')', ']', '}', '.', '!', '?', ';', ',', ':']);
            if let Some(id) = word.strip_prefix("@chat/").and_then(mention_id) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}
fn mention_id(rest: &str) -> Option<i64> {
    if rest.is_empty()
        || !rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    rest.rsplit('-').next()?.parse().ok().filter(|id| *id > 0)
}

const RECEIVED: &str = "Message from chat ";

/// What the receiving chat shows for a message from another chat.
pub fn received_text(from_title: &str, body: &str) -> String {
    format!("{RECEIVED}\"{}\":\n\n{}", one_line(from_title), body.trim())
}
/// A message that came from another chat. Its mentions link nothing: only the
/// user links chats.
pub fn is_received(text: &str) -> bool {
    text.starts_with(RECEIVED)
}
fn one_line(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(80).collect()
}

const RULES: &str = "The other agent does not see this conversation, so write each message so that it stands alone. A message starts a turn in the other chat, or waits there until its current turn ends. An answer arrives here later as a new message that starts with \"Message from chat\", so do not wait or poll for it. Do not send a message only to thank or to confirm.";

fn commands(cli: &str, id: i64) -> String {
    format!(
        "Send a message: {cli} app sessions.send --json '{{\"sessionId\":{id},\"prompt\":\"<message>\"}}'\nRead its latest messages: {cli} app sessions.read --json '{{\"sessionId\":{id}}}'"
    )
}

/// Told to the agent of a chat when its user mentions other chats.
pub fn sender_note(cli: &str, chats: &[Chat]) -> String {
    let mut note = String::from(
        "<linked_chats>\nThe user linked this chat with the MonoCode chats below. Each one has its own agent, and you can work with it as with a coworker.\n",
    );
    for chat in chats {
        note.push_str(&format!(
            "\nChat \"{}\" (sessionId {}, folder {})\n{}\n",
            one_line(&chat.title),
            chat.id,
            chat.cwd,
            commands(cli, chat.id)
        ));
    }
    note.push_str(&format!(
        "\nFor a long message, write the JSON to a file and pass --input FILE. {RULES}\n</linked_chats>"
    ));
    note
}

/// What the agent of the receiving chat gets instead of [`received_text`].
pub fn receiver_prompt(cli: &str, from: &Chat, body: &str) -> String {
    format!(
        "<chat_message from_session=\"{}\" from_title=\"{}\">\n{}\n</chat_message>\n\nThe agent of another MonoCode chat of this user sent the message above. The user linked the two chats, so treat it as a request from a coworker and do the work with the access that this chat has. To answer, or to report a result:\n{}\nFor a long message, write the JSON to a file and pass --input FILE. {RULES}",
        from.id,
        one_line(&from.title).replace('"', "'"),
        body.trim(),
        commands(cli, from.id)
    )
}

/// The title of the chat a finished `sessions.send` reached, read from what
/// the app CLI printed.
pub fn sent_to(output: &str) -> Option<String> {
    let parse = |text: &str| serde_json::from_str::<Value>(text.trim()).ok();
    parse(output)
        .into_iter()
        .chain(output.lines().filter_map(parse))
        .find(|value| value["ok"] == true)
        .and_then(|value| value["result"]["title"].as_str().map(one_line))
        .filter(|title| !title.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_carries_a_readable_title_and_the_chat_id() {
        assert_eq!(mention(34, "Run the tests"), "@chat/run-the-tests-34");
        assert_eq!(mention(7, "Fix #12: login!"), "@chat/fix-12-login-7");
        assert_eq!(mention(9, "漢字"), "@chat/9");
        assert_eq!(
            mention(5, &"long title ".repeat(20)),
            "@chat/long-title-long-title-long-title-5"
        );
    }

    #[test]
    fn mentioned_chats_are_found_once_and_not_in_quotes() {
        assert_eq!(
            mentioned("Ask @chat/run-the-tests-34 and (@chat/9), then @chat/fix-12-login-7."),
            vec![34, 9, 7]
        );
        assert_eq!(mentioned("@chat/a-3 again @chat/b-3"), vec![3]);
        assert_eq!(mentioned("> @chat/quoted-4\nplain"), Vec::<i64>::new());
        // Not a chat: a file path, an email, and a mention without an id.
        assert_eq!(
            mentioned("@chat/Notes.md me@chat/5 @chat/ @chat/tests"),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn a_received_message_names_its_chat_and_is_recognized() {
        let shown = received_text("Billing\nfix", "  run the tests \n");
        assert_eq!(shown, "Message from chat \"Billing fix\":\n\nrun the tests");
        assert!(is_received(&shown));
        assert!(!is_received("Please message the chat"));
    }

    #[test]
    fn both_agents_learn_how_to_reach_the_other_chat() {
        let chat = Chat {
            id: 34,
            title: "Run \"the\" tests".into(),
            cwd: "/work/api".into(),
        };
        let note = sender_note("\"/app/monocode-gpui\"", std::slice::from_ref(&chat));
        assert!(note.contains(
            "\"/app/monocode-gpui\" app sessions.send --json '{\"sessionId\":34,\"prompt\":\"<message>\"}'"
        ));
        assert!(note.contains("app sessions.read --json '{\"sessionId\":34}'"));
        assert!(note.contains("folder /work/api"));
        let prompt = receiver_prompt("cli", &chat, " do it ");
        assert!(prompt.starts_with(
            "<chat_message from_session=\"34\" from_title=\"Run 'the' tests\">\ndo it\n</chat_message>"
        ));
        assert!(prompt.contains("cli app sessions.send --json '{\"sessionId\":34,"));
    }

    #[test]
    fn the_sent_caption_reads_the_title_from_the_cli_output() {
        assert_eq!(
            sent_to("{\"ok\":true,\"result\":{\"sessionId\":3,\"title\":\"Tests\"},\"requestId\":\"a\"}")
                .as_deref(),
            Some("Tests")
        );
        assert_eq!(
            sent_to("warning: slow\n{\"ok\":true,\"result\":{\"title\":\"Tests\"}}\n").as_deref(),
            Some("Tests")
        );
        assert_eq!(
            sent_to("{\"ok\":false,\"error\":\"Session is busy\"}"),
            None
        );
        assert_eq!(sent_to("{\"ok\":true,\"result\":{\"sessionId\":3}}"), None);
    }
}
