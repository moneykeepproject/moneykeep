//! `cargo xtask <task>`
//!
//! - `ci`           names, fmt check, clippy (warnings denied), tests (mk-views' `ui`
//!   test binary one module per process), layering, wasm build of the phone client,
//!   the service worker's Node test, licence policy
//! - `layering`     layering check only
//! - `names`        no removed names outside the allow-listed files (branding.md)
//! - `capture-icons` regenerate the phone app's icons from the calculator drawing
//! - `dist-capture` build the phone app with Trunk (shrunk with `wasm-opt`;
//!   `--no-wasm-opt` skips that) and the capture server with it embedded
//!   (mobile-capture.md revision 2, section D, and revision 18)
//! - `release`      the phone app, the server with it, the desktop app, the CLI and
//!   keep-mcp in release mode, into `dist/MoneyKeep-<version>/` with the licences
//!   and `version.json`; `--dry-run` lists the steps (release-packaging.md)
//! - `installer`    (Windows) `release`, then the setup with NSIS's `makensis`:
//!   `dist/moneykeep-<version>-windows-x64-setup.exe` and `dist/SHA256SUMS.txt`;
//!   (macOS) `packaging/macos/package.sh`: `dist/moneykeep-<version>-macos-universal.dmg`
//!   (macos-release.md P7)
//! - `phone-app`    build the phone app once into `--out DIR`, for `MK_PHONE_APP=DIR`
//! - `workflows`    static check of the workflows: the Windows CI job,
//!   `.github/workflows/windows.yml` (upgrade-resilience-1 UR-8), and the Mac runner
//!   rules (macos-release.md W9)

#![allow(clippy::print_stdout)]

mod layering;
mod licences;
mod names;
mod release;
mod wasm_opt;
mod workflows;

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let rest: Vec<String> = std::env::args().skip(2).collect();
    let result = match task.as_str() {
        "ci" => ci(),
        "layering" => step("layering", layering::check),
        "names" => step("names", names::check),
        "dist-capture" => dist_capture(&rest),
        "capture-icons" => capture_icons(),
        "release" => release::release(&rest),
        "installer" => release::installer(&rest),
        "phone-app" => phone_app_task(&rest),
        "workflows" => step("workflows", workflows::check),
        _ => {
            eprintln!(
                "usage: cargo xtask <ci|layering|names|dist-capture [--no-wasm-opt]|capture-icons|release [--dry-run] [--out DIR] [--no-wasm-opt]|installer [--skip-release] [--no-wasm-opt] (macOS: packaging/macos/package.sh's options)|phone-app --out DIR [--no-wasm-opt]|workflows>"
            );
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nxtask failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn ci() -> Result<(), String> {
    step("names", names::check)?;
    cargo("fmt", &["fmt", "--all", "--", "--check"])?;
    cargo(
        "clippy",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    // The phone client's browser layer, type-checked natively (mobile-capture.md).
    cargo(
        "clippy (capture web layer)",
        &[
            "clippy",
            "-p",
            "moneykeep-capture",
            "--features",
            "web-check",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    // The optional MCP server program (mcp-server.md revision 3).
    cargo("build keep-mcp", &["build", "-p", "keep-mcp"])?;
    tests()?;
    step("layering", layering::check)?;
    step("workflows", workflows::check)?;
    wasm_build()?;
    service_worker_test()?;
    installer_script_note();
    licence_policy()?;
    println!("\nxtask ci: all checks passed");
    Ok(())
}

/// The tests, as `cargo test --workspace` runs them, except that mk-views' `ui`
/// test binary (about two hours in one run) runs one module per process: a cloud
/// container that restarts during a long run then loses one module, not all.
fn tests() -> Result<(), String> {
    println!("==> test (build)");
    let out = Command::new(env!("CARGO"))
        .args([
            "test",
            "--workspace",
            "--no-run",
            "--message-format=json-render-diagnostics",
        ])
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("test: failed to start cargo: {e}"))?;
    if !out.status.success() {
        return Err("test failed (build)".into());
    }
    let runs = test_executables(&String::from_utf8_lossy(&out.stdout));
    let root = release::workspace_root()?;
    let modules = ui_modules(&root.join(UI_TESTS))?;
    println!("==> test");
    for t in &runs {
        if t.is_ui() {
            for (m, skips) in module_filters(&modules) {
                println!("\n     Running {} (module {m}::)", t.label());
                let mut args = vec![format!("{m}::")];
                for s in skips {
                    args.push("--skip".into());
                    args.push(format!("{s}::"));
                }
                run_test(t, &args)?;
            }
        } else {
            println!("\n     Running {}", t.label());
            run_test(t, &[])?;
        }
    }
    cargo("test (doc tests)", &["test", "--workspace", "--doc"])
}

/// The folder of mk-views' `ui` test modules.
const UI_TESTS: &str = "crates/mk-views/tests/ui";

/// A test executable `cargo test --no-run` built.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TestExe {
    exe: std::path::PathBuf,
    /// The package's folder (cargo runs tests there).
    dir: std::path::PathBuf,
    package: String,
    target: String,
    kind: String,
    src: String,
}

impl TestExe {
    fn is_ui(&self) -> bool {
        self.package == "mk-views" && self.target == "ui" && self.kind == "test"
    }
    fn label(&self) -> String {
        format!("{} {} ({})", self.package, self.src, self.exe.display())
    }
}

/// The test executables in `cargo test --no-run --message-format=json` output, in
/// build order. (serde_json's `[]` gives Null for a missing key; it never panics.)
#[allow(clippy::indexing_slicing)]
fn test_executables(json_lines: &str) -> Vec<TestExe> {
    let mut out = Vec::new();
    for line in json_lines.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-artifact" || v["profile"]["test"] != true {
            continue;
        }
        let Some(exe) = v["executable"].as_str() else {
            continue;
        };
        let manifest = std::path::PathBuf::from(v["manifest_path"].as_str().unwrap_or(""));
        let dir = manifest
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        // `path+file:///…/dir#name@version`, or `…/dir#version` when the package
        // is named like its folder.
        let package = v["package_id"]
            .as_str()
            .and_then(|id| id.rsplit_once('#'))
            .and_then(|(_, rest)| rest.split_once('@').map(|(n, _)| n.to_owned()))
            .unwrap_or_else(|| {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let src = v["target"]["src_path"]
            .as_str()
            .map(|p| {
                std::path::Path::new(p)
                    .strip_prefix(&dir)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| p.to_owned())
            })
            .unwrap_or_default();
        out.push(TestExe {
            exe: exe.into(),
            dir,
            package,
            target: v["target"]["name"].as_str().unwrap_or("").to_owned(),
            kind: v["target"]["kind"][0].as_str().unwrap_or("").to_owned(),
            src,
        });
    }
    out
}

/// The `ui` test modules: the `.rs` files in its folder except `main.rs` (the
/// `common` helpers are a folder), sorted.
fn ui_modules(dir: &std::path::Path) -> Result<Vec<String>, String> {
    let mut m: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".rs")?.to_owned();
            (e.path().is_file() && stem != "main" && stem != "common").then_some(stem)
        })
        .collect();
    m.sort();
    Ok(m)
}

/// Each module with the modules whose names end in it (`x_m::` contains `m::`), so
/// that a filter `m::` skips them.
fn module_filters(modules: &[String]) -> Vec<(String, Vec<String>)> {
    modules
        .iter()
        .map(|m| {
            let suffix = format!("_{m}");
            let skips = modules
                .iter()
                .filter(|o| *o != m && (o.ends_with(&suffix) || o.ends_with(m.as_str())))
                .cloned()
                .collect();
            (m.clone(), skips)
        })
        .collect()
}

/// Run one test executable as cargo does (in its package's folder).
fn run_test(t: &TestExe, args: &[String]) -> Result<(), String> {
    let status = Command::new(&t.exe)
        .args(args)
        .current_dir(&t.dir)
        .env("CARGO_MANIFEST_DIR", &t.dir)
        .env("CARGO_PKG_NAME", &t.package)
        .env("CARGO", env!("CARGO"))
        .status()
        .map_err(|e| format!("test: cannot run {}: {e}", t.exe.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("test failed: {} {}", t.label(), args.join(" ")))
    }
}

const WASM: &str = "wasm32-unknown-unknown";

/// Whether the standard library for `target` is installed in the active toolchain.
fn target_installed(target: &str) -> bool {
    Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .is_some_and(|root| {
            std::path::Path::new(&root)
                .join("lib/rustlib")
                .join(target)
                .join("lib")
                .is_dir()
        })
}

/// mobile-capture.md AT17: build the phone client for wasm32, installing the target
/// when it is missing; skipped with a clear message only when it cannot be installed.
fn wasm_build() -> Result<(), String> {
    if !target_installed(WASM) {
        let added = Command::new("rustup")
            .args(["target", "add", WASM])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !added || !target_installed(WASM) {
            println!(
                "==> wasm build: SKIPPED (the {WASM} target is not installed and `rustup target add {WASM}` failed in this environment; the browser layer was type-checked natively with --features web-check)"
            );
            return Ok(());
        }
    }
    cargo(
        "wasm build",
        &["build", "-p", "moneykeep-capture", "--target", WASM],
    )
}

/// mobile-capture.md revision 3: write the phone app's icons (the calculator
/// drawing, crates/mk-ui) into apps/moneykeep-capture/web/icons.
fn capture_icons() -> Result<(), String> {
    println!("==> capture icons");
    let status = Command::new(env!("CARGO"))
        .args(["test", "-p", "mk-ui", "--test", "capture_icons"])
        .env("MK_UPDATE_ICONS", "1")
        .status()
        .map_err(|e| format!("failed to start cargo: {e}"))?;
    if status.success() {
        println!("icons written to apps/moneykeep-capture/web/icons");
        Ok(())
    } else {
        Err("generating the icons failed".into())
    }
}

/// release-packaging.md RP6 runs in `cargo test` (xtask's tests) when makensis is
/// installed; say so when it is not.
fn installer_script_note() {
    if release::find_makensis().is_none() {
        println!(
            "==> installer script: SKIPPED (makensis is not installed; installer/moneykeep.nsi was not compiled)"
        );
    } else {
        println!("==> installer script: compiled by xtask's test rp6_installer_script_compiles");
    }
}

fn tool_runs(program: &str, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// mobile-capture.md revision 2, section D: install the wasm32 target and Trunk when
/// missing, build the phone app into `apps/moneykeep-capture/dist`, then build
/// `moneykeep-server` (release) with `MK_CAPTURE_DIST` pointing at it so the app is
/// embedded. Each step that cannot run says why and what to do.
fn dist_capture(args: &[String]) -> Result<(), String> {
    let (skip, rest) = wasm_opt::take_flag(args);
    if let Some(other) = rest.first() {
        return Err(format!("dist-capture: unknown option {other}"));
    }
    let root = release::workspace_root()?;
    let (dist, _) = phone_app(&root, skip)?;
    dist_capture_server(&root, &dist)
}

/// The phone app built with Trunk into `apps/moneykeep-capture/dist` (installing the
/// wasm32 target and Trunk when missing), its program shrunk with `wasm-opt`
/// (revision 18; `skip_wasm_opt`: `--no-wasm-opt`); returns that folder and the
/// program's sizes.
pub(crate) fn phone_app(
    root: &std::path::Path,
    skip_wasm_opt: bool,
) -> Result<(std::path::PathBuf, wasm_opt::Sizes), String> {
    // A phone app already built by `cargo xtask phone-app --out DIR` (release.yml builds
    // it once, on Linux, for the Windows and macOS jobs).
    if let Some(dir) = std::env::var_os(PHONE_APP_ENV).filter(|d| !d.is_empty()) {
        let dir = std::path::PathBuf::from(dir);
        println!(
            "==> phone app: prebuilt in {} ({PHONE_APP_ENV})",
            dir.display()
        );
        return prebuilt_phone_app(&dir);
    }
    // Before anything is built: without wasm-opt the build stops at once.
    let tool = wasm_opt::find();
    if tool.is_none() && !skip_wasm_opt {
        return Err(wasm_opt::missing_message());
    }
    let app_dir = root.join("apps/moneykeep-capture");
    let dist = app_dir.join("dist");
    println!("==> wasm32 target");
    if !target_installed(WASM) {
        let added = Command::new("rustup")
            .args(["target", "add", WASM])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !added || !target_installed(WASM) {
            return Err(format!(
                "the {WASM} target is not installed and `rustup target add {WASM}` failed. \
                 Install it (rustup target add {WASM}) on a computer with internet access and run \
                 `cargo xtask dist-capture` (or `release`) again. The server built without the phone app serves a \
                 page that says so."
            ));
        }
    }
    println!("==> trunk");
    if !tool_runs("trunk", "--version") {
        println!("trunk is missing; installing it with `cargo install trunk --locked`");
        let ok = Command::new(env!("CARGO"))
            .args(["install", "trunk", "--locked"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok || !tool_runs("trunk", "--version") {
            return Err(
                "Trunk could not be installed (`cargo install trunk --locked` failed). Install it \
                 from https://trunkrs.dev and run `cargo xtask dist-capture` (or `release`) again."
                    .into(),
            );
        }
    }
    println!("==> phone app (trunk build --release)");
    let ok = Command::new("trunk")
        .args(["build", "--release"])
        .current_dir(&app_dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok || !dist.join("index.html").is_file() {
        return Err(format!(
            "`trunk build --release` in {} did not produce dist/index.html",
            app_dir.display()
        ));
    }
    let sizes = wasm_opt::shrink(&dist, tool.as_deref(), skip_wasm_opt)?;
    Ok((dist, sizes))
}

/// The environment variable naming a phone app made by `cargo xtask phone-app --out`.
pub(crate) const PHONE_APP_ENV: &str = "MK_PHONE_APP";
/// In such a folder: the built app (what the server embeds) and its program's sizes.
pub(crate) const PHONE_APP_DIR: &str = "app";
pub(crate) const PHONE_SIZES_FILE: &str = "phone-program.json";

/// `cargo xtask phone-app --out DIR [--no-wasm-opt]`: the phone app built once into
/// `DIR/app`, with its program's sizes in `DIR/phone-program.json`, for `release`,
/// `installer` and `packaging/macos/package.sh` to use through `MK_PHONE_APP=DIR`
/// (macos-release.md: the release builds it on Linux, so the Mac needs no Trunk or
/// wasm-opt).
fn phone_app_task(args: &[String]) -> Result<(), String> {
    let (skip, rest) = wasm_opt::take_flag(args);
    let mut out: Option<std::path::PathBuf> = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(it.next().ok_or("--out needs a folder")?.into()),
            other => return Err(format!("phone-app: unknown option {other}")),
        }
    }
    let out = out.ok_or("phone-app: give the folder to write: --out DIR")?;
    let root = release::workspace_root()?;
    let (dist, sizes) = phone_app(&root, skip)?;
    save_phone_app(&dist, sizes, &out)?;
    println!("\nxtask phone-app: {} ({})", out.display(), sizes.line());
    Ok(())
}

/// Copy the built phone app into `out/app` and write `out/phone-program.json`.
fn save_phone_app(
    dist: &std::path::Path,
    sizes: wasm_opt::Sizes,
    out: &std::path::Path,
) -> Result<(), String> {
    let app = out.join(PHONE_APP_DIR);
    if app.exists() {
        std::fs::remove_dir_all(&app)
            .map_err(|e| format!("cannot empty {}: {e}", app.display()))?;
    }
    copy_dir(dist, &app)?;
    let text = serde_json::to_string_pretty(&sizes.json()).map_err(|e| e.to_string())?;
    let file = out.join(PHONE_SIZES_FILE);
    std::fs::write(&file, text + "\n").map_err(|e| format!("cannot write {}: {e}", file.display()))
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot list {}: {e}", from.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("cannot list {}: {e}", from.display()))?;
        let (src, dest) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dest)?;
        } else {
            std::fs::copy(&src, &dest)
                .map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// A phone app made by `phone-app --out dir`: `dir/app` and the sizes it recorded.
fn prebuilt_phone_app(
    dir: &std::path::Path,
) -> Result<(std::path::PathBuf, wasm_opt::Sizes), String> {
    let app = dir.join(PHONE_APP_DIR);
    if !app.join("index.html").is_file() || !app.join(wasm_opt::PROGRAM).is_file() {
        return Err(format!(
            "{PHONE_APP_ENV}={}: no phone app there (make it with cargo xtask phone-app --out {})",
            dir.display(),
            dir.display()
        ));
    }
    let file = dir.join(PHONE_SIZES_FILE);
    let v: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| format!("{} is missing or not JSON", file.display()))?;
    let num = |k: &str| v.get(k).and_then(serde_json::Value::as_u64);
    let sizes = wasm_opt::Sizes {
        before: num("before").ok_or_else(|| format!("{}: no before", file.display()))?,
        after: num("after").ok_or_else(|| format!("{}: no after", file.display()))?,
        optimized: v.get("wasm_opt").and_then(serde_json::Value::as_bool) == Some(true),
    };
    println!("{}", sizes.line());
    Ok((app, sizes))
}

fn dist_capture_server(root: &std::path::Path, dist: &std::path::Path) -> Result<(), String> {
    println!("==> moneykeep-server with the phone app embedded");
    // Built in its own target folder (revision 4): a service still running the
    // program in target/release keeps that file locked on Windows, so the build
    // must not need to replace it.
    let build_dir = root.join("target").join("capture-build");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "-p",
            "moneykeep-server",
            "--target-dir",
        ])
        .arg(&build_dir)
        .env("MK_CAPTURE_DIST", dist)
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to start cargo: {e}"))?;
    if !status.success() {
        return Err("building moneykeep-server failed".into());
    }
    let name = server_program_name();
    let built = build_dir.join("release").join(name);
    let release = root.join("target").join("release");
    let dest = place_server(&built, &release, now_secs())?;
    println!(
        "\nxtask dist-capture: {} serves the phone app (built from {})",
        dest.display(),
        dist.display()
    );
    let service = Command::new(&dest)
        .args(["services", "status"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| {
            v.get("server_path")
                .and_then(|p| p.as_str())
                .map(str::to_owned)
        });
    println!(
        "{}",
        update_guidance(service.as_deref(), &root.join("target"))
    );
    Ok(())
}

fn server_program_name() -> &'static str {
    if cfg!(windows) {
        "moneykeep-server.exe"
    } else {
        "moneykeep-server"
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Put the built program next to the desktop program (`release`). A copy that is
/// still running (and so locked on Windows) cannot be deleted but can be renamed: it
/// is set aside as `<name>.old-<secs>` and removed by a later run.
fn place_server(
    built: &std::path::Path,
    release: &std::path::Path,
    secs: u64,
) -> Result<std::path::PathBuf, String> {
    let name = built
        .file_name()
        .ok_or("the built program has no file name")?
        .to_string_lossy()
        .into_owned();
    std::fs::create_dir_all(release)
        .map_err(|e| format!("cannot create {}: {e}", release.display()))?;
    // Earlier set-aside copies that are no longer running.
    if let Ok(entries) = std::fs::read_dir(release) {
        for e in entries.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}.old-"))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let dest = release.join(&name);
    if dest.exists() && std::fs::remove_file(&dest).is_err() {
        let aside = release.join(format!("{name}.old-{secs}"));
        std::fs::rename(&dest, &aside).map_err(|e| {
            format!(
                "{} is in use and could not be set aside ({e}). Press Update now in MoneyKeep, or restart the computer, then run cargo xtask dist-capture again.",
                dest.display()
            )
        })?;
    }
    std::fs::copy(built, &dest).map_err(|e| format!("cannot copy to {}: {e}", dest.display()))?;
    Ok(dest)
}

/// What to do next: press Update server, and, when the service still runs the
/// program in the build folder, that Update server moves it to the install folder.
fn update_guidance(service_path: Option<&str>, target: &std::path::Path) -> String {
    let in_build = service_path.is_some_and(|p| {
        let p = std::path::Path::new(p);
        let canon = |x: &std::path::Path| x.canonicalize().unwrap_or_else(|_| x.to_path_buf());
        // Both as given, too: on Windows a path that can't be canonicalized (a missing
        // program) keeps its plain form while the target gains the `\\?\` prefix.
        canon(p).starts_with(canon(target)) || p.starts_with(target)
    });
    if in_build {
        format!(
            "Built — open MoneyKeep and press Update now (Settings > Phone capture). The capture service still runs the program in the build folder ({}); Update now moves it to its own copy in the install folder, so later builds never touch the running program.",
            service_path.unwrap_or_default()
        )
    } else {
        "Built — open MoneyKeep and press Update now (Settings > Phone capture).".to_owned()
    }
}

/// mobile-capture.md AT32 and revision 9 (tests 61, 66): the phone app's service
/// worker and page script, tested with Node when available.
fn service_worker_test() -> Result<(), String> {
    if !tool_runs("node", "--version") {
        println!(
            "==> phone page scripts test: SKIPPED (Node.js is not installed; apps/moneykeep-capture/web/sw.js and shell.js are untested here)"
        );
        return Ok(());
    }
    println!("==> phone page scripts test (service worker, shell)");
    let status = Command::new("node")
        .args([
            "--test",
            "apps/moneykeep-capture/tests-js/sw.test.mjs",
            "apps/moneykeep-capture/tests-js/shell.test.mjs",
            "apps/moneykeep-capture/tests-js/loading.test.mjs",
        ])
        .status()
        .map_err(|e| format!("phone page scripts test: failed to start node: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("phone page scripts test failed".into())
    }
}

fn licence_policy() -> Result<(), String> {
    let installed = Command::new(env!("CARGO"))
        .args(["deny", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if installed {
        cargo(
            "licence policy",
            &["deny", "check", "licenses", "bans", "sources"],
        )
    } else if std::env::var_os("CI").is_some() {
        Err("cargo-deny is required in CI: cargo install cargo-deny --locked".into())
    } else {
        println!(
            "==> licence policy: SKIPPED (cargo-deny not installed; \
             run `cargo install cargo-deny --locked`)"
        );
        Ok(())
    }
}

fn cargo(label: &str, args: &[&str]) -> Result<(), String> {
    println!("==> {label}");
    let status = Command::new(env!("CARGO"))
        .args(args)
        .status()
        .map_err(|e| format!("{label}: failed to start cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed"))
    }
}

fn step(label: &str, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    println!("==> {label}");
    f().map_err(|e| format!("{label}:\n{e}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// mobile-capture.md revision 4: the built program is placed next to the desktop
    /// program; an existing copy is replaced, earlier set-aside copies are removed.
    #[test]
    fn places_the_built_server() {
        let d = scratch("place");
        let built = d.join("build").join("moneykeep-server");
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"NEW").unwrap();
        let release = d.join("release");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(release.join("moneykeep-server"), b"OLD").unwrap();
        std::fs::write(release.join("moneykeep-server.old-1"), b"OLDER").unwrap();
        let dest = place_server(&built, &release, 2).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"NEW");
        assert!(!release.join("moneykeep-server.old-1").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `phone-app --out DIR` writes what `MK_PHONE_APP=DIR` reads back (release.yml builds
    /// the phone app once for the Windows and macOS jobs).
    #[test]
    fn a_saved_phone_app_is_read_back() {
        let d = scratch("phone");
        let dist = d.join("dist");
        std::fs::create_dir_all(dist.join("icons")).unwrap();
        std::fs::write(dist.join("index.html"), b"<html>").unwrap();
        std::fs::write(dist.join(wasm_opt::PROGRAM), b"wasm").unwrap();
        std::fs::write(dist.join("icons").join("a.png"), b"png").unwrap();
        let sizes = wasm_opt::Sizes {
            before: 10,
            after: 4,
            optimized: true,
        };
        let out = d.join("out");
        save_phone_app(&dist, sizes, &out).unwrap();
        // Saving again replaces the app.
        save_phone_app(&dist, sizes, &out).unwrap();
        let (app, read) = prebuilt_phone_app(&out).unwrap();
        assert_eq!(app, out.join(PHONE_APP_DIR));
        assert_eq!(read, sizes);
        assert_eq!(
            std::fs::read(app.join("icons").join("a.png")).unwrap(),
            b"png"
        );
        // The sizes stay outside the folder the server embeds.
        assert!(!app.join(PHONE_SIZES_FILE).exists());
        let e = prebuilt_phone_app(&d.join("missing")).unwrap_err();
        assert!(e.contains("cargo xtask phone-app --out"), "{e}");
        std::fs::remove_file(out.join(PHONE_SIZES_FILE)).unwrap();
        assert!(prebuilt_phone_app(&out).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ui_modules_run_one_per_process() {
        let root = release::workspace_root().unwrap();
        let m = ui_modules(&root.join(UI_TESTS)).unwrap();
        assert!(m.contains(&"capture_r17".to_owned()), "{m:?}");
        assert!(!m.iter().any(|x| x == "main" || x == "common"));
        // Every module file is one main.rs declares.
        let main = std::fs::read_to_string(root.join(UI_TESTS).join("main.rs")).unwrap();
        for x in &m {
            assert!(main.contains(&format!("mod {x};")), "{x}");
        }
        let f = module_filters(&["r4".into(), "app_desktop_r4".into(), "app_desktop".into()]);
        assert_eq!(f[0], ("r4".to_owned(), vec!["app_desktop_r4".to_owned()]));
        assert_eq!(f[1].1, Vec::<String>::new());
        assert_eq!(f[2].1, Vec::<String>::new());
        let json = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/crates/mk-views#0.1.0","manifest_path":"/w/crates/mk-views/Cargo.toml","target":{"kind":["test"],"name":"ui","src_path":"/w/crates/mk-views/tests/ui/main.rs"},"profile":{"test":true},"executable":"/w/target/debug/deps/ui-1"}
{"reason":"compiler-artifact","package_id":"path+file:///w/apps/x#moneykeep-x@0.1.0","manifest_path":"/w/apps/x/Cargo.toml","target":{"kind":["lib"],"name":"x","src_path":"/w/apps/x/src/lib.rs"},"profile":{"test":true},"executable":"/w/target/debug/deps/x-2"}
{"reason":"compiler-artifact","package_id":"p","manifest_path":"/w/apps/x/Cargo.toml","target":{"kind":["example"],"name":"e","src_path":"/w/apps/x/examples/e.rs"},"profile":{"test":false},"executable":"/w/target/debug/examples/e"}
{"reason":"build-finished","success":true}"#;
        let t = test_executables(json);
        assert_eq!(t.len(), 2);
        assert!(t[0].is_ui(), "{t:?}");
        assert_eq!(t[0].dir, std::path::PathBuf::from("/w/crates/mk-views"));
        assert_eq!(t[1].package, "moneykeep-x");
        assert_eq!(t[1].src, "src/lib.rs");
        assert!(!t[1].is_ui());
    }

    #[test]
    fn guidance_mentions_the_build_folder_service() {
        let d = scratch("guide");
        let target = d.join("target");
        std::fs::create_dir_all(target.join("release")).unwrap();
        let in_build = target.join("release").join("moneykeep-server");
        let g = update_guidance(Some(&in_build.display().to_string()), &target);
        assert!(
            g.starts_with(
                "Built — open MoneyKeep and press Update now (Settings > Phone capture)."
            ),
            "{g}"
        );
        assert!(
            g.contains("still runs the program in the build folder"),
            "{g}"
        );
        let g = update_guidance(Some("/opt/elsewhere/moneykeep-server"), &target);
        assert_eq!(
            g,
            "Built — open MoneyKeep and press Update now (Settings > Phone capture)."
        );
        assert_eq!(update_guidance(None, &target), g);
        let _ = std::fs::remove_dir_all(&d);
    }
}
