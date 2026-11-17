use super::*;
use serde_json::json;

fn guard(permissions: &[&str]) -> Guard {
    Guard::new(
        &permissions
            .iter()
            .map(|p| (*p).to_owned())
            .collect::<Vec<_>>(),
    )
}
fn call(request: u64, action: &str, input: serde_json::Value) -> AppCall {
    AppCall {
        request,
        action: action.into(),
        input,
    }
}

#[test]
fn only_permitted_known_actions_pass() {
    let mut guard = guard(&["sessions.list"]);
    let click = guard.click();
    assert!(guard
        .admit(click, vec![call(1, "sessions.list", json!({}))])
        .is_ok());
    let error = guard
        .admit(click, vec![call(2, "worktrees.list", json!({}))])
        .unwrap_err();
    assert!(error.contains("without permission"), "{error}");
    let error = guard
        .admit(click, vec![call(3, "files.delete", json!({}))])
        .unwrap_err();
    assert!(error.contains("unknown action"), "{error}");
}

#[test]
fn a_permission_that_is_no_action_cannot_be_called() {
    let mut guard = guard(&["send_prompts"]);
    let click = guard.click();
    assert!(guard
        .admit(click, vec![call(1, "send_prompts", json!({}))])
        .is_err());
}

#[test]
fn changes_need_a_click_and_reads_do_not() {
    let mut guard = guard(&["worktrees.list", "worktrees.select"]);
    let refresh = guard.refresh();
    assert!(guard
        .admit(refresh, vec![call(1, "worktrees.list", json!({}))])
        .is_ok());
    let error = guard
        .admit(refresh, vec![call(2, "worktrees.select", json!({}))])
        .unwrap_err();
    assert!(error.contains("without a click"), "{error}");
    let click = guard.click();
    assert!(guard
        .admit(click, vec![call(3, "worktrees.select", json!({}))])
        .is_ok());
}

#[test]
fn an_answer_continues_the_chain_that_asked() {
    let mut guard = guard(&["worktrees.list", "worktrees.select"]);
    let refresh = guard.refresh();
    guard
        .admit(refresh, vec![call(1, "worktrees.list", json!({}))])
        .unwrap();
    let origin = guard.answered(1).unwrap();
    assert_eq!(origin, refresh);
    assert!(guard
        .admit(origin, vec![call(2, "worktrees.select", json!({}))])
        .is_err());
    assert_eq!(guard.answered(1), None, "an answer is used once");
}

#[test]
fn a_chat_must_have_been_clicked() {
    let mut guard = guard(&["sessions.rename"]);
    guard.allow_chat("a");
    let click = guard.click();
    let on = |chat: serde_json::Value| vec![call(1, "sessions.rename", json!({"sessionId": chat}))];
    assert!(guard
        .admit(click, on(json!("b")))
        .unwrap_err()
        .contains("did not click"));
    assert!(
        guard.admit(click, on(json!(7))).is_err(),
        "a number cannot name a chat"
    );
    assert!(guard.admit(click, on(json!("a"))).is_ok());
}

#[test]
fn a_repeated_request_number_is_refused() {
    let mut guard = guard(&["sessions.list"]);
    let click = guard.click();
    let twice = vec![
        call(1, "sessions.list", json!({})),
        call(1, "sessions.list", json!({})),
    ];
    assert!(guard.admit(click, twice).unwrap_err().contains("reused"));
    guard
        .admit(click, vec![call(2, "sessions.list", json!({}))])
        .unwrap();
    assert!(guard
        .admit(click, vec![call(2, "sessions.list", json!({}))])
        .is_err());
}

#[test]
fn calls_at_once_are_limited() {
    let mut guard = guard(&["sessions.list"]);
    let click = guard.click();
    let many = (1..=MAX_AT_ONCE as u64 + 1)
        .map(|r| call(r, "sessions.list", json!({})))
        .collect();
    assert!(guard.admit(click, many).unwrap_err().contains("too many"));
}

#[test]
fn one_click_cannot_spend_without_limit() {
    let mut guard = guard(&["models.ask"]);
    let click = guard.click();
    for request in 1..=MAX_TOKEN_CALLS_PER_CLICK as u64 {
        guard
            .admit(click, vec![call(request, "models.ask", json!({}))])
            .unwrap();
        guard.answered(request).unwrap();
    }
    let error = guard
        .admit(click, vec![call(99, "models.ask", json!({}))])
        .unwrap_err();
    assert!(error.contains("too many"), "{error}");
    let next_click = guard.click();
    assert!(guard
        .admit(next_click, vec![call(100, "models.ask", json!({}))])
        .is_ok());
}
