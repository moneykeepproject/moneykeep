//! backups.md revision 2, BK14: every program opens a book through the one opening
//! function (`mk_commands::backups::open_book`), which makes the mandatory backup
//! before a format update. This guard fails if any non-test code outside `mk-store`
//! calls the plain `Store::open` / `Store::open_with` (or the hook underneath) for a
//! book.
//!
//! Not scanned: `mk-store` itself; `apps/moneykeep-server`, whose `Store` is the
//! capture server's own store, not a book; test code (`tests/` folders, `tests.rs`
//! and `#[cfg(test)]` items).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

/// The calls only the one opening function may make.
const PLAIN_OPENS: &[&str] = &["Store::open(", "Store::open_with(", "open_with_hook("];

/// Where the one opening function lives (it may call `open_with_hook`).
const OPENER: &str = "crates/mk-commands/src/backups.rs";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name == "tests" || name == "target" || name.starts_with('.') {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") && name != "tests.rs" {
            out.push(p);
        }
    }
}

/// Byte ranges of `#[cfg(test)]` items (the attribute to the end of the item's
/// braces, or its `;`).
fn test_ranges(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("#[cfg(test)]") {
        let start = from + i;
        let mut j = start + "#[cfg(test)]".len();
        while j < b.len() && b[j] != b'{' && b[j] != b';' {
            j += 1;
        }
        if j < b.len() && b[j] == b'{' {
            let mut depth = 0usize;
            while j < b.len() {
                match b[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        out.push((start, j));
        from = j.max(start + 1);
    }
    out
}

/// The plain opens in one file's non-test code: `(line, text)`.
fn plain_opens(rel: &str, src: &str) -> Vec<(usize, String)> {
    let tests = test_ranges(src);
    let mut out = Vec::new();
    let mut offset = 0;
    for (n, line) in src.lines().enumerate() {
        let at = offset;
        offset += line.len() + 1;
        let code = line.split("//").next().unwrap_or_default();
        if tests.iter().any(|&(a, b)| at >= a && at <= b) {
            continue;
        }
        for pat in PLAIN_OPENS {
            if code.contains(pat) && !(rel == OPENER && *pat == "open_with_hook(") {
                out.push((n + 1, line.trim().to_owned()));
            }
        }
    }
    out
}

#[test]
fn backups_bk14_no_plain_book_open_outside_mk_store() {
    let root = workspace();
    let mut files = Vec::new();
    for top in ["apps", "crates", "xtask"] {
        rust_files(&root.join(top), &mut files);
    }
    assert!(
        files.len() > 50,
        "the workspace was found: {}",
        root.display()
    );
    let mut found = Vec::new();
    let mut opener_seen = false;
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("crates/mk-store/") || rel.starts_with("apps/moneykeep-server/") {
            continue;
        }
        let src = std::fs::read_to_string(&f).unwrap();
        if rel == OPENER {
            opener_seen = src.contains("pub fn open_book(");
        }
        for (line, text) in plain_opens(&rel, &src) {
            found.push(format!("{rel}:{line}: {text}"));
        }
    }
    assert!(opener_seen, "the one opening function is in {OPENER}");
    assert!(
        found.is_empty(),
        "open books with mk_commands::backups::open_book (or Session), not the plain \
         Store::open (backups.md revision 2):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_guard_sees_plain_opens_and_skips_test_code() {
    let src = "fn a() { let s = Store::open(p)?; }\n\
               // Store::open(p) in a comment\n\
               #[cfg(test)]\nmod tests {\n    fn b() { Store::open(p).unwrap(); }\n}\n\
               fn c() { mk_store::Store::open_with(p, None)?; }\n";
    let hits: Vec<usize> = plain_opens("x.rs", src).into_iter().map(|h| h.0).collect();
    assert_eq!(hits, vec![1, 7]);
    assert_eq!(
        plain_opens(OPENER, "fn f() { Store::open_with_hook(p, s, h) }").len(),
        0
    );
    assert_eq!(
        plain_opens("y.rs", "fn f() { Store::open_with_hook(p, s, h) }").len(),
        1
    );
}
