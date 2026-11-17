//! The box a question leaves in the chat: each question and the reply to it.
use super::*;
use gpui::AnyElement;
use std::collections::HashSet;

/// Starts the message that carries answers given after the app was reopened,
/// when the agent that asked is gone and can only be told in a new message.
pub(crate) const RESTORED_ANSWERS_HEADER: &str =
    "Answers to the question you asked before Monocode closed:";
const QUESTION_TOOL: &str = "AskUserQuestion";

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Reply {
    Answer(String),
    Waiting,
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Entry {
    pub title: String,
    pub reply: Reply,
}

pub(super) fn is_question(call: &ToolCall) -> bool {
    call.name == QUESTION_TOOL
}

pub(crate) fn restored_message(answers: &[(String, String)]) -> String {
    let body = answers
        .iter()
        .map(|(prompt, answer)| format!("Q: {prompt}\nA: {answer}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{RESTORED_ANSWERS_HEADER}\n\n{body}")
}

fn prompts(call: &ToolCall) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(&call.input)
        .ok()
        .and_then(|input| {
            input["questions"].as_array().map(|questions| {
                questions
                    .iter()
                    .filter_map(|q| q["question"].as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Claude's reply reads `"question"="answer", "next"="answer". Closing words`.
/// Answers can hold quotes, so each one ends at the next known question.
fn answer_in(output: &str, prompt: &str, prompts: &[String]) -> Option<String> {
    let needle = format!("\"{prompt}\"=\"");
    let start = output.find(&needle)? + needle.len();
    let rest = &output[start..];
    let next = prompts
        .iter()
        .filter(|other| other.as_str() != prompt)
        .filter_map(|other| rest.find(&format!("\", \"{other}\"=\"")))
        .min();
    let closing = ["\". You can now continue", "\". Read the answers carefully"]
        .iter()
        .filter_map(|words| rest.find(words))
        .min();
    let end = match next.into_iter().chain(closing).min() {
        Some(end) => end,
        None => rest.rfind('"')?,
    };
    let answer = rest[..end].trim();
    (!answer.is_empty()).then(|| answer.to_owned())
}

fn restored_answer(later: &[Row], prompt: &str) -> Option<String> {
    let needle = format!("Q: {prompt}\nA: ");
    later
        .iter()
        .filter(|row| row.role == "user" && row.text.starts_with(RESTORED_ANSWERS_HEADER))
        .find_map(|row| {
            let start = row.text.find(&needle)? + needle.len();
            let rest = &row.text[start..];
            let end = rest.find("\n\nQ: ").unwrap_or(rest.len());
            Some(rest[..end].trim().to_owned())
        })
        .filter(|answer| !answer.is_empty())
}

/// One entry per question. `open` names questions still waiting after the app
/// was reopened; `live` is true while the agent that asked is still running.
pub(super) fn entries(
    call: &ToolCall,
    later: &[Row],
    open: &HashSet<String>,
    live: bool,
) -> Vec<Entry> {
    let prompts = prompts(call);
    prompts
        .iter()
        .map(|prompt| {
            let answer = answer_in(&call.output, prompt, &prompts)
                .or_else(|| restored_answer(later, prompt));
            let reply = match answer {
                Some(answer) if answer != "Skipped" => Reply::Answer(answer),
                Some(_) => Reply::Skipped,
                None if open.contains(prompt) => Reply::Waiting,
                None if live && matches!(call.state, ToolState::Running) => Reply::Waiting,
                None => Reply::Skipped,
            };
            Entry {
                title: prompt.clone(),
                reply,
            }
        })
        .collect()
}

pub(super) fn render(entries: &[Entry]) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .py_1()
        .child(
            div()
                .id("question-answer-box")
                .debug_selector(|| "question-answer-box".into())
                .role(Role::Group)
                .aria_label("Your answers")
                .w_full()
                .min_w_0()
                .rounded(px(10.))
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .px_3()
                .py(px(10.))
                .flex()
                .flex_col()
                .gap_3()
                .children(entries.iter().map(|entry| {
                    let (reply, ink) = match &entry.reply {
                        Reply::Answer(answer) => (answer.clone(), content(1.)),
                        Reply::Waiting => ("Waiting for your answer".to_owned(), content(0.5)),
                        Reply::Skipped => ("Skipped".to_owned(), content(0.5)),
                    };
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(17.))
                                .text_color(content(0.55))
                                .child(entry.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(ink)
                                .child(reply),
                        )
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "Should I build this?";
    const TWO: &str = "Which \"sound\" is better?";

    fn call(output: &str, state: ToolState) -> ToolCall {
        let mut call = ToolCall::new("q", QUESTION_TOOL);
        call.input = serde_json::json!({
            "questions": [{"question": ONE}, {"question": TWO}]
        })
        .to_string();
        call.output = output.into();
        call.state = state;
        call
    }

    fn replies(call: &ToolCall, later: &[Row], open: &[&str], live: bool) -> Vec<Reply> {
        let open = open.iter().map(|p| p.to_string()).collect();
        entries(call, later, &open, live)
            .into_iter()
            .map(|entry| entry.reply)
            .collect()
    }

    fn answer(text: &str) -> Reply {
        Reply::Answer(text.into())
    }

    #[test]
    fn both_of_claudes_reply_shapes_are_read_and_quotes_survive() {
        let output = format!(
            "Your questions have been answered: \"{ONE}\"=\"Build it (Recommended)\", \
             \"{TWO}\"=\"the \"unmixed\" one\". You can now continue with these answers in mind."
        );
        assert_eq!(
            replies(&call(&output, ToolState::Completed), &[], &[], false),
            [
                answer("Build it (Recommended)"),
                answer("the \"unmixed\" one")
            ]
        );
        let output = format!(
            "The user answered: \"{ONE}\"=\"why is that?\", \"{TWO}\"=\"Louder\". \
             Read the answers carefully - they may request clarification."
        );
        assert_eq!(
            replies(&call(&output, ToolState::Completed), &[], &[], false),
            [answer("why is that?"), answer("Louder")]
        );
    }

    #[test]
    fn a_skip_and_a_closed_app_read_as_skipped() {
        let skipped = call("User skipped the question", ToolState::Failed);
        assert_eq!(
            replies(&skipped, &[], &[], false),
            [Reply::Skipped, Reply::Skipped]
        );
        let closed = call("", ToolState::Running);
        assert_eq!(
            replies(&closed, &[], &[], false),
            [Reply::Skipped, Reply::Skipped]
        );
    }

    #[test]
    fn an_unanswered_question_waits_while_it_can_still_be_answered() {
        let asked = call("", ToolState::Running);
        assert_eq!(
            replies(&asked, &[], &[], true),
            [Reply::Waiting, Reply::Waiting]
        );
        assert_eq!(
            replies(&asked, &[], &[ONE], false),
            [Reply::Waiting, Reply::Skipped]
        );
    }

    #[test]
    fn an_answer_given_after_reopening_fills_the_box() {
        let text = restored_message(&[
            (ONE.to_owned(), "Yes".to_owned()),
            (TWO.to_owned(), "Skipped".to_owned()),
        ]);
        let later = [Row {
            role: "user".into(),
            text,
            first: true,
            sent_at: None,
        }];
        assert_eq!(
            replies(&call("", ToolState::Running), &later, &[], false),
            [answer("Yes"), Reply::Skipped]
        );
    }
}
