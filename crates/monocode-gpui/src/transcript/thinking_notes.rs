//! Claude often writes its update to the user only in a thinking note: no
//! message text comes before its next tool call or question. The chat then
//! shows nothing for that step, as thinking folds into the work rows. Such a
//! note shows as a message instead. Only this view's copy changes; the saved
//! chat keeps the note as thinking.
use super::*;

/// A message: the row that starts it and the row after its end.
fn message_end(rows: &[Row], start: usize) -> usize {
    (start + 1..rows.len())
        .find(|&at| rows[at].first)
        .unwrap_or(rows.len())
}

fn is_empty(rows: &[Row], start: usize, end: usize) -> bool {
    rows[start..end]
        .iter()
        .all(|row| row.text.trim().is_empty())
}

/// Thinking, or an empty message: neither is words the user can read.
fn unread(rows: &[Row], start: usize, end: usize) -> bool {
    rows[start].role == "reasoning"
        || (rows[start].role == "assistant" && is_empty(rows, start, end))
}

/// Show each settled thinking note that has no message text after it as a
/// message. A note settles once the next tool call or prompt arrives, so a
/// note still being written stays as thinking. Returns the first changed row,
/// or `from` when none changed.
pub(super) fn promote_notes(rows: &mut [Row], from: usize) -> usize {
    // A tool row arriving at `from` settles the notes just before it.
    let mut at = from.min(rows.len());
    while let Some(previous) = rows[..at].iter().rposition(|row| row.first) {
        at = previous;
        if !unread(rows, previous, message_end(rows, previous)) {
            break;
        }
    }
    let mut lowest = from;
    let mut start = at;
    while start < rows.len() {
        let end = message_end(rows, start);
        if rows[start].first && rows[start].role == "reasoning" && !is_empty(rows, start, end) {
            let mut next = end;
            while next < rows.len() && unread(rows, next, message_end(rows, next)) {
                next = message_end(rows, next);
            }
            let settled_without_text = rows
                .get(next)
                .is_some_and(|row| row.role == "tool" || row.is_prompt());
            if settled_without_text {
                for row in &mut rows[start..end] {
                    row.role = "assistant".into();
                }
                lowest = lowest.min(start);
            }
        }
        start = end;
    }
    lowest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }
    fn roles(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.role.as_str()).collect()
    }

    #[test]
    fn a_note_with_no_text_before_the_next_tool_shows_as_a_message() {
        // As Claude saves it: an empty thought, a thought with words, a question.
        let mut rows = vec![
            row("user", "go"),
            row("reasoning", ""),
            row(
                "reasoning",
                "Two answers are settled; please answer the other two.",
            ),
            row("tool", "{}"),
        ];
        assert_eq!(promote_notes(&mut rows, 3), 2);
        assert_eq!(roles(&rows), ["user", "reasoning", "assistant", "tool"]);
    }

    #[test]
    fn notes_with_text_after_them_stay_thinking() {
        let mut rows = vec![
            row("user", "go"),
            row("reasoning", "Let me check the file."),
            row("assistant", "Here are the questions."),
            row("tool", "{}"),
        ];
        assert_eq!(promote_notes(&mut rows, 0), 0);
        assert_eq!(roles(&rows), ["user", "reasoning", "assistant", "tool"]);
    }

    #[test]
    fn a_note_still_being_written_waits_for_what_comes_after_it() {
        let mut rows = vec![row("user", "go"), row("reasoning", "Still thinking")];
        assert_eq!(promote_notes(&mut rows, 1), 1);
        assert_eq!(roles(&rows), ["user", "reasoning"]);
        // The tool call arrives in a later patch and settles it.
        rows.push(row("tool", "{}"));
        assert_eq!(promote_notes(&mut rows, 2), 1);
        assert_eq!(roles(&rows), ["user", "assistant", "tool"]);
        // A note split over several rows changes as one message.
        let mut rows = vec![
            row("reasoning", "Part one, "),
            Row {
                first: false,
                ..row("reasoning", "part two.")
            },
            row("user", "next prompt"),
        ];
        promote_notes(&mut rows, 0);
        assert_eq!(roles(&rows), ["assistant", "assistant", "user"]);
    }

    #[gpui::test]
    fn claude_chats_show_the_note_and_other_agents_keep_thinking(cx: &mut gpui::TestAppContext) {
        let rows = || {
            vec![
                row("user", "go"),
                row("reasoning", "Please answer the two questions in the box."),
            ]
        };
        for (provider, shown) in [("claude", "assistant"), ("codex", "reasoning")] {
            let view = cx.new(|_| TranscriptView::new());
            view.update(cx, |view, cx| {
                view.identity(provider, "Model");
                view.patch(0, rows(), cx);
                assert_eq!(view.source_rows()[1].role, "reasoning", "not settled yet");
                // The question arrives in its own patch, as it streams in.
                view.patch(2, vec![row("tool", "{}")], cx);
                assert_eq!(view.source_rows()[1].role, shown, "{provider}");
                // The chat as saved is never touched: a repeated patch of
                // the same rows comes back the same way.
                view.patch(1, vec![rows().remove(1), row("tool", "{}")], cx);
                assert_eq!(view.source_rows()[1].role, shown, "{provider}");
            });
        }
    }
}
