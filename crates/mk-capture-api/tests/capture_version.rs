//! release-packaging.md "Capture version": test RP2 (desktop-only and docs changes
//! leave it alone; server, phone app and `Cargo.lock` changes change it) and the
//! check that the paths it covers are exactly the workspace crates the capture
//! server and the phone app are built from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

#[path = "../build/capture_version.rs"]
mod capture_version;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use capture_version::{Mode, capture_hash, capture_paths};
use serde_json::{Value, json};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A small workspace: the server, the phone app, a crate both use, a desktop-only
/// crate and docs.
fn layout(root: &Path) {
    write(root, "Cargo.lock", "# lock v1\n");
    write(root, "apps/moneykeep-server/src/main.rs", "fn main() {}\n");
    write(root, "apps/moneykeep-capture/src/lib.rs", "// phone\n");
    write(root, "crates/mk-core/src/lib.rs", "// core\n");
    write(root, "crates/desk/src/lib.rs", "// desktop only\n");
    write(root, "docs/specs/x.md", "# spec\n");
    // Build output never counts.
    write(
        root,
        "apps/moneykeep-capture/dist/index.html",
        "<html>1</html>\n",
    );
}

fn paths() -> Vec<String> {
    [
        "Cargo.lock",
        "apps/moneykeep-capture",
        "apps/moneykeep-server",
        "crates/mk-core",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn hash(root: &Path, git: bool) -> (String, Mode) {
    let (h, m, _) = capture_hash(root, &paths(), git).unwrap();
    (h, m)
}

/// RP2 without git: the hash is over file contents.
#[test]
fn rp2_contents_change_only_with_capture_sources() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    layout(root);
    let (first, mode) = hash(root, false);
    assert_eq!(mode, Mode::Contents);
    assert_eq!(first.len(), 64);
    write(root, "docs/specs/x.md", "# spec, edited\n");
    write(root, "crates/desk/src/lib.rs", "// desktop only, edited\n");
    write(
        root,
        "apps/moneykeep-capture/dist/index.html",
        "<html>2</html>\n",
    );
    assert_eq!(hash(root, false).0, first, "docs and desktop-only changes");
    write(root, "apps/moneykeep-server/src/main.rs", "fn main() { }\n");
    let server = hash(root, false).0;
    assert_ne!(server, first, "a server change");
    write(
        root,
        "apps/moneykeep-capture/src/lib.rs",
        "// phone, edited\n",
    );
    let phone = hash(root, false).0;
    assert_ne!(phone, server, "a phone app change");
    write(root, "crates/mk-core/src/new.rs", "// new file\n");
    let shared = hash(root, false).0;
    assert_ne!(shared, phone, "a new file in a shared crate");
    write(root, "Cargo.lock", "# lock v2\n");
    assert_ne!(hash(root, false).0, shared, "Cargo.lock");
}

fn git_ok() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(root: &Path, msg: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "--allow-empty", "-m", msg]);
}

/// RP2 with git: the hash is over HEAD's tree ids while the sources are committed.
#[test]
fn rp2_git_tree_ids() {
    if !git_ok() {
        println!("SKIPPED: git is not installed");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    layout(root);
    write(root, ".gitignore", "/apps/moneykeep-capture/dist/\n");
    git(root, &["init", "-q"]);
    commit(root, "one");
    let (first, mode) = hash(root, true);
    assert_eq!(mode, Mode::GitTrees);
    write(root, "docs/specs/x.md", "# spec, edited\n");
    write(root, "crates/desk/src/lib.rs", "// desktop only, edited\n");
    commit(root, "docs and desktop");
    // Ignored build output, even uncommitted, keeps the tree ids.
    write(
        root,
        "apps/moneykeep-capture/dist/index.html",
        "<html>3</html>\n",
    );
    assert_eq!(hash(root, true), (first.clone(), Mode::GitTrees));
    // An uncommitted server change: file contents, so it differs at once.
    write(root, "apps/moneykeep-server/src/main.rs", "fn main() { }\n");
    let (dirty, mode) = hash(root, true);
    assert_eq!(mode, Mode::Contents);
    assert_ne!(dirty, first);
    commit(root, "server");
    let (server, mode) = hash(root, true);
    assert_eq!(mode, Mode::GitTrees);
    assert_ne!(server, first, "a committed server change");
    write(
        root,
        "apps/moneykeep-capture/src/lib.rs",
        "// phone, edited\n",
    );
    commit(root, "phone");
    assert_ne!(hash(root, true).0, server, "a committed phone app change");
}

/// The path list from a dependency graph: normal and build dependencies count
/// (whatever the target), development dependencies do not.
#[test]
fn paths_follow_normal_and_build_dependencies() {
    let pkg = |name: &str, dir: &str, deps: Value| {
        json!({"name": name, "version": "1.2.3",
               "manifest_path": format!("/w/{dir}/Cargo.toml"), "dependencies": deps})
    };
    let meta = json!({
        "workspace_root": "/w",
        "packages": [
            pkg("moneykeep-server", "apps/moneykeep-server", json!([
                {"name": "mk-core", "kind": null, "path": "/w/crates/mk-core"},
                {"name": "mk-test", "kind": "dev", "path": "/w/crates/mk-test"},
                {"name": "serde", "kind": null},
            ])),
            pkg("moneykeep-capture", "apps/moneykeep-capture", json!([
                {"name": "mk-ui", "kind": null, "path": "/w/crates/mk-ui", "target": "cfg(target_arch = \"wasm32\")"},
            ])),
            pkg("mk-ui", "crates/mk-ui", json!([
                {"name": "mk-gen", "kind": "build", "path": "/w/crates/mk-gen"},
            ])),
            pkg("mk-core", "crates/mk-core", json!([])),
            pkg("mk-gen", "crates/mk-gen", json!([])),
            pkg("mk-test", "crates/mk-test", json!([])),
            pkg("moneykeep-app", "apps/moneykeep-app", json!([
                {"name": "mk-ui", "kind": null, "path": "/w/crates/mk-ui"},
            ])),
        ]
    });
    assert_eq!(
        capture_paths(&meta).unwrap(),
        [
            "Cargo.lock",
            "Cargo.toml",
            "apps/moneykeep-capture",
            "apps/moneykeep-server",
            "crates/mk-core",
            "crates/mk-gen",
            "crates/mk-ui"
        ]
    );
    assert_eq!(capture_version::server_version(&meta).unwrap(), "1.2.3");
    assert_eq!(
        capture_version::capture_version("1.2.3", &"ab".repeat(32)),
        "1.2.3+abababababab"
    );
}

/// The workspace folder, from this crate's (absolute) folder. Not canonicalized: on
/// Windows that adds `\\?\`, and cargo metadata then gives the workspace root and the
/// crates' manifest paths in different forms.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// `path` without a Windows verbatim prefix (`\\?\`).
fn plain(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

/// The baked-in path list covers every workspace crate the capture server and the
/// phone app depend on (from the resolved graph, independently of the build
/// script), and no desktop-only crate.
#[test]
fn capture_paths_cover_the_dependency_graph() {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--offline"])
        .arg("--manifest-path")
        .arg(workspace().join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();
    let root = plain(Path::new(meta["workspace_root"].as_str().unwrap()));
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let dir_of = |id: &str| -> String {
        let p = meta["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap();
        let dir = plain(Path::new(p["manifest_path"].as_str().unwrap()))
            .parent()
            .unwrap()
            .strip_prefix(&root)
            .unwrap()
            .to_path_buf();
        dir.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    let name_of = |id: &str| -> String {
        meta["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let nodes = meta["resolve"]["nodes"].as_array().unwrap();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = members
        .iter()
        .filter(|id| capture_version::ROOTS.contains(&name_of(id).as_str()))
        .map(|id| (*id).to_owned())
        .collect();
    assert_eq!(queue.len(), 2, "both programs are workspace members");
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = nodes.iter().find(|n| n["id"] == id.as_str()).unwrap();
        for d in node["deps"].as_array().unwrap() {
            let counts = d["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["kind"].is_null() || k["kind"] == "build");
            let pkg = d["pkg"].as_str().unwrap();
            if counts && members.contains(pkg) {
                queue.push(pkg.to_owned());
            }
        }
    }
    let mut expected: Vec<String> = seen.iter().map(|id| dir_of(id)).collect();
    expected.push("Cargo.lock".into());
    expected.push("Cargo.toml".into());
    expected.sort();
    let baked: Vec<String> = mk_capture_api::CAPTURE_PATHS
        .split(';')
        .map(str::to_owned)
        .collect();
    assert_eq!(
        baked, expected,
        "the capture version's paths are exactly the server's and the phone app's sources"
    );
    for desktop_only in [
        "crates/mk-commands",
        "crates/mk-store",
        "apps/moneykeep-app",
    ] {
        assert!(!baked.iter().any(|p| p == desktop_only), "{desktop_only}");
    }
    // The constant has the shape the spec gives.
    let (pkg, hex) = mk_capture_api::CAPTURE_VERSION.split_once('+').unwrap();
    assert_eq!(pkg, env!("CARGO_PKG_VERSION"));
    assert_eq!(hex.len(), 12);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}
