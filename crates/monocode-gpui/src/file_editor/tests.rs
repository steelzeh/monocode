use super::*;
use gpui::{EntityInputHandler, TestAppContext};

#[gpui::test]
fn replace_is_one_undo_and_autosave_formats_with_original_encoding(cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!(
        "monocode-editor-parity-{}.json",
        std::process::id()
    ));
    let original = "\u{feff}{\"word\":\"hello\",\"other\":\"hello\"}\r\n";
    std::fs::write(&path, original).unwrap();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            e.open_search(w, cx);
            e.replace_open = true;
            e.search.update(cx, |i, cx| i.set_text("hello".into(), cx));
            e.replacement
                .update(cx, |i, cx| i.set_text("world".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert_eq!(e.matches.len(), 2);
            e.replace_matches(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    window
        .update(cx, |e, w, cx| {
            assert!(e.dirty);
            e.dismiss_search(w, cx);
            w.dispatch_action(Box::new(input::Undo), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            assert!(!e.dirty);
            e.open_search(w, cx);
            e.find_matches(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert_eq!(e.matches.len(), 2);
            e.replace_matches(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| cx.update_global::<crate::settings::Preferences, _>(|p, _| p.autosave = true));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(999));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "\u{feff}{ \"word\": \"world\", \"other\": \"world\" }\r\n"
    );
    window
        .update(cx, |e, _, _| assert!(!e.dirty && !e.saving))
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn autosave_refuses_external_edits_even_before_the_file_poll(cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!(
        "monocode-autosave-conflict-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, "original\n").unwrap();
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences {
            autosave: true,
            ..Default::default()
        })
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            e.input
                .update(cx, |i, cx| i.replace_text_in_range(None, "draft ", w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    std::fs::write(&path, "external\n").unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    window
        .update(cx, |e, _, _| {
            assert!(e.dirty);
            assert!(e.error.as_ref().unwrap().contains("changed on disk"));
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn saving_a_clean_file_still_applies_the_format_on_save_preference(cx: &mut TestAppContext) {
    let path =
        std::env::temp_dir().join(format!("monocode-clean-format-{}.json", std::process::id()));
    std::fs::write(&path, "{\"value\":1}").unwrap();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert!(!e.dirty);
            e.save(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ \"value\": 1 }\n"
    );
    std::fs::remove_file(path).unwrap();
}

fn save_in_project(
    cx: &mut TestAppContext,
    settings_name: &str,
    settings_text: &str,
) -> (String, Option<String>) {
    let dir = tempfile_dir(settings_name);
    let nested = dir.join("api/src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.join("api").join(settings_name), settings_text).unwrap();
    let path = nested.join("a.ts");
    std::fs::write(&path, "const a = \"x\"\n").unwrap();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx.new(|cx| ProjectGit::new(dir.to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window.update(cx, |e, _, cx| e.save(cx)).unwrap();
    cx.run_until_parked();
    let note = window.update(cx, |e, _, _| e.error.clone()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    (saved, note)
}

fn tempfile_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "monocode-prettier-{}-{}",
        tag.trim_start_matches('.'),
        std::process::id()
    ))
}

#[gpui::test]
fn saving_follows_the_project_prettier_settings(cx: &mut TestAppContext) {
    let (saved, note) = save_in_project(cx, ".prettierrc", "singleQuote: true");
    assert_eq!(saved, "const a = 'x';\n");
    assert_eq!(note, None);
}

#[gpui::test]
fn settings_that_are_code_leave_the_text_alone_and_show_a_note(cx: &mut TestAppContext) {
    let (saved, note) = save_in_project(cx, "prettier.config.js", "export default {};");
    assert_eq!(saved, "const a = \"x\"\n");
    assert!(note.unwrap().contains("prettier.config.js"));
}

mod language_server {
    use super::*;
    use monocode_core::language_server::{
        Connection, LanguageServers, Limits, Tool, Update, message,
    };
    use serde_json::{Value, json};
    use std::{
        io::{self, BufReader},
        path::{Path, PathBuf},
        sync::Arc,
    };

    struct FakeServer;
    impl Tool for FakeServer {
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn handles(&self, file: &Path) -> bool {
            file.extension().is_some_and(|e| e == "ts")
        }
        fn language_id(&self, _: &Path) -> &'static str {
            "typescript"
        }
        fn root(&self, file: &Path, _: &Path) -> Option<PathBuf> {
            file.parent().map(Path::to_path_buf)
        }
        fn setup_note(&self) -> Option<String> {
            None
        }
        fn prepare(&self) -> Result<(), String> {
            Ok(())
        }
        fn launch(&self, _: &Path) -> io::Result<Connection> {
            let (from_app, to_server) = io::pipe()?;
            let (from_server, mut to_app) = io::pipe()?;
            std::thread::spawn(move || {
                let mut reader = BufReader::new(from_app);
                while let Ok(Some(request)) = message::read_message(&mut reader) {
                    let result = match request["method"].as_str() {
                        Some("initialize") => json!({"capabilities": {}}),
                        Some("textDocument/diagnostic") => json!({"kind": "full", "items": [{
                            "range": {
                                "start": {"line": 0, "character": 6},
                                "end": {"line": 0, "character": 7},
                            },
                            "severity": 1,
                            "source": "ts",
                            "code": 2322,
                            "message": "Bad type.",
                        }]}),
                        _ => continue,
                    };
                    let answer: Value = json!({"id": request["id"], "result": result});
                    let _ = message::write_message(&mut to_app, &answer);
                }
            });
            Ok(Connection {
                reader: Box::new(from_server),
                writer: Box::new(to_server),
                stop: Box::new(|| {}),
            })
        }
        fn watches(&self, _: &Path) -> bool {
            false
        }
        fn restarts_server(&self, _: &Path) -> bool {
            false
        }
    }

    fn open_editor(cx: &mut TestAppContext) -> (gpui::WindowHandle<FileEditor>, PathBuf) {
        let dir = tempfile_dir("language-server");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.ts");
        std::fs::write(&path, "const a: number = \"x\";\n").unwrap();
        cx.update(|cx| {
            cx.set_global(crate::settings::Preferences::default());
            cx.set_global(crate::language_tools::LanguageTools::new(
                LanguageServers::start(Arc::new(FakeServer), None, Limits::default()),
            ));
        });
        let file = path.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let git = cx.new(|cx| ProjectGit::new(dir.to_string_lossy().into_owned(), cx));
                cx.new(|cx| FileEditor::new(file.to_string_lossy().into_owned(), git, cx))
            })
            .unwrap()
        });
        (window, path)
    }

    fn wait_for_errors(
        cx: &mut TestAppContext,
        window: &gpui::WindowHandle<FileEditor>,
    ) -> Vec<(usize, usize, String)> {
        for _ in 0..1000 {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(crate::language_tools::TYPING_PAUSE);
            cx.run_until_parked();
            let found = window
                .update(cx, |e, _, cx| {
                    e.input
                        .read(cx)
                        .editor_analysis()
                        .diagnostics
                        .iter()
                        .filter(|d| d.server)
                        .map(|d| (d.from, d.to, d.message.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap();
            if !found.is_empty() {
                return found;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("no errors arrived");
    }

    #[gpui::test]
    fn errors_from_the_server_reach_the_editor(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (window, path) = open_editor(cx);
        let errors = wait_for_errors(cx, &window);
        assert_eq!(errors, [(6, 7, "Bad type. ts(2322)".to_string())]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[gpui::test]
    fn errors_for_other_text_are_not_drawn(cx: &mut TestAppContext) {
        let path = tempfile_dir("old-text");
        std::fs::create_dir_all(&path).unwrap();
        let file = path.join("a.txt");
        std::fs::write(&file, "current text").unwrap();
        cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
        let target = file.clone();
        let root = path.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let git = cx.new(|cx| ProjectGit::new(root.to_string_lossy().into_owned(), cx));
                cx.new(|cx| FileEditor::new(target.to_string_lossy().into_owned(), git, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                e.apply_language_update(
                    Update::Errors {
                        text: "older text".into(),
                        errors: vec![monocode_core::language_server::ServerError {
                            from: 0,
                            to: 1,
                            severity: monocode_core::language_server::Severity::Error,
                            message: "Old.".into(),
                        }],
                    },
                    cx,
                );
                assert!(e.input.read(cx).editor_analysis().diagnostics.is_empty());
                e.apply_language_update(Update::Note(Some("A note.".into())), cx);
                assert_eq!(e.language_note.as_deref(), Some("A note."));
            })
            .unwrap();
        std::fs::remove_dir_all(&path).unwrap();
    }
}
