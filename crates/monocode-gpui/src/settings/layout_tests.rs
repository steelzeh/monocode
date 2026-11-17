use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};

fn window(section: Section, cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        section,
                        Preferences::default(),
                        SettingsContext::default(),
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

#[gpui::test]
fn workspace_style_is_only_offered_under_the_new_layout(cx: &mut TestAppContext) {
    let window = window(Section::Appearance, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for classic in [false, true, false] {
        window
            .update(visual, |view, _, cx| {
                view.ctx.classic = classic;
                view.ctx.new_layout_boxy = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-boxy")
                .is_some(),
            !classic
        );
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-rounded")
                .is_some(),
            !classic
        );
        assert!(
            visual
                .debug_bounds("settings-collapsed-project-rail-hidden")
                .is_none()
        );
    }
}

#[gpui::test]
fn skills_document_has_its_own_full_height_pane_and_compact_actions(cx: &mut TestAppContext) {
    let window = window(Section::Skills, cx);
    window
        .update(cx, |view, _, cx| {
            let path = std::path::PathBuf::from("/project/.agents/skills/review/SKILL.md");
            view.discovery.skills = vec![data::Skill {
                name: "review".into(),
                description: "Review changes".into(),
                path: path.clone(),
                scope: "project".into(),
                provider: "agents".into(),
            }];
            view.skill_preview = Some(path);
            view.skill_preview_text = Some(Ok(skill_document::DocumentData::new(format!(
                "---\nname: review\n---\n# Review\n\n{}",
                "Instructions.\n\n".repeat(150)
            ))));
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    let body = visual.debug_bounds("skill-document").unwrap();
    assert_eq!(left.size.height, px(800.));
    assert_eq!(right.size.height, px(800.));
    assert!(right.left() >= left.right());
    assert!(body.bottom() <= right.bottom());
    assert!(body.size.height > px(600.));
    assert!(visual.debug_bounds("transcript").is_none());
    for id in [
        "skill-preview-action",
        "skill-copy-action",
        "skill-reveal-action",
    ] {
        assert_eq!(
            visual.debug_bounds(id).unwrap().size,
            size(px(20.), px(20.))
        );
    }
    let source = visual
        .debug_bounds("settings-skill-document-source")
        .unwrap();
    assert!(source.size.width < px(100.));
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.skill_source_paths.len(), 1)
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    // Deliver the frame that applies the measured Skills container breakpoint.
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    assert!(
        right.top() >= left.bottom(),
        "list={left:?}, preview={right:?}"
    );
    assert_eq!(left.size.width, px(700.));
    assert_eq!(right.size.width, px(700.));
    assert!(visual.debug_bounds("skill-document").unwrap().size.height > px(200.));
}

#[gpui::test]
fn mono_mascot_is_beside_name_and_sidebar_switch_stays_visible(cx: &mut TestAppContext) {
    let window = window(Section::Monos, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.monos = vec![MonoSetting {
                id: "layout-mono".into(),
                name: "Big Boss".into(),
                mascot: "ghost".into(),
                color: 0xee99bb,
                projects: vec!["monocode".into(), "mono-web".into()],
                show: false,
                folders: false,
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let mascot = visual.debug_bounds("settings-mono-mascot").unwrap();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    let reset = visual.debug_bounds("settings-mono-reset").unwrap();
    assert_eq!(mascot.size, size(px(16.), px(16.)));
    assert_eq!(switch.size, size(px(36.), px(20.)));
    let name = visual.debug_bounds("settings-mono-name").unwrap();
    assert!(mascot.right() < name.left());
    assert!(mascot.left() < px(200.));
    assert!(switch.left() > px(800.));
    assert!(switch.right() <= reset.left());
    assert!(reset.right() <= px(1200.));
    assert!((mascot.top() - switch.top()).abs() < px(4.));
    assert!(
        visual
            .debug_bounds("settings-toggle-mono-sidebar-folders")
            .is_none()
    );
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    window
        .update(visual, |view, window, cx| {
            view.close_management(window, cx);
            cx.notify();
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    assert_eq!(switch.size, size(px(36.), px(20.)));
    assert!(switch.right() <= px(700.));
}

#[gpui::test]
fn a_tweak_row_shows_its_version_author_and_permissions_as_labels(cx: &mut TestAppContext) {
    use monocode_core::tweaks::{Found, Manifest, Tweak};
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "id": "t", "name": "Title", "version": "v1.2.0", "author": "steelzeh",
        "description": "Write a new title.",
        "permissions": ["sessions.read", "models.ask"],
        "script": "main.js",
        "menu": [{"id": "go", "label": "Regenerate title"}],
        "commands": [{"name": "retitle", "prompt": "Retitle"}]
    }))
    .unwrap();
    assert_eq!(
        super::pages::tweak_meta_line(&manifest, None),
        "v1.2.0 · by steelzeh"
    );
    let labels = |manifest: &Manifest| {
        super::pages::tweak_adds_labels(manifest)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(&manifest),
        ["/retitle", "Chat menu: Regenerate title"]
    );
    let mut same_name = manifest.clone();
    same_name.name = "Regenerate title".into();
    assert_eq!(
        labels(&same_name),
        ["/retitle", "Chat menu"],
        "no repeated name"
    );
    let mut plain = manifest.clone();
    plain.author = Some("  ".into());
    plain.version = "2".into();
    assert_eq!(super::pages::tweak_meta_line(&plain, None), "v2");

    let store = monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    cx.update(|cx| {
        crate::tweaks::install(std::sync::Arc::new(std::sync::Mutex::new(store)), cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        crate::tweaks::get(cx).unwrap().update(cx, |center, cx| {
            center.set_found(
                vec![Found::Ready(Tweak {
                    manifest,
                    dir: "/tweaks/t".into(),
                    panel_source: None,
                    script_source: Some("tweak.onMenu('go', () => {});".into()),
                    fingerprint: String::new(),
                    source: None,
                })],
                cx,
            )
        })
    });
    let window = window(Section::Tweaks, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-tweak-meta-t").is_some());
    let first = visual
        .debug_bounds("settings-tweak-permission-t-0")
        .unwrap();
    let second = visual
        .debug_bounds("settings-tweak-permission-t-1")
        .unwrap();
    assert_eq!(first.top(), second.top(), "labels sit in one row");
    assert!(second.left() > first.right());
    assert!(visual.debug_bounds("settings-tweak-adds-t-1").is_some());
}

#[gpui::test]
fn add_from_github_shows_the_prompts_and_installs_only_after_confirming(cx: &mut TestAppContext) {
    use monocode_core::tweaks::{Fetched, Found, Manifest, Source};
    let raw = serde_json::json!({
        "id": "standup", "name": "Standup", "version": "1.0.0", "author": "someone",
        "description": "Write my standup.",
        "permissions": ["send_prompts"],
        "commands": [{"name": "standup", "prompt": "Write my standup about {{args}}"}]
    })
    .to_string();
    let manifest: Manifest = serde_json::from_str(&raw).unwrap();
    let root = std::env::temp_dir().join(format!("monocode-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    cx.update(|cx| {
        crate::tweaks::install(std::sync::Arc::new(std::sync::Mutex::new(store)), cx);
        crate::tweaks::get(cx)
            .unwrap()
            .update(cx, |center, _| center.set_root(root.clone()));
    });
    cx.run_until_parked();
    // A tweak of the same name makes the fingerprint real, through a real load.
    std::fs::create_dir_all(root.join(".probe/standup")).unwrap();
    std::fs::write(root.join(".probe/standup/tweak.json"), &raw).unwrap();
    let probe = monocode_core::tweaks::discover(&root.join(".probe"));
    let Found::Ready(probe) = &probe[0] else {
        panic!("{probe:?}")
    };
    let fingerprint = probe.fingerprint.clone();
    std::fs::remove_dir_all(root.join(".probe")).unwrap();
    let fetched = Fetched {
        manifest,
        raw_manifest: raw,
        files: Vec::new(),
        source: Source {
            repo: "owner/repo".into(),
            path: String::new(),
            commit: "abcdef0123456789abcdef0123456789abcdef01".into(),
        },
        fingerprint,
    };
    let window = window(Section::Tweaks, cx);
    window
        .update(cx, |view, _, cx| view.preview_tweak_for_test(fetched, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-tweak-preview").is_some());
    assert!(
        visual
            .debug_bounds("settings-tweak-preview-prompt-0")
            .is_some(),
        "the prompt is shown in full before installing"
    );
    assert!(
        monocode_core::tweaks::discover(&root).is_empty(),
        "nothing on disk yet"
    );
    let broken = [Found::Broken {
        folder: "standup".into(),
        error: "typo".into(),
    }];
    assert!(super::pages::tweak_replace_note(&broken, "standup").contains("broken folder"));
    assert_eq!(
        super::pages::tweak_replace_note(&[], "standup"),
        "It stays off until you turn it on."
    );
    window
        .update(visual, |view, _, cx| {
            view.confirm_tweak_install_for_test(cx);
            view.cancel_tweak_install(cx);
            assert!(
                view.tweak_install_open(),
                "Cancel waits for a running install"
            );
        })
        .unwrap();
    visual.run_until_parked();
    let found = monocode_core::tweaks::discover(&root);
    assert_eq!(found.len(), 1);
    window
        .update(visual, |view, _, cx| {
            assert!(!view.tweak_install_open(), "the form closes");
            let center = crate::tweaks::get(cx).unwrap();
            let center = center.read(cx);
            let Some(Found::Ready(tweak)) = center.found().first() else {
                panic!("the list reloads")
            };
            assert!(!center.is_on(tweak), "installed tweaks stay off");
            assert_eq!(tweak.source.as_ref().unwrap().repo, "owner/repo");
        })
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}
