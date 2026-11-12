//! `cargo xtask names` (branding.md acceptance test 1): no file outside the internal
//! provenance records names the product's earlier name, the earlier template project
//! or the reference products.
//!
//! Every tracked (and every new, not ignored) file is checked, its path and its
//! content (binary files too). The names are matched without regard to case; the
//! company name only as a whole word, so ordinary words that contain it pass.
//!
//! Allowed places:
//!
//! - **Reference-product names**: the provenance records only, which are the
//!   **Sources** section of each spec in `docs/specs/` (public documentation cited
//!   by its real title). Since security-review-1 L13 (owner decision D4) the agent and
//!   clean-room documents (`AGENTS.md`, `CLAUDE.md`, the pull request checklist and
//!   `docs/clean-room.md`) say "commercial finance products" instead, and the names
//!   fail there like anywhere else.
//! - **The reference products' file-format names** (their proprietary formats):
//!   never in those four documents, which say "their proprietary file formats"
//!   (L13); elsewhere (the roadmap, the specs) they name formats MoneyKeep does not
//!   read, and are not checked.
//! - **The earlier product name** (and its definition file extension): nowhere.
//!   The compatibility constants (`mk_core::legacy`, the phone app's `legacy`
//!   module) build it from parts, and the rename spec writes it with a middle dot,
//!   so a history rewrite that replaces the name cannot change them.
//! - **The template project's name**: nowhere.
//!
//! The names are assembled from parts at run time, so this file does not contain
//! them either.

use std::path::Path;
use std::process::Command;

/// Which kind of name was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The product's name before the rename.
    OldName,
    /// The earlier template project.
    Template,
    /// The reference products and their vendor.
    Reference,
    /// The reference products' proprietary file formats (checked in the agent and
    /// clean-room documents only).
    Format,
}

/// A name to look for: its parts (joined at run time), its kind, and whether it must
/// stand as a whole word.
struct Name {
    parts: &'static [&'static str],
    kind: Kind,
    whole_word: bool,
}

const NAMES: &[Name] = &[
    Name {
        parts: &["money", "craft"],
        kind: Kind::OldName,
        whole_word: false,
    },
    // The old definition file extension.
    Name {
        parts: &["mc", "def"],
        kind: Kind::OldName,
        whole_word: true,
    },
    Name {
        parts: &["art", "craft"],
        kind: Kind::Template,
        whole_word: false,
    },
    Name {
        parts: &["quick", "books"],
        kind: Kind::Reference,
        whole_word: false,
    },
    Name {
        parts: &["quick", "en"],
        kind: Kind::Reference,
        whole_word: false,
    },
    Name {
        parts: &["in", "tuit"],
        kind: Kind::Reference,
        whole_word: true,
    },
    Name {
        parts: &["q", "df"],
        kind: Kind::Format,
        whole_word: true,
    },
    Name {
        parts: &["q", "bw"],
        kind: Kind::Format,
        whole_word: true,
    },
];

/// The agent and clean-room documents (security-review-1 L13, D4): no reference
/// product, vendor or file-format name, anywhere in them.
pub const CLEAN_ROOM_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".github/pull_request_template.md",
    "docs/clean-room.md",
];

/// The heading of a spec's Sources section.
const SOURCES: &str = "## Sources";

/// One finding: where, and which kind of name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    /// 0 for the file's path itself.
    pub line: usize,
    pub kind: Kind,
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Whether `hay` (lower case) contains `needle` (lower case), as a whole word when
/// asked.
fn contains(hay: &[u8], needle: &[u8], whole_word: bool) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    (0..=hay.len() - needle.len()).any(|i| {
        let Some(window) = hay.get(i..i + needle.len()) else {
            return false;
        };
        if window != needle {
            return false;
        }
        if !whole_word {
            return true;
        }
        let before = i.checked_sub(1).and_then(|j| hay.get(j)).copied();
        let after = hay.get(i + needle.len()).copied();
        !before.is_some_and(is_word_byte) && !after.is_some_and(is_word_byte)
    })
}

/// The kinds of names in `text`.
fn kinds_in(text: &[u8]) -> Vec<Kind> {
    let lower = text.to_ascii_lowercase();
    let mut out: Vec<Kind> = Vec::new();
    for n in NAMES {
        let needle = n.parts.concat();
        if contains(&lower, needle.as_bytes(), n.whole_word) && !out.contains(&n.kind) {
            out.push(n.kind);
        }
    }
    out
}

fn heading_level(line: &str) -> Option<usize> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    (level > 0 && line.as_bytes().get(level) == Some(&b' ')).then_some(level)
}

/// Whether a reference-product name is allowed on line `index` of `path`.
fn reference_allowed(path: &str, lines: &[&str], index: usize) -> bool {
    if CLEAN_ROOM_FILES.contains(&path) {
        return false;
    }
    let heading = (path.starts_with("docs/specs/") && path.ends_with(".md")).then_some(SOURCES);
    let Some(heading) = heading else {
        return false;
    };
    // The nearest heading at or above the line, of the section heading's level or
    // higher, must be the allowed one.
    let level = heading_level(heading).unwrap_or(2);
    lines
        .get(..=index)
        .unwrap_or(&[])
        .iter()
        .rev()
        .find(|l| heading_level(l).is_some_and(|h| h <= level))
        .is_some_and(|l| l.starts_with(heading))
}

/// The findings for one file: its path and content.
pub fn findings(path: &str, content: &[u8]) -> Vec<Finding> {
    let mut out = Vec::new();
    let allowed = |kind: Kind, lines: &[&str], index: usize| match kind {
        Kind::OldName | Kind::Template => false,
        Kind::Reference => reference_allowed(path, lines, index),
        Kind::Format => !CLEAN_ROOM_FILES.contains(&path),
    };
    for kind in kinds_in(path.as_bytes()) {
        if kind == Kind::Format {
            continue;
        }
        out.push(Finding {
            path: path.to_owned(),
            line: 0,
            kind,
        });
    }
    let text = String::from_utf8_lossy(content);
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        for kind in kinds_in(line.as_bytes()) {
            if !allowed(kind, &lines, i) {
                out.push(Finding {
                    path: path.to_owned(),
                    line: i + 1,
                    kind,
                });
            }
        }
    }
    out
}

/// The files to check: tracked, and new ones that are not ignored.
fn files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut list: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    list.sort();
    list.dedup();
    Ok(list)
}

fn describe(kind: Kind) -> &'static str {
    match kind {
        Kind::OldName => {
            "the product's earlier name (build it from parts, or write it with a middle dot)"
        }
        Kind::Template => "the earlier template project's name",
        Kind::Reference => {
            "a reference-product name (allowed only in spec Sources; the agent and clean-room documents say \"commercial finance products\")"
        }
        Kind::Format => {
            "a reference product's file-format name (the agent and clean-room documents say \"their proprietary file formats\")"
        }
    }
}

/// `cargo xtask names`.
pub fn check() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace folder")?
        .to_path_buf();
    let mut all = Vec::new();
    let list = files(&root)?;
    for path in &list {
        // A file deleted in the working tree is not there to read.
        let Ok(content) = std::fs::read(root.join(path)) else {
            continue;
        };
        all.extend(findings(path, &content));
    }
    if all.is_empty() {
        println!(
            "names: {} files checked, none names a removed name",
            list.len()
        );
        return Ok(());
    }
    for f in &all {
        eprintln!("{}:{}: {}", f.path, f.line, describe(f.kind));
    }
    Err(format!("{} places name removed names", all.len()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn name(parts: &[&str]) -> String {
        parts.concat()
    }

    #[test]
    fn finds_names_in_content_and_paths() {
        let old = name(&["Money", "Craft"]);
        let f = findings("README.md", format!("# {old}\n").as_bytes());
        assert_eq!(
            f,
            [Finding {
                path: "README.md".into(),
                line: 1,
                kind: Kind::OldName
            }]
        );
        let path = format!("docs/{}.md", name(&["art", "craft"]));
        let f = findings(&path, b"ok");
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].line, f[0].kind), (0, Kind::Template));
        let ext = format!("starter.{}", name(&["mc", "def"]));
        assert_eq!(findings(&ext, b"")[0].kind, Kind::OldName);
        // Upper case and binary content too.
        let mut bin = vec![0u8, 159, 146, 150];
        bin.extend_from_slice(name(&["QUICK", "EN"]).as_bytes());
        assert_eq!(findings("x.der", &bin)[0].kind, Kind::Reference);
    }

    #[test]
    fn the_company_name_only_as_a_word() {
        let company = name(&["In", "tuit"]);
        assert!(findings("a.md", b"an intuitive layout").is_empty());
        assert_eq!(
            findings("a.md", format!("by {company}.").as_bytes()).len(),
            1
        );
    }

    #[test]
    fn reference_names_only_in_spec_sources() {
        let product = name(&["Quick", "Books"]);
        let line = format!("Never read {product} source.\n");
        assert_eq!(findings("README.md", line.as_bytes()).len(), 1);
        // Specs: the Sources section only.
        let spec = format!("# Spec\n## Sources\n| 1 | {line}## Behaviour\n{line}");
        let f = findings("docs/specs/x.md", spec.as_bytes());
        assert_eq!(f.iter().map(|f| f.line).collect::<Vec<_>>(), [5]);
        // Never the old product name or the template there.
        let old = format!("{}\n", name(&["money", "craft"]));
        assert_eq!(
            findings("docs/specs/x.md", format!("## Sources\n{old}").as_bytes()).len(),
            1
        );
    }

    /// security-review-1 SR-B5 (L13): the names check fails if a product name, the
    /// vendor's name or a file-format name is put back into the agent and
    /// clean-room documents, wherever in them (section 1 of AGENTS.md included).
    #[test]
    fn sr_b5_l13_names_cannot_return_to_the_clean_room_documents() {
        let lines = [
            format!("- Never read {} source.\n", name(&["Quick", "en"])),
            format!("- Never read {} source.\n", name(&["Quick", "Books"])),
            format!("- No {} trademarks.\n", name(&["In", "tuit"])),
            format!(
                "- Not their file formats ({}, {}).\n",
                name(&["Q", "DF"]),
                name(&["Q", "BW"])
            ),
        ];
        for path in CLEAN_ROOM_FILES {
            for line in &lines {
                let text = format!("# Doc\n## 1. Clean-room protocol\n## Sources\n{line}");
                let f = findings(path, text.as_bytes());
                assert!(!f.is_empty(), "{path}: {line}");
                assert!(f.iter().all(|f| f.line == 4), "{path}: {f:?}");
            }
            // The D4 wording passes.
            let d4 = "Never read source code of commercial finance products, or reverse-engineer \
                      their proprietary file formats; no trademarks of commercial finance products.\n";
            assert!(findings(path, d4.as_bytes()).is_empty(), "{path}");
        }
        // The real files pass as they are now.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for path in CLEAN_ROOM_FILES {
            let text = std::fs::read(root.join(path)).unwrap();
            assert_eq!(findings(path, &text), [], "{path}");
        }
        // File-format names elsewhere (the roadmap) are not checked; spec Sources
        // keep the product names.
        let fmt = format!("No {} import.\n", name(&["Q", "DF"]));
        assert!(findings("ROADMAP.md", fmt.as_bytes()).is_empty());
        let src = format!("## Sources\n| 1 | {} FAQ |\n", name(&["Quick", "en"]));
        assert!(findings("docs/specs/import-qif.md", src.as_bytes()).is_empty());
    }

    #[test]
    fn old_name_nowhere_in_one_piece() {
        let old = format!("pub const X: &str = \"{}\";\n", name(&["Money", "Craft"]));
        for path in [
            "crates/mk-core/src/legacy.rs",
            "apps/moneykeep-capture/src/legacy.rs",
            "docs/specs/branding.md",
            "crates/mk-core/src/brand.rs",
        ] {
            assert_eq!(findings(path, old.as_bytes()).len(), 1, "{path}");
        }
        // Built from parts, or written with a middle dot: passes.
        let parts = "pub const X: &str = concat!(\"Money\", \"Craft\");\n";
        assert!(findings("crates/mk-core/src/legacy.rs", parts.as_bytes()).is_empty());
        assert!(findings("docs/specs/branding.md", "Money·Craft\n".as_bytes()).is_empty());
        let template = format!("{}\n", name(&["Art", "Craft"]));
        assert_eq!(
            findings("crates/mk-core/src/legacy.rs", template.as_bytes()).len(),
            1
        );
    }

    /// The whole repository passes (branding.md acceptance test 1).
    #[test]
    fn repository_passes() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        assert_eq!(check(), Ok(()));
    }
}
