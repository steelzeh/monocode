//! A project's VS Code workspace: its named folders, and one Git status per
//! repo behind them. The Explorer and Changes tabs read from here.
use crate::git_status::{GitChanged, ProjectGit};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Subscription, Task};
use monocode_core::{
    code_workspace::{self, CodeWorkspace, Repo},
    scm::DiffStats,
};
use std::{collections::HashMap, path::PathBuf, time::Duration};

/// The project's folders or repos changed.
pub(crate) struct WorkspaceChanged;
/// Files changed in a repo other than the project's own.
pub(crate) struct WorkspaceRepoChanged;

pub(crate) struct WorkspaceRepo {
    pub name: String,
    pub path: String,
    pub git: Entity<ProjectGit>,
    _changes: [Subscription; 2],
}

type Known = (Option<CodeWorkspace>, Vec<Repo>, Vec<String>);
/// How often to notice a worktree added or removed while the app is in front.
const WORKTREE_CHECK: Duration = Duration::from_secs(30);

pub(crate) struct WorkspaceFolders {
    primary: Entity<ProjectGit>,
    project: String,
    workspace: Option<CodeWorkspace>,
    repos: Vec<WorkspaceRepo>,
    /// The last answer per project, so switching back shows it at once.
    known: HashMap<String, Known>,
    reading: bool,
    pending: bool,
    polling: bool,
    worktree_check: Option<Task<()>>,
    _primary: Subscription,
}
impl EventEmitter<WorkspaceChanged> for WorkspaceFolders {}
impl EventEmitter<WorkspaceRepoChanged> for WorkspaceFolders {}

impl WorkspaceFolders {
    pub(crate) fn new(primary: Entity<ProjectGit>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&primary, |this, _, _: &GitChanged, cx| this.refresh(cx));
        let mut this = Self {
            primary,
            project: String::new(),
            workspace: None,
            repos: Vec::new(),
            known: HashMap::new(),
            reading: false,
            pending: false,
            polling: false,
            worktree_check: None,
            _primary: subscription,
        };
        this.refresh(cx);
        this
    }
    pub(crate) fn project(&self) -> &str {
        &self.project
    }
    pub(crate) fn workspace(&self) -> Option<&CodeWorkspace> {
        self.workspace.as_ref()
    }
    pub(crate) fn repos(&self) -> &[WorkspaceRepo] {
        &self.repos
    }
    /// Reads the workspace file again, and asks Git only if the folders changed.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            self.pending = true;
            return;
        }
        let project = self.primary.read(cx).project().to_owned();
        if project != self.project {
            let (workspace, repos, _) = self.known.get(&project).cloned().unwrap_or_default();
            self.project = project.clone();
            self.apply(workspace, repos, cx);
        }
        self.reading = true;
        let previous = self.known.get(&project).cloned();
        let path = PathBuf::from(&project);
        let work = cx.background_executor().spawn(async move {
            let workspace = code_workspace::find(&path);
            match previous {
                Some((known, repos, marker))
                    if known == workspace && marker == code_workspace::worktree_marker(&repos) =>
                {
                    (workspace, repos, marker)
                }
                _ => {
                    let repos = workspace
                        .as_ref()
                        .map(code_workspace::repos)
                        .unwrap_or_default();
                    let marker = code_workspace::worktree_marker(&repos);
                    (workspace, repos, marker)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let (workspace, repos, marker) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                this.known
                    .insert(project.clone(), (workspace.clone(), repos.clone(), marker));
                if this.project == project {
                    this.apply(workspace, repos, cx);
                }
                if std::mem::take(&mut this.pending) {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }
    /// A repo keeps its status reader when only the list around it changes.
    fn apply(
        &mut self,
        workspace: Option<CodeWorkspace>,
        repos: Vec<Repo>,
        cx: &mut Context<Self>,
    ) {
        let same = self.workspace == workspace
            && self.repos.len() == repos.len()
            && self
                .repos
                .iter()
                .zip(&repos)
                .all(|(have, want)| have.path == want.path && have.name == want.name);
        if same {
            return;
        }
        self.workspace = workspace;
        let mut old = std::mem::take(&mut self.repos);
        for repo in repos {
            let git = match old.iter().position(|have| have.path == repo.path) {
                Some(ix) => old.swap_remove(ix).git,
                None if repo.path == self.project => self.primary.clone(),
                None => {
                    let git = cx.new(|cx| ProjectGit::new(repo.path.clone(), cx));
                    if self.polling {
                        git.update(cx, |git, cx| git.set_polling(true, cx));
                    }
                    git
                }
            };
            let changes = [
                cx.observe(&git, |_, _, cx| cx.notify()),
                cx.subscribe(&git, |_, _, _: &GitChanged, cx| {
                    cx.emit(WorkspaceRepoChanged)
                }),
            ];
            self.repos.push(WorkspaceRepo {
                name: repo.name,
                path: repo.path,
                git,
                _changes: changes,
            });
        }
        cx.emit(WorkspaceChanged);
        cx.notify();
    }
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
        for repo in &self.repos {
            if repo.path != self.project {
                repo.git.update(cx, |git, cx| git.resume(cx));
            }
        }
    }
    /// The repos other than the project's own, whose polling the shell runs.
    pub(crate) fn set_polling(&mut self, polling: bool, cx: &mut Context<Self>) {
        if self.polling == polling {
            return;
        }
        self.polling = polling;
        self.worktree_check = polling.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(WORKTREE_CHECK).await;
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        return;
                    }
                }
            })
        });
        for repo in &self.repos {
            if repo.path != self.project {
                repo.git.update(cx, |git, cx| git.set_polling(polling, cx));
            }
        }
    }
    /// Every repo added together, for the title bar's change count.
    pub(crate) fn total_stats(&self, cx: &App) -> Option<DiffStats> {
        self.workspace.as_ref()?;
        let mut total: Option<DiffStats> = None;
        for repo in &self.repos {
            if let Some(stats) = &repo.git.read(cx).stats {
                let sum = total.get_or_insert_with(DiffStats::default);
                sum.files += stats.files;
                sum.additions += stats.additions;
                sum.deletions += stats.deletions;
            }
        }
        total
    }
    /// A file or folder's Git status, from whichever repo holds it.
    pub(crate) fn status(&self, path: &str, is_dir: bool, cx: &App) -> Option<String> {
        self.repos.iter().find_map(|repo| {
            let statuses = &repo.git.read(cx).statuses;
            if is_dir {
                statuses.dirs.get(path).cloned()
            } else {
                statuses.files.get(path).cloned()
            }
        })
    }
    /// Whether the repo at the top of this folder has any change.
    pub(crate) fn repo_changed(&self, path: &str, cx: &App) -> bool {
        self.repos
            .iter()
            .any(|repo| repo.path == path && repo.git.read(cx).has_changes())
    }
}
