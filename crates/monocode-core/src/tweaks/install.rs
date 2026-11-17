//! Installing a tweak from GitHub: one exact commit, only the files its
//! manifest names, and nothing on disk until the user has seen what it does.
use super::*;
use std::io::Read;

/// Where an installed tweak came from. Shown to the user; never trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub repo: String,
    pub path: String,
    pub commit: String,
}
impl Source {
    pub fn short_commit(&self) -> &str {
        self.commit.get(..7).unwrap_or(&self.commit)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub owner: String,
    pub repo: String,
    pub reference: Option<String>,
    pub path: Vec<String>,
}

/// A tweak read from GitHub and shown for review. Not on disk yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub manifest: Manifest,
    pub raw_manifest: String,
    pub files: Vec<(String, String)>,
    pub source: Source,
    pub fingerprint: String,
}

const API: &str = "https://api.github.com/repos";
const RAW: &str = "https://raw.githubusercontent.com";
pub(super) const SOURCE_FILE: &str = ".source.json";
/// Where an update keeps the old version. A tweak id cannot start with a dot.
const PREVIOUS: &str = ".previous";

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// `owner/repo`, `owner/repo/folder`, or a github.com link to either. Every
/// part is checked here, because the app builds its web addresses from them.
pub fn parse_link(text: &str) -> Result<Link, String> {
    let invalid = || "Paste a GitHub link, or owner/repo".to_owned();
    let text = text.trim();
    let text = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .map_or(Ok(text), |rest| {
            rest.strip_prefix("www.")
                .unwrap_or(rest)
                .strip_prefix("github.com/")
                .ok_or_else(|| "Only GitHub links are supported".to_owned())
        })?;
    let text = text.strip_prefix("github.com/").unwrap_or(text);
    let text = text.split(['?', '#']).next().unwrap_or(text);
    let segments: Vec<&str> = text.trim_end_matches('/').split('/').collect();
    if segments.len() < 2 {
        return Err(invalid());
    }
    let owner = segments[0];
    if owner.contains('.') {
        return Err("Only GitHub links are supported".into());
    }
    let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    let (reference, path) = match segments.get(2) {
        Some(&"tree") | Some(&"blob") if segments.len() >= 4 => {
            let mut path = &segments[4..];
            if segments[2] == "blob" {
                match path.split_last() {
                    Some((&MANIFEST, rest)) => path = rest,
                    _ => return Err(format!("Link to the tweak's folder or its {MANIFEST}")),
                }
            }
            (Some(segments[3].to_owned()), path)
        }
        _ => (None, &segments[2..]),
    };
    if !valid_segment(owner)
        || !valid_segment(repo)
        || !reference.as_deref().is_none_or(valid_segment)
        || !path.iter().all(|part| valid_segment(part))
    {
        return Err(invalid());
    }
    Ok(Link {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference,
        path: path.iter().map(|part| (*part).to_owned()).collect(),
    })
}

/// A path inside the tweak's folder, with no way out of it.
fn safe_relative(path: &str) -> bool {
    !path.contains('\\') && !path.starts_with('/') && path.split('/').all(valid_segment)
}

type Get<'a> = &'a dyn Fn(&str, u64) -> Result<Option<String>, String>;

pub fn fetch_with(link: &Link, get: Get<'_>) -> Result<Fetched, String> {
    let repo = format!("{}/{}", link.owner, link.repo);
    let reference = link.reference.as_deref().unwrap_or("HEAD");
    let commit = get(&format!("{API}/{repo}/commits/{reference}"), 128)?
        .ok_or_else(|| format!("Could not find {repo} on GitHub. Is it public?"))?;
    let commit = commit.trim().to_owned();
    if commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("GitHub did not name an exact version of that repo".into());
    }
    let path = link.path.join("/");
    let file_url = |name: &str| {
        if path.is_empty() {
            format!("{RAW}/{repo}/{commit}/{name}")
        } else {
            format!("{RAW}/{repo}/{commit}/{path}/{name}")
        }
    };
    let raw_manifest = get(&file_url(MANIFEST), MANIFEST_BYTES)?
        .ok_or_else(|| format!("There is no {MANIFEST} at that link"))?;
    let manifest: Manifest =
        serde_json::from_str(&raw_manifest).map_err(|e| format!("{MANIFEST} is invalid: {e}"))?;
    if !valid_id(&manifest.id) {
        return Err("The id may only use a-z, 0-9 and hyphens".into());
    }
    validate(&manifest, &manifest.id)?;
    let read = |script: &str| -> Result<String, String> {
        if !safe_relative(script) {
            return Err(format!(
                "The script {script} must be a file inside the tweak"
            ));
        }
        let source = get(&file_url(script), crate::visual::MAX_SOURCE_BYTES as u64)?
            .ok_or_else(|| format!("The script {script} is missing"))?;
        crate::visual::validate_source(&source)?;
        Ok(source)
    };
    let mut files = Vec::new();
    let panel_source = match &manifest.panel {
        Some(panel) => {
            let source = read(&panel.script)?;
            files.push((panel.script.clone(), source.clone()));
            Some(source)
        }
        None => None,
    };
    let script_source = match &manifest.script {
        Some(script) => {
            let source = read(script)?;
            if !files.iter().any(|(name, _)| name == script) {
                files.push((script.clone(), source.clone()));
            }
            Some(source)
        }
        None => None,
    };
    let fingerprint = fingerprint(
        &raw_manifest,
        panel_source.as_deref(),
        script_source.as_deref(),
    );
    Ok(Fetched {
        manifest,
        raw_manifest,
        files,
        source: Source { repo, path, commit },
        fingerprint,
    })
}

fn github_get(url: &str, limit: u64) -> Result<Option<String>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let mut request = agent.get(url).set("User-Agent", "monocode-tweaks");
    if url.starts_with(API) {
        // Asks for only the commit's id, not its whole contents.
        request = request.set("Accept", "application/vnd.github.sha");
    }
    match request.call() {
        Ok(response) => {
            let mut text = String::new();
            response
                .into_reader()
                .take(limit + 1)
                .read_to_string(&mut text)
                .map_err(|e| format!("Could not read the answer from GitHub: {e}"))?;
            if text.len() as u64 > limit {
                return Err("A file in that tweak is too large".into());
            }
            Ok(Some(text))
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(ureq::Error::Status(403 | 429, _)) => {
            Err("GitHub is limiting requests right now. Try again later.".into())
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("GitHub answered with error {code}")),
        Err(ureq::Error::Transport(_)) => {
            Err("Could not reach GitHub. Check your connection.".into())
        }
    }
}

/// Reads a tweak from GitHub for review. Blocks, so call it off the main thread.
pub fn fetch(link: &Link) -> Result<Fetched, String> {
    fetch_with(link, &github_get)
}

/// Writes a reviewed tweak into `root`, replacing any tweak with the same id.
/// It is checked exactly as a later load would be, before it takes that place.
pub fn install(root: &Path, fetched: &Fetched) -> Result<(), String> {
    let id = &fetched.manifest.id;
    if !valid_id(id) {
        return Err("The id may only use a-z, 0-9 and hyphens".into());
    }
    if !root.is_absolute() {
        return Err("The tweaks folder is not available".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    let result = stage_and_swap(root, &staging, id, fetched);
    // Never delete the only copy of an installed tweak.
    if result.is_ok() || !staging.join(PREVIOUS).exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn stage_and_swap(root: &Path, staging: &Path, id: &str, fetched: &Fetched) -> Result<(), String> {
    let text = |e: std::io::Error| e.to_string();
    let dir = staging.join(id);
    std::fs::create_dir_all(&dir).map_err(text)?;
    std::fs::write(dir.join(MANIFEST), &fetched.raw_manifest).map_err(text)?;
    for (name, contents) in &fetched.files {
        if !safe_relative(name) {
            return Err(format!("The script {name} must be a file inside the tweak"));
        }
        let target = dir.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(text)?;
        }
        std::fs::write(target, contents).map_err(text)?;
    }
    let source = serde_json::to_string(&fetched.source).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(SOURCE_FILE), source).map_err(text)?;
    if load(&dir, id)?.fingerprint != fetched.fingerprint {
        return Err("The downloaded files do not match what you reviewed".into());
    }
    swap(
        &dir,
        &root.join(id),
        &staging.join(PREVIOUS),
        &|from, to| std::fs::rename(from, to),
    )
}

/// Puts `dir` at `target`. If that fails, the old `target` goes back in place.
fn swap(
    dir: &Path,
    target: &Path,
    previous: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let replacing = target.exists();
    if replacing {
        rename(target, previous).map_err(|e| e.to_string())?;
    }
    let Err(error) = rename(dir, target) else {
        return Ok(());
    };
    if replacing && rename(previous, target).is_err() {
        return Err(format!(
            "The install failed, and your old version is now in {}: {error}",
            previous.display()
        ));
    }
    Err(error.to_string())
}

#[cfg(test)]
mod tests;
