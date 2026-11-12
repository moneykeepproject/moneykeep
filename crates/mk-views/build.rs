#![allow(clippy::print_stdout)]
//! Compiles the views (`ui/app.slint` and what it imports) with Slint's Fluent
//! style (https://docs.slint.dev/latest/docs/slint/guide/development/rust/ —
//! slint-build). The element tree keeps debug information so the testing backend
//! can find elements by accessible name.
//!
//! `ui/item.template` holds the item components. Two files are written from it
//! into the build folder: `item.slint` (ItemView, whose cell groups use the leaf
//! copy) and `leaf.slint` (the same components renamed with a Leaf suffix over
//! `CellVm`), because a Slint component cannot contain itself. They are written
//! into a copy of `ui/` in the build folder and compiled from there, so every
//! import stays relative (see `main`).

use std::path::{Path, PathBuf};

/// Components copied into the leaf file under a `Leaf` suffix.
const RENAMED: &[&str] = &[
    "TipArea",
    "Field",
    "SuggestField",
    "MultiField",
    "TouchField",
    "TouchMultiField",
    "DropDown",
    "MenuButton",
    "Radio",
    "Light",
    "ChartView",
    "Tile",
    "ActionTile",
    "PictureChoice",
    "CropFrame",
    "PaintCanvas",
];

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replace whole identifier `from` with `to`.
fn replace_ident(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(from) {
        let (before, after) = rest.split_at(pos);
        let prev = before.chars().next_back();
        let next = after.get(from.len()..).and_then(|a| a.chars().next());
        out.push_str(before);
        if prev.is_some_and(is_ident) || next.is_some_and(is_ident) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        rest = after.get(from.len()..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Write `bytes` to `path` unless it already holds them. Slint reports every file it
/// reads with `cargo:rerun-if-changed`, so rewriting an unchanged copy would make
/// Cargo rerun this script on every build.
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    std::fs::write(path, bytes)
}

/// Copy `from` (a folder) into `to`, keeping the relative layout; the item
/// template is not copied (item.slint and leaf.slint are written from it).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &dest)?;
        } else if path.extension().is_none_or(|e| e != "template") {
            write_if_changed(&dest, &std::fs::read(&path)?)?;
        }
    }
    Ok(())
}

/// An import whose path is absolute (a leading slash or backslash, or a drive
/// letter): every import in the views must stay relative, or Slint may load one file
/// under two spellings and rename the second copy's structs (`ChartVm2`).
fn absolute_import(text: &str) -> Option<&str> {
    text.lines().find(|line| {
        line.trim_start().starts_with("import")
            && line.split_once("from \"").is_some_and(|(_, p)| {
                p.starts_with('/') || p.starts_with('\\') || p.get(1..2) == Some(":")
            })
    })
}

fn fail(what: &str, e: impl std::fmt::Display) -> ! {
    println!("cargo:warning={what}: {e}");
    std::process::exit(1);
}

fn main() {
    let template = match std::fs::read_to_string("ui/item.template") {
        Ok(t) => t,
        Err(e) => fail("ui/item.template", e),
    };
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets");
    // Everything is compiled from one copy of ui/ in the build folder, with the
    // generated item.slint and leaf.slint beside the files they import, so every
    // import (and every @image-url) is relative to that one folder. Absolute paths
    // are avoided: on Windows a canonicalised path (`\\?\C:\...`) and the path Slint
    // resolves for a relative import differ, and Slint then loads vm.slint twice.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| ".".into()));
    let ui = out.join("ui");
    if let Err(e) = copy_tree(Path::new("ui"), &ui) {
        fail("copying ui into the build folder", e);
    }
    // The start screen's fonts and the preset book pictures (app-desktop.md
    // revision 40), beside the views so their paths stay relative.
    for folder in ["fonts", "book-pictures"] {
        if let Err(e) = copy_tree(
            &Path::new("assets").join(folder),
            &ui.join("assets").join(folder),
        ) {
            fail("copying assets into the build folder", e);
        }
    }
    let item = template
        .replace("// @leaf-import", "import { LeafView } from \"leaf.slint\";")
        .replace(
            "    // @group",
            "    if item.kind == K.group: HorizontalLayout {\n        spacing: Theme.s1;\n        for c in item.children: LeafView { item: c; }\n    }",
        );
    let mut leaf = template
        .replace("// @leaf-import", "")
        .replace("    // @group\n", "");
    for name in RENAMED {
        leaf = replace_ident(&leaf, name, &format!("{name}Leaf"));
    }
    leaf = replace_ident(&leaf, "ItemView", "LeafView");
    leaf = replace_ident(&leaf, "ItemVm", "CellVm");
    // CellVm is already imported once.
    leaf = leaf.replace("import { CellVm, CellVm,", "import { CellVm,");
    let write = |name: &str, text: &str| {
        if let Some(line) = absolute_import(text) {
            fail(name, format!("absolute import path: {line}"));
        }
        if let Err(e) = write_if_changed(&ui.join(name), text.as_bytes()) {
            fail(name, e);
        }
    };
    write("item.slint", &item);
    write("leaf.slint", &leaf);
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(true);
    if let Err(e) = slint_build::compile_with_config(ui.join("app.slint"), config) {
        fail("app.slint", e);
    }
}
