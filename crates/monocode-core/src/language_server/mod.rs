//! A client for language servers: separate programs that report errors and
//! code facts through a standard message format called LSP.
//!
//! `service` owns the servers and the open files, and it decides when a server
//! starts and stops. Each tool, such as `typescript`, says which files it
//! handles and how to start its program.
pub mod message;
mod position;
mod service;
pub mod typescript;

pub use service::{
    Connection, Document, FileChange, FileSink, LanguageServers, Limits, ServerError, Severity,
    Tool, Update, WatchGuard, Watcher,
};

/// A `file://` address for a path. It is `None` for a path that is not absolute.
pub(crate) fn file_uri(path: &std::path::Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::file_uri;
    use std::path::Path;

    #[test]
    fn file_addresses_keep_folder_names_that_need_encoding() {
        for (path, expected) in [
            ("/repo/a.ts", "file:///repo/a.ts"),
            ("/repo/(form)/a.tsx", "file:///repo/(form)/a.tsx"),
            ("/repo/[id]/a.tsx", "file:///repo/[id]/a.tsx"),
            ("/my repo/a.ts", "file:///my%20repo/a.ts"),
            ("/kø/a.ts", "file:///k%C3%B8/a.ts"),
        ] {
            assert_eq!(file_uri(Path::new(path)).as_deref(), Some(expected));
        }
        assert_eq!(file_uri(Path::new("relative/a.ts")), None);
    }
}
