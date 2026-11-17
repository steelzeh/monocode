use super::*;
use crate::visual::{Node, VisualRuntime};
use serde_json::json;

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "monocode-tweaks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn tweak(&self, folder: &str, manifest: serde_json::Value, script: Option<&str>) -> PathBuf {
        let dir = self.0.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
        if let Some(script) = script {
            std::fs::write(dir.join("panel.js"), script).unwrap();
        }
        dir
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn manifest(id: &str) -> serde_json::Value {
    json!({"id": id, "name": "Test", "version": "1.0.0"})
}
fn ready(found: &Found) -> &Tweak {
    match found {
        Found::Ready(tweak) => tweak,
        Found::Broken { error, .. } => panic!("{error}"),
    }
}
fn broken(found: &Found) -> &str {
    match found {
        Found::Broken { error, .. } => error,
        Found::Ready(_) => panic!("expected a broken tweak"),
    }
}

#[test]
fn a_full_tweak_loads_with_its_panel_script() {
    let root = Root::new();
    root.tweak(
        "standup",
        json!({
            "id": "standup", "name": "Standup", "version": "1.2.0",
            "permissions": ["composer.insert", "send_prompts"],
            "panel": {"title": "Standup", "script": "panel.js"},
            "commands": [{"name": "standup", "description": "Write it", "prompt": "Write a standup"}],
            "menu": [{"id": "sum", "label": "Summarize", "prompt": "Summarize this chat"}]
        }),
        Some("return () => visual.text('Hi')"),
    );
    let found = discover(&root.0);
    assert_eq!(found.len(), 1);
    let tweak = ready(&found[0]);
    assert_eq!(tweak.manifest.commands[0].name, "standup");
    assert_eq!(
        tweak.manifest.permissions,
        ["composer.insert", "send_prompts"]
    );
    assert_eq!(
        tweak.panel_source.as_deref(),
        Some("return () => visual.text('Hi')")
    );
}

#[test]
fn a_missing_folder_means_no_tweaks_and_hidden_folders_are_skipped() {
    assert!(discover(Path::new("/nonexistent/monocode-tweaks")).is_empty());
    let root = Root::new();
    root.tweak(".git", manifest(".git"), None);
    assert!(discover(&root.0).is_empty());
}

#[test]
fn broken_tweaks_explain_themselves() {
    let root = Root::new();
    root.tweak("other", manifest("wrong-id"), None);
    root.tweak("Bad_Name", manifest("Bad_Name"), None);
    root.tweak(
        "unknown",
        json!({"id":"unknown","name":"X","version":"1","shell":"rm"}),
        None,
    );
    root.tweak(
        "power",
        json!({"id":"power","name":"X","version":"1","permissions":["shell"]}),
        None,
    );
    root.tweak(
        "twice",
        json!({"id":"twice","name":"X","version":"1","commands":[
            {"name":"a","prompt":"x"},{"name":"a","prompt":"y"}]}),
        None,
    );
    std::fs::create_dir_all(root.0.join("empty")).unwrap();
    let found = discover(&root.0);
    assert_eq!(found.len(), 6);
    for item in &found {
        assert!(!broken(item).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn a_panel_script_cannot_be_read_from_outside_its_folder() {
    let root = Root::new();
    std::fs::write(root.0.join("secret.js"), "return () => visual.text('x')").unwrap();
    root.tweak(
        "escape",
        json!({"id":"escape","name":"X","version":"1",
            "panel":{"title":"X","script":"../secret.js"}}),
        None,
    );
    let dir = root.tweak(
        "linked",
        json!({"id":"linked","name":"X","version":"1",
            "panel":{"title":"X","script":"link.js"}}),
        None,
    );
    std::os::unix::fs::symlink(root.0.join("secret.js"), dir.join("link.js")).unwrap();
    let found = discover(&root.0);
    assert!(broken(&found[0]).contains("inside the tweak"));
    assert!(broken(&found[1]).contains("inside the tweak"));
}

#[test]
fn a_tweak_stays_off_until_granted_and_turns_off_when_its_files_change() {
    let root = Root::new();
    let dir = root.tweak(
        "a",
        json!({"id":"a","name":"A","version":"1","script":"main.js","permissions":["models.ask"]}),
        None,
    );
    std::fs::write(dir.join("main.js"), "tweak.onMenu('x', () => {});").unwrap();
    let load = || match discover(&root.0).remove(0) {
        Found::Ready(tweak) => tweak,
        Found::Broken { error, .. } => panic!("{error}"),
    };
    let mut grants = Grants::default();
    let tweak = load();
    assert!(!grants.allows(&tweak));
    assert!(!grants.changed(&tweak), "never turned on");
    grants.set(&tweak, true);
    let grants = Grants::parse(Some(&grants.encode()));
    assert!(grants.allows(&tweak));

    // The script changes, while the manifest and its permissions stay.
    std::fs::write(
        dir.join("main.js"),
        "tweak.onMenu('x', () => { /* new */ });",
    )
    .unwrap();
    let changed_script = load();
    assert!(!grants.allows(&changed_script));
    assert!(grants.changed(&changed_script));

    // A manifest change counts too, and accepting the new files turns it on.
    std::fs::write(dir.join("main.js"), "tweak.onMenu('x', () => {});").unwrap();
    assert!(
        grants.allows(&load()),
        "the original files are accepted again"
    );
    std::fs::write(
        dir.join(MANIFEST),
        json!({"id":"a","name":"A","version":"1","script":"main.js",
               "permissions":["models.ask","sessions.read"]})
        .to_string(),
    )
    .unwrap();
    let more = load();
    assert!(grants.changed(&more));
    let mut grants = grants;
    grants.set(&more, true);
    assert!(grants.allows(&more));
    grants.set(&more, false);
    assert!(!grants.allows(&more) && !grants.changed(&more));

    assert_eq!(Grants::parse(Some("not json")), Grants::default());
    // Switches saved before fingerprints existed start off, and are safe to read.
    assert_eq!(Grants::parse(Some(r#"{"a":["model"]}"#)), Grants::default());
}

#[test]
fn a_panel_can_name_its_icon_but_not_with_odd_characters() {
    let root = Root::new();
    let script = Some("return () => visual.text('Hi')");
    let panel = |folder: &str, icon: &str| {
        json!({"id":folder,"name":"X","version":"1",
               "panel":{"title":"X","script":"panel.js","icon":icon}})
    };
    root.tweak("good", panel("good", "folder-tree"), script);
    root.tweak("odd", panel("odd", "../secret"), script);
    let found = discover(&root.0);
    let icon = ready(&found[0])
        .manifest
        .panel
        .as_ref()
        .unwrap()
        .icon
        .as_deref();
    assert_eq!(icon, Some("folder-tree"));
    assert!(broken(&found[1]).contains("icon name"));
}

#[test]
fn prompts_must_be_declared_as_a_permission() {
    let root = Root::new();
    root.tweak(
        "silent",
        json!({"id":"silent","name":"X","version":"1",
               "commands":[{"name":"go","prompt":"Delete everything"}]}),
        None,
    );
    root.tweak(
        "menu",
        json!({"id":"menu","name":"X","version":"1",
               "menu":[{"id":"go","label":"Go","prompt":"Delete everything"}]}),
        None,
    );
    root.tweak(
        "open",
        json!({"id":"open","name":"X","version":"1","permissions":["send_prompts"],
               "commands":[{"name":"go","prompt":"Say hi"}]}),
        None,
    );
    let found = discover(&root.0);
    assert!(broken(&found[0]).contains("send_prompts"));
    assert!(
        ready(&found[1]).manifest.commands.len() == 1,
        "sorted by folder"
    );
    assert!(broken(&found[2]).contains("send_prompts"));
}

#[test]
fn command_arguments_fill_the_placeholder_or_follow_the_prompt() {
    assert_eq!(
        expand_prompt("Fix {{args}} now", " the bug "),
        "Fix the bug now"
    );
    assert_eq!(expand_prompt("Write it", "shorter"), "Write it\n\nshorter");
    assert_eq!(expand_prompt("Write it", " "), "Write it");
}

fn calls(runtime: &mut VisualRuntime) -> Vec<AppCall> {
    runtime.take_calls().unwrap()
}
fn app_call(request: u64, action: &str, input: serde_json::Value) -> AppCall {
    AppCall {
        request,
        action: action.into(),
        input,
    }
}
const CHAT: &str = r#"{"id":"7","title":"Old","project":"p"}"#;

#[test]
fn a_script_runs_from_a_menu_click_and_continues_with_the_answer() {
    let mut script = VisualRuntime::tweak_script(
        "tweak.onMenu('go', (chat) => {
           tweak.app('models.ask', {prompt: 'Title: ' + chat.title}, (answer, error) => {
             tweak.app('sessions.rename', {sessionId: chat.id, title: error || answer.text});
           });
         });",
    )
    .unwrap();
    assert!(calls(&mut script).is_empty());
    script.run_menu("go", CHAT).unwrap();
    assert_eq!(
        calls(&mut script),
        [app_call(1, "models.ask", json!({"prompt": "Title: Old"}))]
    );
    script
        .deliver(1, r#"{"result":{"text":"Login fix"}}"#)
        .unwrap();
    assert_eq!(
        calls(&mut script),
        [app_call(
            2,
            "sessions.rename",
            json!({"sessionId": "7", "title": "Login fix"})
        )]
    );
    // An answer is used once, and an unknown request is ignored.
    script.deliver(1, r#"{"result":{"text":"Again"}}"#).unwrap();
    assert!(calls(&mut script).is_empty());
}
#[test]
fn a_tweak_calls_the_app_only_from_a_handler_and_each_kind_has_its_own_hooks() {
    let loading = VisualRuntime::tweak_script("tweak.app('sessions.list');");
    assert!(loading.err().unwrap().contains("handlers"));
    let mut panel =
        VisualRuntime::with_host("return () => visual.text(typeof tweak.onMenu)", CONTEXT).unwrap();
    let Node::Text { text, .. } = panel.render().unwrap() else {
        panic!()
    };
    assert_eq!(text, "undefined");
    let mut script = VisualRuntime::tweak_script(
        "const kind = typeof tweak.onRefresh;
         tweak.onMenu('go', () => tweak.app('sessions.list', {note: kind}));",
    )
    .unwrap();
    script.run_menu("go", CHAT).unwrap();
    assert_eq!(calls(&mut script)[0].input["note"], "undefined");
}
#[test]
fn a_failing_script_stops_and_says_why() {
    let mut script = VisualRuntime::tweak_script(
        "tweak.onMenu('go', () => { throw new Error('No messages'); });",
    )
    .unwrap();
    assert!(script
        .run_menu("go", CHAT)
        .unwrap_err()
        .contains("No messages"));
    assert!(script.run_menu("go", CHAT).is_err());
    let mut unknown = VisualRuntime::tweak_script("tweak.onMenu('a', () => {});").unwrap();
    assert!(unknown.run_menu("b", CHAT).is_err());
}

#[test]
fn a_menu_item_without_a_prompt_needs_a_script() {
    let root = Root::new();
    root.tweak(
        "nothing",
        json!({"id":"nothing","name":"X","version":"1","menu":[{"id":"a","label":"A"}]}),
        None,
    );
    let dir = root.tweak(
        "scripted",
        json!({"id":"scripted","name":"X","version":"1","script":"main.js",
               "menu":[{"id":"a","label":"A","busy_label":"Working"}]}),
        None,
    );
    std::fs::write(dir.join("main.js"), "tweak.onMenu('a', () => {});").unwrap();
    let found = discover(&root.0);
    assert!(broken(&found[0]).contains("incomplete"));
    assert_eq!(
        ready(&found[1]).script_source.as_deref(),
        Some("tweak.onMenu('a', () => {});")
    );
}

#[test]
fn a_permission_must_be_on_the_app_list() {
    let root = Root::new();
    root.tweak(
        "known",
        json!({"id":"known","name":"X","version":"1","permissions":["sessions.list"]}),
        None,
    );
    root.tweak(
        "unknown",
        json!({"id":"unknown","name":"X","version":"1","permissions":["files.delete"]}),
        None,
    );
    let found = discover(&root.0);
    assert_eq!(ready(&found[0]).manifest.id, "known");
    assert!(broken(&found[1]).contains("files.delete"));
}
#[test]
fn the_title_example_does_its_whole_job_through_app_calls() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/tweaks");
    let found = discover(&examples);
    let tweak = found
        .iter()
        .map(ready)
        .find(|tweak| tweak.manifest.id == "regenerate-title")
        .unwrap();
    let source = tweak.script_source.as_deref().unwrap();
    let mut script = VisualRuntime::tweak_script(source).unwrap();
    script.run_menu("regenerate", CHAT).unwrap();
    let first = calls(&mut script);
    assert_eq!(first[0].action, "sessions.setWorking");
    assert_eq!(first[1].action, "sessions.read");
    assert_eq!(first[1].input["sessionId"], "7");
    let page = r#"{"result":{"turns":[{"turnId":"a","user":{"text":"Fix login"},"assistant":{"text":"Done"}}]}}"#;
    script.deliver(first[1].request, page).unwrap();
    let older = calls(&mut script);
    assert_eq!(older[0].input["before"], 3);
    script.deliver(older[0].request, page).unwrap();
    let ask = calls(&mut script);
    assert_eq!(ask[0].action, "models.ask");
    let prompt = ask[0].input["prompt"].as_str().unwrap();
    assert_eq!(prompt.matches("USER:\nFix login").count(), 1, "{prompt}");
    assert!(prompt.contains("ASSISTANT:\nDone"), "{prompt}");
    script
        .deliver(
            ask[0].request,
            r#"{"result":{"text":"\"Fix   the login bug\"\nExtra line"}}"#,
        )
        .unwrap();
    let rename = calls(&mut script);
    assert_eq!(rename[0].action, "sessions.rename");
    assert_eq!(rename[0].input["title"], "Fix the login bug");

    let mut empty = VisualRuntime::tweak_script(source).unwrap();
    empty.run_menu("regenerate", CHAT).unwrap();
    let start = calls(&mut empty);
    empty
        .deliver(start[1].request, r#"{"result":{"turns":[]}}"#)
        .unwrap();
    let older = calls(&mut empty);
    let error = empty
        .deliver(older[0].request, r#"{"result":{"turns":[]}}"#)
        .unwrap_err();
    assert!(error.contains("no messages"), "{error}");
}
#[test]
fn a_runaway_tweak_is_stopped_after_the_longer_tweak_budget() {
    let mut script =
        VisualRuntime::tweak_script("tweak.onMenu('go', () => { while (true) {} });").unwrap();
    let error = script.run_menu("go", CHAT).unwrap_err();
    assert!(error.contains("250 ms"), "{error}");
    let mut panel = VisualRuntime::with_host("return () => { while (true) {} };", CONTEXT).unwrap();
    assert!(panel.render().unwrap_err().contains("250 ms"));
    let mut visual = VisualRuntime::new("return () => { while (true) {} };").unwrap();
    assert!(visual.render().unwrap_err().contains("60 ms"));
}

#[test]
fn a_script_cannot_get_around_the_call_limit() {
    let mut script = VisualRuntime::tweak_script(
        "tweak.onMenu('go', () => {
           Array.prototype.toJSON = function () {
             return Array(40).fill({request: 1, action: 'sessions.list', input: {}});
           };
           tweak.app('sessions.list');
         });",
    )
    .unwrap();
    script.run_menu("go", CHAT).unwrap();
    assert!(script.take_calls().unwrap_err().contains("Too many"));
    assert!(script.is_stopped());
}
const CONTEXT: &str = r#"{"project":"p"}"#;

#[test]
fn a_panel_reads_context_and_keeps_its_state_when_context_changes() {
    let mut panel = VisualRuntime::with_host(
        "let clicks = 0; return () => visual.column([
           visual.text(tweak.context.project + ':' + clicks),
           visual.button('Add', () => { clicks++; })
         ])",
        CONTEXT,
    )
    .unwrap();
    let text = |node: &Node| match node {
        Node::Column { children, .. } => match &children[0] {
            Node::Text { text, .. } => text.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    let first = panel.render().unwrap();
    assert_eq!(text(&first), "p:0");
    let Node::Column { children, .. } = &first else {
        panic!()
    };
    let Node::Button { handler, .. } = &children[1] else {
        panic!()
    };
    assert_eq!(text(&panel.dispatch(handler, json!(null)).unwrap()), "p:1");
    let moved = panel.set_context(r#"{"project":"q"}"#).unwrap();
    assert_eq!(text(&moved), "q:1");
}

#[test]
fn a_panel_calls_the_app_from_a_click_but_not_while_drawing() {
    let mut panel = VisualRuntime::with_host(
        "return () => visual.button('Go', () => {
           tweak.app('composer.insert', {text: 'hello'});
           tweak.app('worktrees.select', {});
         })",
        CONTEXT,
    )
    .unwrap();
    let Node::Button { handler, .. } = panel.render().unwrap() else {
        panic!()
    };
    assert!(calls(&mut panel).is_empty());
    panel.dispatch(&handler, json!(null)).unwrap();
    assert_eq!(
        calls(&mut panel),
        [
            app_call(1, "composer.insert", json!({"text": "hello"})),
            app_call(2, "worktrees.select", json!({})),
        ]
    );
    let mut drawing = VisualRuntime::with_host(
        "return () => { tweak.app('sessions.list'); return visual.text('y'); }",
        CONTEXT,
    )
    .unwrap();
    assert!(drawing.render().is_err());
}

#[test]
fn a_panel_loads_on_refresh_and_draws_the_answer() {
    let mut panel = VisualRuntime::with_host(
        "let names = ['none'];
         tweak.onRefresh(() => tweak.app('sessions.list', {}, (result) => { names = result.names; }));
         return () => visual.text(names.join(','));",
        CONTEXT,
    )
    .unwrap();
    let text = |node: Node| match node {
        Node::Text { text, .. } => text,
        other => panic!("{other:?}"),
    };
    assert_eq!(text(panel.render().unwrap()), "none");
    panel.refresh_panel().unwrap();
    let asked = calls(&mut panel);
    assert_eq!(asked[0].action, "sessions.list");
    let tree = panel
        .deliver_to_panel(asked[0].request, r#"{"result":{"names":["a","b"]}}"#)
        .unwrap();
    assert_eq!(text(tree), "a,b");
}
#[test]
fn a_panel_stays_sandboxed_and_plain_visuals_get_no_tweak_api() {
    for name in ["require", "process", "fetch", "XMLHttpRequest", "std", "os"] {
        let mut panel =
            VisualRuntime::with_host(&format!("return () => visual.text(typeof {name})"), CONTEXT)
                .unwrap();
        let Node::Text { text, .. } = panel.render().unwrap() else {
            panic!()
        };
        assert_eq!(text, "undefined", "{name}");
    }
    let mut plain = VisualRuntime::new("return () => visual.text(typeof tweak)").unwrap();
    let Node::Text { text, .. } = plain.render().unwrap() else {
        panic!()
    };
    assert_eq!(text, "undefined");
}

#[test]
fn the_tweak_api_cannot_be_replaced_by_the_script() {
    let mut panel = VisualRuntime::with_host(
        "'use strict'; let result = 'kept';
         try { tweak.app = () => {}; result = 'replaced'; } catch (e) {}
         return () => visual.text(result)",
        CONTEXT,
    )
    .unwrap();
    let Node::Text { text, .. } = panel.render().unwrap() else {
        panic!()
    };
    assert_eq!(text, "kept");
}

#[test]
fn the_worktrees_example_lists_the_copies_and_switches_through_app_calls() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/tweaks");
    let found = discover(&examples);
    let tweak = found
        .iter()
        .map(ready)
        .find(|tweak| tweak.manifest.id == "worktrees")
        .unwrap();
    assert_eq!(
        tweak.manifest.permissions,
        ["worktrees.list", "worktrees.select"]
    );
    let mut panel =
        VisualRuntime::with_host(tweak.panel_source.as_deref().unwrap(), CONTEXT).unwrap();
    panel.render().unwrap();
    panel.refresh_panel().unwrap();
    let asked = calls(&mut panel);
    assert_eq!(asked[0].action, "worktrees.list");
    let answer = json!({"result": {"focused": "/a", "worktrees": [
        {"path": "/a", "branch": "main", "main": true, "missing": false},
        {"path": "/b", "branch": "fix", "main": false, "missing": false},
        {"path": "/c", "branch": "gone", "main": false, "missing": true},
    ]}})
    .to_string();
    let tree = panel.deliver_to_panel(asked[0].request, &answer).unwrap();
    let drawn = serde_json::to_string(&tree).unwrap();
    assert!(
        drawn.contains("fix") && drawn.contains("All working copies"),
        "{drawn}"
    );
    assert!(!drawn.contains("gone"), "{drawn}");
}
