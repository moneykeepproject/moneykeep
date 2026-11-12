//! `THIRD-PARTY-LICENSES.txt` (security-review-1 B4): every crate compiled into the
//! shipped programs, with its version and licence, and the licence texts (with their
//! copyright lines) the crates ship, each distinct text once with the crates that
//! carry it.
//!
//! Made from `cargo metadata` (the dependency graph, filtered to the platform each
//! program is built for) and the licence files in each crate's own source folder
//! (the registry copy cargo already downloaded): no new dependency. The shipped
//! programs are the desktop app, the capture server, the CLI and keep-mcp for this
//! computer's platform, and the phone app (`moneykeep-capture`, built for
//! `wasm32-unknown-unknown`) that the capture server carries. Normal dependencies
//! are followed (build scripts' dependencies and test-only ones are not compiled
//! into a program). A crate that ships no licence file is listed with its licence
//! and authors; the MIT and Apache-2.0 texts it then falls under are given at the end
//! (from MoneyKeep's own `LICENSE-MIT` body and `LICENSE-APACHE`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The file shipped beside the programs.
pub const FILE: &str = "THIRD-PARTY-LICENSES.txt";

/// The phone app's package and the target it is built for.
pub const PHONE: (&str, &str) = ("moneykeep-capture", "wasm32-unknown-unknown");

/// One crate in the list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Crate {
    pub name: String,
    pub version: String,
    /// The SPDX expression from its manifest (`unknown` when it gives none).
    pub licence: String,
    pub authors: Vec<String>,
    /// MoneyKeep's own (a workspace member).
    pub own: bool,
    /// The licence files in its source folder: (file name, text).
    pub files: Vec<(String, String)>,
}

/// This computer's platform (`rustc -vV`'s host).
pub fn host_triple() -> Result<String, String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("cannot run rustc: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV gave no host".to_owned())
}

/// `cargo metadata` for `root`, its graph filtered to `target`.
pub fn metadata(root: &Path, target: &str) -> Result<Value, String> {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            target,
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// The package ids reached from the packages named `roots` by normal dependencies.
pub fn reached(meta: &Value, roots: &[&str]) -> Result<BTreeSet<String>, String> {
    let packages = meta["packages"].as_array().ok_or("no packages")?;
    let nodes = meta
        .pointer("/resolve/nodes")
        .and_then(Value::as_array)
        .ok_or("no resolve")?;
    let by_id: BTreeMap<String, &Value> = nodes.iter().map(|n| (text(&n["id"]), n)).collect();
    let workspace: BTreeSet<String> = meta["workspace_members"]
        .as_array()
        .map(|a| a.iter().map(text).collect())
        .unwrap_or_default();
    let mut todo: Vec<String> = packages
        .iter()
        .filter(|p| {
            workspace.contains(&text(&p["id"])) && roots.contains(&p["name"].as_str().unwrap_or(""))
        })
        .map(|p| text(&p["id"]))
        .collect();
    if todo.len() != roots.len() {
        return Err(format!(
            "not every program was found in cargo metadata: {roots:?}"
        ));
    }
    let mut seen = BTreeSet::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = by_id.get(&id) else { continue };
        for d in node["deps"].as_array().into_iter().flatten() {
            let normal = d["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            if normal {
                todo.push(text(&d["pkg"]));
            }
        }
    }
    Ok(seen)
}

/// Whether `name` is a licence file's name (`LICENSE*`, `LICENCE*`, `COPYING*`,
/// `COPYRIGHT*`, `NOTICE*`, `UNLICENSE*`).
pub fn is_licence_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "license",
        "licence",
        "copying",
        "copyright",
        "notice",
        "unlicense",
    ]
    .iter()
    .any(|p| n.starts_with(p))
}

/// The licence files in a crate's folder (and its `LICENSES/` folder), sorted.
fn licence_files(dir: &Path, declared: Option<&str>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |rel: String, path: PathBuf| {
        if out.iter().any(|(r, _)| *r == rel) {
            return;
        }
        if let Ok(t) = std::fs::read_to_string(&path) {
            out.push((rel, t.replace("\r\n", "\n")));
        }
    };
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && is_licence_file(&name) {
                add(name, e.path());
            }
        }
    }
    let sub = dir.join("LICENSES");
    if let Ok(rd) = std::fs::read_dir(&sub) {
        for e in rd.flatten() {
            if e.path().is_file() {
                add(
                    format!("LICENSES/{}", e.file_name().to_string_lossy()),
                    e.path(),
                );
            }
        }
    }
    if let Some(f) = declared {
        add(f.replace('\\', "/"), dir.join(f));
    }
    out.sort();
    out
}

/// The crates of `ids` in `meta`.
pub fn crates_of(meta: &Value, ids: &BTreeSet<String>) -> Vec<Crate> {
    let workspace: BTreeSet<String> = meta["workspace_members"]
        .as_array()
        .map(|a| a.iter().map(text).collect())
        .unwrap_or_default();
    let mut out: Vec<Crate> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| ids.contains(&text(&p["id"])))
        .map(|p| {
            let own = workspace.contains(&text(&p["id"]));
            let dir = Path::new(p["manifest_path"].as_str().unwrap_or("."))
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            Crate {
                name: text(&p["name"]),
                version: text(&p["version"]),
                licence: p["license"]
                    .as_str()
                    .map_or_else(|| "unknown".to_owned(), str::to_owned),
                authors: p["authors"]
                    .as_array()
                    .map(|a| a.iter().map(text).collect())
                    .unwrap_or_default(),
                own,
                files: if own {
                    Vec::new()
                } else {
                    licence_files(&dir, p["license_file"].as_str())
                },
            }
        })
        .collect();
    out.sort();
    out
}

/// Every crate compiled into the shipped programs: the programs on `host`, and the
/// phone app on its own target.
pub fn shipped(root: &Path, host: &str) -> Result<Vec<Crate>, String> {
    let programs: Vec<&str> = crate::release::PROGRAMS.iter().map(|(p, _)| *p).collect();
    let meta = metadata(root, host)?;
    let mut all = crates_of(&meta, &reached(&meta, &programs)?);
    let phone_meta = metadata(root, PHONE.1)?;
    for c in crates_of(&phone_meta, &reached(&phone_meta, &[PHONE.0])?) {
        if !all
            .iter()
            .any(|a| a.name == c.name && a.version == c.version)
        {
            all.push(c);
        }
    }
    all.sort();
    Ok(all)
}

fn rule(title: &str) -> String {
    format!("\n{}\n{title}\n{}\n", "=".repeat(78), "=".repeat(78))
}

/// The file's text.
pub fn render(version: &str, crates: &[Crate], mit: &str, apache: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("Third-party licences of MoneyKeep {version}\n\n"));
    s.push_str(
        "This file lists every crate compiled into the programs MoneyKeep ships (the\n\
         desktop app, the capture server with the phone app it carries, the command\n\
         line and keep-mcp), with its version and licence, followed by the licence\n\
         texts those crates ship, with their copyright lines. Where a crate offers a\n\
         choice of licences, MoneyKeep uses it under any one of them that MoneyKeep's\n\
         licence policy allows. Slint is used under the Slint Royalty-free License 2.0\n\
         (see NOTICE). MoneyKeep's own crates are under MIT OR Apache-2.0 (LICENSE-MIT,\n\
         LICENSE-APACHE). Made by `cargo xtask release` from `cargo metadata` and the\n\
         crates' own licence files.\n",
    );
    s.push_str(&rule("Crates"));
    for c in crates {
        let own = if c.own { "  (MoneyKeep's own)" } else { "" };
        s.push_str(&format!("{} {}: {}{own}\n", c.name, c.version, c.licence));
    }
    // Each distinct text once, with the crates that carry it.
    let mut texts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut bare: Vec<&Crate> = Vec::new();
    for c in crates.iter().filter(|c| !c.own) {
        if c.files.is_empty() {
            bare.push(c);
        }
        for (file, t) in &c.files {
            let key = t.trim().to_owned();
            if !texts.contains_key(&key) {
                order.push(key.clone());
            }
            texts
                .entry(key)
                .or_default()
                .push(format!("{} {} ({file})", c.name, c.version));
        }
    }
    s.push_str(&rule("Licence texts shipped by the crates"));
    for key in &order {
        let users = texts.get(key).cloned().unwrap_or_default();
        s.push_str(&format!("\n--- {}\n\n{key}\n", users.join(", ")));
    }
    if !bare.is_empty() {
        s.push_str(&rule("Crates that ship no licence file"));
        s.push_str(
            "These crates name their licence in their manifest but carry no licence\n\
             file. The copyright holders are the crates' authors, listed here. The MIT\n\
             and Apache-2.0 texts follow; for any other licence, its standard text is\n\
             at https://spdx.org/licenses/.\n\n",
        );
        for c in &bare {
            let authors = if c.authors.is_empty() {
                "the crate's authors".to_owned()
            } else {
                c.authors.join(", ")
            };
            s.push_str(&format!(
                "{} {}: {}; copyright {authors}\n",
                c.name, c.version, c.licence
            ));
        }
        s.push_str(&rule("MIT License (for the crates above under MIT)"));
        s.push_str(&format!(
            "\nCopyright (c) the authors named above for each crate\n\n{}\n",
            mit.trim()
        ));
        s.push_str(&rule(
            "Apache License 2.0 (for the crates above under Apache-2.0)",
        ));
        s.push_str(&format!("\n{}\n", apache.trim()));
    }
    s
}

/// The MIT text without MoneyKeep's own copyright line (its first lines up to the
/// permission notice).
pub fn mit_body(licence_mit: &str) -> String {
    let t = licence_mit.replace("\r\n", "\n");
    match t.find("Permission is hereby granted") {
        Some(i) => {
            let title = t
                .lines()
                .next()
                .filter(|l| l.contains("MIT"))
                .unwrap_or("MIT License");
            format!("{title}\n\n{}", t.get(i..).unwrap_or_default())
        }
        None => t,
    }
}

/// Write `THIRD-PARTY-LICENSES.txt` into `folder`; returns its path and how many
/// crates it lists.
pub fn write(root: &Path, folder: &Path, version: &str) -> Result<(PathBuf, usize), String> {
    let crates = shipped(root, &host_triple()?)?;
    let read = |f: &str| {
        std::fs::read_to_string(root.join(f)).map_err(|e| format!("cannot read {f}: {e}"))
    };
    let body = render(
        version,
        &crates,
        &mit_body(&read("LICENSE-MIT")?),
        &read("LICENSE-APACHE")?,
    );
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    let path = folder.join(FILE);
    // Windows line ends: the file is opened in Notepad from About.
    std::fs::write(&path, body.replace('\n', "\r\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((path, crates.len()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn licence_file_names() {
        for n in [
            "LICENSE",
            "LICENSE-MIT",
            "license.txt",
            "LICENCE",
            "COPYING",
            "NOTICE",
        ] {
            assert!(is_licence_file(n), "{n}");
        }
        for n in ["README.md", "Cargo.toml", "src"] {
            assert!(!is_licence_file(n), "{n}");
        }
    }

    #[test]
    fn mit_body_drops_the_copyright_line() {
        let b =
            mit_body("MIT License\n\nCopyright (c) 2026 X\n\nPermission is hereby granted, free\n");
        assert!(b.starts_with("MIT License\n\nPermission is hereby granted"));
        assert!(!b.contains("2026 X"));
    }

    #[test]
    fn render_lists_crates_and_each_text_once() {
        let c = |name: &str, files: Vec<(String, String)>| Crate {
            name: name.into(),
            version: "1.0.0".into(),
            licence: "MIT".into(),
            authors: vec!["A <a@example.org>".into()],
            own: false,
            files,
        };
        let same = (
            "LICENSE".to_owned(),
            "Copyright (c) A\n\nPermission".to_owned(),
        );
        let text = render(
            "0.1.0",
            &[
                c("a", vec![same.clone()]),
                c("b", vec![same]),
                c("c", Vec::new()),
            ],
            "MIT License\n\nPermission",
            "Apache License",
        );
        assert!(text.contains("a 1.0.0: MIT\nb 1.0.0: MIT\nc 1.0.0: MIT\n"));
        assert_eq!(text.matches("Copyright (c) A\n\nPermission").count(), 1);
        assert!(text.contains("--- a 1.0.0 (LICENSE), b 1.0.0 (LICENSE)"));
        assert!(text.contains("c 1.0.0: MIT; copyright A <a@example.org>"));
        assert!(text.contains("Apache License"));
    }
}
