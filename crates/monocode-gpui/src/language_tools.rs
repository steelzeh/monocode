//! Connects open editors to language servers.
use crate::editor_analysis::Diagnostic;
use gpui::{App, Global};
use monocode_core::language_server::{
    Document, LanguageServers, Limits, ServerError, Severity, Update, Watcher,
    typescript::TypeScript,
};
use std::{path::Path, sync::Arc, time::Duration};

/// How long the typing must rest before the server gets the text.
pub(crate) const TYPING_PAUSE: Duration = Duration::from_millis(150);
/// Larger files are not sent to a server.
pub(crate) const MAX_TEXT: usize = 2 * 1024 * 1024;

pub(crate) struct LanguageTools {
    typescript: LanguageServers,
}
impl Global for LanguageTools {}

impl LanguageTools {
    pub(crate) fn new(typescript: LanguageServers) -> Self {
        Self { typescript }
    }
    pub(crate) fn open(
        &self,
        file: &Path,
        project: &Path,
    ) -> Option<(Document, async_channel::Receiver<Update>)> {
        self.typescript.open(file, project)
    }
}

pub(crate) fn install(data_dir: &Path, cx: &mut App) {
    let watcher: Watcher = Box::new(|root, files| {
        let watch = crate::fs_watch::watch_file_names(root, move |changes| files.send(changes))?;
        Some(Box::new(watch))
    });
    let typescript = LanguageServers::start(
        Arc::new(TypeScript::new(data_dir)),
        Some(watcher),
        Limits::default(),
    );
    cx.set_global(LanguageTools::new(typescript));
}

pub(crate) fn diagnostics(errors: Vec<ServerError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|error| Diagnostic {
            from: error.from,
            to: error.to,
            severity: match error.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            }
            .to_string(),
            message: error.message,
            server: true,
        })
        .collect()
}
