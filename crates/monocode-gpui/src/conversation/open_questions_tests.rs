use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::{
    agent::{QuestionItem, QuestionOption},
    store::Store,
};
use std::sync::Mutex;

const PROMPT: &str = "Which layout do you want?";

type Shared = Arc<Mutex<Store>>;

fn store() -> (Shared, monocode_core::store::Session) {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("codex", "/missing/open-questions").unwrap();
    summary.provider = "fixture".into();
    (Arc::new(Mutex::new(store)), summary)
}

/// A chat as it opens after a restart: a new view on the same saved data.
fn open(
    store: &Shared,
    summary: &monocode_core::store::Session,
    cx: &mut TestAppContext,
) -> WindowHandle<Conversation> {
    let (store, summary) = (store.clone(), summary.clone());
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Conversation::new(summary, store, cx))
        })
        .unwrap()
    });
    window.update(cx, |view, _, cx| view.load(cx)).unwrap();
    cx.run_until_parked();
    window
}

fn question() -> Question {
    Question {
        id: "\"request-1\"".into(),
        title: String::new(),
        items: vec![QuestionItem {
            id: PROMPT.into(),
            prompt: PROMPT.into(),
            multiple: false,
            allow_custom: true,
            options: vec![QuestionOption {
                id: "left".into(),
                label: "Left".into(),
                description: String::new(),
            }],
        }],
        auto_resolve_at: Some(1),
    }
}

/// Asks a question in one view, then reopens the chat in another.
fn asked_and_reopened(
    store: &Shared,
    summary: &monocode_core::store::Session,
    cx: &mut TestAppContext,
) -> WindowHandle<Conversation> {
    let first = open(store, summary, cx);
    first
        .update(cx, |view, _, cx| {
            view.add_question_card(question(), cx);
            view.save_open_questions(cx);
        })
        .unwrap();
    cx.run_until_parked();
    open(store, summary, cx)
}

fn saved_questions(store: &Shared, summary: &monocode_core::store::Session) -> String {
    store
        .lock()
        .unwrap()
        .setting(&format!("open-questions:{}", summary.id))
        .unwrap()
        .unwrap_or_default()
}

fn reply(
    window: WindowHandle<Conversation>,
    answers: Option<Vec<Vec<String>>>,
    cx: &mut TestAppContext,
) {
    window
        .update(cx, |view, _, cx| {
            let card = view.questions.front().unwrap().0.clone();
            card.update(cx, |card, cx| card.reply(answers, cx));
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn unsent_text_is_still_in_the_box_after_a_restart(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let first = open(&store, &summary, cx);
    first
        .update(cx, |view, _, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("half a thought".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    let second = open(&store, &summary, cx);
    second
        .update(cx, |view, _, cx| {
            assert_eq!(view.input.read(cx).content.as_ref(), "half a thought");
            // Emptying the box, as sending does, saves at once.
            view.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    let third = open(&store, &summary, cx);
    third
        .update(cx, |view, _, cx| {
            assert!(view.input.read(cx).content.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn an_open_question_comes_back_after_a_restart(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let reopened = asked_and_reopened(&store, &summary, cx);
    reopened
        .update(cx, |view, _, cx| {
            assert_eq!(view.questions.len(), 1);
            assert_eq!(view.status.to_string(), "Waiting for your answer");
            let request = view.questions[0].0.read(cx).request().clone();
            assert_eq!(request.items[0].prompt, PROMPT);
            assert_eq!(
                request.auto_resolve_at, None,
                "it waits for as long as needed"
            );
        })
        .unwrap();
}

#[gpui::test]
fn an_answer_after_a_restart_goes_out_as_a_message(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let reopened = asked_and_reopened(&store, &summary, cx);
    // Busy keeps the message in the queue, without starting an agent.
    reopened.update(cx, |view, _, _| view.busy = true).unwrap();
    reply(reopened, Some(vec![vec!["Left".into()]]), cx);
    reopened
        .update(cx, |view, _, _| {
            assert!(view.questions.is_empty());
            let sent = &view.follow_ups.state.messages[0].text;
            assert!(sent.starts_with(crate::transcript::question_box::RESTORED_ANSWERS_HEADER));
            assert!(sent.contains(&format!("Q: {PROMPT}\nA: Left")));
        })
        .unwrap();
    assert_eq!(saved_questions(&store, &summary), "[]");
}

#[gpui::test]
fn skipping_after_a_restart_sends_nothing(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let reopened = asked_and_reopened(&store, &summary, cx);
    reply(reopened, None, cx);
    reopened
        .update(cx, |view, _, _| {
            assert!(view.questions.is_empty());
            assert!(view.follow_ups.state.messages.is_empty());
            assert_eq!(view.status.to_string(), "Ready");
        })
        .unwrap();
    assert_eq!(saved_questions(&store, &summary), "[]");
}

#[gpui::test]
fn an_agent_that_ends_leaves_its_question_open(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let view = open(&store, &summary, cx);
    view.update(cx, |view, _, cx| {
        view.add_question_card(question(), cx);
        view.save_open_questions(cx);
        view.keep_unanswered_questions(cx);
        assert_eq!(view.questions.len(), 1);
        // A new turn starting does not take it away either.
        view.clear_live_questions(cx);
        assert_eq!(view.questions.len(), 1);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(saved_questions(&store, &summary).contains(PROMPT));
}

#[gpui::test]
fn pressing_stop_ends_a_question_for_good(cx: &mut TestAppContext) {
    let (store, summary) = store();
    let view = open(&store, &summary, cx);
    view.update(cx, |view, _, cx| {
        view.add_question_card(question(), cx);
        view.save_open_questions(cx);
        view.busy = true;
        view.stop(cx);
        assert!(view.questions.is_empty());
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(saved_questions(&store, &summary), "[]");
}
