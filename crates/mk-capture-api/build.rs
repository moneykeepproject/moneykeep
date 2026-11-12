//! Two versions for the capture server's health answer and the desktop's comparison:
//!
//! - `MK_BUILD_VERSION` (mobile-capture.md revision 4): `<package version>+<commit
//!   time>.<commit>` from git when the source is a git checkout, else the package
//!   version alone.
//! - `MK_CAPTURE_VERSION` (release-packaging.md "Capture version"): `<server package
//!   version>+<12 hex>` over exactly the sources the capture server and the phone app
//!   are built from (see `build/capture_version.rs`), so desktop-only changes do not
//!   ask for a server update. `MK_CAPTURE_PATHS` lists those paths (`;`-separated)
//!   for the test that checks they cover the dependency graph.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build/capture_version.rs"]
mod capture_version;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn main() {
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    // Rebuild when the checked-out commit changes.
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        let dir = Path::new(&dir);
        println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("packed-refs").display()
        );
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", dir.join(head).display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/capture_version.rs");
    let version = match (
        git(&["log", "-1", "--format=%ct"]),
        git(&["rev-parse", "--short=12", "HEAD"]),
    ) {
        (Some(time), Some(commit)) => format!("{pkg}+{time}.{commit}"),
        _ => pkg.clone(),
    };
    println!("cargo:rustc-env=MK_BUILD_VERSION={version}");

    // The capture version.
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.clone());
    match capture_version::compute(&cargo, &manifest_dir.join("Cargo.toml"), &workspace, &pkg) {
        Ok(capture_version::Computed {
            capture_version: capture,
            paths,
            watch,
            note,
        }) => {
            if let Some(n) = note {
                println!("cargo:warning={n}");
            }
            // A watched path that does not exist would rerun this script every build.
            for w in watch.iter().filter(|w| w.exists()) {
                println!("cargo:rerun-if-changed={}", w.display());
            }
            println!("cargo:rustc-env=MK_CAPTURE_VERSION={capture}");
            println!("cargo:rustc-env=MK_CAPTURE_PATHS={}", paths.join(";"));
        }
        Err(e) => {
            // Never equal to a real capture version: the desktop then offers an
            // update rather than staying quiet.
            println!("cargo:warning=the capture version could not be computed: {e}");
            println!("cargo:rustc-env=MK_CAPTURE_VERSION={pkg}+unknown");
            println!("cargo:rustc-env=MK_CAPTURE_PATHS=");
        }
    }
}
