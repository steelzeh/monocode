//! VS Code's `.code-workspace` files: one project shown as several named
//! folders, and the Git repos those folders belong to.
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

const EXTENSION: &str = ".code-workspace";
const FILE_BYTES: u64 = 1024 * 1024;
/// VS Code's default for `git.detectWorktreesLimit`.
const WORKTREE_LIMIT: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub path: String,
}

/// One Git repo in the workspace. Several folders can share one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeWorkspace {
    /// The file's name without its extension, as VS Code titles the window.
    pub title: String,
    pub folders: Vec<Folder>,
    /// `git.detectWorktrees`: also list each repo's linked worktrees.
    pub detect_worktrees: bool,
    pub worktree_limit: usize,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    folders: Vec<Entry>,
    #[serde(default)]
    settings: serde_json::Value,
}
#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

/// Resolves `.` and `..` without touching the disk, so a folder keeps the
/// spelling of the project path and matches the paths the file list shows.
fn normalize(base: &Path, path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in base.join(path).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn base_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The folders a workspace file names. Folders that do not exist are left out,
/// as are repeats. The file may have comments and trailing commas.
pub fn parse(text: &str, base: &Path) -> Result<Vec<Folder>, String> {
    Ok(resolve_folders(read_file(text)?, base))
}

fn read_file(text: &str) -> Result<File, String> {
    serde_json::from_str(&crate::mcp::strip_jsonc(text))
        .map_err(|e| format!("The workspace file is invalid: {e}"))
}

fn resolve_folders(file: File, base: &Path) -> Vec<Folder> {
    let mut folders: Vec<Folder> = Vec::new();
    for entry in file.folders {
        let path = normalize(base, &entry.path);
        if !path.is_dir() {
            continue;
        }
        let path = path.to_string_lossy().into_owned();
        if folders.iter().any(|folder| folder.path == path) {
            continue;
        }
        let name = entry
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| base_name(&path));
        folders.push(Folder { name, path });
    }
    folders
}

/// The workspace file directly inside `project`. With several, the one named
/// after the folder wins, and otherwise the first by name.
pub fn find(project: &Path) -> Option<CodeWorkspace> {
    if project.as_os_str().is_empty()
        || project == Path::new("~")
        || crate::files::remote::is_remote(project)
    {
        return None;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(project)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(EXTENSION))
        })
        .collect();
    files.sort();
    let preferred = format!("{}{EXTENSION}", project.file_name()?.to_string_lossy());
    let file = files
        .iter()
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy() == preferred)
        })
        .or(files.first())?;
    if std::fs::metadata(file).ok()?.len() > FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(file).ok()?;
    let parsed = read_file(&text).ok()?;
    let detect_worktrees = parsed.settings["git.detectWorktrees"].as_bool() == Some(true);
    let worktree_limit = parsed.settings["git.detectWorktreesLimit"]
        .as_u64()
        .map_or(WORKTREE_LIMIT, |limit| limit as usize);
    let folders = resolve_folders(parsed, project);
    if folders.is_empty() {
        return None;
    }
    Some(CodeWorkspace {
        detect_worktrees,
        worktree_limit,
        title: file
            .file_name()?
            .to_string_lossy()
            .trim_end_matches(EXTENSION)
            .to_owned(),
        folders,
    })
}

/// The Git repos behind a workspace's folders, in folder order. A repo takes
/// the name of the folder at its top; folders inside it add no second repo.
/// Runs Git, so call it off the main thread.
pub fn repos(workspace: &CodeWorkspace) -> Vec<Repo> {
    let tops: Vec<(usize, String)> = workspace
        .folders
        .iter()
        .enumerate()
        .filter_map(|(ix, folder)| {
            let top =
                crate::git::git(Path::new(&folder.path), &["rev-parse", "--show-toplevel"]).ok()?;
            Some((ix, top.trim().to_owned()))
        })
        .collect();
    let mut repos: Vec<(String, Repo)> = Vec::new();
    for (_, top) in &tops {
        if repos.iter().any(|(known, _)| known == top) {
            continue;
        }
        let owner = tops.iter().find(|(ix, candidate)| {
            candidate == top && same_folder(&workspace.folders[*ix].path, top)
        });
        let repo = match owner {
            Some((ix, _)) => Repo {
                name: workspace.folders[*ix].name.clone(),
                path: workspace.folders[*ix].path.clone(),
            },
            None => Repo {
                name: base_name(top),
                path: top.clone(),
            },
        };
        repos.push((top.clone(), repo));
    }
    let mut repos: Vec<Repo> = repos.into_iter().map(|(_, repo)| repo).collect();
    if workspace.detect_worktrees {
        add_worktrees(&mut repos, workspace.worktree_limit);
    }
    repos
}

/// Each repo's linked worktrees follow the repos, named after their folders,
/// as VS Code's Source Control lists them. A submodule's worktrees belong to
/// the submodule, so each repo is asked for its own.
fn add_worktrees(repos: &mut Vec<Repo>, limit: usize) {
    let mains = repos.clone();
    let mut added = 0;
    for main in mains {
        let path = Path::new(&main.path);
        if linked_worktree_names(path).is_empty() {
            continue;
        }
        for tree in crate::git::worktrees(path).unwrap_or_default() {
            if tree.main || tree.missing || added >= limit {
                continue;
            }
            if repos.iter().any(|repo| same_folder(&repo.path, &tree.path)) {
                continue;
            }
            repos.push(Repo {
                name: base_name(&tree.path),
                path: tree.path,
            });
            added += 1;
        }
    }
}

/// The names Git keeps for a checkout's linked worktrees. A submodule has a
/// `.git` file pointing at its real Git folder. Starts no process.
fn linked_worktree_names(checkout: &Path) -> Vec<String> {
    let dot_git = checkout.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let Some(target) = std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|text| Some(text.trim().strip_prefix("gitdir:")?.trim().to_owned()))
        else {
            return Vec::new();
        };
        checkout.join(target)
    };
    let Ok(entries) = std::fs::read_dir(git_dir.join("worktrees")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Changes when a worktree is added to or removed from any repo. Reads only
/// Git's folders, so it is cheap enough to check again and again.
pub fn worktree_marker(repos: &[Repo]) -> Vec<String> {
    repos
        .iter()
        .flat_map(|repo| linked_worktree_names(Path::new(&repo.path)))
        .collect()
}

fn same_folder(a: &str, b: &str) -> bool {
    match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("monocode-workspace-{}", uuid::Uuid::new_v4()))
                .canonicalize_or_create();
            Self(path)
        }
        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
        fn repo(&self, name: &str) -> PathBuf {
            let path = self.dir(name);
            let done = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&path)
                .status()
                .unwrap();
            assert!(done.success());
            path
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    trait Create {
        fn canonicalize_or_create(self) -> PathBuf;
    }
    impl Create for PathBuf {
        fn canonicalize_or_create(self) -> PathBuf {
            std::fs::create_dir_all(&self).unwrap();
            self.canonicalize().unwrap()
        }
    }

    const PLATFORM: &str = r#"{
      // Comments and trailing commas are normal in these files.
      "folders": [
        { "path": "api", "name": "⚙️ API" },
        { "path": "packages", "name": "📦 Packages" },
        { "path": "plain" },
        { "path": "missing", "name": "Gone" },
        { "path": ".", "name": "🏠 Root (config)" },
        { "path": "./api", "name": "Repeat" },
      ],
      "settings": { "url": "https://example.com/not-a-comment", },
    }"#;

    #[test]
    fn a_workspace_file_gives_named_folders_in_order() {
        let root = Root::new();
        root.dir("api");
        root.dir("packages");
        root.dir("plain");
        std::fs::write(root.0.join("platform.code-workspace"), PLATFORM).unwrap();
        let workspace = find(&root.0).unwrap();
        assert_eq!(workspace.title, "platform");
        let names: Vec<_> = workspace.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["⚙️ API", "📦 Packages", "plain", "🏠 Root (config)"]
        );
        let paths: Vec<_> = workspace.folders.iter().map(|f| f.path.as_str()).collect();
        let base = root.0.to_string_lossy();
        assert_eq!(paths[0], format!("{base}/api"));
        assert_eq!(paths[3], base, "the root keeps the project's own spelling");
    }

    #[test]
    fn folders_may_sit_outside_the_project() {
        let root = Root::new();
        root.dir("project");
        root.dir("shared");
        let folders = parse(
            r#"{"folders":[{"path":"../shared"}]}"#,
            &root.0.join("project"),
        )
        .unwrap();
        assert_eq!(folders[0].path, root.0.join("shared").to_string_lossy());
        assert_eq!(folders[0].name, "shared");
    }

    #[test]
    fn the_file_named_after_the_project_wins() {
        let root = Root::new();
        let project = root.dir("platform");
        for name in ["a", "b"] {
            std::fs::create_dir_all(project.join(name)).unwrap();
        }
        let text = |name: &str| format!(r#"{{"folders":[{{"path":"{name}"}}]}}"#);
        std::fs::write(project.join("aaa.code-workspace"), text("a")).unwrap();
        std::fs::write(project.join("platform.code-workspace"), text("b")).unwrap();
        assert_eq!(find(&project).unwrap().title, "platform");
        std::fs::remove_file(project.join("platform.code-workspace")).unwrap();
        assert_eq!(find(&project).unwrap().title, "aaa");
    }

    #[test]
    fn no_usable_workspace_file_means_a_normal_project() {
        let root = Root::new();
        assert_eq!(find(&root.0), None);
        std::fs::write(root.0.join("x.code-workspace"), "not json").unwrap();
        assert_eq!(find(&root.0), None);
        std::fs::write(root.0.join("x.code-workspace"), r#"{"folders":[]}"#).unwrap();
        assert_eq!(find(&root.0), None);
        assert_eq!(find(Path::new("~")), None);
    }

    #[test]
    fn folders_inside_one_repo_become_one_section_named_after_its_top() {
        let root = Root::new();
        let platform = root.repo("platform");
        let api = platform.join("api");
        std::fs::create_dir_all(&api).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&api)
            .status()
            .unwrap()
            .success());
        std::fs::create_dir_all(platform.join("packages")).unwrap();
        std::fs::create_dir_all(root.0.join("no-git")).unwrap();
        std::fs::write(
            platform.join("platform.code-workspace"),
            r#"{"folders":[
              {"path":"api","name":"API"},
              {"path":"packages","name":"Packages"},
              {"path":"../no-git","name":"Plain"},
              {"path":".","name":"Root"}
            ]}"#,
        )
        .unwrap();
        let repos = repos(&find(&platform).unwrap());
        let found: Vec<_> = repos
            .iter()
            .map(|r| (r.name.as_str(), r.path.as_str()))
            .collect();
        let (api, platform) = (api.to_string_lossy(), platform.to_string_lossy());
        assert_eq!(found, [("API", api.as_ref()), ("Root", platform.as_ref())]);
    }

    #[test]
    fn a_repo_with_no_folder_at_its_top_uses_its_directory_name() {
        let root = Root::new();
        let repo = root.repo("service");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let workspace = CodeWorkspace {
            title: "x".into(),
            folders: vec![Folder {
                name: "Source".into(),
                path: repo.join("src").to_string_lossy().into_owned(),
            }],
            detect_worktrees: false,
            worktree_limit: WORKTREE_LIMIT,
        };
        assert_eq!(
            repos(&workspace),
            [Repo {
                name: "service".into(),
                path: repo.to_string_lossy().into_owned(),
            }]
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let done = Command::new("git")
            .args(["-c", "user.email=a@example.com", "-c", "user.name=A"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(done.status.success(), "{done:?}");
    }

    fn committed(path: &Path) {
        std::fs::write(path.join("file.txt"), "one\n").unwrap();
        git(path, &["add", "-A"]);
        git(path, &["commit", "-q", "-m", "first"]);
    }

    fn names(repos: &[Repo]) -> Vec<&str> {
        repos.iter().map(|repo| repo.name.as_str()).collect()
    }

    fn workspace_with(project: &Path, settings: &str) -> CodeWorkspace {
        std::fs::write(
            project.join("p.code-workspace"),
            format!(r#"{{"folders":[{{"path":".","name":"Main"}}],"settings":{{{settings}}}}}"#),
        )
        .unwrap();
        find(project).unwrap()
    }

    #[test]
    fn worktrees_are_listed_only_when_the_workspace_asks_for_them() {
        let root = Root::new();
        let project = root.repo("project");
        committed(&project);
        git(
            &project,
            &["worktree", "add", "-q", "-b", "feat", "../feat-one"],
        );
        let off = workspace_with(&project, "");
        assert_eq!(names(&repos(&off)), ["Main"]);
        let on = workspace_with(&project, r#""git.detectWorktrees": true"#);
        let found = repos(&on);
        assert_eq!(names(&found), ["Main", "feat-one"]);
        assert!(found[1].path.ends_with("/feat-one"));
        let none = workspace_with(
            &project,
            r#""git.detectWorktrees": true, "git.detectWorktreesLimit": 0"#,
        );
        assert_eq!(names(&repos(&none)), ["Main"]);
    }

    /// A submodule has a `.git` file, and Git reports its "main" worktree as
    /// the module's folder inside the parent. That one must not be listed.
    #[test]
    fn a_submodules_worktrees_are_found_through_its_git_file() {
        let root = Root::new();
        let checkout = root.0.join("api");
        std::fs::create_dir_all(root.0.join("modules")).unwrap();
        git(
            &root.0,
            &["init", "-q", "--separate-git-dir=modules/api", "api"],
        );
        assert!(checkout.join(".git").is_file());
        committed(&checkout);
        git(
            &checkout,
            &["worktree", "add", "-q", "-b", "feat", "../api-feat"],
        );
        let on = workspace_with(&checkout, r#""git.detectWorktrees": true"#);
        let found = repos(&on);
        assert_eq!(names(&found), ["Main", "api-feat"], "{found:?}");
    }

    #[test]
    fn the_marker_changes_when_a_worktree_comes_or_goes() {
        let root = Root::new();
        let project = root.repo("project");
        committed(&project);
        let repo = [Repo {
            name: "Main".into(),
            path: project.to_string_lossy().into_owned(),
        }];
        let before = worktree_marker(&repo);
        assert!(before.is_empty());
        git(&project, &["worktree", "add", "-q", "-b", "feat", "../one"]);
        let added = worktree_marker(&repo);
        assert_eq!(added.len(), 1);
        git(&project, &["worktree", "remove", "../one"]);
        assert_eq!(worktree_marker(&repo), before);
    }
}
