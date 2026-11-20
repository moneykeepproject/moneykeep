//! `cargo xtask standalone` (crates-for-other-apps.md, "The check"): the crates other
//! Keep apps copy build, and their own tests pass, in a workspace that holds only them.
//!
//! 1. The copy set ([`layering::COPY_SET`]) is copied to a temporary folder, with
//!    `Cargo.lock` (the same versions as here) and the shared test files in
//!    `examples/` (see [`SHARED_TEST_FILES`]).
//! 2. A workspace `Cargo.toml` is written there with this workspace's
//!    `[workspace.package]`, `[workspace.dependencies]` (only the entries the set
//!    uses) and `[workspace.lints]`.
//! 3. `cargo build --workspace` and `cargo test --workspace` run there; the task fails
//!    if either fails, or if the build output holds a `warning:` line from a build
//!    script.
//!
//! Where the network or the toolchain make the build impossible, the task says it was
//! skipped and why; it never reports a pass it did not run. The temporary folder is
//! removed afterwards; when it cannot be (a file held open on Windows) the task names
//! it and still gives the build's result.
//!
//! The layering part of the check (no dependency from the set on an app or another
//! crate) is in `layering.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::layering::COPY_SET;
use crate::release;

/// Test files the set's tests read from outside the crates: `examples/fixtures`
/// (import files, with their provenance in `examples/README.md`), read by
/// `mk-import`'s and `mk-commands`' tests as `../../examples/fixtures`. Copied next to
/// the crates, at the same place, so those tests run unchanged.
pub const SHARED_TEST_FILES: &str = "examples";

/// The sections of the root `Cargo.toml` the temporary workspace takes as they are.
const KEPT_SECTIONS: &[&str] = &[
    "workspace.package",
    "workspace.lints.rust",
    "workspace.lints.clippy",
];

/// What the task found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Built and tested.
    Passed,
    /// Could not run, and why.
    Skipped(String),
}

pub fn check() -> Result<(), String> {
    match run()? {
        Outcome::Passed => {
            println!(
                "standalone: the copy set ({}) builds and its tests pass on its own",
                COPY_SET.join(", ")
            );
            Ok(())
        }
        Outcome::Skipped(why) => {
            println!("==> standalone: SKIPPED ({why}); the copy set was NOT built on its own");
            Ok(())
        }
    }
}

fn run() -> Result<Outcome, String> {
    let root = release::workspace_root()?;
    let root_manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    let mut used = BTreeSet::new();
    for c in COPY_SET {
        let path = root.join("crates").join(c).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        used.extend(workspace_deps_used(&text));
    }
    let manifest = standalone_manifest(&root_manifest, &used)?;

    // One fixed folder: the build output in target/standalone is kept per package
    // folder, so a new folder per run would leave a new copy there every time.
    let dir = std::env::temp_dir().join("mk-standalone");
    let result = build_in(&root, &dir, &manifest);
    if dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&dir)
    {
        println!(
            "standalone: the temporary folder {} could not be removed ({e}); remove it by hand",
            dir.display()
        );
    }
    result
}

/// Copy the set into `dir`, write `manifest` there, then build and test.
fn build_in(root: &Path, dir: &Path, manifest: &str) -> Result<Outcome, String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("cannot empty {}: {e}", dir.display()))?;
    }
    for c in COPY_SET {
        copy_tree(&root.join("crates").join(c), &dir.join("crates").join(c))?;
    }
    copy_tree(&root.join(SHARED_TEST_FILES), &dir.join(SHARED_TEST_FILES))?;
    std::fs::copy(root.join("Cargo.lock"), dir.join("Cargo.lock"))
        .map_err(|e| format!("cannot copy Cargo.lock: {e}"))?;
    std::fs::write(dir.join("Cargo.toml"), manifest)
        .map_err(|e| format!("cannot write the temporary Cargo.toml: {e}"))?;
    println!("standalone: the copy set is in {}", dir.display());

    // Its own target folder, kept between runs (later runs only rebuild what changed).
    let target = root.join("target").join("standalone");
    let cargo = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO"));
        c.args(args)
            .current_dir(dir)
            .env("CARGO_TARGET_DIR", &target)
            // No debug information: this workspace has none of MoneyKeep's
            // `[profile]` settings (which leave it out for dependencies), and with it
            // the build output is several gigabytes. Nothing here is debugged.
            .env("CARGO_PROFILE_DEV_DEBUG", "false")
            .env("CARGO_PROFILE_TEST_DEBUG", "false")
            // This task runs under `cargo run`; the temporary workspace is its own.
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_PKG_NAME");
        c
    };

    // The manifests must load (a path dependency outside the set fails here): an
    // error, never a skip. No download is needed.
    let meta = cargo(&["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"));
    let meta = match meta {
        Ok(m) => m,
        Err(e) => return Ok(Outcome::Skipped(e)),
    };
    if !meta.status.success() {
        return Err(format!(
            "the copy set does not load as a workspace of its own:\n{}",
            String::from_utf8_lossy(&meta.stderr).trim()
        ));
    }
    // Downloads (the network): a failure here is a skip, with cargo's reason.
    println!("==> standalone: cargo fetch");
    let fetch = cargo(&["fetch"])
        .output()
        .map_err(|e| format!("cannot run cargo fetch: {e}"))?;
    if !fetch.status.success() {
        return Ok(Outcome::Skipped(format!(
            "the dependencies could not be downloaded: {}",
            last_lines(&String::from_utf8_lossy(&fetch.stderr), 3)
        )));
    }

    println!("==> standalone: cargo build --workspace");
    let build = run_captured(cargo(&["build", "--workspace"]))?;
    if !build.0 {
        return Err("cargo build --workspace failed in the temporary workspace".into());
    }
    let warnings = build_script_warnings(&build.1);
    if !warnings.is_empty() {
        return Err(format!(
            "the build printed warnings from build scripts (crates-for-other-apps.md rule 3):\n{}",
            warnings.join("\n")
        ));
    }
    println!("==> standalone: cargo test --workspace (build)");
    let test_build = run_captured(cargo(&["test", "--workspace", "--no-run"]))?;
    if !test_build.0 {
        return Err("cargo test --workspace failed to build in the temporary workspace".into());
    }
    let warnings = build_script_warnings(&test_build.1);
    if !warnings.is_empty() {
        return Err(format!(
            "the test build printed warnings from build scripts (crates-for-other-apps.md rule 3):\n{}",
            warnings.join("\n")
        ));
    }
    println!("==> standalone: cargo test --workspace");
    let status = cargo(&["test", "--workspace"])
        .status()
        .map_err(|e| format!("cannot run cargo test: {e}"))?;
    if !status.success() {
        return Err("cargo test --workspace failed in the temporary workspace".into());
    }
    Ok(Outcome::Passed)
}

/// Run `cmd`, showing its error output as it is and returning (success, that output).
fn run_captured(mut cmd: Command) -> Result<(bool, String), String> {
    let out = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    eprint!("{text}");
    Ok((out.status.success(), text))
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines
        .get(lines.len().saturating_sub(n)..)
        .unwrap_or(&[])
        .join(" / ")
}

/// Copy a folder, leaving out build output (`target`).
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot list {}: {e}", from.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("cannot list {}: {e}", from.display()))?;
        let (src, dest): (PathBuf, PathBuf) = (e.path(), to.join(e.file_name()));
        let kind = e
            .file_type()
            .map_err(|e| format!("cannot read {}: {e}", src.display()))?;
        if kind.is_dir() {
            if e.file_name() == "target" {
                continue;
            }
            copy_tree(&src, &dest)?;
        } else if kind.is_file() {
            std::fs::copy(&src, &dest)
                .map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// The `[...]` header of a line, if it is one.
fn section(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .filter(|s| !s.starts_with('['))
        .map(str::trim)
}

fn is_dependency_section(name: &str) -> bool {
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .any(|d| name == *d || name.ends_with(&format!(".{d}")))
}

/// The key of a `key = ...` or `key.sub = ...` line.
fn entry_key(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let (left, _) = t.split_once('=')?;
    let key = left.trim().split('.').next()?.trim();
    (!key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(key)
}

/// The names a crate's manifest takes from `[workspace.dependencies]`
/// (`name.workspace = true` or `name = { workspace = true, ... }`), in any
/// dependency section.
pub fn workspace_deps_used(manifest: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        if let Some(s) = section(line) {
            in_deps = is_dependency_section(s);
            continue;
        }
        if !in_deps {
            continue;
        }
        let Some(key) = entry_key(line) else { continue };
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.starts_with(&format!("{key}.workspace=true"))
            || compact.contains("workspace=true")
        {
            out.insert(key.to_owned());
        }
    }
    out
}

/// The temporary workspace's `Cargo.toml`: the set as its members, and from the root
/// manifest `root` the sections in [`KEPT_SECTIONS`] as they are and the
/// `[workspace.dependencies]` entries named in `used` (an entry that spans lines is
/// taken whole).
pub fn standalone_manifest(root: &str, used: &BTreeSet<String>) -> Result<String, String> {
    let members: Vec<String> = COPY_SET.iter().map(|c| format!("\"crates/{c}\"")).collect();
    let mut out = format!(
        "# Written by `cargo xtask standalone` (crates-for-other-apps.md): the crates other\n\
         # Keep apps copy, in a workspace of their own.\n\
         [workspace]\nresolver = \"3\"\nmembers = [{}]\n",
        members.join(", ")
    );
    let mut kept: Vec<String> = Vec::new();
    let mut deps: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;
    // An unfinished dependency entry (open braces or brackets) and its key.
    let mut open: Option<(String, String, i32)> = None;
    for line in root.lines() {
        if let Some((key, mut text, depth)) = open.take() {
            text.push('\n');
            text.push_str(line);
            let depth = depth + nesting(line);
            if depth > 0 {
                open = Some((key, text, depth));
            } else {
                deps.push((key, text));
            }
            continue;
        }
        if let Some(s) = section(line) {
            current = Some(s.to_owned());
            if KEPT_SECTIONS.contains(&s) {
                kept.push(format!("\n[{s}]"));
            }
            continue;
        }
        match current.as_deref() {
            Some(s) if KEPT_SECTIONS.contains(&s) => {
                if let Some(last) = kept.last_mut() {
                    last.push('\n');
                    last.push_str(line);
                }
            }
            Some("workspace.dependencies") => {
                let Some(key) = entry_key(line) else { continue };
                let depth = nesting(line);
                if depth > 0 {
                    open = Some((key.to_owned(), line.to_owned(), depth));
                } else {
                    deps.push((key.to_owned(), line.to_owned()));
                }
            }
            _ => {}
        }
    }
    for s in KEPT_SECTIONS {
        if !kept.iter().any(|k| k.starts_with(&format!("\n[{s}]"))) {
            return Err(format!("the root Cargo.toml has no [{s}]"));
        }
    }
    let mut missing: Vec<&str> = Vec::new();
    out.push_str("\n[workspace.dependencies]\n");
    for name in used {
        match deps.iter().find(|(k, _)| k == name) {
            Some((_, text)) => {
                out.push_str(text);
                out.push('\n');
            }
            None => missing.push(name),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "the copy set uses workspace dependencies the root Cargo.toml does not define: {}",
            missing.join(", ")
        ));
    }
    for k in kept {
        out.push_str(k.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// Open minus closed braces and brackets on a line (outside strings and comments,
/// which is enough for Cargo.toml dependency entries).
fn nesting(line: &str) -> i32 {
    let mut depth = 0;
    let mut in_str = false;
    for c in line.chars() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => break,
            '{' | '[' if !in_str => depth += 1,
            '}' | ']' if !in_str => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// The `warning:` lines cargo prints for build scripts (`warning: <package>@<version>:
/// ...`), as opposed to compiler warnings.
pub fn build_script_warnings(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|l| {
            l.strip_prefix("warning: ")
                .and_then(|rest| rest.split_once(": "))
                .is_some_and(|(who, _)| {
                    !who.contains(char::is_whitespace)
                        && who
                            .split_once('@')
                            .is_some_and(|(name, version)| !name.is_empty() && !version.is_empty())
                })
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn root() -> PathBuf {
        release::workspace_root().unwrap()
    }

    fn used_by_set() -> BTreeSet<String> {
        let mut used = BTreeSet::new();
        for c in COPY_SET {
            let text =
                std::fs::read_to_string(root().join("crates").join(c).join("Cargo.toml")).unwrap();
            used.extend(workspace_deps_used(&text));
        }
        used
    }

    #[test]
    fn workspace_deps_are_read_from_every_dependency_section() {
        let m = r#"[package]
name = "x"
version.workspace = true

[dependencies]
serde.workspace = true
time = { workspace = true, features = ["local-offset"] }
ureq = { version = "3.4", default-features = false }
# mk-ui.workspace = true

[dev-dependencies]
tempfile.workspace = true

[build-dependencies]
sha2.workspace = true

[target.'cfg(windows)'.dependencies]
windows-sys.workspace = true

[lints]
workspace = true
"#;
        let got: Vec<String> = workspace_deps_used(m).into_iter().collect();
        assert_eq!(got, ["serde", "sha2", "tempfile", "time", "windows-sys"]);
    }

    /// crates-for-other-apps.md "The check" 2: the temporary workspace's manifest has
    /// the seven crates as members, this workspace's package settings and lints as
    /// they are, and only the dependency entries the set uses.
    #[test]
    fn the_temporary_manifest() {
        let text = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
        let used = used_by_set();
        let m = standalone_manifest(&text, &used).unwrap();
        let members = COPY_SET
            .iter()
            .map(|c| format!("\"crates/{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(m.contains(&format!("members = [{members}]")), "{m}");
        for s in KEPT_SECTIONS {
            assert_eq!(m.matches(&format!("[{s}]")).count(), 1, "{s}: {m}");
        }
        // The package settings and the lints, exactly as in the root manifest.
        for line in [
            "edition = \"2024\"",
            "unwrap_used = \"deny\"",
            "unsafe_code = \"forbid\"",
        ] {
            assert!(m.contains(line), "{line}: {m}");
        }
        assert!(m.contains("mk-core = { path = \"crates/mk-core\" }"), "{m}");
        assert!(m.contains("rusqlite = "), "{m}");
        for not_used in [
            "slint", "mk-ui", "mk-views", "mk-mcp", "clap", "brotli", "mk-dpapi",
        ] {
            assert!(
                !m.lines().any(|l| l.starts_with(&format!("{not_used} "))),
                "{not_used}: {m}"
            );
        }
        // No other section of the root manifest (profiles, the member list).
        assert!(!m.contains("[profile"), "{m}");
        assert!(!m.contains("apps/"), "{m}");
        // Each used dependency has exactly one entry.
        for u in &used {
            assert_eq!(
                m.lines()
                    .filter(|l| entry_key(l) == Some(u.as_str())
                        && l.contains('=')
                        && !l.starts_with(' '))
                    .count(),
                1,
                "{u}"
            );
        }
    }

    #[test]
    fn a_multi_line_entry_is_taken_whole_and_a_missing_one_fails() {
        let root = "[workspace]\nmembers = [\n  \"a\",\n]\n\n[workspace.package]\nversion = \"1.0.0\"\n\n\
                    [workspace.dependencies]\n# a comment\nfoo = { version = \"1\", features = [\n  \"x\",\n] }\nbar = \"2\"\n\n\
                    [workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n[workspace.lints.clippy]\npanic = \"deny\"\n\n[profile.dev]\nopt-level = 1\n";
        let used: BTreeSet<String> = ["foo".to_owned()].into();
        let m = standalone_manifest(root, &used).unwrap();
        assert!(
            m.contains("foo = { version = \"1\", features = [\n  \"x\",\n] }"),
            "{m}"
        );
        assert!(!m.contains("bar ="), "{m}");
        assert!(!m.contains("opt-level"), "{m}");
        assert!(!m.contains("\"a\""), "{m}");
        let used: BTreeSet<String> = ["nope".to_owned()].into();
        let e = standalone_manifest(root, &used).unwrap_err();
        assert!(e.contains("nope"), "{e}");
        let e = standalone_manifest("[workspace.dependencies]\n", &BTreeSet::new()).unwrap_err();
        assert!(e.contains("[workspace.package]"), "{e}");
    }

    /// crates-for-other-apps.md "The check" 3 and acceptance test 3: a build
    /// script's warning fails the task; compiler warnings are not build-script
    /// warnings.
    #[test]
    fn build_script_warnings_are_found() {
        let out = "   Compiling mk-capture-api v0.1.3 (/tmp/x/crates/mk-capture-api)\n\
                   warning: mk-capture-api@0.1.3: the capture version covers all of apps/ and crates/ (moneykeep-server is not a workspace package)\n\
                   warning: unused import: `std::fs`\n\
                   warning: `mk-core` (lib) generated 1 warning\n\
                   warning: build failed, waiting for other jobs to finish...\n\
                       Finished `dev` profile";
        assert_eq!(
            build_script_warnings(out),
            [
                "warning: mk-capture-api@0.1.3: the capture version covers all of apps/ and crates/ (moneykeep-server is not a workspace package)"
            ]
        );
        assert!(build_script_warnings("   Compiling x\n    Finished").is_empty());
    }

    #[test]
    fn the_copy_leaves_out_build_output() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("from");
        std::fs::create_dir_all(from.join("src")).unwrap();
        std::fs::create_dir_all(from.join("target").join("debug")).unwrap();
        std::fs::write(from.join("src").join("lib.rs"), "//").unwrap();
        std::fs::write(from.join("target").join("debug").join("x"), "x").unwrap();
        copy_tree(&from, &d.path().join("to")).unwrap();
        assert!(d.path().join("to").join("src").join("lib.rs").is_file());
        assert!(!d.path().join("to").join("target").exists());
    }

    /// `path` with `.` and `..` worked out (no file system access).
    fn lexical(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                other => out.push(other),
            }
        }
        out
    }

    /// The shared test files the set's tests read are the ones copied: every
    /// `../../…` path in the set's sources that leaves its crate (from the crate's
    /// folder for `CARGO_MANIFEST_DIR` paths, from the file's folder for
    /// `include_str!` and the like) is under [`SHARED_TEST_FILES`].
    #[test]
    fn the_set_reads_only_the_shared_test_files_outside_itself() {
        let root = lexical(&root());
        let mut outside = BTreeSet::new();
        let mut seen = 0;
        for c in COPY_SET {
            let krate = root.join("crates").join(c);
            let mut files = Vec::new();
            rust_files(&krate, &mut files);
            for f in &files {
                let text = std::fs::read_to_string(f).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    for (i, _) in line.match_indices("../../") {
                        let before = &line[..i];
                        let near = lines[n.saturating_sub(1)..(n + 2).min(lines.len())].join(" ");
                        let base = if before.ends_with("{}/") || near.contains("CARGO_MANIFEST_DIR")
                        {
                            krate.clone()
                        } else {
                            f.parent().unwrap().to_path_buf()
                        };
                        let rel: String = line[i..]
                            .chars()
                            .take_while(|ch| !matches!(ch, '"' | '{' | ')' | ' '))
                            .collect();
                        let target = lexical(&base.join(&rel));
                        seen += 1;
                        if !target.starts_with(&krate) {
                            outside.insert(
                                target
                                    .strip_prefix(&root)
                                    .map(|r| {
                                        r.components()
                                            .next()
                                            .unwrap()
                                            .as_os_str()
                                            .to_string_lossy()
                                            .into_owned()
                                    })
                                    .unwrap_or_else(|_| target.display().to_string()),
                            );
                        }
                    }
                }
            }
        }
        assert!(seen > 5, "the sources were read");
        assert_eq!(outside.into_iter().collect::<Vec<_>>(), [SHARED_TEST_FILES]);
        assert!(root.join(SHARED_TEST_FILES).join("fixtures").is_dir());
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if e.file_name() != "target" {
                    rust_files(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// The phone-capture tests that ran the real capture server in mk-connect, and
    /// where each went (crates-for-other-apps.md acceptance test 4).
    const MOVED: &[(&str, &[&str])] = &[
        (
            "capture.rs",
            &[
                "at11_collect",
                "at12_unknown_category_makes_a_draft",
                "at13_lists_hold_names_only",
                "test_pair_and_devices",
            ],
        ),
        (
            "capture_mileage.rs",
            &[
                "m11_desktop_takes_trips_from_the_phone",
                "m13_lists_hold_active_vehicles_and_saved_trips",
                "mileage_r2_trips_arrive_tagged",
            ],
        ),
    ];

    /// crates-for-other-apps.md acceptance test 4: every phone-capture case that was
    /// tested in mk-connect is still a test, in crates/mk-capture-tests, against the
    /// real capture server; mk-connect's tests no longer reach the server.
    #[test]
    fn at4_moved_capture_tests_are_still_tests() {
        let new = root().join("crates/mk-capture-tests/tests");
        for (file, tests) in MOVED {
            let text = std::fs::read_to_string(new.join(file)).unwrap();
            assert!(
                text.contains("moneykeep_server::testing::{LocalServer, call}"),
                "{file}"
            );
            for t in *tests {
                assert!(
                    text.contains(&format!("#[test]\nfn {t}()")),
                    "{t} is a test in crates/mk-capture-tests/tests/{file}"
                );
            }
            assert!(
                !root().join("crates/mk-connect/tests").join(file).exists(),
                "{file}"
            );
        }
        let mut files = Vec::new();
        rust_files(&root().join("crates/mk-connect"), &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            assert!(!text.contains("moneykeep_server"), "{}", f.display());
        }
    }
}
