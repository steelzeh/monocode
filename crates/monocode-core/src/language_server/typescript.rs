//! The TypeScript 7 language server, a native program that the app downloads
//! one time and keeps next to its data.
use super::service::{Connection, Tool};
use crate::files::remote::is_remote;
use sha2::{Digest, Sha512};
use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const VERSION: &str = "7.0.2";
/// The packages are about 10 MB. A larger answer is not what we asked for.
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;

struct Package {
    name: &'static str,
    /// SHA-512 of the package file, from the npm registry.
    sha512: &'static str,
    program: &'static str,
}

fn current_package() -> Option<Package> {
    let package = |name, sha512, program| {
        Some(Package {
            name,
            sha512,
            program,
        })
    };
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => package(
            "typescript-darwin-arm64",
            "828c336abf4cc12fda456a7a7f76b8294ab34630198ceb2618d08ce8b713817ba6f9d05f82c05530df8082f38209b82e5f289c93a2c986906ccf131e6c9f2cc8",
            "lib/tsc",
        ),
        ("macos", "x86_64") => package(
            "typescript-darwin-x64",
            "499f716489ea02936534673db345b5552b24b5848e7bd705a8d388aa63751acf129a48ca65816dd35ec6e15c0fc404889ce0ee01d6d35bbb1788551ff3ddd180",
            "lib/tsc",
        ),
        ("linux", "aarch64") => package(
            "typescript-linux-arm64",
            "421e1e538ff2df20e39df8e3c8f62284c8f9fce0c8966b7e073bb5ece23e7e24910d6e7b4265394a237addec4f44d24f294cdc73520d6b535776b27e32a1e351",
            "lib/tsc",
        ),
        ("linux", "x86_64") => package(
            "typescript-linux-x64",
            "11875fd9c360eeb8025899f109d27e177577f4ef2285bdfb78702ed4b2bca00162ce04d06cf38aef31c75db3edf2b5f6e023aa3835de237b087f47c25c6ec7fc",
            "lib/tsc",
        ),
        ("windows", "aarch64") => package(
            "typescript-win32-arm64",
            "1b2975572e8eb167ac2f39aaf843f415bedbe0d89de76df602f700d9215c758ade95da4db58145a1f3e39edeb6cbd850cbb553699a7ae480898ee01045a55c21",
            "lib/tsc.exe",
        ),
        ("windows", "x86_64") => package(
            "typescript-win32-x64",
            "d014371e40071e528b4a9d6a46f7f7494846a46b03ba107f8e0170ef982ecaa6f126a11a4b40b0fd514ef22da71e094953340246d30c47f20100a137113302e2",
            "lib/tsc.exe",
        ),
        _ => None,
    }
}

pub struct TypeScript {
    folder: PathBuf,
}

impl TypeScript {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            folder: data_dir
                .join("language-servers")
                .join(format!("typescript-{VERSION}")),
        }
    }

    fn program(&self) -> Option<PathBuf> {
        Some(self.folder.join(current_package()?.program))
    }
}

impl Tool for TypeScript {
    fn name(&self) -> &'static str {
        "TypeScript"
    }

    fn handles(&self, file: &Path) -> bool {
        !is_remote(file)
            && matches!(extension(file), Some("ts" | "tsx" | "mts" | "cts"))
            && !file.components().any(|c| c.as_os_str() == "node_modules")
    }

    fn language_id(&self, file: &Path) -> &'static str {
        if extension(file) == Some("tsx") {
            "typescriptreact"
        } else {
            "typescript"
        }
    }

    /// The Git repo or worktree of `file`, if it has a `tsconfig.json` above it.
    fn root(&self, file: &Path, project: &Path) -> Option<PathBuf> {
        if project.as_os_str().is_empty() || !file.starts_with(project) {
            return None;
        }
        let mut has_config = false;
        for folder in file.ancestors().skip(1) {
            if !folder.starts_with(project) {
                break;
            }
            has_config = has_config || folder.join("tsconfig.json").is_file();
            if folder.join(".git").exists() {
                return has_config.then(|| folder.to_path_buf());
            }
        }
        has_config.then(|| project.to_path_buf())
    }

    fn setup_note(&self) -> Option<String> {
        let installed = self.program().is_some_and(|program| program.is_file());
        (!installed).then(|| "Setting up TypeScript. This happens one time.".to_string())
    }

    fn prepare(&self) -> Result<(), String> {
        let package =
            current_package().ok_or_else(|| "there is no build for this system".to_string())?;
        install(&self.folder, &package, download)
    }

    fn launch(&self, root: &Path) -> io::Result<Connection> {
        let program = self
            .program()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no build"))?;
        let mut command = Command::new(program);
        command
            .args(["--lsp", "--stdio"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::process::isolate(&mut command);
        let mut child = command.spawn()?;
        let closed = || io::Error::new(io::ErrorKind::BrokenPipe, "no pipe");
        let reader = child.stdout.take().ok_or_else(closed)?;
        let writer = child.stdin.take().ok_or_else(closed)?;
        Ok(Connection {
            reader: Box::new(reader),
            writer: Box::new(writer),
            stop: Box::new(move || crate::process::terminate(&mut child)),
        })
    }

    fn watches(&self, path: &Path) -> bool {
        matches!(
            extension(path),
            Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "json")
        )
    }

    /// Installed packages changed, and the servers do not watch `node_modules`.
    fn restarts_server(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name,
                    "package.json"
                        | "package-lock.json"
                        | "pnpm-lock.yaml"
                        | "yarn.lock"
                        | "bun.lock"
                        | "bun.lockb"
                )
            })
    }
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|_| "the download failed".to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut bytes)
        .map_err(|_| "the download failed".to_string())?;
    Ok(bytes)
}

fn install(
    folder: &Path,
    package: &Package,
    get: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let name = package.name;
    let bytes = get(&format!(
        "https://registry.npmjs.org/@typescript/{name}/-/{name}-{VERSION}.tgz"
    ))?;
    let digest = Sha512::digest(&bytes);
    if digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        != package.sha512
    {
        return Err("the download did not match its checksum".to_string());
    }
    let failed = |_| "the program could not be saved".to_string();
    let parent = folder
        .parent()
        .ok_or_else(|| "the program could not be saved".to_string())?;
    std::fs::create_dir_all(parent).map_err(failed)?;
    // Unpack beside the target, so that a half-written folder is never used.
    let unpacked = tempfile::Builder::new()
        .prefix(".typescript-")
        .tempdir_in(parent)
        .map_err(failed)?;
    unpack(&bytes, unpacked.path()).map_err(failed)?;
    let program = unpacked.path().join(package.program);
    if !program.is_file() {
        return Err("the download did not contain the program".to_string());
    }
    make_executable(&program).map_err(failed)?;
    if folder.exists() {
        std::fs::remove_dir_all(folder).map_err(failed)?;
    }
    std::fs::rename(unpacked.path(), folder).map_err(failed)?;
    Ok(())
}

/// Unpacks the files under `package/`. An entry that points outside is an error.
fn unpack(bytes: &[u8], target: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix("package") else {
            continue;
        };
        if !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unsafe path"));
        }
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&destination)?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tgz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            // `set_path` refuses `..`, and a real attack would not use it.
            let bytes = header.as_old_mut();
            bytes.name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            archive.append(&header, *data).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }
    fn hex(bytes: &[u8]) -> &'static str {
        let text: String = Sha512::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Box::leak(text.into_boxed_str())
    }
    fn package(sha512: &'static str) -> Package {
        Package {
            name: "typescript-test",
            sha512,
            program: "lib/tsc",
        }
    }

    #[test]
    fn a_good_download_is_installed_and_can_run() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("language-servers/typescript");
        let bytes = tgz(&[
            ("package/lib/tsc", b"program"),
            ("package/lib/a.d.ts", b"x"),
        ]);
        let expected = package(hex(&bytes));
        install(&folder, &expected, |url| {
            assert!(url.contains("/@typescript/typescript-test/-/typescript-test-7"));
            Ok(bytes.clone())
        })
        .unwrap();
        assert_eq!(fs::read(folder.join("lib/tsc")).unwrap(), b"program");
        assert_eq!(fs::read(folder.join("lib/a.d.ts")).unwrap(), b"x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(folder.join("lib/tsc"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let leftovers = fs::read_dir(folder.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "the temporary folder is gone");
    }
    #[test]
    fn a_wrong_checksum_stops_the_install() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("typescript");
        let bytes = tgz(&[("package/lib/tsc", b"program")]);
        let error = install(&folder, &package("00"), |_| Ok(bytes.clone())).unwrap_err();
        assert!(error.contains("checksum"));
        assert!(!folder.exists());
    }
    #[test]
    fn a_failed_download_reports_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let error = install(&dir.path().join("t"), &package("00"), |_| {
            Err("the download failed".to_string())
        })
        .unwrap_err();
        assert_eq!(error, "the download failed");
    }
    #[test]
    fn an_entry_that_points_outside_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("t");
        let bytes = tgz(&[("package/../../evil", b"x"), ("package/lib/tsc", b"p")]);
        let error = install(&folder, &package(hex(&bytes)), |_| Ok(bytes.clone())).unwrap_err();
        assert!(error.contains("saved"), "{error}");
        assert!(!dir.path().join("evil").exists());
        assert!(!folder.exists());
    }
    #[test]
    fn a_package_without_the_program_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = tgz(&[("package/readme", b"x")]);
        let error = install(&dir.path().join("t"), &package(hex(&bytes)), |_| {
            Ok(bytes.clone())
        })
        .unwrap_err();
        assert!(error.contains("did not contain"));
    }
    #[test]
    fn only_typescript_files_outside_node_modules_are_handled() {
        let tool = TypeScript::new(Path::new("/data"));
        for file in ["/r/a.ts", "/r/a.tsx", "/r/a.mts", "/r/a.cts"] {
            assert!(tool.handles(Path::new(file)), "{file}");
        }
        for file in [
            "/r/a.js",
            "/r/a.rs",
            "/r/node_modules/x/a.ts",
            "remote://h/a.ts",
        ] {
            assert!(!tool.handles(Path::new(file)), "{file}");
        }
        assert_eq!(tool.language_id(Path::new("/r/a.tsx")), "typescriptreact");
        assert_eq!(tool.language_id(Path::new("/r/a.ts")), "typescript");
    }
    #[test]
    fn the_root_is_the_git_repo_when_a_tsconfig_is_above_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = TypeScript::new(Path::new("/data"));
        let project = dir.path();
        fs::create_dir_all(project.join("api/.git")).unwrap();
        fs::create_dir_all(project.join("api/src")).unwrap();
        fs::write(project.join("api/tsconfig.json"), "{}").unwrap();
        fs::create_dir_all(project.join("worktree/src")).unwrap();
        fs::write(project.join("worktree/.git"), "gitdir: x").unwrap();
        fs::write(project.join("worktree/tsconfig.json"), "{}").unwrap();
        fs::create_dir_all(project.join("scripts")).unwrap();

        let root = |file: &str| tool.root(&project.join(file), project);
        assert_eq!(root("api/src/a.ts"), Some(project.join("api")));
        assert_eq!(root("worktree/src/a.ts"), Some(project.join("worktree")));
        assert_eq!(root("scripts/a.ts"), None, "no tsconfig above the file");
        assert_eq!(tool.root(Path::new("/elsewhere/a.ts"), project), None);
        assert_eq!(tool.root(Path::new("a.ts"), Path::new("")), None);

        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(root("scripts/a.ts"), Some(project.to_path_buf()));
    }
    #[test]
    fn setup_is_needed_until_the_program_exists() {
        let dir = tempfile::tempdir().unwrap();
        let tool = TypeScript::new(dir.path());
        assert!(tool.setup_note().is_some());
        let program = tool.program().unwrap();
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "x").unwrap();
        assert!(tool.setup_note().is_none());
    }
    #[test]
    fn package_and_lock_files_restart_the_server() {
        let tool = TypeScript::new(Path::new("/data"));
        for file in [
            "package.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "package-lock.json",
        ] {
            assert!(tool.restarts_server(&Path::new("/r").join(file)), "{file}");
        }
        assert!(!tool.restarts_server(Path::new("/r/tsconfig.json")));
        assert!(tool.watches(Path::new("/r/tsconfig.json")));
        assert!(tool.watches(Path::new("/r/a.tsx")));
        assert!(!tool.watches(Path::new("/r/a.png")));
    }
}
