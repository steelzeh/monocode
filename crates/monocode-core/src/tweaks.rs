//! Tweaks: small user-installed additions that stay out of the app itself.
//!
//! A tweak is a folder with a `tweak.json`. It can add slash commands, chat menu
//! items and one native panel. Its scripts run in the visual sandbox, so they
//! have no files, network or shell. They reach the app only through its general
//! actions, and only those the user accepted. Every tweak starts turned off.
use crate::app_access::{tweak_permission, SEND_PROMPTS};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

mod guard;
mod install;
pub use guard::{Guard, Origin};
pub use install::{fetch, install, parse_link, Fetched, Link, Source};

pub const MANIFEST: &str = "tweak.json";
const SETTING: &str = "tweaks-enabled";
const MANIFEST_BYTES: u64 = 64 * 1024;
const TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub title: String,
    pub script: String,
    /// The name of one of the app's icons, for the panel's tab.
    #[serde(default)]
    pub icon: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuEntry {
    pub id: String,
    pub label: String,
    /// Sent to the chat. Without it, the tweak's script handles the click.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The row's text while the tweak is working on that chat.
    #[serde(default)]
    pub busy_label: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    /// Names from `app_access::TWEAK_PERMISSIONS`.
    pub permissions: Vec<String>,
    #[serde(default)]
    pub panel: Option<Panel>,
    /// Runs in the sandbox with no view, to handle menu clicks.
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub menu: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tweak {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub panel_source: Option<String>,
    pub script_source: Option<String>,
    /// Identifies these exact files, so a change can be noticed.
    pub fingerprint: String,
    pub source: Option<Source>,
}
/// One folder of the tweaks directory; a broken one is shown, not hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // A handful of these, read once per reload.
pub enum Found {
    Ready(Tweak),
    Broken { folder: String, error: String },
}

/// A script's request to run one app action, with the answer going back to it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppCall {
    pub request: u64,
    pub action: String,
    pub input: serde_json::Value,
}

/// The read-only data a panel sees as `tweak.context`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Context {
    pub project: String,
}

/// What a script receives for the chat that was right-clicked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ChatInfo {
    /// The id to give back to app actions. Text, because JavaScript numbers
    /// cannot hold every id exactly.
    pub id: String,
    pub title: String,
    pub project: String,
}

/// Which tweaks are on, as the fingerprint of the files the user accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Grants(BTreeMap<String, String>);
impl Grants {
    pub fn parse(raw: Option<&str>) -> Self {
        raw.and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default()
    }
    pub fn setting() -> &'static str {
        SETTING
    }
    /// Any change to a tweak's files, including its permissions, turns it off.
    pub fn allows(&self, tweak: &Tweak) -> bool {
        self.0.get(&tweak.manifest.id) == Some(&tweak.fingerprint)
    }
    /// Was turned on, but its files are no longer the ones accepted.
    pub fn changed(&self, tweak: &Tweak) -> bool {
        self.0.contains_key(&tweak.manifest.id) && !self.allows(tweak)
    }
    pub fn set(&mut self, tweak: &Tweak, enabled: bool) {
        if enabled {
            self.0
                .insert(tweak.manifest.id.clone(), tweak.fingerprint.clone());
        } else {
            self.0.remove(&tweak.manifest.id);
        }
    }
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

fn fingerprint(manifest: &str, panel: Option<&str>, script: Option<&str>) -> String {
    let mut hash = Sha256::new();
    for part in [Some(manifest), panel, script] {
        match part {
            Some(text) => {
                hash.update([1]);
                hash.update((text.len() as u64).to_le_bytes());
                hash.update(text);
            }
            None => hash.update([0]),
        }
    }
    format!("{:x}", hash.finalize())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn validate(manifest: &Manifest, folder: &str) -> Result<(), String> {
    if manifest.id != folder {
        return Err(format!(
            "The id \"{}\" must match the folder name \"{folder}\"",
            manifest.id
        ));
    }
    if !valid_id(&manifest.id) {
        return Err("The id may only use a-z, 0-9 and hyphens".into());
    }
    if manifest.name.trim().is_empty() || manifest.version.trim().is_empty() {
        return Err("A tweak needs a name and a version".into());
    }
    if let Some(unknown) = manifest
        .permissions
        .iter()
        .find(|permission| tweak_permission(permission).is_none())
    {
        return Err(format!("{unknown} is not a permission tweaks can ask for"));
    }
    let sends_prompts =
        !manifest.commands.is_empty() || manifest.menu.iter().any(|entry| entry.prompt.is_some());
    if sends_prompts
        && !manifest
            .permissions
            .iter()
            .any(|permission| permission == SEND_PROMPTS)
    {
        return Err("A tweak with prompts must ask for the send_prompts permission".into());
    }
    let mut names = HashSet::new();
    for command in &manifest.commands {
        if !valid_id(&command.name) || !names.insert(command.name.as_str()) {
            return Err(format!(
                "The command name \"{}\" is invalid or used twice",
                command.name
            ));
        }
        if command.prompt.trim().is_empty() || command.prompt.len() > TEXT_BYTES {
            return Err(format!(
                "The command \"{}\" has no usable prompt",
                command.name
            ));
        }
    }
    let mut entries = HashSet::new();
    for entry in &manifest.menu {
        if !valid_id(&entry.id) || !entries.insert(entry.id.as_str()) {
            return Err(format!(
                "The menu id \"{}\" is invalid or used twice",
                entry.id
            ));
        }
        let prompt_ok = match &entry.prompt {
            Some(prompt) => !prompt.trim().is_empty() && prompt.len() <= TEXT_BYTES,
            None => manifest.script.is_some(),
        };
        if entry.label.trim().is_empty() || !prompt_ok {
            return Err(format!("The menu item \"{}\" is incomplete", entry.id));
        }
    }
    if manifest
        .panel
        .as_ref()
        .is_some_and(|p| p.title.trim().is_empty())
    {
        return Err("A panel needs a title".into());
    }
    if manifest
        .panel
        .as_ref()
        .and_then(|panel| panel.icon.as_deref())
        .is_some_and(|icon| !valid_id(icon))
    {
        return Err("A panel's icon name may only use a-z, 0-9 and hyphens".into());
    }
    Ok(())
}

/// Reads the script only from inside the tweak's own folder, following links.
fn read_script(dir: &Path, script: &str) -> Result<String, String> {
    let root = dir.canonicalize().map_err(|e| e.to_string())?;
    let path = root
        .join(script)
        .canonicalize()
        .map_err(|e| format!("Cannot read {script}: {e}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(format!(
            "The script {script} must be a file inside the tweak"
        ));
    }
    let source =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {script}: {e}"))?;
    crate::visual::validate_source(&source)?;
    Ok(source)
}

fn load(dir: &Path, folder: &str) -> Result<Tweak, String> {
    let path = dir.join(MANIFEST);
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("Cannot read {MANIFEST}: {e}"))?
        .len();
    if size > MANIFEST_BYTES {
        return Err(format!("{MANIFEST} is too large"));
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {MANIFEST}: {e}"))?;
    let manifest: Manifest =
        serde_json::from_str(&raw).map_err(|e| format!("{MANIFEST} is invalid: {e}"))?;
    validate(&manifest, folder)?;
    let panel_source = manifest
        .panel
        .as_ref()
        .map(|panel| read_script(dir, &panel.script))
        .transpose()?;
    let script_source = manifest
        .script
        .as_deref()
        .map(|script| read_script(dir, script))
        .transpose()?;
    let fingerprint = fingerprint(&raw, panel_source.as_deref(), script_source.as_deref());
    let source = std::fs::read_to_string(dir.join(install::SOURCE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());
    Ok(Tweak {
        manifest,
        dir: dir.to_owned(),
        panel_source,
        script_source,
        fingerprint,
        source,
    })
}

/// Every tweak folder in `root`, by name. A missing `root` means no tweaks.
pub fn discover(root: &Path) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut folders: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    folders
        .into_iter()
        .filter_map(|dir| {
            let folder = dir.file_name()?.to_string_lossy().into_owned();
            if folder.starts_with('.') {
                return None;
            }
            Some(match load(&dir, &folder) {
                Ok(tweak) => Found::Ready(tweak),
                Err(error) => Found::Broken { folder, error },
            })
        })
        .collect()
}

/// A command's text for the agent: `{{args}}` is what followed the command.
pub fn expand_prompt(prompt: &str, args: &str) -> String {
    let args = args.trim();
    if prompt.contains("{{args}}") {
        prompt.replace("{{args}}", args)
    } else if args.is_empty() {
        prompt.to_owned()
    } else {
        format!("{prompt}\n\n{args}")
    }
}

#[cfg(test)]
mod tests;
