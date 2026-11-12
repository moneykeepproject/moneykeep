//! The capture version (release-packaging.md "Capture version"): which sources the
//! capture server program and the phone app are built from, and a short hash of them.
//!
//! Shared by `crates/mk-capture-api/build.rs` (which bakes `CAPTURE_VERSION` into
//! the server and the desktop), `cargo xtask release` (which writes it into
//! `version.json`) and the tests, through `#[path]` includes. It runs at build time
//! only: nothing here is part of the shipped library.
//!
//! - The paths come from the dependency graph (`cargo metadata --no-deps`): the
//!   folders of `moneykeep-server`, `moneykeep-capture` and every workspace crate
//!   they reach through normal or build dependencies (any target), plus
//!   `Cargo.lock` and the root `Cargo.toml` (its workspace settings, such as the
//!   version and lints, change the built server; release-prep.md §4).
//! - With git, and when none of those paths has uncommitted changes, the hash is
//!   over their tree (and blob) ids in `HEAD`. Otherwise (no git, or changes not yet
//!   committed) it is over the files' contents (git's ignore rules apply when git is
//!   there).
//! - `capture_version` = `<server package version>+<first 12 hex of the SHA-256>`.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The two programs whose sources make up the capture version.
pub const ROOTS: [&str; 2] = ["moneykeep-server", "moneykeep-capture"];

/// `cargo metadata --no-deps` for the workspace of `manifest`.
pub fn metadata(cargo: &std::ffi::OsStr, manifest: &Path) -> Result<Value, String> {
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad cargo metadata: {e}"))
}

fn package_dir(pkg: &Value) -> Option<PathBuf> {
    pkg.get("manifest_path")
        .and_then(Value::as_str)
        .and_then(|m| Path::new(m).parent().map(Path::to_path_buf))
}

/// The workspace folder of `metadata`.
pub fn workspace_root(meta: &Value) -> Option<PathBuf> {
    meta.get("workspace_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// The server package's version (the first part of the capture version).
pub fn server_version(meta: &Value) -> Option<String> {
    meta.get("packages")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(ROOTS[0]))?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// The workspace packages the two programs are built from (themselves included),
/// by name, through normal and build dependencies on path crates.
pub fn capture_packages(meta: &Value) -> Result<BTreeSet<String>, String> {
    let packages = meta
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("cargo metadata has no packages")?;
    // Workspace package folder -> (name, its path dependencies' folders).
    let mut by_dir: BTreeMap<PathBuf, (String, Vec<PathBuf>)> = BTreeMap::new();
    for p in packages {
        let (Some(name), Some(dir)) = (p.get("name").and_then(Value::as_str), package_dir(p))
        else {
            continue;
        };
        let deps = p
            .get("dependencies")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|d| d.get("kind").and_then(Value::as_str) != Some("dev"))
            .filter_map(|d| d.get("path").and_then(Value::as_str).map(PathBuf::from))
            .collect();
        by_dir.insert(dir, (name.to_owned(), deps));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = Vec::new();
    for root in ROOTS {
        let dir = by_dir
            .iter()
            .find(|(_, (n, _))| n == root)
            .map(|(d, _)| d.clone())
            .ok_or_else(|| format!("{root} is not a workspace package"))?;
        queue.push(dir);
    }
    while let Some(dir) = queue.pop() {
        let Some((name, deps)) = by_dir.get(&dir) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        queue.extend(deps.iter().cloned());
    }
    Ok(seen)
}

/// The paths (relative to the workspace folder, `/`-separated, sorted) the capture
/// version covers: the packages' folders, `Cargo.lock` and the root `Cargo.toml`.
pub fn capture_paths(meta: &Value) -> Result<Vec<String>, String> {
    let root = workspace_root(meta).ok_or("cargo metadata has no workspace_root")?;
    let names = capture_packages(meta)?;
    let mut out: Vec<String> = Vec::new();
    for p in meta
        .get("packages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(name) = p.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !names.contains(name) {
            continue;
        }
        let dir = package_dir(p).ok_or("a package has no manifest_path")?;
        let rel = dir
            .strip_prefix(&root)
            .map_err(|_| format!("{name} is outside the workspace folder"))?;
        out.push(slashes(rel));
    }
    out.push("Cargo.lock".to_owned());
    out.push("Cargo.toml".to_owned());
    out.sort();
    out.dedup();
    Ok(out)
}

fn slashes(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// How the hash was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Tree ids from git's `HEAD`.
    GitTrees,
    /// File contents (uncommitted changes, or no git).
    Contents,
}

/// The hash (64 hex) over `paths` under `root`, how it was made, and the files and
/// folders a build script should watch.
pub fn capture_hash(
    root: &Path,
    paths: &[String],
    use_git: bool,
) -> Result<(String, Mode, Vec<PathBuf>), String> {
    let mut watch: Vec<PathBuf> = paths.iter().map(|p| root.join(p)).collect();
    let in_git = use_git && git(root, &["rev-parse", "--git-dir"]).is_some();
    if in_git {
        if let Some(dir) = git(root, &["rev-parse", "--absolute-git-dir"]) {
            let dir = PathBuf::from(String::from_utf8_lossy(&dir).trim().to_owned());
            watch.push(dir.join("HEAD"));
            watch.push(dir.join("index"));
            watch.push(dir.join("packed-refs"));
            if let Some(head) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
                watch.push(dir.join(String::from_utf8_lossy(&head).trim()));
            }
        }
        let mut status = vec![
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
        ];
        status.extend(paths.iter().map(String::as_str));
        let clean = git(root, &status).is_some_and(|o| o.is_empty());
        if clean {
            let specs: Vec<String> = paths.iter().map(|p| format!("HEAD:./{p}")).collect();
            let mut args = vec!["rev-parse"];
            args.extend(specs.iter().map(String::as_str));
            if let Some(out) = git(root, &args) {
                let ids: Vec<String> = String::from_utf8_lossy(&out)
                    .lines()
                    .map(str::to_owned)
                    .collect();
                if ids.len() == paths.len() {
                    let mut h = Sha256::new();
                    for (p, id) in paths.iter().zip(&ids) {
                        h.update(format!("git {p} {id}\n").as_bytes());
                    }
                    return Ok((hex(&h.finalize()), Mode::GitTrees, watch));
                }
            }
        }
    }
    // File contents.
    let files = if in_git {
        let mut args = vec![
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        let out = git(root, &args).ok_or("git ls-files failed")?;
        let mut v: Vec<String> = out
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        let mut v = Vec::new();
        for p in paths {
            walk(root, &root.join(p), &mut v)?;
        }
        v.sort();
        v.dedup();
        v
    };
    let mut h = Sha256::new();
    for rel in &files {
        let path = root.join(rel);
        // A tracked file deleted in the working tree is simply absent.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        h.update(format!("file {rel} {}\n", bytes.len()).as_bytes());
        h.update(&bytes);
        h.update(b"\n");
    }
    Ok((hex(&h.finalize()), Mode::Contents, watch))
}

/// Folders never part of the sources (build output, the phone app's Trunk output).
const SKIPPED: [&str; 4] = ["target", "dist", "node_modules", ".git"];

fn walk(root: &Path, path: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if meta.is_file() {
        let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
        if rel.file_name().is_some_and(|n| n == ".DS_Store") {
            return Ok(());
        }
        out.push(slashes(rel));
    } else if meta.is_dir() {
        if path
            .file_name()
            .is_some_and(|n| SKIPPED.iter().any(|s| n == *s))
        {
            return Ok(());
        }
        let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for e in entries {
            let e = e.map_err(|e| e.to_string())?;
            walk(root, &e.path(), out)?;
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The package version of a capture version (`0.9.3` of `0.9.3+…`).
pub fn package_version_of(v: &str) -> &str {
    v.split_once('+').map_or(v, |(p, _)| p)
}

/// `<package version>+<first 12 hex>`.
pub fn capture_version(package_version: &str, hash: &str) -> String {
    format!("{package_version}+{}", hash.get(..12).unwrap_or(hash))
}

/// What [`compute`] found.
#[derive(Debug, Clone)]
pub struct Computed {
    pub capture_version: String,
    /// The paths covered (relative, `/`-separated).
    pub paths: Vec<String>,
    /// What a build script watches.
    pub watch: Vec<PathBuf>,
    /// Why the fallback path list was used, if it was.
    pub note: Option<String>,
}

/// Everything at once for the workspace of `manifest`. Without usable metadata the
/// paths fall back to all of `apps/`, `crates/` and `Cargo.lock` (more updates
/// offered, never fewer), with a note.
pub fn compute(
    cargo: &std::ffi::OsStr,
    manifest: &Path,
    fallback_root: &Path,
    fallback_version: &str,
) -> Result<Computed, String> {
    let (root, paths, version, note) = match metadata(cargo, manifest).and_then(|m| {
        let root = workspace_root(&m).ok_or("cargo metadata has no workspace_root")?;
        let paths = capture_paths(&m)?;
        let version = server_version(&m).unwrap_or_else(|| fallback_version.to_owned());
        Ok((root, paths, version))
    }) {
        Ok((r, p, v)) => (r, p, v, None),
        Err(e) => (
            fallback_root.to_path_buf(),
            vec![
                "Cargo.lock".into(),
                "Cargo.toml".into(),
                "apps".into(),
                "crates".into(),
            ],
            fallback_version.to_owned(),
            Some(format!(
                "the capture version covers all of apps/ and crates/ ({e})"
            )),
        ),
    };
    let (hash, _, watch) = capture_hash(&root, &paths, true)?;
    Ok(Computed {
        capture_version: capture_version(&version, &hash),
        paths,
        watch,
        note,
    })
}
