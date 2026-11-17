//! One project's pending agent work, across every chat recorded in its
//! checkout: what the agents changed that nobody kept, undid or committed.
//! Each change can be kept or undone on its own. Keeping one moves that file's
//! baseline forward in every chat that recorded it; undoing one writes the
//! file, but only while it still holds what the review showed.
//!
//! A turn's recording only sees its own checkout, and a project's linked
//! worktrees and submodules are other checkouts. Edits an agent reports there
//! go in a journal per project and are compared against that checkout's commit.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// A file with pending agent changes, and the chats that recorded them.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub path: String,
    pub relative: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub sessions: Vec<String>,
}

/// A recorded chat with pending files. `running` chats are mid-turn: their
/// files come from the edits reported so far.
struct ProjectSession {
    id: String,
    status: CheckpointStatus,
    running: bool,
}

/// The project's edits in other checkouts, by review key.
#[derive(Default, Serialize, Deserialize)]
struct SideEdits {
    files: BTreeMap<String, SideFile>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SideFile {
    checkout: String,
    relative: String,
    /// The checkout's commit that `kept` was made against.
    head: Option<String>,
    /// A baseline that includes the changes already kept.
    kept: Option<String>,
    pending: bool,
}

impl SideFile {
    fn checkout(&self) -> &Path {
        Path::new(&self.checkout)
    }
    /// What this file's review compares against.
    fn baseline(&self) -> Option<Blob> {
        if let Some(oid) = &self.kept {
            return Some(Blob {
                mode: workspace_mode(self.checkout(), &self.relative),
                oid: oid.clone(),
            });
        }
        tree_blob(self.checkout(), "HEAD", &self.relative)
            .ok()
            .flatten()
    }
}

fn workspace_mode(root: &Path, relative: &str) -> String {
    if file_mode(&root.join(relative)).is_some_and(|mode| mode & 0o111 != 0) {
        "100755".into()
    } else {
        "100644".into()
    }
}

fn head_of(checkout: &Path) -> Option<String> {
    run_git(checkout, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

/// The blob at `relative` in a commit, or None when the commit lacks the file.
fn tree_blob(root: &Path, commit: &str, relative: &str) -> Result<Option<Blob>, String> {
    let output = run_git(root, &["ls-tree", "-z", commit, "--", relative])?;
    let Some(row) = output.split(|byte| *byte == 0).find(|row| !row.is_empty()) else {
        return Ok(None);
    };
    let row = String::from_utf8_lossy(row);
    let mut fields = row.split_whitespace();
    let (Some(mode), Some(kind), Some(oid)) = (fields.next(), fields.next(), fields.next()) else {
        return Err("Invalid checkpoint tree entry".into());
    };
    Ok((kind == "blob").then(|| Blob {
        mode: mode.into(),
        oid: oid.into(),
    }))
}

fn canonical_project(cwd: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(project_root(cwd)?).map_err(|e| e.to_string())
}

/// The turn still recording, when its process is alive and it has a baseline.
fn running_turn(review: &TurnReview) -> Option<&RecordedTurn> {
    review.active.as_ref().filter(|turn| {
        turn.before.is_some()
            && turn.error.is_none()
            && turn.owner.as_ref().is_some_and(TurnOwner::running)
    })
}

/// A running turn's baseline for one file.
fn running_baseline(root: &Path, turn: &RecordedTurn, relative: &str) -> Option<Blob> {
    if let Some(oid) = turn.kept.get(relative) {
        return Some(Blob {
            mode: workspace_mode(root, relative),
            oid: oid.clone(),
        });
    }
    tree_blob(root, turn.before.as_deref()?, relative)
        .ok()
        .flatten()
}

/// Files a running turn reported editing that differ from its baseline.
fn running_status(root: &Path, turn: &RecordedTurn) -> CheckpointStatus {
    let files = turn
        .edited
        .iter()
        .filter(|relative| !turn.dismissed.contains(*relative))
        .filter_map(|relative| {
            let before = running_baseline(root, turn, relative);
            if workspace_matches(root, relative, before.as_ref()) {
                return None;
            }
            Some(CheckpointFile {
                path: path_to_js(&root.join(relative)),
                relative: relative.clone(),
                status: file_status(before.is_some(), root.join(relative).exists()).into(),
                additions: 0,
                deletions: 0,
                exact: true,
                undoable: false,
            })
        })
        .collect();
    CheckpointStatus { files }
}

fn file_status(had_baseline: bool, on_disk: bool) -> &'static str {
    match (had_baseline, on_disk) {
        (false, _) => "added",
        (true, false) => "deleted",
        (true, true) => "modified",
    }
}

impl Store {
    /// Chats recorded in `cwd`'s checkout that have pending files.
    fn project_sessions(&self, cwd: &str) -> Result<Vec<ProjectSession>, String> {
        let root = canonical_project(cwd)?;
        let root_text = path_to_js(&root);
        let mut sessions = Vec::new();
        if !self.root.exists() {
            return Ok(sessions);
        }
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let Ok(entry) = entry else { continue };
            let id = entry.file_name().to_string_lossy().into_owned();
            if validate_id(&id, "session").is_err() {
                continue;
            }
            // One unreadable recording must not hide every other chat's work.
            let Ok(Some(review)) = read_review(&entry.path()) else {
                continue;
            };
            if review.cwd != root_text {
                continue;
            }
            let (status, running) = if let Some(turn) = running_turn(&review) {
                (running_status(&root, turn), true)
            } else if review.active.is_some() {
                continue;
            } else {
                match self.turn_status_inner(&id, cwd, true) {
                    Ok(Some(status)) => (status, false),
                    _ => continue,
                }
            };
            if !status.files.is_empty() {
                sessions.push(ProjectSession {
                    id,
                    status,
                    running,
                });
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }

    fn side_path(&self, root: &Path) -> PathBuf {
        let digest = Sha256::digest(path_to_js(root).as_bytes());
        let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.root.join("side-edits").join(format!("{name}.json"))
    }

    fn read_side(&self, root: &Path) -> Result<SideEdits, String> {
        match std::fs::read(self.side_path(root)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SideEdits::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The journal after settling it: files back at their baseline stop
    /// being pending, and a new commit drops what was kept against the old one.
    fn settled_side(&self, root: &Path) -> Result<SideEdits, String> {
        let mut side = self.read_side(root)?;
        let mut changed = false;
        side.files.retain(|_, file| {
            let exists = file.checkout().is_dir();
            changed |= !exists;
            exists
        });
        let mut heads: HashMap<String, Option<String>> = HashMap::new();
        for file in side.files.values_mut().filter(|file| file.pending) {
            let head = heads
                .entry(file.checkout.clone())
                .or_insert_with(|| head_of(file.checkout()))
                .clone();
            if file.kept.is_some() && file.head != head {
                file.kept = None;
                file.head = head;
                changed = true;
            }
            if workspace_matches(file.checkout(), &file.relative, file.baseline().as_ref()) {
                file.pending = false;
                changed = true;
            }
        }
        if changed {
            write_json(&self.side_path(root), &side)?;
        }
        Ok(side)
    }

    /// Journal the edits that landed in another checkout of `project`. Returns
    /// the paths left for the turn's own recording. Paths in neither stay out.
    pub fn project_record_edits(
        &self,
        project: &str,
        workdir: &str,
        paths: &[String],
    ) -> Result<Vec<String>, String> {
        let root = canonical_project(project)?;
        let workdir = expand_home(Path::new(workdir));
        let common = git_common_dir(&root);
        let mut side = self.read_side(&root)?;
        let mut own = Vec::new();
        let mut changed = false;
        for path in paths {
            let full = workdir.join(expand_home(Path::new(path)));
            let Some(found) = locate_checkout(&full) else {
                continue;
            };
            let (checkout, absolute) = found;
            if checkout == root {
                own.push(path.clone());
                continue;
            }
            let inside = checkout.starts_with(&root);
            if !inside && (common.is_none() || git_common_dir(&checkout) != common) {
                continue;
            }
            let Ok(relative) = absolute.strip_prefix(&checkout) else {
                continue;
            };
            let key = absolute.strip_prefix(&root).unwrap_or(&absolute);
            let key = path_to_js(key);
            let head = head_of(&checkout);
            let entry = side.files.entry(key).or_insert_with(|| SideFile {
                checkout: path_to_js(&checkout),
                relative: path_to_js(relative),
                head: head.clone(),
                kept: None,
                pending: false,
            });
            if entry.head != head {
                entry.head = head;
                entry.kept = None;
                changed = true;
            }
            if !entry.pending {
                entry.pending = true;
                changed = true;
            }
        }
        if changed {
            write_json(&self.side_path(&root), &side)?;
        }
        Ok(own)
    }

    /// Every file with pending agent changes in `cwd`'s project.
    pub fn project_status(&self, cwd: &str) -> Result<Vec<ProjectFile>, String> {
        let root = canonical_project(cwd)?;
        let mut files: BTreeMap<String, ProjectFile> = BTreeMap::new();
        for session in self.project_sessions(cwd)? {
            for file in session.status.files {
                files
                    .entry(file.relative.clone())
                    .or_insert_with(|| ProjectFile {
                        path: file.path,
                        relative: file.relative,
                        status: file.status,
                        additions: file.additions,
                        deletions: file.deletions,
                        sessions: Vec::new(),
                    })
                    .sessions
                    .push(session.id.clone());
            }
        }
        for (key, file) in self.settled_side(&root)?.files {
            if !file.pending {
                continue;
            }
            let on_disk = file.checkout().join(&file.relative).exists();
            files.insert(
                key.clone(),
                ProjectFile {
                    path: path_to_js(&file.checkout().join(&file.relative)),
                    relative: key,
                    status: file_status(file.baseline().is_some(), on_disk).into(),
                    additions: 0,
                    deletions: 0,
                    sessions: Vec::new(),
                },
            );
        }
        Ok(files.into_values().collect())
    }

    /// The version a file's review compares against. When chats disagree,
    /// the committed version wins, as it is where uncommitted work started.
    fn project_baseline(
        &self,
        root: &Path,
        cwd: &str,
        relative: &str,
    ) -> Result<Option<Option<Blob>>, String> {
        let mut candidates = Vec::new();
        for session in self.project_sessions(cwd)? {
            if !session
                .status
                .files
                .iter()
                .any(|file| file.relative == relative)
            {
                continue;
            }
            let Some(review) = self.matching_turn_review(&session.id, cwd)? else {
                continue;
            };
            if session.running {
                if let Some(turn) = running_turn(&review) {
                    candidates.push(running_baseline(root, turn, relative));
                }
                continue;
            }
            let turn = completed_turn(&review)?;
            if let Some(change) = changes(root, turn)?
                .into_iter()
                .find(|change| change.relative == relative)
            {
                candidates.push(change.before);
            }
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        let head = run_git(
            root,
            &["rev-parse", "--verify", &format!("HEAD:{relative}")],
        )
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string());
        let committed = candidates
            .iter()
            .position(|blob| blob.as_ref().map(|b| &b.oid) == head.as_ref());
        Ok(Some(candidates.swap_remove(committed.unwrap_or(0))))
    }

    fn side_file(&self, root: &Path, key: &str) -> Result<Option<SideFile>, String> {
        Ok(self
            .read_side(root)?
            .files
            .get(key)
            .filter(|file| file.pending)
            .cloned())
    }

    /// The file's baseline against what is on disk now.
    pub fn project_file_diff(
        &self,
        cwd: &str,
        relative: &str,
    ) -> Result<crate::scm::FileDiff, String> {
        let root = canonical_project(cwd)?;
        if let Some(file) = self.side_file(&root, relative)? {
            let baseline = file.baseline();
            return file_diff(file.checkout(), &file.relative, relative, baseline.as_ref());
        }
        let relative = resolve_turn_path(relative)?;
        let baseline = self
            .project_baseline(&root, cwd, &relative)?
            .ok_or("This file has no pending agent changes")?;
        file_diff(&root, &relative, &relative, baseline.as_ref())
    }

    /// Mark what is on disk as accepted. In a running turn the turn's own
    /// baseline moves, as its result is not recorded yet.
    fn keep_running(
        &self,
        session: &str,
        cwd: &str,
        relative: Option<&str>,
        status: &CheckpointStatus,
    ) -> Result<(), String> {
        let root = canonical_project(cwd)?;
        let Some(mut review) = self.matching_turn_review(session, cwd)? else {
            return Ok(());
        };
        let Some(turn) = review.active.as_mut() else {
            return Ok(());
        };
        for file in &status.files {
            if relative.is_some_and(|relative| relative != file.relative) {
                continue;
            }
            if root.join(&file.relative).exists() {
                let oid = run_git(
                    &root,
                    &[
                        "hash-object",
                        "-w",
                        &format!("--path={}", file.relative),
                        "--",
                        &file.relative,
                    ],
                )?;
                turn.kept.insert(
                    file.relative.clone(),
                    String::from_utf8_lossy(&oid).trim().to_string(),
                );
            } else {
                turn.dismissed.insert(file.relative.clone());
            }
        }
        write_review(&self.session_dir(session), &review)
    }

    fn keep_side(&self, root: &Path, key: &str) -> Result<(), String> {
        let mut side = self.read_side(root)?;
        let Some(file) = side.files.get_mut(key).filter(|file| file.pending) else {
            return Ok(());
        };
        if file.checkout().join(&file.relative).exists() {
            let oid = run_git(
                file.checkout(),
                &[
                    "hash-object",
                    "-w",
                    &format!("--path={}", file.relative),
                    "--",
                    &file.relative,
                ],
            )?;
            file.kept = Some(String::from_utf8_lossy(&oid).trim().to_string());
            file.head = head_of(file.checkout());
        }
        file.pending = false;
        write_json(&self.side_path(root), &side)
    }

    /// Keep one file's changes, or every file's when `relative` is None.
    pub fn project_keep(&self, cwd: &str, relative: Option<&str>) -> Result<(), String> {
        let root = canonical_project(cwd)?;
        match relative {
            Some(key) if self.side_file(&root, key)?.is_some() => {
                return self.keep_side(&root, key);
            }
            None => {
                let keys: Vec<_> = self.settled_side(&root)?.files.into_keys().collect();
                for key in keys {
                    self.keep_side(&root, &key)?;
                }
            }
            Some(_) => {}
        }
        let relative = relative.map(resolve_turn_path).transpose()?;
        for session in self.project_sessions(cwd)? {
            let has_file = relative.as_ref().is_none_or(|relative| {
                session.status.files.iter().any(|f| &f.relative == relative)
            });
            if !has_file {
                continue;
            }
            if session.running {
                self.keep_running(&session.id, cwd, relative.as_deref(), &session.status)?;
            } else {
                self.keep_turn(&session.id, cwd, relative.as_deref())?;
            }
        }
        Ok(())
    }

    /// Keep some of a file's changes: `baseline` is the reviewed baseline with
    /// those changes applied. The rest of the file stays pending.
    pub fn project_keep_part(
        &self,
        cwd: &str,
        relative: &str,
        baseline: &str,
    ) -> Result<(), String> {
        let root = canonical_project(cwd)?;
        if let Some(file) = self.side_file(&root, relative)? {
            let oid = hash_text(file.checkout(), &file.relative, baseline)?;
            let head = head_of(file.checkout());
            let mut side = self.read_side(&root)?;
            if let Some(entry) = side.files.get_mut(relative) {
                entry.kept = Some(oid);
                entry.head = head;
            }
            write_json(&self.side_path(&root), &side)?;
            self.settled_side(&root)?;
            return Ok(());
        }
        let relative = resolve_turn_path(relative)?;
        let oid = hash_text(&root, &relative, baseline)?;
        for session in self.project_sessions(cwd)? {
            if !session
                .status
                .files
                .iter()
                .any(|file| file.relative == relative)
            {
                continue;
            }
            let Some(mut review) = self.matching_turn_review(&session.id, cwd)? else {
                continue;
            };
            let turn = if session.running {
                review.active.as_mut()
            } else {
                review.latest.as_mut()
            };
            let Some(turn) = turn else {
                continue;
            };
            turn.kept.insert(relative.clone(), oid.clone());
            write_review(&self.session_dir(&session.id), &review)?;
            // A file whose changes are now all kept leaves the review.
            if !session.running {
                self.turn_status_inner(&session.id, cwd, true)?;
            }
        }
        Ok(())
    }

    /// Write `next` (or remove the file for None) when the file still holds
    /// `expected`, the text the review showed. Undo builds on this.
    pub fn project_write(
        &self,
        cwd: &str,
        relative: &str,
        expected: &str,
        next: Option<&str>,
    ) -> Result<(), String> {
        let root = canonical_project(cwd)?;
        if let Some(file) = self.side_file(&root, relative)? {
            write_expected(file.checkout(), &file.relative, expected, next)?;
            self.settled_side(&root)?;
            return Ok(());
        }
        let relative = resolve_turn_path(relative)?;
        write_expected(&root, &relative, expected, next)?;
        // A file back at its baseline leaves the review.
        for session in self.project_sessions(cwd)? {
            if !session.running {
                self.turn_status_inner(&session.id, cwd, true)?;
            }
        }
        Ok(())
    }

    /// Put the file back at its reviewed baseline, when it still holds `expected`.
    pub fn project_undo(&self, cwd: &str, relative: &str, expected: &str) -> Result<(), String> {
        let root = canonical_project(cwd)?;
        if let Some(file) = self.side_file(&root, relative)? {
            let baseline = file.baseline();
            let next = undo_text(file.checkout(), baseline.as_ref())?;
            write_expected(file.checkout(), &file.relative, expected, next.as_deref())?;
            self.settled_side(&root)?;
            return Ok(());
        }
        let relative = resolve_turn_path(relative)?;
        let baseline = self
            .project_baseline(&root, cwd, &relative)?
            .ok_or("This file has no pending agent changes")?;
        let next = undo_text(&root, baseline.as_ref())?;
        self.project_write(cwd, &relative, expected, next.as_deref())?;
        self.project_keep(cwd, Some(&relative))
    }
}

/// The text Undo restores, or None when the file did not exist.
fn undo_text(root: &Path, baseline: Option<&Blob>) -> Result<Option<String>, String> {
    Ok(match read_blob(root, baseline)? {
        FileState::Missing => None,
        FileState::Skipped => return Err("This file is too large to undo here".into()),
        state if state_is_binary(&state) => {
            return Err("Binary files cannot be undone here".into());
        }
        state => Some(state_text(state)),
    })
}

fn file_diff(
    checkout: &Path,
    relative: &str,
    key: &str,
    baseline: Option<&Blob>,
) -> Result<crate::scm::FileDiff, String> {
    let original = read_blob(checkout, baseline)?;
    let current = read_workspace(checkout, relative)?;
    let status = match (&original, &current) {
        (FileState::Missing, _) => "added",
        (_, FileState::Missing) => "deleted",
        _ => "modified",
    };
    let too_large = matches!(original, FileState::Skipped) || matches!(current, FileState::Skipped);
    let binary = state_is_binary(&original) || state_is_binary(&current);
    let (original, current) = if too_large || binary {
        (String::new(), String::new())
    } else {
        (state_text(original), state_text(current))
    };
    Ok(crate::scm::FileDiff {
        path: path_to_js(&checkout.join(relative)),
        relative: key.into(),
        status: status.into(),
        original,
        current,
        binary,
        too_large,
    })
}

fn write_expected(
    checkout: &Path,
    relative: &str,
    expected: &str,
    next: Option<&str>,
) -> Result<(), String> {
    if path_contains_symlink(checkout, relative) {
        return Err("Cannot undo a change behind a symbolic link".into());
    }
    let current = state_text(read_workspace(checkout, relative)?);
    if current != expected {
        return Err("This file changed after the review loaded, so it was left as it is".into());
    }
    let path = checkout.join(relative);
    match next {
        Some(text) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
        }
        None => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.to_string());
            }
            _ => {}
        },
    }
    Ok(())
}

/// The checkout holding `path`, and the path with symlinks resolved.
fn locate_checkout(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let parent = std::fs::canonicalize(path.parent()?).ok()?;
    let top = run_git(&parent, &["rev-parse", "--show-toplevel"]).ok()?;
    let top = String::from_utf8(top).ok()?;
    let checkout = std::fs::canonicalize(top.trim_end_matches(['\r', '\n'])).ok()?;
    Some((checkout, parent.join(path.file_name()?)))
}

fn git_common_dir(checkout: &Path) -> Option<PathBuf> {
    let dir = run_git(checkout, &["rev-parse", "--git-common-dir"]).ok()?;
    let dir = String::from_utf8(dir).ok()?;
    std::fs::canonicalize(checkout.join(dir.trim_end_matches(['\r', '\n']))).ok()
}

fn read_workspace(root: &Path, relative: &str) -> Result<FileState, String> {
    let path = root.join(relative);
    match std::fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileState::Missing),
        Err(error) => Err(error.to_string()),
        Ok(meta) if meta.len() > MAX_TEXT_FILE_BYTES => Ok(FileState::Skipped),
        Ok(_) => std::fs::read(&path)
            .map(FileState::Contents)
            .map_err(|e| e.to_string()),
    }
}

/// Store `text` as a blob, filtered as git would store the file at `relative`.
fn hash_text(root: &Path, relative: &str, text: &str) -> Result<String, String> {
    use std::process::Stdio;
    let mut child = git_command(
        root,
        &[
            "hash-object",
            "-w",
            &format!("--path={relative}"),
            "--stdin",
        ],
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("git hash-object has no input")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
