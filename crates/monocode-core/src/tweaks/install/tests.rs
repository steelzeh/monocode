use super::*;
use serde_json::json;
use std::collections::HashMap;

const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

fn link(text: &str) -> Link {
    parse_link(text).unwrap()
}

#[test]
fn links_are_read_in_every_form_github_shows() {
    let plain = link("owner/repo");
    assert_eq!(
        (plain.owner.as_str(), plain.repo.as_str()),
        ("owner", "repo")
    );
    assert_eq!(plain.reference, None);
    assert!(plain.path.is_empty());
    assert_eq!(link("https://github.com/owner/repo.git/"), plain);
    assert_eq!(link("github.com/owner/repo"), plain);
    assert_eq!(link("https://www.github.com/owner/repo?tab=readme"), plain);
    assert_eq!(link("owner/repo/tweaks/title").path, ["tweaks", "title"]);
    let tree = link("https://github.com/owner/repo/tree/main/tweaks/title");
    assert_eq!(tree.reference.as_deref(), Some("main"));
    assert_eq!(tree.path, ["tweaks", "title"]);
    let file = link("https://github.com/owner/repo/blob/v2/tweaks/title/tweak.json");
    assert_eq!(file.reference.as_deref(), Some("v2"));
    assert_eq!(file.path, ["tweaks", "title"]);
}

#[test]
fn links_that_could_reach_anywhere_else_are_refused() {
    for bad in [
        "",
        "owner",
        "https://evil.example/owner/repo",
        "https://github.com.evil.example/owner/repo",
        "gitlab.com/owner/repo",
        "owner/../secret",
        "owner/repo/../../other",
        "owner/repo/a b",
        "owner/repo/blob/main/other.json",
        "https://github.com/owner/repo/tree/ma%2Fin/x",
    ] {
        assert!(parse_link(bad).is_err(), "{bad}");
    }
}

fn site(files: &[(&str, &str)]) -> HashMap<String, String> {
    let mut site = HashMap::new();
    site.insert(format!("{API}/owner/repo/commits/HEAD"), SHA.to_owned());
    for (name, contents) in files {
        site.insert(
            format!("{RAW}/owner/repo/{SHA}/{name}"),
            (*contents).to_owned(),
        );
    }
    site
}

fn fetched_from(site: &HashMap<String, String>, text: &str) -> Result<Fetched, String> {
    fetch_with(&link(text), &|url, _| Ok(site.get(url).cloned()))
}

fn manifest() -> String {
    json!({"id":"title","name":"Title","version":"1","author":"a",
           "permissions":["sessions.read"],"script":"lib/main.js",
           "menu":[{"id":"go","label":"Go"}]})
    .to_string()
}

const SCRIPT: &str = "tweak.onMenu('go', () => {});";

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("monocode-install-{}", uuid::Uuid::new_v4()));
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_tweak_installs_one_exact_commit_and_records_where_it_came_from() {
    let manifest = manifest();
    let site = site(&[("tweak.json", &manifest), ("lib/main.js", SCRIPT)]);
    let fetched = fetched_from(&site, "owner/repo").unwrap();
    assert_eq!(fetched.source.commit, SHA);
    assert_eq!(fetched.source.short_commit(), "abcdef0");
    let root = Root::new();
    assert!(!root.0.exists(), "reviewing writes nothing");
    install(&root.0, &fetched).unwrap();
    let found = discover(&root.0);
    assert_eq!(found.len(), 1, "no staging folder is left behind");
    let Found::Ready(tweak) = &found[0] else {
        panic!("{found:?}")
    };
    assert_eq!(tweak.fingerprint, fetched.fingerprint);
    assert_eq!(tweak.script_source.as_deref(), Some(SCRIPT));
    assert_eq!(tweak.source.as_ref().unwrap().repo, "owner/repo");
}

#[test]
fn a_folder_inside_a_repo_and_a_new_version_both_install() {
    let first = manifest();
    let mut files = site(&[
        ("tweaks/title/tweak.json", &first),
        ("tweaks/title/lib/main.js", SCRIPT),
    ]);
    let root = Root::new();
    let old = fetched_from(&files, "owner/repo/tweaks/title").unwrap();
    assert_eq!(old.source.path, "tweaks/title");
    install(&root.0, &old).unwrap();
    std::fs::write(root.0.join("title/stale.txt"), "left from before").unwrap();
    files.insert(
        format!("{RAW}/owner/repo/{SHA}/tweaks/title/lib/main.js"),
        "tweak.onMenu('go', () => {}); // new".to_owned(),
    );
    let new = fetched_from(&files, "owner/repo/tweaks/title").unwrap();
    assert_ne!(new.fingerprint, old.fingerprint);
    install(&root.0, &new).unwrap();
    assert!(!root.0.join("title/stale.txt").exists());
    assert_eq!(discover(&root.0).len(), 1);
}

#[test]
fn a_manifest_cannot_write_outside_its_folder() {
    for script in [
        "../evil.js",
        "/etc/evil.js",
        "a/../../evil.js",
        "a\\evil.js",
    ] {
        let manifest = json!({"id":"title","name":"T","version":"1","script":script}).to_string();
        let site = site(&[("tweak.json", &manifest), (script, SCRIPT)]);
        let error = fetched_from(&site, "owner/repo").unwrap_err();
        assert!(error.contains("inside the tweak"), "{script}: {error}");
    }
    for id in ["../evil", "A", "a/b"] {
        let manifest = json!({"id":id,"name":"T","version":"1"}).to_string();
        let site = site(&[("tweak.json", &manifest)]);
        assert!(fetched_from(&site, "owner/repo").is_err(), "{id}");
    }
    // Even a tampered review cannot put a file outside.
    let manifest = manifest();
    let site = site(&[("tweak.json", &manifest), ("lib/main.js", SCRIPT)]);
    let mut fetched = fetched_from(&site, "owner/repo").unwrap();
    fetched.files[0].0 = "../evil.js".into();
    let root = Root::new();
    assert!(install(&root.0, &fetched).is_err());
    assert!(!root.0.join("evil.js").exists());
}

#[test]
fn missing_files_and_changed_files_are_refused_with_a_reason() {
    let empty = HashMap::new();
    assert!(fetched_from(&empty, "owner/repo")
        .unwrap_err()
        .contains("Could not find"));
    let no_manifest = site(&[]);
    assert!(fetched_from(&no_manifest, "owner/repo")
        .unwrap_err()
        .contains("no tweak.json"));
    let manifest = manifest();
    let no_script = site(&[("tweak.json", &manifest)]);
    assert!(fetched_from(&no_script, "owner/repo")
        .unwrap_err()
        .contains("missing"));
    let mut wrong = site(&[]);
    wrong.insert(format!("{API}/owner/repo/commits/HEAD"), "main".to_owned());
    assert!(fetched_from(&wrong, "owner/repo")
        .unwrap_err()
        .contains("exact version"));
    let good = site(&[("tweak.json", &manifest), ("lib/main.js", SCRIPT)]);
    let mut fetched = fetched_from(&good, "owner/repo").unwrap();
    fetched.files[0].1 = "tweak.onMenu('go', () => { /* swapped */ });".into();
    let root = Root::new();
    let error = install(&root.0, &fetched).unwrap_err();
    assert!(error.contains("do not match"), "{error}");
    assert!(discover(&root.0).is_empty());
}

fn swap_folders(root: &Root) -> (PathBuf, PathBuf, PathBuf) {
    let (dir, target) = (root.0.join("new"), root.0.join("old"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("old.txt"), "keep me").unwrap();
    (dir, target, root.0.join(PREVIOUS))
}

#[test]
fn a_failed_update_puts_the_old_version_back() {
    let root = Root::new();
    let (dir, target, previous) = swap_folders(&root);
    let error = swap(&dir, &target, &previous, &|from, to| {
        if from == dir.as_path() {
            Err(std::io::Error::other("disk full"))
        } else {
            std::fs::rename(from, to)
        }
    })
    .unwrap_err();
    assert!(error.contains("disk full"), "{error}");
    assert!(target.join("old.txt").exists());
    assert!(!previous.exists());
}

#[test]
fn a_failed_restore_says_where_the_old_version_is() {
    let root = Root::new();
    let (dir, target, previous) = swap_folders(&root);
    let error = swap(&dir, &target, &previous, &|from, to| {
        if from == dir.as_path() || from == previous.as_path() {
            Err(std::io::Error::other("disk full"))
        } else {
            std::fs::rename(from, to)
        }
    })
    .unwrap_err();
    assert!(error.contains(PREVIOUS), "{error}");
    assert!(previous.join("old.txt").exists());
}

#[test]
fn a_tweak_named_previous_can_be_updated() {
    let manifest = json!({"id":"previous","name":"T","version":"1"}).to_string();
    let site = site(&[("tweak.json", &manifest)]);
    let fetched = fetched_from(&site, "owner/repo").unwrap();
    let root = Root::new();
    install(&root.0, &fetched).unwrap();
    install(&root.0, &fetched).unwrap();
    assert_eq!(discover(&root.0).len(), 1);
}

#[test]
#[ignore = "uses the real GitHub"]
fn the_real_github_answers_as_expected() {
    let error = fetch(&link("hardbeat920/monocode")).unwrap_err();
    assert!(error.contains("no tweak.json"), "{error}");
    let text = github_get(
        &format!("{RAW}/hardbeat920/monocode/HEAD/README.md"),
        1 << 20,
    )
    .unwrap()
    .unwrap();
    assert!(text.contains("MonoCode") || text.contains("Monocode"));
    assert!(fetch(&link("hardbeat920/no-such-repo-here"))
        .unwrap_err()
        .contains("Could not find"));
}
