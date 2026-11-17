//! Finds the Prettier settings that apply to one file.
//!
//! The search follows Prettier's own rules and stops at the project folder.
//! It only reads files: settings that are programs are reported as unusable,
//! so opening a cloned repository never runs its code.
use crate::files::remote::is_remote;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Prettier's own search order inside one folder.
const CONFIG_FILE_NAMES: &[&str] = &[
    "package.json",
    "package.yaml",
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yml",
    ".prettierrc.yaml",
    ".prettierrc.json5",
    ".prettierrc.js",
    "prettier.config.js",
    ".prettierrc.ts",
    "prettier.config.ts",
    ".prettierrc.mjs",
    "prettier.config.mjs",
    ".prettierrc.mts",
    "prettier.config.mts",
    ".prettierrc.cjs",
    "prettier.config.cjs",
    ".prettierrc.cts",
    "prettier.config.cts",
    ".prettierrc.toml",
];
const CODE_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "mts", "cjs", "cts"];
/// No hand-written settings file is this large.
const MAX_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub config: Option<SettingsFile>,
    pub ignore: Option<SettingsFile>,
    /// Nearest folder first.
    pub editorconfig: Vec<SettingsFile>,
    /// Why the settings that apply cannot be used without running repo code.
    pub unusable: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SettingsFile {
    pub name: String,
    pub folder: String,
    pub text: String,
}

/// The settings for `file`. Files outside `project` get none, as do remote
/// projects, whose files cannot be read here.
pub fn find(file: &Path, project: &Path) -> Settings {
    let mut settings = Settings::default();
    if is_remote(file) {
        return settings;
    }
    for folder in folders(file, project) {
        if settings.ignore.is_none() {
            settings.ignore = read(&folder, ".prettierignore");
        }
        settings.editorconfig.extend(read(&folder, ".editorconfig"));
        if settings.config.is_none() && settings.unusable.is_none() {
            find_config(&folder, &mut settings);
        }
    }
    settings
}

fn folders(file: &Path, project: &Path) -> Vec<PathBuf> {
    if project.as_os_str().is_empty() || !file.starts_with(project) {
        return Vec::new();
    }
    file.ancestors()
        .skip(1)
        .take_while(|folder| folder.starts_with(project))
        .map(Path::to_path_buf)
        .collect()
}

fn find_config(folder: &Path, settings: &mut Settings) {
    for name in CONFIG_FILE_NAMES {
        if !folder.join(name).is_file() {
            continue;
        }
        if is_code(name) {
            settings.unusable = Some(format!(
                "{name} is code, and Monocode does not run repo code for Prettier"
            ));
            return;
        }
        let file = read(folder, name);
        if matches!(*name, "package.json" | "package.yaml") {
            // Like Prettier, skip a package file that holds no settings.
            let Some(file) = file.filter(has_prettier_key) else {
                continue;
            };
            settings.config = Some(file);
            return;
        }
        match file {
            Some(file) => settings.config = Some(file),
            None => settings.unusable = Some(format!("{name} could not be read")),
        }
        return;
    }
}

fn is_code(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| CODE_EXTENSIONS.contains(&extension))
}

fn has_prettier_key(file: &SettingsFile) -> bool {
    if file.name == "package.yaml" {
        return file.text.lines().any(|line| line.starts_with("prettier:"));
    }
    use serde_json::Value;
    let Ok(package) = serde_json::from_str::<Value>(&file.text) else {
        return false;
    };
    match package.get("prettier") {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64() != Some(0.0),
        Some(_) => true,
    }
}

fn read(folder: &Path, name: &str) -> Option<SettingsFile> {
    let path = folder.join(name);
    if !path.is_file() || path.metadata().ok()?.len() > MAX_SIZE {
        return None;
    }
    Some(SettingsFile {
        name: name.to_string(),
        folder: folder.to_string_lossy().into_owned(),
        text: std::fs::read_to_string(path).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }
    fn find_for(dir: &tempfile::TempDir, file: &str) -> Settings {
        find(&dir.path().join(file), dir.path())
    }
    fn config_name(settings: &Settings) -> Option<&str> {
        settings.config.as_ref().map(|file| file.name.as_str())
    }

    #[test]
    fn the_nearest_settings_file_wins() {
        let dir = project(&[(".prettierrc", "a: 1"), ("api/.prettierrc", "b: 2")]);
        let settings = find_for(&dir, "api/src/a.ts");
        let config = settings.config.unwrap();
        assert_eq!(config.text, "b: 2");
        assert!(config.folder.ends_with("api"));
    }
    #[test]
    fn prettier_order_decides_inside_one_folder() {
        let dir = project(&[
            (".prettierrc.toml", "a = 1"),
            (".prettierrc.json", "{}"),
            (".prettierrc", "b: 2"),
        ]);
        assert_eq!(
            config_name(&find_for(&dir, "a.ts")),
            Some(".prettierrc"),
            "the file without an extension comes first"
        );
    }
    #[test]
    fn the_search_stops_at_the_project_folder() {
        let dir = project(&[(".prettierrc", "a: 1"), ("repo/src/a.ts", "")]);
        let settings = find(&dir.path().join("repo/src/a.ts"), &dir.path().join("repo"));
        assert_eq!(settings, Settings::default());
    }
    #[test]
    fn a_file_outside_the_project_gets_no_settings() {
        let dir = project(&[("repo/.prettierrc", "a: 1")]);
        let settings = find(Path::new("/elsewhere/a.ts"), &dir.path().join("repo"));
        assert_eq!(settings, Settings::default());
        assert_eq!(find(Path::new("a.ts"), Path::new("")), Settings::default());
    }
    #[test]
    fn a_remote_file_gets_no_settings() {
        let settings = find(
            Path::new("remote://host/repo/a.ts"),
            Path::new("remote://host/repo"),
        );
        assert_eq!(settings, Settings::default());
    }
    #[test]
    fn a_package_file_without_settings_is_skipped() {
        let dir = project(&[("package.json", r#"{"name":"x"}"#), (".prettierrc", "a: 1")]);
        assert_eq!(config_name(&find_for(&dir, "a.ts")), Some(".prettierrc"));

        let dir = project(&[("package.json", "not json"), (".prettierrc", "a: 1")]);
        assert_eq!(config_name(&find_for(&dir, "a.ts")), Some(".prettierrc"));

        let dir = project(&[
            ("api/package.json", r#"{"name":"x"}"#),
            ("package.json", r#"{"prettier":{"semi":false}}"#),
        ]);
        assert_eq!(
            config_name(&find_for(&dir, "api/a.ts")),
            Some("package.json")
        );
    }
    #[test]
    fn a_package_yaml_with_settings_counts() {
        let dir = project(&[("package.yaml", "name: x\nprettier:\n  semi: false\n")]);
        assert_eq!(config_name(&find_for(&dir, "a.ts")), Some("package.yaml"));

        let dir = project(&[("package.yaml", "name: x\n")]);
        assert_eq!(config_name(&find_for(&dir, "a.ts")), None);
    }
    #[test]
    fn settings_that_are_code_are_unusable_and_never_read() {
        for name in [
            ".prettierrc.js",
            "prettier.config.ts",
            "prettier.config.mjs",
            ".prettierrc.cts",
        ] {
            let dir = project(&[(name, "export default {}")]);
            let settings = find_for(&dir, "a.ts");
            assert_eq!(settings.config, None, "{name}");
            assert!(settings.unusable.unwrap().contains(name), "{name}");
        }
    }
    #[test]
    fn a_data_file_before_a_code_file_wins() {
        let dir = project(&[(".prettierrc", "a: 1"), ("prettier.config.js", "x")]);
        let settings = find_for(&dir, "a.ts");
        assert_eq!(config_name(&settings), Some(".prettierrc"));
        assert_eq!(settings.unusable, None);
    }
    #[test]
    fn an_unreadable_settings_file_is_unusable() {
        let dir = project(&[]);
        fs::write(dir.path().join(".prettierrc"), [0xff, 0xfe, 0x00]).unwrap();
        let settings = find_for(&dir, "a.ts");
        assert_eq!(settings.config, None);
        assert!(settings.unusable.unwrap().contains(".prettierrc"));
    }
    #[test]
    fn the_nearest_ignore_file_wins_and_editorconfig_lists_every_folder() {
        let dir = project(&[
            (".prettierignore", "a"),
            ("api/.prettierignore", "b"),
            (".editorconfig", "root = true"),
            ("api/src/.editorconfig", "[*]"),
        ]);
        let settings = find_for(&dir, "api/src/a.ts");
        assert_eq!(settings.ignore.unwrap().text, "b");
        let texts: Vec<_> = settings
            .editorconfig
            .iter()
            .map(|f| f.text.as_str())
            .collect();
        assert_eq!(texts, ["[*]", "root = true"]);
    }
}
