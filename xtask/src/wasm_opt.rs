//! Shrinking the phone program with Binaryen's `wasm-opt -Oz` after Trunk
//! (mobile-capture.md revision 18). `release`, `installer` and `dist-capture` run it;
//! without `wasm-opt` they stop with how to get it, unless `--no-wasm-opt` is given
//! (developers: the program is then larger). The gate (`ci`) never needs it.
//!
//! Binaryen (Apache-2.0) is a build tool only: nothing of it ships in MoneyKeep.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The phone program Trunk writes into `dist/`.
pub const PROGRAM: &str = "moneykeep-capture_bg.wasm";

/// Where to get Binaryen (its GitHub releases page).
pub const RELEASES: &str = "https://github.com/WebAssembly/binaryen/releases";

/// What the build says when `wasm-opt` is missing.
pub fn missing_message() -> String {
    format!(
        "wasm-opt (Binaryen) is needed to shrink the phone app's program. Download Binaryen from its GitHub releases and put its bin folder on PATH: {RELEASES} — then run this again. (Developers: --no-wasm-opt builds without it; the phone program is then larger.)"
    )
}

/// The note `--no-wasm-opt` prints.
pub const SKIPPED_NOTE: &str = "note: --no-wasm-opt: the phone program was not shrunk with wasm-opt, so it is larger and phones take longer to load it";

/// The WebAssembly features the Rust wasm32 build uses (its defaults), kept by
/// wasm-opt.
pub const FEATURES: [&str; 6] = [
    "--enable-bulk-memory",
    "--enable-mutable-globals",
    "--enable-nontrapping-float-to-int",
    "--enable-sign-ext",
    "--enable-reference-types",
    "--enable-multivalue",
];

/// The program's size before and after (equal when not shrunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub before: u64,
    pub after: u64,
    /// wasm-opt ran.
    pub optimized: bool,
}

impl Sizes {
    /// `version.json`'s `phone_program_bytes`.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "before": self.before,
            "after": self.after,
            "wasm_opt": self.optimized,
        })
    }

    /// The line the build prints.
    pub fn line(&self) -> String {
        if self.optimized {
            format!(
                "phone program: {} bytes before wasm-opt, {} bytes after",
                self.before, self.after
            )
        } else {
            format!(
                "phone program: {} bytes (not shrunk: --no-wasm-opt)",
                self.before
            )
        }
    }
}

/// `wasm-opt` on PATH (or `MK_WASM_OPT`, a path to the program).
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MK_WASM_OPT").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let name = if cfg!(windows) {
        "wasm-opt.exe"
    } else {
        "wasm-opt"
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// The arguments for shrinking `input` into `output`.
pub fn args(input: &Path, output: &Path) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = vec!["-Oz".into()];
    a.extend(FEATURES.iter().map(Into::into));
    a.push(input.as_os_str().to_owned());
    a.push("-o".into());
    a.push(output.as_os_str().to_owned());
    a
}

fn size(p: &Path) -> Result<u64, String> {
    std::fs::metadata(p)
        .map(|m| m.len())
        .map_err(|e| format!("cannot read {}: {e}", p.display()))
}

/// Shrink `dist/<PROGRAM>` in place with `tool` (`None`: wasm-opt is missing).
/// Without the tool the build stops with [`missing_message`], unless `skip`
/// (`--no-wasm-opt`).
pub fn shrink(dist: &Path, tool: Option<&Path>, skip: bool) -> Result<Sizes, String> {
    let program = dist.join(PROGRAM);
    let before = size(&program)?;
    if skip {
        println!("{SKIPPED_NOTE}");
        let s = Sizes {
            before,
            after: before,
            optimized: false,
        };
        println!("{}", s.line());
        return Ok(s);
    }
    let tool = tool.ok_or_else(missing_message)?;
    println!("==> wasm-opt -Oz {}", program.display());
    let out = dist.join(format!("{PROGRAM}.opt"));
    let status = Command::new(tool)
        .args(args(&program, &out))
        .status()
        .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
    if !status.success() || !out.is_file() {
        let _ = std::fs::remove_file(&out);
        return Err(format!("wasm-opt failed on {}", program.display()));
    }
    let after = size(&out)?;
    if after == 0 {
        let _ = std::fs::remove_file(&out);
        return Err("wasm-opt wrote an empty program".into());
    }
    std::fs::rename(&out, &program)
        .map_err(|e| format!("cannot replace {}: {e}", program.display()))?;
    let s = Sizes {
        before,
        after,
        optimized: true,
    };
    println!("{}", s.line());
    Ok(s)
}

/// `--no-wasm-opt` among the arguments; the others are returned.
pub fn take_flag(args: &[String]) -> (bool, Vec<String>) {
    let skip = args.iter().any(|a| a == "--no-wasm-opt");
    (
        skip,
        args.iter()
            .filter(|a| *a != "--no-wasm-opt")
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-wasmopt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(PROGRAM), vec![7u8; 1000]).unwrap();
        d
    }

    /// mobile-capture.md test 103: wasm-opt missing stops the build with the
    /// instructions (nothing changed); --no-wasm-opt builds with a note.
    #[test]
    fn t103_missing_wasm_opt_stops_unless_skipped() {
        let d = scratch("missing");
        let e = shrink(&d, None, false).unwrap_err();
        assert!(
            e.contains("Download Binaryen from its GitHub releases and put its bin folder on PATH"),
            "{e}"
        );
        assert!(e.contains(RELEASES), "{e}");
        assert!(e.contains("--no-wasm-opt"), "{e}");
        assert_eq!(std::fs::read(d.join(PROGRAM)).unwrap().len(), 1000);
        let s = shrink(&d, None, true).unwrap();
        assert_eq!(
            s,
            Sizes {
                before: 1000,
                after: 1000,
                optimized: false
            }
        );
        assert_eq!(s.json()["wasm_opt"], false);
        assert!(s.line().contains("--no-wasm-opt"));
        assert!(SKIPPED_NOTE.contains("larger"));
        let (skip, rest) = take_flag(&["--dry-run".into(), "--no-wasm-opt".into()]);
        assert!(skip);
        assert_eq!(rest, vec!["--dry-run".to_owned()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Test 103: the build runs wasm-opt (-Oz, the features kept) and records both
    /// sizes; a fake wasm-opt stands in for Binaryen.
    #[cfg(unix)]
    #[test]
    fn t103_wasm_opt_runs_and_both_sizes_are_recorded() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("run");
        let fake = d.join("wasm-opt");
        let log = d.join("args.txt");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}'\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then shift; out=\"$1\"; fi; shift; done\nhead -c 400 /dev/zero > \"$out\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = shrink(&d, Some(&fake), false).unwrap();
        assert_eq!(
            s,
            Sizes {
                before: 1000,
                after: 400,
                optimized: true
            }
        );
        assert_eq!(std::fs::read(d.join(PROGRAM)).unwrap().len(), 400);
        assert!(!d.join(format!("{PROGRAM}.opt")).exists());
        let used = std::fs::read_to_string(&log).unwrap();
        assert!(used.starts_with("-Oz "), "{used}");
        for f in FEATURES {
            assert!(used.contains(f), "{f}: {used}");
        }
        assert_eq!(
            s.json(),
            serde_json::json!({"before": 1000, "after": 400, "wasm_opt": true})
        );
        assert_eq!(
            s.line(),
            "phone program: 1000 bytes before wasm-opt, 400 bytes after"
        );
        // A failing wasm-opt stops the build and leaves the program alone.
        std::fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
        assert!(shrink(&d, Some(&fake), false).is_err());
        assert_eq!(std::fs::read(d.join(PROGRAM)).unwrap().len(), 400);
        let _ = std::fs::remove_dir_all(&d);
    }
}
