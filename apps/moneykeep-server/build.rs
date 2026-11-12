//! Embeds the phone app's built files (mobile-capture.md revision 2, section D).
//!
//! `cargo xtask dist-capture` builds the phone app with Trunk into
//! `apps/moneykeep-capture/dist` and then builds this server with `MK_CAPTURE_DIST`
//! pointing at that folder. Without the variable (ordinary builds and tests) nothing
//! is embedded and the server serves a page explaining how to build it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// mobile-capture.md revision 18: the Brotli and gzip copies, made here.
#[path = "src/precompress.rs"]
#[allow(dead_code)]
mod precompress;

/// Files whose text the server changes when it takes them in (the build's capture
/// version, revision 15): their copies are made then, not here.
const STAMPED: [&str; 2] = ["/sw.js", "/index.html"];

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            walk(root, &path, out)?;
        } else if kind.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((format!("/{rel}"), path));
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=MK_CAPTURE_DIST");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/precompress.rs");
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    if let Some(dir) = std::env::var_os("MK_CAPTURE_DIST").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        if dir.join("index.html").is_file() {
            println!("cargo:rerun-if-changed={}", dir.display());
            if let Err(e) = walk(&dir, &dir, &mut files) {
                println!("cargo:warning=the phone app could not be embedded: {e}");
                files.clear();
            }
        } else {
            println!(
                "cargo:warning=MK_CAPTURE_DIST={} has no index.html; the phone app is not embedded",
                dir.display()
            );
        }
    }
    files.sort();
    let Some(out) = std::env::var_os("OUT_DIR") else {
        println!("cargo:warning=OUT_DIR is not set");
        return;
    };
    let out = PathBuf::from(out);
    // (path, file, Brotli copy, gzip copy).
    let mut code = String::from("pub static FILES: &[EmbeddedFile] = &[\n");
    for (i, (rel, abs)) in files.iter().enumerate() {
        let abs = abs.canonicalize().unwrap_or_else(|_| abs.clone());
        let copies = if precompress::compressible(rel) && !STAMPED.contains(&rel.as_str()) {
            std::fs::read(&abs).ok().map(|bytes| {
                let write = |ext: &str, data: Option<Vec<u8>>| -> String {
                    let Some(data) = data else {
                        return "None".to_owned();
                    };
                    let path = out.join(format!("phone-{i}.{ext}"));
                    match std::fs::write(&path, data) {
                        Ok(()) => format!("Some(include_bytes!({:?}))", path.display().to_string()),
                        Err(e) => {
                            println!("cargo:warning=cannot write {}: {e}", path.display());
                            "None".to_owned()
                        }
                    }
                };
                (
                    write("br", precompress::brotli(&bytes)),
                    write("gz", precompress::gzip(&bytes)),
                )
            })
        } else {
            None
        };
        let (br, gz) = copies.unwrap_or_else(|| ("None".to_owned(), "None".to_owned()));
        let _ = writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?}), {br}, {gz}),",
            abs.display().to_string()
        );
    }
    code.push_str("];\n");
    if let Err(e) = std::fs::write(out.join("phone_app.rs"), code) {
        println!("cargo:warning=cannot write phone_app.rs: {e}");
    }
}
