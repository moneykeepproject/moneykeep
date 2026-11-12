//! backups.md revision 4: on Windows, flushing a file to disk (`FlushFileBuffers`,
//! behind `File::sync_all` and `File::sync_data`) needs a handle opened for writing,
//! so flushing a handle from `File::open` (read-only) fails there while it passes on
//! Linux. This guard fails if `sync_all`/`sync_data` is called on a handle from
//! `File::open` anywhere in the workspace: chained (`File::open(p)?.sync_all()`) or
//! through a binding (`let f = File::open(p)?; … f.sync_all()`).
//!
//! Not counted: code under `#[cfg(unix)]`, where flushing a directory opened with
//! `File::open` is how a rename is made durable (it never runs on Windows).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

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
            if name == "target" || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Byte ranges of the items under `attr` (the attribute to the end of the item's
/// braces, or its `;`).
fn ranges(src: &str, attr: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find(attr) {
        let start = from + i;
        let mut j = start + attr.len();
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

/// The end of the parenthesised arguments starting at `open` (the `(`).
fn close_paren(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The name bound to the `File::open` call at `at`: `let [mut] NAME =`,
/// `Ok(NAME) =` or `Some(NAME) =` just before it on its line.
fn bound_name(src: &str, at: usize) -> Option<String> {
    let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = src[line_start..at].trim_end();
    let before = before.strip_suffix("std::fs::").unwrap_or(before);
    let before = before.strip_suffix("fs::").unwrap_or(before).trim_end();
    let lhs = before.strip_suffix('=')?.trim_end();
    let lhs = lhs
        .strip_suffix(')')
        .and_then(|l| l.rsplit_once('('))
        .map_or(lhs, |(_, n)| n);
    let name: String = lhs
        .bytes()
        .rev()
        .take_while(|c| is_ident(*c))
        .map(char::from)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!name.is_empty() && name != "mut").then_some(name)
}

/// `(line, text)` of every flush of a read-only handle in `src`.
fn read_only_flushes(src: &str) -> Vec<(usize, String)> {
    let skip = ranges(src, "#[cfg(unix)]");
    let b = src.as_bytes();
    let line_of = |i: usize| src[..i].bytes().filter(|c| *c == b'\n').count() + 1;
    let text_of = |i: usize| {
        src[i..]
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("File::open(") {
        let at = from + i;
        from = at + 1;
        if skip.iter().any(|(s, e)| at >= *s && at <= *e) {
            continue;
        }
        let Some(end) = close_paren(b, at + "File::open".len()) else {
            continue;
        };
        // Chained: `File::open(p)?.sync_all()` (whitespace and `?` allowed).
        let rest = src[end + 1..].trim_start();
        let rest = rest.strip_prefix('?').unwrap_or(rest).trim_start();
        if rest.starts_with(".sync_all(") || rest.starts_with(".sync_data(") {
            out.push((line_of(at), text_of(at)));
            continue;
        }
        // Bound: the name flushed later in the same block — up to the end of the
        // block the binding is in, or until the name is bound again.
        if let Some(name) = bound_name(src, at) {
            let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
            let indent = src[line_start..].bytes().take_while(|c| *c == b' ').count();
            let rebinds = [format!("let {name} ="), format!("let mut {name} =")];
            let mut lines = src[end..].lines();
            let mut window: Vec<&str> = lines.next().into_iter().collect();
            for line in lines {
                let t = line.trim_start();
                let ind = line.len() - t.len();
                if (t.starts_with('}') && ind < indent)
                    || rebinds.iter().any(|r| t.contains(r.as_str()))
                {
                    break;
                }
                window.push(line);
            }
            let window = window.join("\n");
            for call in [".sync_all(", ".sync_data("] {
                let needle = format!("{name}{call}");
                if let Some(k) = window.find(&needle) {
                    let preceded = k > 0 && is_ident(window.as_bytes()[k - 1]);
                    if !preceded {
                        out.push((line_of(at), text_of(at)));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn no_flush_through_a_read_only_handle() {
    let root = workspace();
    let mut files = Vec::new();
    for d in ["crates", "apps", "xtask"] {
        rust_files(&root.join(d), &mut files);
    }
    assert!(files.len() > 50, "scanned the workspace");
    let me = Path::new(file!()).file_name().unwrap();
    let mut found = Vec::new();
    for f in &files {
        if f.file_name() == Some(me) {
            continue;
        }
        let src = std::fs::read_to_string(f).unwrap();
        for (line, text) in read_only_flushes(&src) {
            let rel = f.strip_prefix(&root).unwrap_or(f).display().to_string();
            found.push(format!("{rel}:{line}: {text}"));
        }
    }
    assert!(
        found.is_empty(),
        "flushing a handle from File::open fails on Windows; open it for writing \
         (e.g. mk_commands::backups::copy_flushed):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_guard_finds_both_forms() {
    let chained =
        "fn a() { std::fs::copy(a, &tmp).and_then(|_| std::fs::File::open(&tmp)?.sync_all()); }";
    assert_eq!(read_only_flushes(chained).len(), 1);
    let bound = "fn b() {\n    let f = File::open(&p)?;\n    f.sync_all()?;\n}";
    assert_eq!(read_only_flushes(bound).len(), 1);
    let if_let = "fn c() {\n    if let Ok(d) = std::fs::File::open(dir) {\n        let _ = d.sync_data();\n    }\n}";
    assert_eq!(read_only_flushes(if_let).len(), 1);
    let unix = "fn d() {\n    #[cfg(unix)]\n    if let Ok(d) = File::open(dir) {\n        let _ = d.sync_all();\n    }\n}";
    assert!(read_only_flushes(unix).is_empty());
    let writable = "fn e() {\n    let mut f = OpenOptions::new().write(true).open(&p)?;\n    f.sync_all()?;\n    let g = File::open(&p)?;\n    read(g);\n}";
    assert!(read_only_flushes(writable).is_empty());
    // A handle of the same name in the next function is another handle.
    let next_fn = "fn f() -> R {\n    let mut f = File::open(path)?;\n    f.read_to_end(&mut b)?;\n}\n\nfn g() {\n    let mut f = OpenOptions::new().write(true).open(t)?;\n    f.sync_all()?;\n}";
    assert!(read_only_flushes(next_fn).is_empty());
}
