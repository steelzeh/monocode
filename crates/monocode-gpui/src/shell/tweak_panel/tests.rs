use super::*;
use crate::tweaks::{command_prompt, commands, get, menu_items, menu_prompt};
use gpui::{TestAppContext, WindowHandle};
use monocode_core::tweaks::{Found, Tweak};
use serde_json::json;
use std::path::PathBuf;

fn tweak(value: serde_json::Value, source: Option<&str>) -> Found {
    Found::Ready(Tweak {
        manifest: serde_json::from_value(value).unwrap(),
        dir: PathBuf::from("/tweaks/x"),
        panel_source: source.map(str::to_owned),
        script_source: None,
        fingerprint: String::new(),
        source: None,
    })
}

fn install(window: &WindowHandle<Shell>, found: Vec<Found>, cx: &mut TestAppContext) {
    window
        .update(cx, |_, _, cx| {
            let center = get(cx).unwrap();
            center.update(cx, |center, cx| center.set_found(found, cx));
        })
        .unwrap();
    cx.run_until_parked();
}

fn enable(window: &WindowHandle<Shell>, id: &str, on: bool, cx: &mut TestAppContext) {
    window
        .update(cx, |_, _, cx| {
            let center = get(cx).unwrap();
            center.update(cx, |center, cx| {
                let tweak = center
                    .found()
                    .iter()
                    .find_map(|found| match found {
                        Found::Ready(tweak) if tweak.manifest.id == id => Some(tweak.clone()),
                        _ => None,
                    })
                    .unwrap();
                center.set_enabled(&tweak, on, cx);
            });
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
}

fn open_tab(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    window
        .update(cx, |shell, w, cx| {
            let open = shell
                .right_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).visible_tab().is_some());
            if !open {
                shell.toggle_right_panel(w, cx);
            }
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let panel = shell.right_panel.clone().unwrap();
            panel.update(cx, |panel, cx| panel.select(RightTab::Tweaks, cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
}

fn click(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, _, cx| {
            let panel = shell.right_panel.clone().unwrap();
            let tweaks = panel.read(cx).tweaks().unwrap().clone();
            let view = tweaks.read(cx).view_for_test().unwrap();
            view.update(cx, |view, cx| view.click_first_button_for_test(cx));
        })
        .unwrap();
    cx.run_until_parked();
}

fn composer(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |shell, _, cx| {
            let id = shell.workspace.active.unwrap();
            shell.sessions[&shell.workspace.session_for_pane(id)]
                .read(cx)
                .composer_text_for_test(cx)
        })
        .unwrap()
}

const DRAFT: &str =
    "return () => visual.button('Go', () => tweak.app('composer.insert', {text: 'from a tweak'}))";

fn panel_tweak(id: &str, permissions: serde_json::Value) -> Found {
    tweak(
        json!({"id": id, "name": id, "version": "1", "permissions": permissions,
               "panel": {"title": id, "script": "panel.js"}}),
        Some(DRAFT),
    )
}

#[gpui::test]
fn the_tab_shows_only_while_a_panel_tweak_is_on(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![panel_tweak("one", json!(["composer.insert"]))],
        cx,
    );
    open_tab(&window, cx);
    let has_tab = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .right_panel
                    .clone()
                    .unwrap()
                    .read(cx)
                    .tabs()
                    .contains(&RightTab::Tweaks)
            })
            .unwrap()
    };
    assert!(!has_tab(&window, cx), "off until the user turns it on");
    enable(&window, "one", true, cx);
    assert!(has_tab(&window, cx));
    open_tab(&window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("tweak-panel").is_some());
    assert!(visual.debug_bounds("native-session-visual").is_some());
    enable(&window, "one", false, cx);
    assert!(!has_tab(&window, cx));
    let tab = window
        .update(cx, |shell, _, cx| {
            shell.right_panel.clone().unwrap().read(cx).tab()
        })
        .unwrap();
    assert_eq!(tab, RightTab::Files);
}

#[gpui::test]
fn a_click_acts_only_with_the_matching_permission(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(&window, vec![panel_tweak("nope", json!([]))], cx);
    enable(&window, "nope", true, cx);
    open_tab(&window, cx);
    click(&window, cx);
    assert_eq!(composer(&window, cx), "", "no permission");
    let error = window
        .update(cx, |shell, _, cx| {
            let panel = shell.right_panel.clone().unwrap();
            let tweaks = panel.read(cx).tweaks().unwrap().clone();
            let view = tweaks.read(cx).view_for_test().unwrap();
            view.read(cx).error_for_test()
        })
        .unwrap()
        .unwrap();
    assert!(error.contains("without permission"), "{error}");

    install(
        &window,
        vec![panel_tweak("yes", json!(["composer.insert"]))],
        cx,
    );
    enable(&window, "yes", true, cx);
    open_tab(&window, cx);
    click(&window, cx);
    assert_eq!(composer(&window, cx), "from a tweak");
}
#[gpui::test]
fn a_panel_can_switch_working_copy_through_an_app_action(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let found = tweak(
        json!({"id":"one","name":"One","version":"1","permissions":["worktrees.select"],
               "panel":{"title":"One","script":"panel.js"}}),
        Some("return () => visual.button('All', () => tweak.app('worktrees.select', {}))"),
    );
    install(&window, vec![found], cx);
    enable(&window, "one", true, cx);
    open_tab(&window, cx);
    window
        .update(cx, |shell, _, _| {
            let project = shell.workspace.project.clone();
            shell.worktree_focus.insert(
                project.clone(),
                crate::worktree_switcher::WorktreeFocus {
                    path: format!("{project}-tree"),
                    branch: None,
                },
            );
        })
        .unwrap();
    click(&window, cx);
    window
        .update(cx, |shell, _, _| assert!(shell.worktree_focus.is_empty()))
        .unwrap();
}
#[gpui::test]
fn commands_and_menu_items_exist_only_while_the_tweak_is_on(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![tweak(
            json!({"id":"t","name":"T","version":"1",
                "commands":[
                    {"name":"standup","description":"Write it","prompt":"Write about {{args}}"},
                    {"name":"compact","prompt":"Hijack"}],
                "menu":[{"id":"sum","label":"Summarize","prompt":"Summarize this chat"}]}),
            None,
        )],
        cx,
    );
    let probe = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |_, _, cx| {
                (
                    command_prompt("/standup today", cx),
                    commands(cx),
                    menu_items(cx),
                    menu_prompt("tweak:t:sum", cx),
                    command_prompt("/compact", cx),
                )
            })
            .unwrap()
    };
    let (prompt, listed, menu, entry, _) = probe(&window, cx);
    assert_eq!(prompt, None);
    assert!(listed.is_empty() && menu.is_empty() && entry.is_none());
    enable(&window, "t", true, cx);
    let (prompt, listed, menu, entry, reserved) = probe(&window, cx);
    assert_eq!(prompt.as_deref(), Some("Write about today"));
    assert_eq!(listed, [("standup".to_owned(), "Write it".to_owned())]);
    assert_eq!(menu.len(), 1);
    assert_eq!(menu[0].key, "tweak:t:sum");
    assert_eq!(menu[0].label, "Summarize");
    assert!(menu[0].needs_idle);
    assert_eq!(entry.unwrap().1, "Summarize this chat");
    assert_eq!(reserved, None, "a tweak cannot replace /compact");
}

#[gpui::test]
fn a_broken_panel_shows_its_error_without_breaking_the_app(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![tweak(
            json!({"id":"bad","name":"Bad","version":"1",
                   "panel":{"title":"Bad","script":"panel.js"}}),
            Some("this is not javascript ("),
        )],
        cx,
    );
    enable(&window, "bad", true, cx);
    open_tab(&window, cx);
    let error = window
        .update(cx, |shell, _, cx| {
            let panel = shell.right_panel.clone().unwrap();
            let tweaks = panel.read(cx).tweaks().unwrap().clone();
            let view = tweaks.read(cx).view_for_test().unwrap();
            view.read(cx).error_for_test()
        })
        .unwrap();
    assert!(error.is_some());
}

fn script_tweak(permissions: serde_json::Value, source: &str) -> Found {
    Found::Ready(Tweak {
        manifest: serde_json::from_value(json!({
            "id": "t", "name": "T", "version": "1", "permissions": permissions,
            "script": "main.js",
            "menu": [{"id": "go", "label": "Go", "busy_label": "Going"}]
        }))
        .unwrap(),
        dir: PathBuf::from("/tweaks/t"),
        panel_source: None,
        script_source: Some(source.to_owned()),
        fingerprint: String::new(),
        source: None,
    })
}

/// A saved chat with one message, shown in the sidebar.
fn saved_chat(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> i64 {
    let id = window
        .update(cx, |shell, _, cx| {
            let cwd = shell.workspace.project.clone();
            let mut store = locked(&shell.store).unwrap();
            let mut summary = store.create("codex", &cwd).unwrap();
            store
                .write_rows(
                    summary.id,
                    0,
                    &[monocode_core::transcript::Row {
                        role: "user".into(),
                        text: "Fix the login bug".into(),
                        first: true,
                        sent_at: None,
                    }],
                )
                .unwrap();
            drop(store);
            summary.has_messages = true;
            summary.title = "Old title".into();
            let id = summary.id;
            shell.session_summaries.insert(id, summary);
            shell.order.insert(0, id);
            shell.notify_sidebar(cx);
            id
        })
        .unwrap();
    cx.run_until_parked();
    id
}

fn click_menu(window: &WindowHandle<Shell>, chat: i64, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, w, cx| {
            shell.run_menu(MenuTarget::Session(chat), "tweak:t:go", None, w, cx)
        })
        .unwrap();
    cx.run_until_parked();
}

fn title(window: &WindowHandle<Shell>, chat: i64, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |shell, _, _| {
            shell.session_summaries[&chat].title.clone()
        })
        .unwrap()
}

const RENAME: &str = "tweak.onMenu('go', (chat) => {
    tweak.app('sessions.setWorking', {sessionId: chat.id, working: true});
    tweak.app('sessions.read', {sessionId: chat.id}, (page) => {
        tweak.app('models.ask', {prompt: page.turns[0].user.text}, (answer) => {
            tweak.app('sessions.rename', {sessionId: chat.id, title: answer.text});
        });
    });
});";

#[gpui::test]
fn a_script_item_reads_the_chat_asks_the_model_and_renames(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![script_tweak(
            json!([
                "sessions.read",
                "models.ask",
                "sessions.rename",
                "sessions.setWorking"
            ]),
            RENAME,
        )],
        cx,
    );
    let chat = saved_chat(&window, cx);
    let row = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .session_menu_items(chat, cx)
                    .iter()
                    .any(|item| matches!(item, MenuItem::Action(a) if a.id == "tweak:t:go"))
            })
            .unwrap()
    };
    assert!(!row(&window, cx), "no row while the tweak is off");
    enable(&window, "t", true, cx);
    assert!(row(&window, cx));
    click_menu(&window, chat, cx);
    // The 17 characters of the message reached the model.
    assert_eq!(title(&window, chat, cx), "Answer from 17 characters");
    window
        .update(cx, |shell, _, _| {
            assert!(shell.working_titles.is_empty());
            assert!(shell.error.is_none());
        })
        .unwrap();
    let saved = window
        .update(cx, |shell, _, _| {
            locked(&shell.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == chat)
                .unwrap()
                .title
        })
        .unwrap();
    assert_eq!(saved, "Answer from 17 characters");
}

#[gpui::test]
fn a_script_without_permissions_changes_nothing(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![script_tweak(
            json!([]),
            "tweak.onMenu('go', (chat) => tweak.app('sessions.rename', {sessionId: chat.id, title: 'Hacked'}));",
        )],
        cx,
    );
    enable(&window, "t", true, cx);
    let chat = saved_chat(&window, cx);
    click_menu(&window, chat, cx);
    assert_eq!(title(&window, chat, cx), "Old title");
    assert!(
        last_error(&window, cx)
            .unwrap()
            .contains("without permission")
    );
}
#[gpui::test]
fn a_script_cannot_touch_a_chat_the_user_did_not_click(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let other = saved_chat(&window, cx);
    let source = format!(
        "tweak.onMenu('go', () => tweak.app('sessions.rename', {{sessionId: '{other}', title: 'Hacked'}}));"
    );
    install(
        &window,
        vec![script_tweak(json!(["sessions.rename"]), &source)],
        cx,
    );
    enable(&window, "t", true, cx);
    let clicked = saved_chat(&window, cx);
    click_menu(&window, clicked, cx);
    assert_eq!(title(&window, other, cx), "Old title");
    assert!(last_error(&window, cx).unwrap().contains("did not click"));
}
#[gpui::test]
fn a_failing_script_reports_and_clears_its_mark(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![script_tweak(
            json!(["sessions.setWorking"]),
            "tweak.onMenu('go', (chat) => {
                tweak.app('sessions.setWorking', {sessionId: chat.id, working: true}, () => {
                    throw new Error('Nothing to do');
                });
            });",
        )],
        cx,
    );
    enable(&window, "t", true, cx);
    let chat = saved_chat(&window, cx);
    click_menu(&window, chat, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.error.as_deref(), Some("Nothing to do"));
            assert!(shell.working_titles.is_empty());
        })
        .unwrap();
}
#[gpui::test]
fn the_tab_takes_the_name_of_the_panel_it_shows(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let name = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .right_panel
                    .clone()
                    .unwrap()
                    .read(cx)
                    .tab_title(RightTab::Tweaks, cx)
                    .to_string()
            })
            .unwrap()
    };
    install(
        &window,
        vec![
            panel_tweak("alpha", json!(["composer.insert"])),
            panel_tweak("beta", json!(["composer.insert"])),
        ],
        cx,
    );
    enable(&window, "alpha", true, cx);
    open_tab(&window, cx);
    assert_eq!(name(&window, cx), "alpha");
    enable(&window, "beta", true, cx);
    open_tab(&window, cx);
    assert_eq!(name(&window, cx), "alpha", "the open panel keeps its name");
    window
        .update(cx, |shell, _, cx| {
            let tweaks = shell
                .right_panel
                .clone()
                .unwrap()
                .read(cx)
                .tweaks()
                .unwrap()
                .clone();
            tweaks.update(cx, |view, cx| view.select("beta".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(name(&window, cx), "beta");
    enable(&window, "alpha", false, cx);
    enable(&window, "beta", false, cx);
    assert_eq!(
        name(&window, cx),
        "Tweaks",
        "the plain name when none is on"
    );
}

#[gpui::test]
fn the_tab_shows_the_icon_the_tweak_names_or_the_default(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let tab_icon = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .right_panel
                    .clone()
                    .unwrap()
                    .read(cx)
                    .tweak_tab_icon(cx)
            })
            .unwrap()
    };
    for (icon, expected) in [
        (json!("folder-tree"), "folder-tree"),
        (json!("no-such-icon"), "command-visualize"),
        (serde_json::Value::Null, "command-visualize"),
    ] {
        let mut panel = json!({"title": "One", "script": "panel.js"});
        if !icon.is_null() {
            panel["icon"] = icon;
        }
        install(
            &window,
            vec![tweak(
                json!({"id": "one", "name": "One", "version": "1", "panel": panel}),
                Some(DRAFT),
            )],
            cx,
        );
        enable(&window, "one", true, cx);
        open_tab(&window, cx);
        assert_eq!(tab_icon(&window, cx), expected);
    }
}

fn last_error(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<String> {
    window
        .update(cx, |shell, _, _| shell.error.clone())
        .unwrap()
}

#[gpui::test]
fn an_endless_chain_of_model_calls_is_stopped(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![script_tweak(
            json!(["models.ask"]),
            "const again = () => tweak.app('models.ask', {prompt: 'x'}, again); tweak.onMenu('go', again);",
        )],
        cx,
    );
    enable(&window, "t", true, cx);
    let chat = saved_chat(&window, cx);
    click_menu(&window, chat, cx);
    assert!(last_error(&window, cx).unwrap().contains("too many times"));
    window
        .update(cx, |shell, _, _| assert_eq!(shell.tweak_requests["t"], 0))
        .unwrap();
}
#[gpui::test]
fn too_many_model_calls_from_one_click_are_refused(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![script_tweak(
            json!(["models.ask", "sessions.rename"]),
            "tweak.onMenu('go', (chat) => {
                for (let i = 0; i < 5; i++) {
                    tweak.app('models.ask', {prompt: 'x'}, () =>
                        tweak.app('sessions.rename', {sessionId: chat.id, title: 'Asked'}));
                }
            });",
        )],
        cx,
    );
    enable(&window, "t", true, cx);
    let chat = saved_chat(&window, cx);
    click_menu(&window, chat, cx);
    assert!(last_error(&window, cx).unwrap().contains("too many times"));
    assert_eq!(title(&window, chat, cx), "Old title", "no call was started");
}
#[gpui::test]
fn an_answer_for_an_earlier_start_of_the_script_is_ignored(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (window, _, _) = super::super::tests::window(cx);
    let found = script_tweak(
        json!(["models.ask", "sessions.rename"]),
        "tweak.onMenu('go', (chat) => tweak.app('models.ask', {prompt: 'x'}, (answer) =>
            tweak.app('sessions.rename', {sessionId: chat.id, title: answer.text})));",
    );
    let Found::Ready(tweak) = found.clone() else {
        panic!()
    };
    install(&window, vec![found], cx);
    enable(&window, "t", true, cx);
    window
        .update(cx, |_, _, cx| {
            get(cx).unwrap().update(cx, |center, cx| {
                let chat = |id: &str| format!(r#"{{"id":"{id}","title":"","project":""}}"#);
                let first = center.run_menu("t", "go", "1", &chat("1")).unwrap();
                assert!(!first.cancel.load(Ordering::Relaxed));
                center.set_enabled(&tweak, false, cx);
                assert!(first.cancel.load(Ordering::Relaxed), "turning off cancels");
                center.set_enabled(&tweak, true, cx);
                let second = center.run_menu("t", "go", "2", &chat("2")).unwrap();
                assert_ne!(first.generation, second.generation);
                let stale = center
                    .deliver("t", first.generation, 1, r#"{"result":{"text":"From A"}}"#)
                    .unwrap();
                assert!(stale.calls.is_empty());
                let fresh = center
                    .deliver("t", second.generation, 1, r#"{"result":{"text":"From B"}}"#)
                    .unwrap();
                assert_eq!(fresh.calls[0].input["sessionId"], "2");
                assert_eq!(fresh.calls[0].input["title"], "From B");
            })
        })
        .unwrap();
}

#[gpui::test]
fn turning_one_tweak_off_leaves_another_tweaks_calls_running(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (window, _, _) = super::super::tests::window(cx);
    let script = "tweak.onMenu('go', () => tweak.app('models.ask', {prompt: 'x'}));";
    let tweak_named = |id: &str| {
        let Found::Ready(mut tweak) = script_tweak(json!(["models.ask"]), script) else {
            panic!()
        };
        tweak.manifest.id = id.into();
        tweak
    };
    let (a, b) = (tweak_named("a"), tweak_named("b"));
    install(&window, vec![Found::Ready(a), Found::Ready(b.clone())], cx);
    enable(&window, "a", true, cx);
    enable(&window, "b", true, cx);
    window
        .update(cx, |_, _, cx| {
            get(cx).unwrap().update(cx, |center, cx| {
                let chat = r#"{"id":"1","title":"","project":""}"#;
                let running = center.run_menu("a", "go", "1", chat).unwrap();
                center.set_enabled(&b, false, cx);
                assert!(!running.cancel.load(Ordering::Relaxed));
            })
        })
        .unwrap();
}
#[gpui::test]
fn the_model_call_uses_the_account_of_the_chat(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let chat = saved_chat(&window, cx);
    window
        .update(cx, |shell, _, cx| {
            let view = shell.ensure_session(chat, cx).unwrap();
            view.update(cx, |view, _| view.settings.account.id = "work".into());
            assert_eq!(shell.text_model_for_session(chat, cx).account.id, "work");
        })
        .unwrap();
}

#[gpui::test]
fn capital_letters_cannot_take_over_a_built_in_command(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    install(
        &window,
        vec![tweak(
            json!({"id":"t","name":"T","version":"1",
                "commands":[{"name":"compact","prompt":"Hijack"}]}),
            None,
        )],
        cx,
    );
    enable(&window, "t", true, cx);
    window
        .update(cx, |_, _, cx| {
            assert_eq!(command_prompt("/COMPACT", cx), None)
        })
        .unwrap();
}

#[gpui::test]
fn a_stopped_panel_starts_again_when_the_list_reloads(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let found = tweak(
        json!({"id":"one","name":"One","version":"1",
               "panel":{"title":"One","script":"panel.js"}}),
        Some("return () => visual.button('Go', () => { while (true) {} })"),
    );
    install(&window, vec![found.clone()], cx);
    enable(&window, "one", true, cx);
    open_tab(&window, cx);
    click(&window, cx);
    let stopped = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, cx| {
                let panel = shell.right_panel.clone().unwrap();
                let tweaks = panel.read(cx).tweaks().unwrap().clone();
                tweaks
                    .read(cx)
                    .view_for_test()
                    .unwrap()
                    .read(cx)
                    .is_stopped()
            })
            .unwrap()
    };
    assert!(stopped(&window, cx));
    install(&window, vec![found], cx);
    assert!(!stopped(&window, cx));
}

fn open_with_saved_tab(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> RightTab {
    cx.update(|cx| cx.set_reduce_motion(true));
    window
        .update(cx, |shell, w, cx| {
            shell.toggle_right_panel(w, cx);
            // As a restored layout does: on this tab, with no tweak to fill it.
            let panel = shell.right_panel.clone().unwrap();
            panel.update(cx, |panel, cx| panel.select(RightTab::Tweaks, cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            shell.right_panel.clone().unwrap().read(cx).tab()
        })
        .unwrap()
}

#[gpui::test]
fn a_saved_tweaks_tab_moves_to_explorer_when_no_tweak_can_fill_it(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    cx.run_until_parked();
    assert_eq!(open_with_saved_tab(&window, cx), RightTab::Files);
}

#[gpui::test]
fn a_saved_tweaks_tab_waits_for_the_first_read_of_the_tweaks(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    cx.run_until_parked();
    window
        .update(cx, |_, _, cx| {
            get(cx)
                .unwrap()
                .update(cx, |center, _| center.forget_first_read())
        })
        .unwrap();
    assert_eq!(open_with_saved_tab(&window, cx), RightTab::Tweaks);
}

#[gpui::test]
fn a_changed_tweak_list_redraws_the_window_by_itself(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let found = panel_tweak("one", json!(["composer.insert"]));
    let Found::Ready(tweak) = found.clone() else {
        panic!()
    };
    cx.update(|cx| cx.set_reduce_motion(true));
    window
        .update(cx, |shell, w, cx| shell.toggle_right_panel(w, cx))
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, _, cx| {
            get(cx).unwrap().update(cx, |center, cx| {
                center.set_found(vec![found], cx);
                center.set_enabled(&tweak, true, cx);
            })
        })
        .unwrap();
    cx.run_until_parked();
    let shown = window
        .update(cx, |shell, _, cx| {
            shell
                .right_panel
                .clone()
                .unwrap()
                .read(cx)
                .tabs()
                .contains(&RightTab::Tweaks)
        })
        .unwrap();
    assert!(shown);
}
