//! The Changes tab for a VS Code workspace: one section per Git repo, each
//! with the commit box and file rows of the normal Changes view.
use crate::{
    changes::{Changes, ChangesEvent, ChangesView},
    explorer::changed_range,
    right_panel::pane_header,
    theme::*,
    workspace_folders::{WorkspaceChanged, WorkspaceFolders},
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, ListAlignment, ListState, Role, Subscription,
    Window, div, list, prelude::*, px,
};
use monocode_core::provider::Kind;
use std::collections::HashMap;

struct Section {
    name: String,
    path: String,
    changes: Entity<Changes>,
    _subscriptions: [Subscription; 2],
}

#[derive(Clone, PartialEq)]
enum Row {
    Header(usize),
    Commit(usize),
    Line(usize, usize),
    Note(usize, String),
}

pub(crate) struct WorkspaceChanges {
    folders: Entity<WorkspaceFolders>,
    sections: Vec<Section>,
    /// Sections the user folded or opened. The rest open when they have changes.
    toggled: HashMap<String, bool>,
    rows: Vec<Row>,
    list: ListState,
    enabled: bool,
    view: ChangesView,
    text_provider: Option<Kind>,
    _folders: Subscription,
}
impl EventEmitter<ChangesEvent> for WorkspaceChanges {}

impl WorkspaceChanges {
    pub(crate) fn new(folders: Entity<WorkspaceFolders>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(&folders, |this, _, _: &WorkspaceChanged, cx| this.sync(cx));
        let mut this = Self {
            folders,
            sections: Vec::new(),
            toggled: HashMap::new(),
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(300.)),
            enabled: false,
            view: ChangesView::List,
            text_provider: None,
            _folders: subscription,
        };
        this.sync(cx);
        this
    }
    /// With no Git repos in a workspace, the normal Changes view stays.
    pub(crate) fn active(&self) -> bool {
        !self.sections.is_empty()
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.enabled = enabled;
        for section in &self.sections {
            section
                .changes
                .update(cx, |changes, cx| changes.set_enabled(enabled, cx));
        }
    }
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        for section in &self.sections {
            section.changes.update(cx, |changes, cx| changes.resume(cx));
        }
    }
    pub(crate) fn set_view(&mut self, view: ChangesView, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        self.view = view;
        for section in &self.sections {
            section
                .changes
                .update(cx, |changes, cx| changes.set_view(view, cx));
        }
    }
    pub(crate) fn set_text_provider(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.text_provider = Some(kind);
        for section in &self.sections {
            section
                .changes
                .update(cx, |changes, _| changes.text_provider = kind);
        }
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<_> = self
            .folders
            .read(cx)
            .repos()
            .iter()
            .map(|repo| (repo.name.clone(), repo.path.clone(), repo.git.clone()))
            .collect();
        let mut old = std::mem::take(&mut self.sections);
        for (name, path, git) in wanted {
            let section = match old.iter().position(|have| have.path == path) {
                Some(ix) => {
                    let mut section = old.swap_remove(ix);
                    section.name = name;
                    section
                }
                None => self.new_section(name, path, git, cx),
            };
            self.sections.push(section);
        }
        self.rebuild(cx);
    }
    fn new_section(
        &self,
        name: String,
        path: String,
        git: Entity<crate::git_status::ProjectGit>,
        cx: &mut Context<Self>,
    ) -> Section {
        let (enabled, view, provider) = (self.enabled, self.view, self.text_provider);
        let changes = cx.new(|cx| {
            let mut changes = Changes::detached(git, cx);
            changes.set_view(view, cx);
            changes.set_enabled(enabled, cx);
            if let Some(kind) = provider {
                changes.text_provider = kind;
            }
            changes
        });
        Section {
            name,
            path,
            _subscriptions: [
                cx.observe(&changes, |this, _, cx| this.rebuild(cx)),
                cx.subscribe(&changes, |this, _, event: &ChangesEvent, cx| match event {
                    ChangesEvent::View(view) => {
                        this.set_view(*view, cx);
                        cx.emit(ChangesEvent::View(*view));
                    }
                    ChangesEvent::OpenDiff { cwd, path, staged } => {
                        cx.emit(ChangesEvent::OpenDiff {
                            cwd: cwd.clone(),
                            path: path.clone(),
                            staged: *staged,
                        });
                    }
                }),
            ],
            changes,
        }
    }
    fn is_open(&self, section: &Section, cx: &Context<Self>) -> bool {
        self.toggled
            .get(&section.path)
            .copied()
            .unwrap_or_else(|| section.changes.read(cx).has_files(cx))
    }
    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(section) = self.sections.iter().find(|section| section.path == path) else {
            return;
        };
        let open = !self.is_open(section, cx);
        self.toggled.insert(path.to_owned(), open);
        self.rebuild(cx);
    }
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut rows = Vec::new();
        let mut commit_rows = Vec::new();
        for (ix, section) in self.sections.iter().enumerate() {
            rows.push(Row::Header(ix));
            if !self.is_open(section, cx) {
                continue;
            }
            commit_rows.push(rows.len());
            rows.push(Row::Commit(ix));
            let changes = section.changes.read(cx);
            match changes.empty_text(cx) {
                Some(text) => rows.push(Row::Note(ix, text)),
                None => rows.extend((0..changes.line_count()).map(|line| Row::Line(ix, line))),
            }
        }
        let (start, old_end, new_end) = changed_range(&self.rows, &rows);
        self.rows = rows;
        if start < old_end || start < new_end {
            self.list.splice(start..old_end, new_end - start);
        }
        // The commit box grows with its message and its sync actions.
        for row in commit_rows {
            self.list.remeasure_items(row..row + 1);
        }
        cx.notify();
    }
    fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let body = match row {
            Row::Header(section) => {
                let open = self.is_open(&self.sections[section], cx);
                let name = self.sections[section].name.clone();
                let path = self.sections[section].path.clone();
                let changes = self.sections[section].changes.clone();
                let entity = cx.entity().downgrade();
                changes.update(cx, |changes, cx| {
                    changes.repo_header(
                        name,
                        open,
                        move |cx| {
                            let _ = entity.update(cx, |this, cx| this.toggle(&path, cx));
                        },
                        cx,
                    )
                })
            }
            Row::Commit(section) => {
                let changes = self.sections[section].changes.clone();
                changes.update(cx, |changes, cx| changes.commit_box_element(cx))
            }
            Row::Line(section, line) => {
                let changes = self.sections[section].changes.clone();
                changes.update(cx, |changes, cx| changes.line_element(line, cx))
            }
            Row::Note(_, text) => div()
                .px_3()
                .py_2()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child(text)
                .into_any_element(),
        };
        // Rows of different repos reuse the same inner ids, so each gets its own.
        div()
            .id(("workspace-change-row", ix))
            .debug_selector(move || format!("workspace-change-row-{ix}"))
            .w_full()
            .child(body)
            .into_any_element()
    }
}

impl Render for WorkspaceChanges {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let menus: Vec<AnyElement> = self
            .sections
            .iter()
            .map(|section| section.changes.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|changes| changes.update(cx, |changes, cx| changes.menu_element(cx)))
            .collect();
        div()
            .id("workspace-changes")
            .debug_selector(|| "workspace-changes".into())
            .role(Role::TabPanel)
            .aria_label("Changes")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(pane_header("changes-header", "Changes").child("Changes"))
            .child(
                div().flex_1().min_h_0().child(
                    list(self.list.clone(), move |ix, _, cx| {
                        entity.update(cx, |this, cx| this.row(ix, cx))
                    })
                    .size_full(),
                ),
            )
            .children(menus)
    }
}

#[cfg(test)]
impl WorkspaceChanges {
    pub(crate) fn section_names(&self) -> Vec<String> {
        self.sections.iter().map(|s| s.name.clone()).collect()
    }
    pub(crate) fn row_summary(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| match row {
                Row::Header(s) => format!("header {s}"),
                Row::Commit(s) => format!("commit {s}"),
                Row::Line(s, l) => format!("line {s} {l}"),
                Row::Note(s, t) => format!("note {s} {t}"),
            })
            .collect()
    }
    pub(crate) fn toggle_for_test(&mut self, ix: usize, cx: &mut Context<Self>) {
        let path = self.sections[ix].path.clone();
        self.toggle(&path, cx);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{path::PathBuf, process::Command};

    /// A folder like a real multi-repo project: a repo with a nested repo in
    /// it, a plain folder of the outer repo, and a workspace file naming them.
    pub(crate) struct Platform(pub PathBuf);
    impl Drop for Platform {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn git(dir: &PathBuf, args: &[&str]) {
        let done = Command::new("git")
            .args(["-c", "user.email=a@example.com", "-c", "user.name=A"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(done.status.success(), "{done:?}");
    }
    impl Platform {
        pub(crate) fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "monocode-platform-{}",
                monocode_core::mono::new_id()
            ));
            std::fs::create_dir_all(root.join("api")).unwrap();
            std::fs::create_dir_all(root.join("packages/ui")).unwrap();
            let root = root.canonicalize().unwrap();
            let this = Self(root.clone());
            std::fs::write(
                root.join("platform.code-workspace"),
                r#"{
                  // VS Code writes files like this one.
                  "folders": [
                    { "path": "api", "name": "API" },
                    { "path": "packages", "name": "Packages" },
                    { "path": ".", "name": "Root" },
                  ],
                }"#,
            )
            .unwrap();
            std::fs::write(root.join(".gitignore"), "api/\n").unwrap();
            std::fs::write(root.join("packages/ui/index.ts"), "one\n").unwrap();
            std::fs::write(root.join("config.txt"), "one\n").unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["add", "-A"]);
            git(&root, &["commit", "-q", "-m", "first"]);
            let api = root.join("api");
            std::fs::write(api.join("server.ts"), "one\n").unwrap();
            git(&api, &["init", "-q", "-b", "staging"]);
            git(&api, &["add", "-A"]);
            git(&api, &["commit", "-q", "-m", "first"]);
            std::fs::write(api.join("server.ts"), "two\nthree\n").unwrap();
            this
        }
        pub(crate) fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Platform;
    use super::*;
    use crate::{git_status::ProjectGit, workspace_folders::WorkspaceFolders};
    use gpui::{TestAppContext, VisualTestContext, size};

    type Opened = std::rc::Rc<std::cell::RefCell<Vec<(String, Option<String>)>>>;

    struct Root {
        view: Entity<WorkspaceChanges>,
        events: Opened,
        _subscription: Subscription,
    }
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().child(self.view.clone())
        }
    }

    fn open(project: String, cx: &mut TestAppContext) -> gpui::WindowHandle<Root> {
        cx.update(crate::popover::bind);
        cx.update(crate::input::bind);
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                        gpui::point(px(0.), px(0.)),
                        size(px(420.), px(800.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    let git = cx.new(|cx| ProjectGit::new(project, cx));
                    let folders = cx.new(|cx| WorkspaceFolders::new(git, cx));
                    let view = cx.new(|cx| WorkspaceChanges::new(folders, cx));
                    view.update(cx, |view, cx| view.set_enabled(true, cx));
                    cx.new(|cx| {
                        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                        let seen = events.clone();
                        let subscription = cx.subscribe(&view, move |_, _, event, _| {
                            if let ChangesEvent::OpenDiff { cwd, path, .. } = event {
                                seen.borrow_mut().push((cwd.clone(), path.clone()));
                            }
                        });
                        Root {
                            view,
                            events,
                            _subscription: subscription,
                        }
                    })
                },
            )
            .unwrap()
        });
        settle(cx);
        window
    }

    /// Git runs for real here, so give its worker threads time to finish.
    fn settle(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        for _ in 0..40 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        cx.run_until_parked();
    }

    #[gpui::test]
    fn each_repo_gets_a_section_and_folders_inside_a_repo_add_none(cx: &mut TestAppContext) {
        let platform = Platform::new();
        let window = open(platform.path(), cx);
        window
            .update(cx, |root, _, cx| {
                let view = root.view.read(cx);
                assert!(view.active());
                assert_eq!(view.section_names(), ["API", "Root"]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn sections_with_changes_start_open_and_can_fold(cx: &mut TestAppContext) {
        let platform = Platform::new();
        let window = open(platform.path(), cx);
        let rows = |window: &gpui::WindowHandle<Root>, cx: &mut TestAppContext| {
            window
                .update(cx, |root, _, cx| root.view.read(cx).row_summary())
                .unwrap()
        };
        // API has a changed file; Root has none, so it stays folded.
        let open_rows = rows(&window, cx);
        assert_eq!(open_rows[0], "header 0");
        assert_eq!(open_rows[1], "commit 0");
        assert!(open_rows[2].starts_with("line 0"), "{open_rows:?}");
        assert_eq!(open_rows.last().unwrap(), "header 1");
        window
            .update(cx, |root, _, cx| {
                root.view.update(cx, |view, cx| view.toggle_for_test(0, cx))
            })
            .unwrap();
        assert_eq!(rows(&window, cx), ["header 0", "header 1"]);
        window
            .update(cx, |root, _, cx| {
                root.view.update(cx, |view, cx| view.toggle_for_test(1, cx))
            })
            .unwrap();
        let root_open = rows(&window, cx);
        assert_eq!(root_open[2], "commit 1");
        assert!(
            root_open[3].contains("No uncommitted changes"),
            "{root_open:?}"
        );
    }

    #[gpui::test]
    fn clicking_a_changed_file_opens_its_diff_in_its_own_repo(cx: &mut TestAppContext) {
        let platform = Platform::new();
        let window = open(platform.path(), cx);
        let api = format!("{}/api", platform.path());
        let summary = window
            .update(cx, |root, _, cx| root.view.read(cx).row_summary())
            .unwrap();
        // The header, the commit box, "Changes", then the file.
        assert!(summary[3].starts_with("line 0"), "{summary:?}");
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("workspace-change-row-3").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        let events = window
            .update(visual, |root, _, _| root.events.borrow().clone())
            .unwrap();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, api);
        assert!(events[0].1.as_deref().unwrap().ends_with("server.ts"));
    }

    #[gpui::test]
    fn the_title_bar_count_adds_up_every_repo(cx: &mut TestAppContext) {
        let platform = Platform::new();
        std::fs::write(platform.0.join("config.txt"), "two\nthree\nfour\n").unwrap();
        let window = open(platform.path(), cx);
        window
            .update(cx, |root, _, cx| {
                let folders = root.view.read(cx).folders.read(cx);
                let total = folders.total_stats(cx).unwrap();
                assert_eq!(total.files, 2, "one file in each repo");
                assert!(total.additions >= 2);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_changes_tab_shows_the_sections(cx: &mut TestAppContext) {
        use crate::right_panel::{RightPanel, RightTab};
        let platform = Platform::new();
        let project = platform.path();
        cx.update(crate::popover::bind);
        cx.update(crate::input::bind);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let git = cx.new(|cx| ProjectGit::new(project.clone(), cx));
                let folders = cx.new(|cx| WorkspaceFolders::new(git.clone(), cx));
                cx.new(|cx| {
                    let mut panel =
                        RightPanel::new(git, folders, RightTab::Changes, Vec::new(), window, cx);
                    panel.set_shown(true, cx);
                    panel
                })
            })
            .unwrap()
        });
        settle(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("workspace-changes").is_some());
        assert!(visual.debug_bounds("workspace-change-row-0").is_some());
    }

    #[gpui::test]
    fn worktrees_get_sections_when_the_workspace_asks_and_new_ones_appear(cx: &mut TestAppContext) {
        use super::test_support::git;
        let platform = Platform::new();
        std::fs::write(
            platform.0.join("platform.code-workspace"),
            r#"{
              "folders": [{ "path": "api", "name": "API" }, { "path": ".", "name": "Root" }],
              "settings": { "git.detectWorktrees": true },
            }"#,
        )
        .unwrap();
        let api = platform.0.join("api");
        git(
            &api,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "one",
                "../.worktrees/api-one",
            ],
        );
        let window = open(platform.path(), cx);
        let names = |window: &gpui::WindowHandle<Root>, cx: &mut TestAppContext| {
            window
                .update(cx, |root, _, cx| root.view.read(cx).section_names())
                .unwrap()
        };
        assert_eq!(names(&window, cx), ["API", "Root", "api-one"]);
        git(
            &api,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "two",
                "../.worktrees/api-two",
            ],
        );
        window
            .update(cx, |root, _, cx| {
                let folders = root.view.read(cx).folders.clone();
                folders.update(cx, |folders, cx| folders.refresh(cx));
            })
            .unwrap();
        settle(cx);
        assert_eq!(names(&window, cx), ["API", "Root", "api-one", "api-two"]);
        git(
            &api,
            &["worktree", "remove", "--force", "../.worktrees/api-one"],
        );
        window
            .update(cx, |root, _, cx| {
                let folders = root.view.read(cx).folders.clone();
                folders.update(cx, |folders, cx| folders.refresh(cx));
            })
            .unwrap();
        settle(cx);
        assert_eq!(names(&window, cx), ["API", "Root", "api-two"]);
    }

    #[gpui::test]
    fn a_project_without_a_workspace_file_keeps_the_normal_view(cx: &mut TestAppContext) {
        let dir =
            std::env::temp_dir().join(format!("monocode-plain-{}", monocode_core::mono::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let window = open(dir.to_string_lossy().into_owned(), cx);
        window
            .update(cx, |root, _, cx| assert!(!root.view.read(cx).active()))
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
