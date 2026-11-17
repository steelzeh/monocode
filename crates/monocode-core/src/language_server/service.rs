//! Owns the language servers and the files that are open in editors.
//!
//! One thread holds all the state and handles one command at a time, so no
//! state is shared between threads. Each server also has a thread that reads
//! its answers and a thread that writes to it, so a busy server can never
//! block this one.
use super::{file_uri, message};
use crate::language_server::position::Lines;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// When servers stop, and how long the app waits for them.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub idle: Duration,
    pub max_servers: usize,
    pub first_answer: Duration,
    pub answer: Duration,
    pub crash_window: Duration,
    pub file_change_pause: Duration,
    pub refresh_pause: Duration,
    pub retry_setup: Duration,
    pub retry_busy: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(10 * 60),
            max_servers: 3,
            first_answer: Duration::from_secs(30),
            answer: Duration::from_secs(10),
            crash_window: Duration::from_secs(60),
            file_change_pause: Duration::from_millis(200),
            refresh_pause: Duration::from_millis(300),
            retry_setup: Duration::from_secs(60),
            retry_busy: Duration::from_millis(200),
        }
    }
}

/// A running server. `stop` ends it, and it is called at most once.
pub struct Connection {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub stop: Box<dyn FnOnce() + Send>,
}

/// One kind of language server.
pub trait Tool: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn handles(&self, file: &Path) -> bool;
    fn language_id(&self, file: &Path) -> &'static str;
    /// The folder that one server covers, or `None` if `file` has no server.
    fn root(&self, file: &Path, project: &Path) -> Option<PathBuf>;
    /// A line for the user when `prepare` still has work to do.
    fn setup_note(&self) -> Option<String>;
    /// Gets the program ready. This runs on its own thread and can take long.
    fn prepare(&self) -> Result<(), String>;
    fn launch(&self, root: &Path) -> io::Result<Connection>;
    /// Whether the server needs to hear about a change to this file.
    fn watches(&self, path: &Path) -> bool;
    /// Whether a change to this file needs a new server.
    fn restarts_server(&self, path: &Path) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One problem in a file, with byte offsets into the text that was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerError {
    pub from: usize,
    pub to: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum Update {
    /// The problems in `text`. Drop them if the editor no longer holds `text`.
    Errors {
        text: Arc<str>,
        errors: Vec<ServerError>,
    },
    /// One line for the user, or `None` when the line should go away.
    Note(Option<String>),
}

/// A file on disk that changed. `exists` is false when it was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub exists: bool,
}

/// Where a file watcher reports the changes in one server's folder.
#[derive(Clone)]
pub struct FileSink {
    commands: mpsc::Sender<Command>,
    root: PathBuf,
}

impl FileSink {
    pub fn send(&self, changes: Vec<FileChange>) {
        let _ = self.commands.send(Command::FilesChanged {
            root: self.root.clone(),
            changes,
        });
    }
}

/// Keeps a file watcher alive for as long as its server runs.
pub type WatchGuard = Box<dyn Send>;
pub type Watcher = Box<dyn Fn(&Path, FileSink) -> Option<WatchGuard> + Send>;

enum Command {
    Open {
        owner: u64,
        file: PathBuf,
        project: PathBuf,
        updates: async_channel::Sender<Update>,
    },
    SetText {
        file: PathBuf,
        text: Arc<str>,
    },
    Close {
        owner: u64,
        file: PathBuf,
    },
    FilesChanged {
        root: PathBuf,
        changes: Vec<FileChange>,
    },
    FromServer {
        server: u64,
        root: PathBuf,
        message: Value,
    },
    ServerGone {
        server: u64,
        root: PathBuf,
    },
    Prepared(Result<(), String>),
    Stop,
}

struct Shared {
    commands: mpsc::Sender<Command>,
    tool: Arc<dyn Tool>,
    next_owner: AtomicU64,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

/// The handle that editors use. Clones share the same servers.
#[derive(Clone)]
pub struct LanguageServers {
    shared: Arc<Shared>,
}

impl LanguageServers {
    pub fn start(tool: Arc<dyn Tool>, watcher: Option<Watcher>, limits: Limits) -> Self {
        let (commands, receiver) = mpsc::channel();
        let service = Service {
            tool: tool.clone(),
            watcher,
            limits,
            commands: commands.clone(),
            docs: HashMap::new(),
            servers: HashMap::new(),
            crashes: HashMap::new(),
            notes: HashMap::new(),
            program: Program::Unknown,
            next_server: 0,
        };
        thread::Builder::new()
            .name("language-servers".to_string())
            .spawn(move || service.run(receiver))
            .expect("the language server thread can start");
        Self {
            shared: Arc::new(Shared {
                commands,
                tool,
                next_owner: AtomicU64::new(1),
            }),
        }
    }

    /// Opens `file` for one editor, or returns `None` if no server handles it.
    pub fn open(
        &self,
        file: &Path,
        project: &Path,
    ) -> Option<(Document, async_channel::Receiver<Update>)> {
        if !self.shared.tool.handles(file) {
            return None;
        }
        let owner = self.shared.next_owner.fetch_add(1, Ordering::Relaxed);
        let (updates, receiver) = async_channel::unbounded();
        let _ = self.shared.commands.send(Command::Open {
            owner,
            file: file.to_path_buf(),
            project: project.to_path_buf(),
            updates,
        });
        let document = Document {
            shared: self.shared.clone(),
            owner,
            file: file.to_path_buf(),
        };
        Some((document, receiver))
    }
}

/// One editor's hold on an open file. Dropping it closes the file.
pub struct Document {
    shared: Arc<Shared>,
    owner: u64,
    file: PathBuf,
}

impl Document {
    /// Sends the editor's current text. It may hold unsaved changes.
    pub fn set_text(&self, text: Arc<str>) {
        let _ = self.shared.commands.send(Command::SetText {
            file: self.file.clone(),
            text,
        });
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        let _ = self.shared.commands.send(Command::Close {
            owner: self.owner,
            file: self.file.clone(),
        });
    }
}

struct Owner {
    id: u64,
    updates: async_channel::Sender<Update>,
}

struct Doc {
    project: PathBuf,
    root: Option<PathBuf>,
    owners: Vec<Owner>,
    text: Option<Arc<str>>,
    version: i32,
    /// The server and the text version that it holds.
    sent: Option<(u64, i32)>,
    in_flight: bool,
    check_again: bool,
    retry_at: Option<Instant>,
    delivered: Option<i32>,
    errors: Vec<ServerError>,
    note: Option<String>,
}

enum Pending {
    Initialize,
    Errors { file: PathBuf, version: i32 },
}

struct Server {
    id: u64,
    out: mpsc::Sender<Value>,
    stop: Option<Box<dyn FnOnce() + Send>>,
    _watch: Option<WatchGuard>,
    ready: bool,
    answered: bool,
    last_work: Instant,
    next_request: i64,
    pending: HashMap<i64, (Instant, Pending)>,
    file_changes: Vec<FileChange>,
    flush_at: Option<Instant>,
    refresh_at: Option<Instant>,
}

impl Server {
    fn notify(&self, method: &str, params: Value) {
        let _ = self
            .out
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value, pending: Pending) {
        self.next_request += 1;
        let id = self.next_request;
        self.pending.insert(id, (Instant::now(), pending));
        let _ = self
            .out
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn answer(&self, id: &Value, result: Value) {
        let _ = self
            .out
            .send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
}

enum Program {
    Unknown,
    Preparing(String),
    Ready,
    Failed { at: Instant, message: String },
}

struct Service {
    tool: Arc<dyn Tool>,
    watcher: Option<Watcher>,
    limits: Limits,
    commands: mpsc::Sender<Command>,
    docs: HashMap<PathBuf, Doc>,
    servers: HashMap<PathBuf, Server>,
    crashes: HashMap<PathBuf, Vec<Instant>>,
    notes: HashMap<PathBuf, String>,
    program: Program,
    next_server: u64,
}

impl Service {
    fn run(mut self, commands: mpsc::Receiver<Command>) {
        loop {
            let wait = self
                .next_deadline()
                .map_or(Duration::from_secs(60), |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
            match commands.recv_timeout(wait) {
                Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.run_due_work();
        }
        let roots: Vec<PathBuf> = self.servers.keys().cloned().collect();
        for root in roots {
            self.stop_server(&root);
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Open {
                owner,
                file,
                project,
                updates,
            } => self.open(owner, file, project, updates),
            Command::SetText { file, text } => self.set_text(&file, text),
            Command::Close { owner, file } => self.close(owner, &file),
            Command::FilesChanged { root, changes } => self.files_changed(&root, changes),
            Command::FromServer {
                server,
                root,
                message,
            } => self.server_message(server, &root, message),
            Command::ServerGone { server, root } => {
                if self.servers.get(&root).map(|s| s.id) == Some(server) {
                    self.crashed(&root);
                }
            }
            Command::Prepared(result) => self.prepared(result),
            Command::Stop => {}
        }
    }

    fn open(
        &mut self,
        owner: u64,
        file: PathBuf,
        project: PathBuf,
        updates: async_channel::Sender<Update>,
    ) {
        let root = self.tool.root(&file, &project);
        let doc = self.docs.entry(file).or_insert_with(|| Doc {
            project,
            root,
            owners: Vec::new(),
            text: None,
            version: 0,
            sent: None,
            in_flight: false,
            check_again: false,
            retry_at: None,
            delivered: None,
            errors: Vec::new(),
            note: None,
        });
        if let (Some(text), true) = (&doc.text, doc.delivered.is_some()) {
            let _ = updates.try_send(Update::Errors {
                text: text.clone(),
                errors: doc.errors.clone(),
            });
        }
        if doc.note.is_some() {
            let _ = updates.try_send(Update::Note(doc.note.clone()));
        }
        doc.owners.push(Owner { id: owner, updates });
    }

    fn set_text(&mut self, file: &Path, text: Arc<str>) {
        let Some(doc) = self.docs.get_mut(file) else {
            return;
        };
        if doc.root.is_none() {
            doc.root = self.tool.root(file, &doc.project);
        }
        let Some(root) = doc.root.clone() else {
            return;
        };
        let changed = doc.text.as_deref() != Some(&*text);
        if !changed && doc.delivered == Some(doc.version) {
            return;
        }
        if changed {
            doc.text = Some(text);
            doc.version += 1;
        }
        self.ensure_server(&root);
        let has_other_files = self.docs_of(&root).len() > 1;
        if let Some(server) = self.servers.get_mut(&root) {
            server.last_work = Instant::now();
            // A change in this file can create or remove errors in the others.
            if changed && has_other_files {
                server.refresh_at = Some(Instant::now() + self.limits.refresh_pause);
            }
        }
        self.sync(file);
        self.check(file);
        self.refresh_notes();
    }

    fn close(&mut self, owner: u64, file: &Path) {
        let Some(doc) = self.docs.get_mut(file) else {
            return;
        };
        doc.owners.retain(|o| o.id != owner);
        if !doc.owners.is_empty() {
            return;
        }
        let doc = self.docs.remove(file).expect("the file is open");
        let server = doc.root.as_ref().and_then(|root| self.servers.get(root));
        if let (Some(server), Some((id, _)), Some(uri)) = (server, doc.sent, file_uri(file)) {
            if id == server.id {
                server.notify(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": uri}}),
                );
            }
        }
    }

    fn files_changed(&mut self, root: &Path, changes: Vec<FileChange>) {
        let Some(server) = self.servers.get_mut(root) else {
            return;
        };
        let before = server.file_changes.len();
        let tool = &self.tool;
        server.file_changes.extend(
            changes
                .into_iter()
                .filter(|change| tool.watches(&change.path) || tool.restarts_server(&change.path)),
        );
        if server.file_changes.len() != before {
            server.flush_at = Some(Instant::now() + self.limits.file_change_pause);
        }
    }

    fn flush_file_changes(&mut self, root: &Path) {
        let Some(server) = self.servers.get_mut(root) else {
            return;
        };
        server.flush_at = None;
        let changes = std::mem::take(&mut server.file_changes);
        if changes
            .iter()
            .any(|change| self.tool.restarts_server(&change.path))
        {
            self.stop_server(root);
            if self.has_docs(root) {
                self.ensure_server(root);
            }
            return;
        }
        // The server ignores "created" for a file it already knows, and it
        // handles "changed" for a new file. So a file that exists is "changed".
        let changes: Vec<Value> = changes
            .iter()
            .filter_map(|change| {
                let kind = if change.exists { 2 } else { 3 };
                Some(json!({"uri": file_uri(&change.path)?, "type": kind}))
            })
            .collect();
        server.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": changes }),
        );
        server.refresh_at = Some(Instant::now() + self.limits.refresh_pause);
    }

    fn server_message(&mut self, id: u64, root: &Path, message: Value) {
        // A message from a process that was already replaced means nothing.
        if self.servers.get(root).map(|s| s.id) != Some(id) {
            return;
        }
        match (message.get("id"), message.get("method")) {
            (Some(request), Some(method)) => {
                let method = method.as_str().unwrap_or_default().to_string();
                self.server_request(root, request.clone(), &method, &message);
            }
            (Some(response), None) => {
                if let Some(response) = response.as_i64() {
                    self.server_response(root, response, &message);
                }
            }
            _ => {}
        }
    }

    fn server_request(&mut self, root: &Path, id: Value, method: &str, message: &Value) {
        let Some(server) = self.servers.get_mut(root) else {
            return;
        };
        // An empty answer to everything else means that no server waits forever.
        let result = match method {
            "workspace/configuration" => {
                let count = message["params"]["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![Value::Null; count])
            }
            "workspace/workspaceFolders" => workspace_folders(root),
            "workspace/diagnostic/refresh" => {
                server.refresh_at = Some(Instant::now() + self.limits.refresh_pause);
                Value::Null
            }
            _ => Value::Null,
        };
        server.answer(&id, result);
    }

    fn server_response(&mut self, root: &Path, id: i64, message: &Value) {
        let Some(server) = self.servers.get_mut(root) else {
            return;
        };
        let Some((_, pending)) = server.pending.remove(&id) else {
            return;
        };
        match pending {
            Pending::Initialize => {
                if message.get("error").is_some() {
                    self.crashed(root);
                    return;
                }
                server.ready = true;
                server.notify("initialized", json!({}));
                self.notes.remove(root);
                self.check_all(root);
                self.refresh_notes();
            }
            Pending::Errors { file, version } => {
                server.answered = true;
                self.errors_answer(&file, version, message);
            }
        }
    }

    fn errors_answer(&mut self, file: &Path, version: i32, message: &Value) {
        let retry = Instant::now() + self.limits.retry_busy;
        let Some(doc) = self.docs.get_mut(file) else {
            return;
        };
        doc.in_flight = false;
        let current = version == doc.version;
        match message.get("error") {
            // The server was busy or the text moved on. Neither is a failure.
            Some(error) if matches!(error["code"].as_i64(), Some(-32801 | -32802)) => {
                doc.retry_at = Some(retry);
            }
            Some(_) => {}
            None if current => deliver(doc, &message["result"]),
            None => {}
        }
        if doc.check_again || !current {
            self.check(file);
        }
    }

    fn ensure_server(&mut self, root: &Path) {
        if self.servers.contains_key(root) || !self.ensure_program() || self.crash_blocked(root) {
            return;
        }
        while self.servers.len() >= self.limits.max_servers {
            let oldest = self
                .servers
                .iter()
                .min_by_key(|(_, server)| server.last_work)
                .map(|(root, _)| root.clone());
            match oldest {
                Some(oldest) => self.stop_server(&oldest),
                None => break,
            }
        }
        let connection = match self.tool.launch(root) {
            Ok(connection) => connection,
            Err(_) => {
                self.notes.insert(
                    root.to_path_buf(),
                    format!("{} could not start in this repo.", self.tool.name()),
                );
                self.refresh_notes();
                return;
            }
        };
        self.next_server += 1;
        let id = self.next_server;
        let out = self.spawn_io(id, root, connection.reader, connection.writer);
        let sink = FileSink {
            commands: self.commands.clone(),
            root: root.to_path_buf(),
        };
        let mut server = Server {
            id,
            out,
            stop: Some(connection.stop),
            _watch: self.watcher.as_ref().and_then(|watch| watch(root, sink)),
            ready: false,
            answered: false,
            last_work: Instant::now(),
            next_request: 0,
            pending: HashMap::new(),
            file_changes: Vec::new(),
            flush_at: None,
            refresh_at: None,
        };
        server.request("initialize", initialize_params(root), Pending::Initialize);
        self.servers.insert(root.to_path_buf(), server);
    }

    fn spawn_io(
        &self,
        id: u64,
        root: &Path,
        reader: Box<dyn Read + Send>,
        mut writer: Box<dyn Write + Send>,
    ) -> mpsc::Sender<Value> {
        let (out, outgoing) = mpsc::channel::<Value>();
        thread::spawn(move || {
            while let Ok(message) = outgoing.recv() {
                if message::write_message(&mut writer, &message).is_err() {
                    break;
                }
            }
        });
        let commands = self.commands.clone();
        let root = root.to_path_buf();
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(message)) = message::read_message(&mut reader) {
                let sent = commands.send(Command::FromServer {
                    server: id,
                    root: root.clone(),
                    message,
                });
                if sent.is_err() {
                    return;
                }
            }
            let _ = commands.send(Command::ServerGone { server: id, root });
        });
        out
    }

    fn ensure_program(&mut self) -> bool {
        match &self.program {
            Program::Ready => return true,
            Program::Preparing(_) => return false,
            Program::Failed { at, .. } if at.elapsed() < self.limits.retry_setup => return false,
            Program::Unknown | Program::Failed { .. } => {}
        }
        let Some(note) = self.tool.setup_note() else {
            self.program = Program::Ready;
            return true;
        };
        self.program = Program::Preparing(note);
        let tool = self.tool.clone();
        let commands = self.commands.clone();
        thread::spawn(move || {
            let _ = commands.send(Command::Prepared(tool.prepare()));
        });
        self.refresh_notes();
        false
    }

    fn prepared(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.program = Program::Ready;
                let roots: Vec<PathBuf> =
                    self.docs.values().filter_map(|d| d.root.clone()).collect();
                for root in roots {
                    self.ensure_server(&root);
                }
            }
            Err(message) => {
                self.program = Program::Failed {
                    at: Instant::now(),
                    message: format!("{} errors are not available: {message}.", self.tool.name()),
                };
            }
        }
        self.refresh_notes();
    }

    fn crash_blocked(&self, root: &Path) -> bool {
        self.crashes.get(root).is_some_and(|crashes| {
            crashes
                .iter()
                .filter(|at| at.elapsed() < self.limits.crash_window)
                .count()
                >= 2
        })
    }

    fn crashed(&mut self, root: &Path) {
        self.stop_server(root);
        let window = self.limits.crash_window;
        let crashes = self.crashes.entry(root.to_path_buf()).or_default();
        crashes.retain(|at| at.elapsed() < window);
        crashes.push(Instant::now());
        if self.crash_blocked(root) {
            self.notes.insert(
                root.to_path_buf(),
                format!("{} stopped working in this repo.", self.tool.name()),
            );
            self.refresh_notes();
        } else if self.has_docs(root) {
            self.ensure_server(root);
        }
    }

    fn stop_server(&mut self, root: &Path) {
        let Some(mut server) = self.servers.remove(root) else {
            return;
        };
        if let Some(stop) = server.stop.take() {
            stop();
        }
        for doc in self.docs.values_mut() {
            if doc.root.as_deref() == Some(root) {
                doc.sent = None;
                doc.in_flight = false;
                doc.check_again = false;
                doc.retry_at = None;
            }
        }
    }

    fn has_docs(&self, root: &Path) -> bool {
        self.docs
            .values()
            .any(|doc| doc.root.as_deref() == Some(root))
    }

    fn docs_of(&self, root: &Path) -> Vec<PathBuf> {
        self.docs
            .iter()
            .filter(|(_, doc)| doc.root.as_deref() == Some(root))
            .map(|(file, _)| file.clone())
            .collect()
    }

    fn check_all(&mut self, root: &Path) {
        for file in self.docs_of(root) {
            self.sync(&file);
            self.check(&file);
        }
    }

    /// Gives the server the current text of `file`.
    fn sync(&mut self, file: &Path) {
        let Some(doc) = self.docs.get_mut(file) else {
            return;
        };
        let (Some(root), Some(text)) = (doc.root.as_ref(), doc.text.as_ref()) else {
            return;
        };
        let Some(server) = self.servers.get(root).filter(|server| server.ready) else {
            return;
        };
        let Some(uri) = file_uri(file) else {
            return;
        };
        match doc.sent {
            Some((id, version)) if id == server.id => {
                if version == doc.version {
                    return;
                }
                server.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": uri, "version": doc.version},
                        "contentChanges": [{"text": &**text}],
                    }),
                );
            }
            _ => server.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": uri,
                    "languageId": self.tool.language_id(file),
                    "version": doc.version,
                    "text": &**text,
                }}),
            ),
        }
        doc.sent = Some((server.id, doc.version));
    }

    /// Asks for the errors in `file`. At most one question per file is open.
    fn check(&mut self, file: &Path) {
        let Some(doc) = self.docs.get_mut(file) else {
            return;
        };
        if doc.text.is_none() {
            return;
        }
        if doc.in_flight {
            doc.check_again = true;
            return;
        }
        let Some(root) = doc.root.as_ref() else {
            return;
        };
        let Some(server) = self.servers.get_mut(root).filter(|server| server.ready) else {
            return;
        };
        let Some(uri) = file_uri(file) else {
            return;
        };
        doc.in_flight = true;
        doc.check_again = false;
        doc.retry_at = None;
        server.request(
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": uri}}),
            Pending::Errors {
                file: file.to_path_buf(),
                version: doc.version,
            },
        );
    }

    fn refresh_notes(&mut self) {
        let program_note = match &self.program {
            Program::Preparing(note) => Some(note.clone()),
            Program::Failed { message, .. } => Some(message.clone()),
            Program::Unknown | Program::Ready => None,
        };
        for doc in self.docs.values_mut() {
            let note = doc.root.as_ref().and_then(|root| {
                self.notes
                    .get(root)
                    .cloned()
                    .or_else(|| program_note.clone())
            });
            if doc.note != note {
                doc.note = note;
                for owner in &doc.owners {
                    let _ = owner.updates.try_send(Update::Note(doc.note.clone()));
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let limits = &self.limits;
        let servers = self.servers.values().flat_map(|server| {
            let limit = if server.answered {
                limits.answer
            } else {
                limits.first_answer
            };
            [
                server.flush_at,
                server.refresh_at,
                Some(server.last_work + limits.idle),
                server
                    .pending
                    .values()
                    .map(|(sent, _)| *sent)
                    .min()
                    .map(|sent| sent + limit),
            ]
        });
        servers
            .chain(self.docs.values().map(|doc| doc.retry_at))
            .flatten()
            .min()
    }

    fn run_due_work(&mut self) {
        let now = Instant::now();
        let roots: Vec<PathBuf> = self.servers.keys().cloned().collect();
        for root in roots {
            let Some(server) = self.servers.get_mut(&root) else {
                continue;
            };
            let limit = if server.answered {
                self.limits.answer
            } else {
                self.limits.first_answer
            };
            if server
                .pending
                .values()
                .any(|(sent, _)| *sent + limit <= now)
            {
                self.crashed(&root);
                continue;
            }
            if server.last_work + self.limits.idle <= now {
                self.stop_server(&root);
                continue;
            }
            if server.flush_at.is_some_and(|at| at <= now) {
                self.flush_file_changes(&root);
            }
            let Some(server) = self.servers.get_mut(&root) else {
                continue;
            };
            if server.refresh_at.is_some_and(|at| at <= now) {
                server.refresh_at = None;
                self.check_all(&root);
            }
        }
        let due: Vec<PathBuf> = self
            .docs
            .iter()
            .filter(|(_, doc)| doc.retry_at.is_some_and(|at| at <= now))
            .map(|(file, _)| file.clone())
            .collect();
        for file in due {
            self.check(&file);
        }
    }
}

fn workspace_folders(root: &Path) -> Value {
    let name = root.file_name().map(|name| name.to_string_lossy());
    json!([{"uri": file_uri(root), "name": name}])
}

fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "rootUri": file_uri(root),
        "workspaceFolders": workspace_folders(root),
        "initializationOptions": {},
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "textDocument": {
                "synchronization": {"didSave": true},
                "diagnostic": {"dynamicRegistration": false},
            },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeWatchedFiles": {"dynamicRegistration": true},
                "diagnostics": {"refreshSupport": true},
            },
        },
    })
}

fn deliver(doc: &mut Doc, result: &Value) {
    let Some(text) = doc.text.clone() else {
        return;
    };
    let lines = Lines::new(&text);
    let items = result["items"].as_array().map_or(&[][..], Vec::as_slice);
    let errors: Vec<ServerError> = items
        .iter()
        .filter_map(|item| server_error(&text, &lines, item))
        .collect();
    if doc.delivered == Some(doc.version) && doc.errors == errors {
        return;
    }
    doc.delivered = Some(doc.version);
    doc.errors = errors.clone();
    for owner in &doc.owners {
        let _ = owner.updates.try_send(Update::Errors {
            text: text.clone(),
            errors: errors.clone(),
        });
    }
}

fn server_error(text: &str, lines: &Lines, item: &Value) -> Option<ServerError> {
    // Severity 4 marks hints, such as an unused name, which are not errors.
    let severity = match item["severity"].as_i64() {
        Some(2) => Severity::Warning,
        Some(3) => Severity::Info,
        Some(4) => return None,
        _ => Severity::Error,
    };
    let at = |edge: &str| {
        let position = &item["range"][edge];
        Some(lines.byte(
            usize::try_from(position["line"].as_u64()?).ok()?,
            usize::try_from(position["character"].as_u64()?).ok()?,
        ))
    };
    let from = at("start")?;
    let mut to = at("end")?.max(from);
    if from == to {
        // An empty range would draw nothing, so cover the next character.
        to = text[from..]
            .chars()
            .next()
            .map_or(from, |ch| from + ch.len_utf8());
    }
    let mut message = item["message"].as_str()?.to_string();
    if let Some(source) = item["source"].as_str() {
        match &item["code"] {
            Value::Number(code) => message.push_str(&format!(" {source}({code})")),
            Value::String(code) => message.push_str(&format!(" {source}({code})")),
            _ => {}
        }
    }
    Some(ServerError {
        from,
        to,
        severity,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{pipe, PipeWriter},
        sync::{
            atomic::{AtomicBool, AtomicUsize},
            Mutex,
        },
    };

    /// What the fake server does. Tests change it while it runs.
    #[derive(Default)]
    struct Script {
        items: Vec<Value>,
        hold_checks: bool,
        exit_on_check: bool,
        ignore_checks: bool,
        busy_once: bool,
        ask_on_start: Option<Value>,
    }

    #[derive(Default)]
    struct State {
        script: Script,
        log: Vec<(usize, Value)>,
        held: Vec<(usize, Value)>,
        writers: HashMap<usize, PipeWriter>,
        stops: Vec<usize>,
    }

    #[derive(Clone, Default)]
    struct Fake {
        state: Arc<Mutex<State>>,
        launches: Arc<AtomicUsize>,
        setup_note: Arc<Mutex<Option<String>>>,
        setup_error: Arc<Mutex<Option<String>>>,
        prepared: Arc<AtomicBool>,
    }

    impl Fake {
        fn new() -> Self {
            Self::default()
        }
        fn script(&self, change: impl FnOnce(&mut Script)) {
            change(&mut self.state.lock().unwrap().script);
        }
        fn methods(&self, launch: usize) -> Vec<String> {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .filter(|(number, _)| *number == launch)
                .filter_map(|(_, message)| message["method"].as_str().map(str::to_string))
                .collect()
        }
        fn messages(&self, method: &str) -> Vec<Value> {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .filter(|(_, message)| message["method"] == method)
                .map(|(_, message)| message.clone())
                .collect()
        }
        fn answers(&self) -> Vec<Value> {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .filter(|(_, message)| {
                    message.get("id").is_some() && message.get("method").is_none()
                })
                .map(|(_, message)| message.clone())
                .collect()
        }
        fn launches(&self) -> usize {
            self.launches.load(Ordering::SeqCst)
        }
        fn stops(&self) -> Vec<usize> {
            self.state.lock().unwrap().stops.clone()
        }
        fn answer_held(&self, items: Vec<Value>) {
            let mut state = self.state.lock().unwrap();
            for (launch, id) in std::mem::take(&mut state.held) {
                let answer = json!({"id": id, "result": {"kind": "full", "items": items}});
                let writer = state.writers.get_mut(&launch).unwrap();
                message::write_message(writer, &answer).unwrap();
            }
        }
        fn send_to_client(&self, launch: usize, value: Value) {
            let mut state = self.state.lock().unwrap();
            let writer = state.writers.get_mut(&launch).unwrap();
            message::write_message(writer, &value).unwrap();
        }
        fn kill(&self, launch: usize) {
            self.state.lock().unwrap().writers.remove(&launch);
        }
    }

    fn serve(fake: Fake, launch: usize, reader: std::io::PipeReader) {
        let mut reader = BufReader::new(reader);
        while let Ok(Some(message)) = message::read_message(&mut reader) {
            let mut state = fake.state.lock().unwrap();
            state.log.push((launch, message.clone()));
            let id = message.get("id").cloned();
            let method = message["method"].as_str().unwrap_or_default().to_string();
            let answer = match (method.as_str(), id) {
                ("initialize", Some(id)) => {
                    let question = state.script.ask_on_start.take();
                    let writer = state.writers.get_mut(&launch).unwrap();
                    if let Some(question) = question {
                        message::write_message(writer, &question).unwrap();
                    }
                    Some(json!({"id": id, "result": {"capabilities": {}}}))
                }
                ("textDocument/diagnostic", Some(id)) => {
                    if state.script.exit_on_check {
                        state.writers.remove(&launch);
                        return;
                    }
                    if state.script.ignore_checks {
                        None
                    } else if state.script.hold_checks {
                        state.held.push((launch, id));
                        None
                    } else if state.script.busy_once {
                        state.script.busy_once = false;
                        Some(json!({"id": id, "error": {"code": -32802, "message": "busy"}}))
                    } else {
                        Some(json!({"id": id, "result": {
                            "kind": "full", "items": state.script.items.clone(),
                        }}))
                    }
                }
                _ => None,
            };
            if let Some(answer) = answer {
                if let Some(writer) = state.writers.get_mut(&launch) {
                    let _ = message::write_message(writer, &answer);
                }
            }
        }
    }

    impl Tool for Fake {
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
            if file.starts_with("/none") {
                return None;
            }
            file.parent().map(Path::to_path_buf)
        }
        fn setup_note(&self) -> Option<String> {
            if self.prepared.load(Ordering::SeqCst) {
                None
            } else {
                self.setup_note.lock().unwrap().clone()
            }
        }
        fn prepare(&self) -> Result<(), String> {
            if let Some(error) = self.setup_error.lock().unwrap().clone() {
                return Err(error);
            }
            self.prepared.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn launch(&self, _: &Path) -> io::Result<Connection> {
            let launch = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
            let (to_server_read, to_server_write) = pipe()?;
            let (from_server_read, from_server_write) = pipe()?;
            self.state
                .lock()
                .unwrap()
                .writers
                .insert(launch, from_server_write);
            let fake = self.clone();
            thread::spawn(move || serve(fake, launch, to_server_read));
            let fake = self.clone();
            Ok(Connection {
                reader: Box::new(from_server_read),
                writer: Box::new(to_server_write),
                stop: Box::new(move || {
                    let mut state = fake.state.lock().unwrap();
                    state.stops.push(launch);
                    state.writers.remove(&launch);
                }),
            })
        }
        fn watches(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "ts" || e == "json")
        }
        fn restarts_server(&self, path: &Path) -> bool {
            path.ends_with("package.json")
        }
    }

    const SOON: Duration = Duration::from_secs(3);

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < SOON, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn limits() -> Limits {
        Limits {
            idle: Duration::from_secs(120),
            max_servers: 3,
            first_answer: Duration::from_secs(30),
            answer: Duration::from_secs(30),
            crash_window: Duration::from_secs(60),
            file_change_pause: Duration::from_millis(10),
            refresh_pause: Duration::from_millis(10),
            retry_setup: Duration::from_millis(1),
            retry_busy: Duration::from_millis(5),
        }
    }

    fn start(fake: &Fake) -> LanguageServers {
        LanguageServers::start(Arc::new(fake.clone()), None, limits())
    }

    fn open(servers: &LanguageServers, file: &str) -> (Document, async_channel::Receiver<Update>) {
        servers.open(Path::new(file), Path::new("/")).unwrap()
    }

    fn next_update(updates: &async_channel::Receiver<Update>) -> Update {
        let start = Instant::now();
        loop {
            match updates.try_recv() {
                Ok(update) => return update,
                Err(_) => {
                    assert!(start.elapsed() < SOON, "no update arrived");
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    fn next_errors(updates: &async_channel::Receiver<Update>) -> (Arc<str>, Vec<ServerError>) {
        loop {
            if let Update::Errors { text, errors } = next_update(updates) {
                return (text, errors);
            }
        }
    }

    fn item(line: u64, start: u64, end: u64, severity: u64) -> Value {
        json!({
            "range": {
                "start": {"line": line, "character": start},
                "end": {"line": line, "character": end},
            },
            "severity": severity,
            "source": "ts",
            "code": 2322,
            "message": "Bad type.",
        })
    }

    #[test]
    fn errors_arrive_with_byte_positions_and_hints_are_dropped() {
        let fake = Fake::new();
        fake.script(|s| s.items = vec![item(1, 1, 2, 1), item(0, 0, 1, 2), item(0, 0, 1, 4)]);
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        let text = "🙂\nxøy";
        document.set_text(text.into());
        let (checked, errors) = next_errors(&updates);
        assert_eq!(&*checked, text);
        assert_eq!(errors.len(), 2);
        let bad = text.find('ø').unwrap();
        assert_eq!((errors[0].from, errors[0].to), (bad, bad + 'ø'.len_utf8()));
        assert_eq!(errors[0].severity, Severity::Error);
        assert_eq!(errors[0].message, "Bad type. ts(2322)");
        assert_eq!(errors[1].severity, Severity::Warning);
    }

    #[test]
    fn an_empty_range_still_covers_one_character() {
        let fake = Fake::new();
        fake.script(|s| s.items = vec![item(0, 1, 1, 1)]);
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("añb".into());
        let (_, errors) = next_errors(&updates);
        assert_eq!((errors[0].from, errors[0].to), (1, 3));
    }

    #[test]
    fn the_first_text_opens_the_file_and_later_text_changes_it() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("one".into());
        next_errors(&updates);
        document.set_text("two".into());
        next_errors(&updates);
        assert_eq!(
            fake.methods(1),
            [
                "initialize",
                "initialized",
                "textDocument/didOpen",
                "textDocument/diagnostic",
                "textDocument/didChange",
                "textDocument/diagnostic",
            ]
        );
        let change = &fake.messages("textDocument/didChange")[0];
        assert_eq!(change["params"]["contentChanges"][0]["text"], "two");
        assert_eq!(change["params"]["textDocument"]["version"], 2);
    }

    #[test]
    fn the_same_text_is_not_checked_again() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("one".into());
        next_errors(&updates);
        document.set_text("one".into());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(fake.messages("textDocument/diagnostic").len(), 1);
    }

    #[test]
    fn an_answer_for_old_text_is_dropped() {
        let fake = Fake::new();
        fake.script(|s| s.hold_checks = true);
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("old".into());
        wait_until("the first check", || {
            !fake.messages("textDocument/diagnostic").is_empty()
        });
        document.set_text("new".into());
        wait_until("the new text", || {
            !fake.messages("textDocument/didChange").is_empty()
        });
        fake.script(|s| {
            s.hold_checks = false;
            s.items = vec![item(0, 0, 1, 1)];
        });
        fake.answer_held(vec![item(0, 0, 3, 1)]);
        let (text, errors) = next_errors(&updates);
        assert_eq!(&*text, "new");
        assert_eq!(errors[0].to, 1, "the answer came from the second check");
    }

    #[test]
    fn a_busy_server_is_asked_again() {
        let fake = Fake::new();
        fake.script(|s| s.busy_once = true);
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        assert_eq!(fake.messages("textDocument/diagnostic").len(), 2);
    }

    #[test]
    fn requests_from_a_server_always_get_an_answer() {
        let fake = Fake::new();
        fake.script(|s| {
            s.ask_on_start = Some(json!({
                "id": 77, "method": "workspace/configuration",
                "params": {"items": [{"section": "a"}, {"section": "b"}]},
            }));
        });
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        fake.send_to_client(1, json!({"id": "x", "method": "made/up", "params": {}}));
        wait_until("both answers", || fake.answers().len() == 2);
        let answers = fake.answers();
        assert_eq!(answers[0]["id"], 77);
        assert_eq!(answers[0]["result"], json!([null, null]));
        assert_eq!(answers[1]["id"], "x");
        assert_eq!(answers[1]["result"], Value::Null);
    }

    #[test]
    fn a_refresh_request_checks_open_files_again() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        fake.script(|s| s.items = vec![item(0, 0, 1, 1)]);
        fake.send_to_client(
            1,
            json!({"id": 5, "method": "workspace/diagnostic/refresh"}),
        );
        let (_, errors) = next_errors(&updates);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn a_crash_starts_a_new_server_and_opens_the_files_again() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        fake.kill(1);
        wait_until("a new server", || fake.methods(2).len() >= 4);
        assert_eq!(fake.methods(2)[2], "textDocument/didOpen");
        assert_eq!(fake.launches(), 2);
        drop(document);
    }

    #[test]
    fn two_crashes_stop_the_server_and_show_a_note() {
        let fake = Fake::new();
        fake.script(|s| s.exit_on_check = true);
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        loop {
            if let Update::Note(Some(note)) = next_update(&updates) {
                assert_eq!(note, "Fake stopped working in this repo.");
                break;
            }
        }
        assert_eq!(fake.launches(), 2);
        document.set_text("newer".into());
        thread::sleep(Duration::from_millis(30));
        assert_eq!(fake.launches(), 2, "no restart loop");
    }

    #[test]
    fn a_server_that_never_answers_counts_as_a_crash() {
        let fake = Fake::new();
        fake.script(|s| s.ignore_checks = true);
        let mut limits = limits();
        limits.first_answer = Duration::from_millis(20);
        limits.answer = Duration::from_millis(20);
        let servers = LanguageServers::start(Arc::new(fake.clone()), None, limits);
        let (document, _updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        wait_until("a stop", || !fake.stops().is_empty());
        wait_until("a new server", || fake.launches() >= 2);
    }

    #[test]
    fn an_idle_server_stops_and_the_next_edit_starts_a_new_one() {
        let fake = Fake::new();
        let mut limits = limits();
        limits.idle = Duration::from_millis(40);
        let servers = LanguageServers::start(Arc::new(fake.clone()), None, limits);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("one".into());
        next_errors(&updates);
        wait_until("the idle stop", || fake.stops() == [1]);
        document.set_text("two".into());
        next_errors(&updates);
        assert_eq!(fake.launches(), 2);
        assert_eq!(fake.methods(2)[2], "textDocument/didOpen");
    }

    #[test]
    fn a_fourth_repo_stops_the_server_used_least_recently() {
        let fake = Fake::new();
        let servers = start(&fake);
        let mut open_files = Vec::new();
        for repo in ["a", "b", "c"] {
            let (document, updates) = open(&servers, &format!("/{repo}/x.ts"));
            document.set_text("text".into());
            next_errors(&updates);
            open_files.push((document, updates));
            thread::sleep(Duration::from_millis(3));
        }
        open_files[0].0.set_text("newer".into());
        next_errors(&open_files[0].1);
        let (document, updates) = open(&servers, "/d/x.ts");
        document.set_text("text".into());
        next_errors(&updates);
        assert_eq!(fake.stops(), [2], "repo b was used least recently");
    }

    #[test]
    fn changed_file_names_arrive_in_one_batch_and_open_files_are_checked_again() {
        let fake = Fake::new();
        let sink = Arc::new(Mutex::new(None));
        let saved = sink.clone();
        let watcher: Watcher = Box::new(move |_, files| {
            *saved.lock().unwrap() = Some(files);
            Some(Box::new(()))
        });
        let servers = LanguageServers::start(Arc::new(fake.clone()), Some(watcher), limits());
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        fake.script(|s| s.items = vec![item(0, 0, 1, 1)]);
        let files = sink.lock().unwrap().clone().unwrap();
        files.send(vec![
            FileChange {
                path: "/r/b.ts".into(),
                exists: true,
            },
            FileChange {
                path: "/r/image.png".into(),
                exists: true,
            },
        ]);
        files.send(vec![FileChange {
            path: "/r/c.ts".into(),
            exists: false,
        }]);
        let (_, errors) = next_errors(&updates);
        assert_eq!(errors.len(), 1);
        let batches = fake.messages("workspace/didChangeWatchedFiles");
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0]["params"]["changes"],
            json!([
                {"uri": "file:///r/b.ts", "type": 2},
                {"uri": "file:///r/c.ts", "type": 3},
            ])
        );
    }

    #[test]
    fn a_package_file_change_starts_a_new_server() {
        let fake = Fake::new();
        let sink = Arc::new(Mutex::new(None));
        let saved = sink.clone();
        let watcher: Watcher = Box::new(move |_, files| {
            *saved.lock().unwrap() = Some(files);
            None
        });
        let servers = LanguageServers::start(Arc::new(fake.clone()), Some(watcher), limits());
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        let files = sink.lock().unwrap().clone().unwrap();
        files.send(vec![FileChange {
            path: "/r/package.json".into(),
            exists: true,
        }]);
        wait_until("a new server", || fake.methods(2).len() >= 4);
        assert_eq!(fake.stops(), [1]);
        assert_eq!(fake.methods(2)[2], "textDocument/didOpen");
    }

    #[test]
    fn the_file_closes_only_after_its_last_editor_leaves() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (first, updates) = open(&servers, "/r/a.ts");
        first.set_text("text".into());
        next_errors(&updates);
        let (second, second_updates) = open(&servers, "/r/a.ts");
        let (text, _) = next_errors(&second_updates);
        assert_eq!(&*text, "text", "a second editor gets the known errors");
        drop(first);
        thread::sleep(Duration::from_millis(20));
        assert!(fake.messages("textDocument/didClose").is_empty());
        drop(second);
        wait_until("the close", || {
            fake.messages("textDocument/didClose").len() == 1
        });
    }

    #[test]
    fn a_file_without_a_root_starts_no_server() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, _updates) = open(&servers, "/none/a.ts");
        document.set_text("text".into());
        thread::sleep(Duration::from_millis(20));
        assert_eq!(fake.launches(), 0);
    }

    #[test]
    fn other_files_are_not_opened_by_this_tool() {
        let fake = Fake::new();
        let servers = start(&fake);
        assert!(servers
            .open(Path::new("/r/a.rs"), Path::new("/r"))
            .is_none());
    }

    #[test]
    fn a_change_in_one_file_checks_the_other_open_files() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (first, first_updates) = open(&servers, "/r/a.ts");
        first.set_text("a".into());
        next_errors(&first_updates);
        let (second, second_updates) = open(&servers, "/r/b.ts");
        second.set_text("b".into());
        next_errors(&second_updates);
        fake.script(|s| s.items = vec![item(0, 0, 1, 1)]);
        second.set_text("changed".into());
        let (text, errors) = next_errors(&first_updates);
        assert_eq!(&*text, "a");
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn setup_shows_a_note_then_the_server_starts() {
        let fake = Fake::new();
        *fake.setup_note.lock().unwrap() = Some("Setting up.".to_string());
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        let mut notes = Vec::new();
        let (text, _) = loop {
            match next_update(&updates) {
                Update::Note(note) => notes.push(note),
                Update::Errors { text, errors } => break (text, errors),
            }
        };
        assert_eq!(&*text, "text");
        assert_eq!(notes[0].as_deref(), Some("Setting up."));
        wait_until("the note to go", || {
            matches!(updates.try_recv(), Ok(Update::Note(None))) || notes.last() == Some(&None)
        });
    }

    #[test]
    fn a_failed_setup_shows_a_note_and_tries_again_later() {
        let fake = Fake::new();
        *fake.setup_note.lock().unwrap() = Some("Setting up.".to_string());
        *fake.setup_error.lock().unwrap() = Some("the download failed".to_string());
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("one".into());
        loop {
            if let Update::Note(Some(note)) = next_update(&updates) {
                if note.contains("failed") {
                    assert_eq!(note, "Fake errors are not available: the download failed.");
                    break;
                }
            }
        }
        assert_eq!(fake.launches(), 0);
        *fake.setup_error.lock().unwrap() = None;
        document.set_text("two".into());
        let (text, _) = next_errors(&updates);
        assert_eq!(&*text, "two");
        assert_eq!(fake.launches(), 1);
    }

    #[test]
    fn dropping_the_last_handle_stops_every_server() {
        let fake = Fake::new();
        let servers = start(&fake);
        let (document, updates) = open(&servers, "/r/a.ts");
        document.set_text("text".into());
        next_errors(&updates);
        drop(document);
        drop(servers);
        wait_until("the stop", || fake.stops() == [1]);
    }
}
