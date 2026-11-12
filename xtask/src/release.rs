//! `cargo xtask release` and `cargo xtask installer` (release-packaging.md).
//!
//! - `release [--dry-run] [--out DIR]`: the phone app (wasm, Trunk), then the capture
//!   server with it embedded, the desktop app, the CLI and keep-mcp, all in release
//!   mode from this checkout, copied into `DIR/MoneyKeep-<version>/` (default
//!   `dist/`) with the licences, `THIRD-PARTY-LICENSES.txt` (security-review-1 B4) and
//!   `version.json`. `--dry-run` builds nothing: it lists the steps and files and
//!   writes only `version.json` and `THIRD-PARTY-LICENSES.txt`.
//! - `installer [--skip-release]` (Windows): `release`, then `makensis` on
//!   `installer/moneykeep.nsi`, giving `dist/moneykeep-<version>-windows-x64-setup.exe`
//!   (the name keep-launcher installs) and `dist/SHA256SUMS.txt` (release-packaging.md
//!   revision 2); signed with `signtool` when `MK_SIGN_THUMBPRINT` (a certificate in
//!   the user's store) or `MK_SIGN_CERT` (a .pfx without a password, in a file only
//!   the user can read) is set, before the checksums are taken. A password is never
//!   put on a command line (release-prep.md §4).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../crates/mk-capture-api/build/capture_version.rs"]
mod capture_version;

/// The programs in a release: (cargo package, program name without `.exe`).
pub const PROGRAMS: [(&str, &str); 4] = [
    ("moneykeep-app", "moneykeep-app"),
    ("moneykeep-server", "moneykeep-server"),
    ("moneykeep-cli", "moneykeep"),
    ("keep-mcp", "keep-mcp"),
];

/// The documents shipped beside the programs.
pub const DOCUMENTS: [&str; 4] = ["LICENSE-APACHE", "LICENSE-MIT", "NOTICE", "ATTRIBUTION.md"];

/// What `release` will make.
#[derive(Debug, Clone)]
pub struct Plan {
    pub version: String,
    pub capture_version: String,
    pub commit: Option<String>,
    pub built: String,
    /// `<out>/MoneyKeep-<version>`.
    pub folder: PathBuf,
    /// (source, file name in the folder)
    pub files: Vec<(PathBuf, String)>,
    pub steps: Vec<String>,
    /// The phone program's size before and after wasm-opt (mobile-capture.md
    /// revision 18), once built.
    pub phone_program: Option<crate::wasm_opt::Sizes>,
}

pub fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask has no parent folder".to_owned())
}

/// `name` with this system's program suffix (`.exe` on Windows).
pub fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Days since 1970-01-01 as (year, month, day) (the proleptic Gregorian calendar).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t / 60 % 60,
        t % 60
    )
}

/// When the build was made: `SOURCE_DATE_EPOCH` when set (reproducible builds),
/// otherwise now.
fn built_secs() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
        })
}

/// The plan for `root`, with the release folder under `out`.
pub fn plan(root: &Path, out: &Path) -> Result<Plan, String> {
    let cargo = OsString::from(env!("CARGO"));
    let capture_version::Computed {
        capture_version,
        paths,
        note,
        ..
    } = capture_version::compute(
        &cargo,
        &root.join("Cargo.toml"),
        root,
        env!("CARGO_PKG_VERSION"),
    )?;
    if let Some(n) = note {
        println!("note: {n}");
    }
    let version = capture_version::package_version_of(&capture_version).to_owned();
    let folder = out.join(format!("MoneyKeep-{version}"));
    let release = root.join("target").join("release");
    let mut files: Vec<(PathBuf, String)> = PROGRAMS
        .iter()
        .map(|(_, name)| (release.join(exe(name)), exe(name)))
        .collect();
    files.extend(DOCUMENTS.iter().map(|d| (root.join(d), (*d).to_owned())));
    let packages: Vec<&str> = PROGRAMS.iter().map(|(p, _)| *p).collect();
    let steps = vec![
        format!(
            "phone app: install the {} target and Trunk when missing, then trunk build --release in apps/moneykeep-capture",
            crate::WASM
        ),
        format!(
            "wasm-opt -Oz on apps/moneykeep-capture/dist/{} (Binaryen; --no-wasm-opt skips it), both sizes into version.json",
            crate::wasm_opt::PROGRAM
        ),
        format!(
            "cargo build --release -p {} (MK_CAPTURE_DIST=apps/moneykeep-capture/dist: the server embeds the phone app)",
            packages.join(" -p ")
        ),
        format!(
            "check: moneykeep-server version reports capture_version {capture_version} with the phone app"
        ),
        format!("copy the programs and documents into {}", folder.display()),
        format!(
            "write {} (every crate compiled into the programs, from cargo metadata and the crates' licence files)",
            crate::licences::FILE
        ),
        format!(
            "write version.json (capture version over: {})",
            paths.join(", ")
        ),
    ];
    Ok(Plan {
        version,
        capture_version,
        commit: git(root, &["rev-parse", "HEAD"]),
        built: rfc3339(built_secs()),
        folder,
        files,
        steps,
        phone_program: None,
    })
}

/// `version.json`: `{"version", "capture_version", "commit", "built",
/// "phone_program_bytes"}` (the last `{"before", "after", "wasm_opt"}`, revision 18;
/// null in a dry run).
pub fn version_json(p: &Plan) -> serde_json::Value {
    serde_json::json!({
        "version": p.version,
        "capture_version": p.capture_version,
        "commit": p.commit,
        "built": p.built,
        "phone_program_bytes": p.phone_program.map(|s| s.json()),
    })
}

fn write_version(p: &Plan) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&p.folder)
        .map_err(|e| format!("cannot create {}: {e}", p.folder.display()))?;
    let path = p.folder.join("version.json");
    let text = serde_json::to_string_pretty(&version_json(p)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n")
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// `cargo xtask release`.
pub fn release(args: &[String]) -> Result<(), String> {
    let mut dry_run = false;
    let mut out: Option<PathBuf> = None;
    let (skip_wasm_opt, args) = crate::wasm_opt::take_flag(args);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a folder")?)),
            other => return Err(format!("release: unknown option {other}")),
        }
    }
    let root = workspace_root()?;
    let out = out.unwrap_or_else(|| root.join("dist"));
    run_release(&root, &out, dry_run, skip_wasm_opt).map(|_| ())
}

/// Carry out (or, with `dry_run`, list) a release; returns the plan.
pub fn run_release(
    root: &Path,
    out: &Path,
    dry_run: bool,
    skip_wasm_opt: bool,
) -> Result<Plan, String> {
    let mut p = plan(root, out)?;
    if dry_run {
        println!("==> release (dry run): MoneyKeep {}", p.version);
        for (i, s) in p.steps.iter().enumerate() {
            println!("  {}. {s}", i + 1);
        }
        println!("  files in {}:", p.folder.display());
        for (_, name) in &p.files {
            println!("    {name}");
        }
        println!("    {}", crate::licences::FILE);
        println!("    version.json");
        println!("  installer (cargo xtask installer, on Windows), in dist:");
        for name in installer_outputs(&p.version) {
            println!("    {name}");
        }
        println!(
            "    sign: {}",
            sign_line(
                signing_from(|k| std::env::var_os(k)),
                Path::new(&setup_name(&p.version))
            )
        );
        let (t, n) = crate::licences::write(root, &p.folder, &p.version)?;
        println!("wrote {} ({n} crates)", t.display());
        let v = write_version(&p)?;
        println!(
            "wrote {} (capture_version {})",
            v.display(),
            p.capture_version
        );
        return Ok(p);
    }
    let (dist, sizes) = crate::phone_app(root, skip_wasm_opt)?;
    p.phone_program = Some(sizes);
    println!("==> release build (server with the phone app, desktop app, CLI, keep-mcp)");
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "--release"]);
    for (pkg, _) in PROGRAMS {
        cmd.args(["-p", pkg]);
    }
    let status = cmd
        .env("MK_CAPTURE_DIST", &dist)
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to start cargo: {e}"))?;
    if !status.success() {
        return Err("the release build failed".into());
    }
    // The server must carry the phone app and this checkout's capture version (the
    // desktop is built from the same checkout in the same build).
    println!("==> check the capture server");
    let server = root.join("target/release").join(exe("moneykeep-server"));
    let out_v = Command::new(&server)
        .arg("version")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", server.display()))?;
    let v: serde_json::Value = serde_json::from_slice(&out_v.stdout)
        .map_err(|e| format!("moneykeep-server version was not understood: {e}"))?;
    if v.get("capture_version").and_then(|c| c.as_str()) != Some(p.capture_version.as_str()) {
        return Err(format!(
            "the built server reports capture version {} but this checkout is {}: a file changed during the build; run cargo xtask release again",
            v.get("capture_version").unwrap_or(&serde_json::Value::Null),
            p.capture_version
        ));
    }
    if v.get("has_app").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("the built server has no phone app built in".into());
    }
    println!("==> copy into {}", p.folder.display());
    if p.folder.exists() {
        std::fs::remove_dir_all(&p.folder)
            .map_err(|e| format!("cannot empty {}: {e}", p.folder.display()))?;
    }
    std::fs::create_dir_all(&p.folder)
        .map_err(|e| format!("cannot create {}: {e}", p.folder.display()))?;
    for (src, name) in &p.files {
        std::fs::copy(src, p.folder.join(name))
            .map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
    }
    let (t, n) = crate::licences::write(root, &p.folder, &p.version)?;
    println!("wrote {} ({n} crates)", t.display());
    write_version(&p)?;
    if let Some(s) = p.phone_program {
        println!("{}", s.line());
    }
    println!("\nxtask release: {}", p.folder.display());
    println!("Run MoneyKeep from this folder, or build the installer with cargo xtask installer.");
    Ok(p)
}

// ----- the Windows installer -------------------------------------------------------------

/// A semver version's parts: (major, minor, patch, pre-release identifiers); build
/// metadata (`+...`) is left out.
fn semver_parts(v: &str) -> Result<([u64; 3], Vec<&str>), String> {
    let v = v.split_once('+').map_or(v, |(a, _)| a);
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, p.split('.').collect()),
        None => (v, Vec::new()),
    };
    let nums: Vec<u64> = core
        .split('.')
        .map(|n| n.parse::<u64>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{v} is not a semver version (major.minor.patch)"))?;
    let &[a, b, c] = nums.as_slice() else {
        return Err(format!("{v} is not a semver version (major.minor.patch)"));
    };
    Ok(([a, b, c], pre))
}

/// The pre-release stage and number: `alpha.1` is (1, 1), `beta` (2, 0), `rc.2`
/// (3, 2); a release is (9, 0). Other pre-release forms are refused (the installer
/// orders only these).
fn stage(v: &str, pre: &[&str]) -> Result<(u64, u64), String> {
    let refuse = || {
        format!("{v}: the installer orders releases and alpha.N, beta.N and rc.N pre-releases only")
    };
    let rank = match pre.first() {
        None => return Ok((9, 0)),
        Some(&"alpha") => 1,
        Some(&"beta") => 2,
        Some(&"rc") => 3,
        Some(_) => return Err(refuse()),
    };
    match pre.get(1..) {
        Some([]) | None => Ok((rank, 0)),
        Some([n]) => n.parse::<u64>().map(|n| (rank, n)).map_err(|_| refuse()),
        Some(_) => Err(refuse()),
    }
}

/// The version as the four numbers Windows file versions use (release-prep.md §3):
/// `0.1.0-alpha.1` is `0.1.0.1` (the pre-release number is the fourth), `0.1.0` is
/// `0.1.0.0`.
pub fn version_quad(v: &str) -> Result<String, String> {
    let ([a, b, c], pre) = semver_parts(v)?;
    let (_, n) = stage(v, &pre)?;
    for x in [a, b, c, n] {
        if x > u64::from(u16::MAX) {
            return Err(format!("{v}: Windows versions take numbers up to 65535"));
        }
    }
    Ok(format!("{a}.{b}.{c}.{n}"))
}

/// The version as dotted numbers whose numeric comparison (NSIS's `VersionCompare`,
/// `dotted_cmp` in the tests) is semver order: `major.minor.patch.stage.number`, stage 1 alpha,
/// 2 beta, 3 rc, 9 a release. The installer's upgrade check compares these.
pub fn version_order(v: &str) -> Result<String, String> {
    let ([a, b, c], pre) = semver_parts(v)?;
    let (st, n) = stage(v, &pre)?;
    Ok(format!("{a}.{b}.{c}.{st}.{n}"))
}

#[cfg(test)]
/// (Tests only.) Dotted numbers compared part by part, missing parts as 0: what NSIS's
/// `VersionCompare` (WordFunc.nsh) does with `VERSION_ORDER` values.
pub fn dotted_cmp(x: &str, y: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (px, py) = (parts(x), parts(y));
    let n = px.len().max(py.len());
    (0..n)
        .map(|i| {
            px.get(i)
                .copied()
                .unwrap_or(0)
                .cmp(&py.get(i).copied().unwrap_or(0))
        })
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// The setup's file name: `<app>-<version>-<os>-<arch>-setup.exe`, the name
/// keep-launcher installs (release-packaging.md revision 2).
pub fn setup_name(version: &str) -> String {
    format!("moneykeep-{version}-windows-x64-setup.exe")
}

/// The checksum list published beside the setup.
pub const SUMS_FILE: &str = "SHA256SUMS.txt";

/// What `cargo xtask installer` writes into `dist/`: the setup, then the checksums.
pub fn installer_outputs(version: &str) -> Vec<String> {
    vec![setup_name(version), SUMS_FILE.to_owned()]
}

/// The SHA-256 of a file, as lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let Some(chunk) = buf.get(..n).filter(|c| !c.is_empty()) else {
            break;
        };
        hasher.update(chunk);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `SHA256SUMS.txt`'s text for the files `names` in `dir`: one `<sha256>  <name>`
/// line each (two spaces, as `sha256sum` writes).
pub fn sums_text(dir: &Path, names: &[String]) -> Result<String, String> {
    let mut text = String::new();
    for name in names {
        text.push_str(&sha256_file(&dir.join(name))?);
        text.push_str("  ");
        text.push_str(name);
        text.push('\n');
    }
    Ok(text)
}

/// Writes `dir/SHA256SUMS.txt` for the files `names` in `dir`.
pub fn write_sums(dir: &Path, names: &[String]) -> Result<PathBuf, String> {
    let sums = dir.join(SUMS_FILE);
    std::fs::write(&sums, sums_text(dir, names)?)
        .map_err(|e| format!("cannot write {}: {e}", sums.display()))?;
    Ok(sums)
}

/// `makensis`'s arguments (the `-` option form works on Windows and elsewhere).
pub fn makensis_args(
    version: &str,
    source: &Path,
    out_file: &Path,
    script: &Path,
) -> Result<Vec<OsString>, String> {
    let define = |k: &str, v: &str| OsString::from(format!("-D{k}={v}"));
    Ok(vec![
        OsString::from("-V2"),
        define("VERSION", version),
        define("VERSION_QUAD", &version_quad(version)?),
        define("VERSION_ORDER", &version_order(version)?),
        define("SOURCE_DIR", &source.display().to_string()),
        define("OUT_FILE", &out_file.display().to_string()),
        script.as_os_str().to_owned(),
    ])
}

/// `makensis` on `PATH`, or in NSIS's usual install folders on Windows.
pub fn find_makensis() -> Option<PathBuf> {
    let name = exe("makensis");
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&name))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        ["ProgramFiles(x86)", "ProgramFiles"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join("NSIS").join(&name))
            .find(|p| p.is_file())
    })
}

/// The licence page's text: NOTICE with Windows line ends.
pub fn licence_text(root: &Path) -> Result<String, String> {
    let notice = std::fs::read_to_string(root.join("NOTICE"))
        .map_err(|e| format!("cannot read NOTICE: {e}"))?;
    Ok(notice.replace("\r\n", "\n").replace('\n', "\r\n"))
}

/// The installer's own inputs in the release folder: `licence.txt` and
/// `moneykeep.ico` (the calculator drawing from crates/mk-ui, written by its
/// app_ico test).
fn installer_inputs(root: &Path, folder: &Path) -> Result<(), String> {
    std::fs::write(folder.join("licence.txt"), licence_text(root)?)
        .map_err(|e| format!("cannot write licence.txt: {e}"))?;
    println!("==> icon (moneykeep.ico from the calculator drawing)");
    let ico = folder.join("moneykeep.ico");
    let status = Command::new(env!("CARGO"))
        .args(["test", "-p", "mk-ui", "--test", "app_ico"])
        .env("MK_ICO_OUT", &ico)
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to start cargo: {e}"))?;
    if !status.success() || !ico.is_file() {
        return Err("making moneykeep.ico failed".into());
    }
    Ok(())
}

/// `signtool` on `PATH`, or the newest one in the Windows SDK.
fn find_signtool() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("signtool.exe"))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        let kits = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?)
            .join("Windows Kits")
            .join("10")
            .join("bin");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&kits)
            .ok()?
            .flatten()
            .map(|e| e.path().join("x64").join("signtool.exe"))
            .filter(|p| p.is_file())
            .collect();
        found.sort();
        found.pop()
    })
}

/// How the programs are signed (release-prep.md §4): never with a password on a
/// command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignWith {
    /// `MK_SIGN_THUMBPRINT`: a certificate in the user's certificate store.
    Thumbprint(String),
    /// `MK_SIGN_CERT`: a .pfx without a password, in a file only the user can read.
    File(OsString),
}

/// Signing settings: what to sign with, and the timestamp server
/// (`MK_SIGN_TIMESTAMP`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signing {
    pub with: SignWith,
    pub timestamp: String,
}

pub const SIGN_PASSWORD_REFUSED: &str = "MK_SIGN_PASSWORD is set, but signtool takes a certificate's password only on its command line, where other programs can read it. Install the certificate in your certificate store (certutil -user -importpfx <file>), set MK_SIGN_THUMBPRINT to its thumbprint and unset MK_SIGN_PASSWORD; or keep a copy without a password in a file only you can read and set MK_SIGN_CERT to it.";

/// Signing settings from the environment through `var` (the thumbprint wins over a
/// file); `None` when neither is set.
pub fn signing_from(var: impl Fn(&str) -> Option<OsString>) -> Result<Option<Signing>, String> {
    let get = |k: &str| var(k).filter(|v| !v.is_empty());
    let timestamp = get("MK_SIGN_TIMESTAMP")
        .map(|t| t.to_string_lossy().into_owned())
        .unwrap_or_else(|| "http://timestamp.digicert.com".to_owned());
    if let Some(t) = get("MK_SIGN_THUMBPRINT") {
        let t: String = t
            .to_string_lossy()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("MK_SIGN_THUMBPRINT is not a certificate thumbprint (hex digits)".into());
        }
        return Ok(Some(Signing {
            with: SignWith::Thumbprint(t),
            timestamp,
        }));
    }
    let Some(cert) = get("MK_SIGN_CERT") else {
        return Ok(None);
    };
    if get("MK_SIGN_PASSWORD").is_some() {
        return Err(SIGN_PASSWORD_REFUSED.into());
    }
    Ok(Some(Signing {
        with: SignWith::File(cert),
        timestamp,
    }))
}

fn signing() -> Result<Option<(PathBuf, Signing)>, String> {
    let Some(s) = signing_from(|k| std::env::var_os(k))? else {
        return Ok(None);
    };
    let tool = find_signtool().ok_or(
        "signing is set up (MK_SIGN_THUMBPRINT or MK_SIGN_CERT) but signtool.exe was not found: install the Windows SDK's signing tools, or put signtool on PATH",
    )?;
    Ok(Some((tool, s)))
}

/// signtool's arguments for `file`.
pub fn sign_args(s: &Signing, file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "sign",
        "/fd",
        "SHA256",
        "/td",
        "SHA256",
        "/tr",
        s.timestamp.as_str(),
    ]
    .iter()
    .map(OsString::from)
    .collect();
    match &s.with {
        SignWith::Thumbprint(t) => {
            args.push("/sha1".into());
            args.push(t.into());
        }
        SignWith::File(f) => {
            args.push("/f".into());
            args.push(f.clone());
        }
    }
    args.push(file.as_os_str().to_owned());
    args
}

/// The signing step as `release --dry-run` prints it.
pub fn sign_line(s: Result<Option<Signing>, String>, file: &Path) -> String {
    match s {
        Ok(Some(s)) => {
            let args: Vec<String> = sign_args(&s, file)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            format!("signtool {}", args.join(" "))
        }
        Ok(None) => {
            "not signed (set MK_SIGN_THUMBPRINT, or MK_SIGN_CERT to a .pfx without a password)"
                .into()
        }
        Err(e) => format!("cannot sign: {e}"),
    }
}

fn sign(tool: &Path, s: &Signing, file: &Path) -> Result<(), String> {
    println!("==> sign {}", file.display());
    let status = Command::new(tool)
        .args(sign_args(s, file))
        .status()
        .map_err(|e| format!("cannot run signtool: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("signing {} failed", file.display()))
    }
}

/// The macOS packaging script (macos-release.md Part P).
pub const MAC_PACKAGE: &str = "packaging/macos/package.sh";

/// `cargo xtask installer` on a Mac (P7): `packaging/macos/package.sh` with the same
/// options (`--arch`, `--skip-build`, `--no-wasm-opt`, `--dry-run`).
fn mac_installer(args: &[String]) -> Result<(), String> {
    let root = workspace_root()?;
    println!("==> {MAC_PACKAGE} {}", args.join(" "));
    let status = Command::new("bash")
        .arg(root.join(MAC_PACKAGE))
        .args(args)
        .current_dir(&root)
        .status()
        .map_err(|e| format!("cannot run {MAC_PACKAGE}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{MAC_PACKAGE} failed"))
    }
}

pub const NO_MAKENSIS: &str = "makensis (NSIS) was not found. Install NSIS 3 from https://nsis.sourceforge.io/Download (or: winget install NSIS.NSIS), open a new terminal, and run cargo xtask installer again.";

/// `cargo xtask installer`.
pub fn installer(args: &[String]) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        return mac_installer(args);
    }
    let (skip_wasm_opt, args) = crate::wasm_opt::take_flag(args);
    let skip_release = args.iter().any(|a| a == "--skip-release");
    if let Some(other) = args.iter().find(|a| *a != "--skip-release") {
        return Err(format!("installer: unknown option {other}"));
    }
    if !cfg!(windows) {
        return Err("cargo xtask installer makes the Windows setup on Windows and the macOS disk image on a Mac. Here, cargo xtask release builds the programs for this system.".into());
    }
    let makensis = find_makensis().ok_or(NO_MAKENSIS)?;
    let sign_with = signing()?;
    let root = workspace_root()?;
    let out = root.join("dist");
    let p = if skip_release {
        let p = plan(&root, &out)?;
        if !p.folder.join("version.json").is_file() {
            return Err(format!(
                "{} has no release: run cargo xtask installer without --skip-release",
                p.folder.display()
            ));
        }
        p
    } else {
        run_release(&root, &out, false, skip_wasm_opt)?
    };
    if let Some((tool, s)) = &sign_with {
        for name in ["moneykeep-app", "moneykeep-server", "moneykeep", "keep-mcp"] {
            sign(tool, s, &p.folder.join(exe(name)))?;
        }
    }
    installer_inputs(&root, &p.folder)?;
    let setup = out.join(setup_name(&p.version));
    println!("==> makensis installer/moneykeep.nsi");
    let status = Command::new(&makensis)
        .args(makensis_args(
            &p.version,
            &p.folder,
            &setup,
            &root.join("installer").join("moneykeep.nsi"),
        )?)
        .current_dir(&root)
        .status()
        .map_err(|e| format!("cannot run makensis: {e}"))?;
    if !status.success() || !setup.is_file() {
        return Err("makensis failed".into());
    }
    match &sign_with {
        Some((tool, s)) => sign(tool, s, &setup)?,
        None => println!("Not signed: Windows SmartScreen will warn on other computers."),
    }
    // After signing: the checksum is of the file that is published.
    let sums = write_sums(&out, &[setup_name(&p.version)])?;
    println!("\nxtask installer: {}", setup.display());
    println!("                 {}", sums.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// RP1: the dry run lists the steps and files and writes version.json, whose
    /// capture version is the one built into the server and the desktop (both take
    /// it from mk-capture-api).
    #[test]
    fn rp1_release_dry_run_writes_version_json() {
        let out = tempfile::tempdir().unwrap();
        let root = workspace_root().unwrap();
        let p = run_release(&root, out.path(), true, false).unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(p.folder.join("version.json")).unwrap()).unwrap();
        assert_eq!(v["capture_version"], mk_capture_api::CAPTURE_VERSION);
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert!(v["built"].as_str().unwrap().ends_with('Z'));
        assert!(v.get("commit").is_some());
        // mobile-capture.md test 103: the build step and the sizes' place.
        assert!(
            v.get("phone_program_bytes")
                .is_some_and(serde_json::Value::is_null)
        );
        assert!(
            p.steps.iter().any(|s| s.starts_with("wasm-opt -Oz")),
            "{:?}",
            p.steps
        );
        assert_eq!(
            p.folder,
            out.path()
                .join(format!("MoneyKeep-{}", env!("CARGO_PKG_VERSION")))
        );
        // release-prep.md §3.
        assert!(p.folder.ends_with("MoneyKeep-0.1.0"), "{p:?}");
        assert_eq!(
            setup_name(&p.version),
            "moneykeep-0.1.0-windows-x64-setup.exe"
        );
        let names: Vec<&str> = p.files.iter().map(|(_, n)| n.as_str()).collect();
        for name in ["moneykeep-app", "moneykeep-server", "moneykeep", "keep-mcp"] {
            assert!(names.contains(&exe(name).as_str()), "{name}");
        }
        for d in DOCUMENTS {
            assert!(names.contains(&d), "{d}");
        }
        assert!(p.steps.iter().any(|s| s.contains("trunk build --release")));
        // Nothing but version.json and THIRD-PARTY-LICENSES.txt was made.
        let mut made: Vec<String> = std::fs::read_dir(&p.folder)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        made.sort();
        assert_eq!(made, ["THIRD-PARTY-LICENSES.txt", "version.json"]);
        sr_b4_third_party_licences(&p.folder);
    }

    /// security-review-1 SR-B4: the dry run's THIRD-PARTY-LICENSES.txt lists every
    /// crate `cargo metadata` gives for the shipped programs (this platform's
    /// programs and the phone app on wasm), with its licence, and licence texts.
    fn sr_b4_third_party_licences(folder: &Path) {
        let text = std::fs::read_to_string(folder.join(crate::licences::FILE)).unwrap();
        assert!(text.contains("\r\n"), "Windows line ends");
        let text = text.replace("\r\n", "\n");
        let root = workspace_root().unwrap();
        let host = crate::licences::host_triple().unwrap();
        let programs: Vec<&str> = PROGRAMS.iter().map(|(p, _)| *p).collect();
        let mut expected = Vec::new();
        for (target, roots) in [
            (host.as_str(), programs),
            (crate::licences::PHONE.1, vec![crate::licences::PHONE.0]),
        ] {
            let meta = crate::licences::metadata(&root, target).unwrap();
            let ids = crate::licences::reached(&meta, &roots).unwrap();
            assert!(ids.len() > 100, "{target}: {}", ids.len());
            expected.extend(crate::licences::crates_of(&meta, &ids));
        }
        for c in &expected {
            let line = format!("\n{} {}: {}", c.name, c.version, c.licence);
            assert!(text.contains(&line), "{line:?} is listed");
        }
        for name in ["slint", "rusqlite", "serde", "argon2", "wasm-bindgen"] {
            assert!(expected.iter().any(|c| c.name == name), "{name}");
        }
        assert!(text.contains("Licence texts shipped by the crates"));
        assert!(text.contains("Permission is hereby granted"));
        assert!(text.contains("Apache License"));
        assert!(text.contains("Royalty-free"), "Slint's own licence text");
    }

    #[test]
    fn dates_and_versions() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_794_052_800), "2026-11-07T12:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(version_quad("0.0.1").unwrap(), "0.0.1.0");
        assert_eq!(version_quad("0.1.0-alpha.1").unwrap(), "0.1.0.1");
        assert_eq!(version_quad("1.2.3-beta.2+abc").unwrap(), "1.2.3.2");
        assert!(version_quad("1.2").is_err());
        assert!(version_quad("1.2.3-preview.1").is_err());
        assert_eq!(setup_name("0.9.3"), "moneykeep-0.9.3-windows-x64-setup.exe");
        assert_eq!(
            setup_name(env!("CARGO_PKG_VERSION")),
            "moneykeep-0.1.0-windows-x64-setup.exe"
        );
    }

    /// release-prep.md §3 (revised 2026-11-11): the workspace version is 0.1.0,
    /// Windows' four numbers are 0.1.0.0.
    #[test]
    fn rp_version_is_the_first_release() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
        assert_eq!(version_quad(env!("CARGO_PKG_VERSION")).unwrap(), "0.1.0.0");
        assert_eq!(
            env!("CARGO_PKG_REPOSITORY"),
            "https://github.com/moneykeepproject/moneykeep"
        );
        assert_eq!(env!("CARGO_PKG_AUTHORS"), "MoneyKeep Project contributors");
    }

    /// release-prep.md §3: the installer's upgrade check (VERSION_ORDER compared by
    /// NSIS's VersionCompare) is semver order: 0.1.0-alpha.2 is newer than
    /// 0.1.0-alpha.1, and 0.1.0 newer than both.
    #[test]
    fn rp_installer_version_comparison_is_semver() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let cmp =
            |a: &str, b: &str| dotted_cmp(&version_order(a).unwrap(), &version_order(b).unwrap());
        assert_eq!(cmp("0.1.0-alpha.2", "0.1.0-alpha.1"), Greater);
        assert_eq!(cmp("0.1.0", "0.1.0-alpha.1"), Greater);
        assert_eq!(cmp("0.1.0", "0.1.0-alpha.2"), Greater);
        assert_eq!(cmp("0.1.0-alpha.1", "0.1.0"), Less);
        assert_eq!(cmp("0.1.0-alpha.1", "0.1.0-alpha.1"), Equal);
        // More of semver's order.
        assert_eq!(cmp("0.1.0-alpha.10", "0.1.0-alpha.9"), Greater);
        assert_eq!(cmp("0.1.0-alpha.1", "0.1.0-alpha"), Greater);
        assert_eq!(cmp("0.1.0-beta.1", "0.1.0-alpha.5"), Greater);
        assert_eq!(cmp("0.1.0-rc.1", "0.1.0-beta.3"), Greater);
        assert_eq!(cmp("0.1.1-alpha.1", "0.1.0"), Greater);
        assert_eq!(cmp("0.10.0", "0.9.0"), Greater);
        assert_eq!(version_order("0.1.0-alpha.1").unwrap(), "0.1.0.1.1");
        // The script uses it.
        let nsi = std::fs::read_to_string(
            workspace_root()
                .unwrap()
                .join("installer")
                .join("moneykeep.nsi"),
        )
        .unwrap();
        assert!(nsi.contains("!include WordFunc.nsh"));
        assert!(nsi.contains(r#"${VersionCompare} "$OldOrder" "${VERSION_ORDER}" $2"#));
        assert!(nsi.contains(r#""VersionOrder" "${VERSION_ORDER}""#));
    }

    /// release-prep.md §2 (RP6 extension): the uninstaller reads the service's
    /// command line, takes its --data folder (before removing the service), offers
    /// that folder by name and removes it only with the server's store file.
    #[test]
    fn rp6_uninstaller_removes_the_service_data_folder() {
        let nsi = std::fs::read_to_string(
            workspace_root()
                .unwrap()
                .join("installer")
                .join("moneykeep.nsi"),
        )
        .unwrap();
        let un = &nsi[nsi.find(r#"Section "Uninstall""#).unwrap()..];
        // The command line is read for its --data value.
        // Revision 18: one function for Setup and the uninstaller (a macro).
        assert!(nsi.contains(r#"!insertmacro SERVICE_DATA_FN "un.""#));
        let f = &nsi[nsi.find("Function ${un}ServiceDataDir").unwrap()..];
        let f = &f[..f.find("FunctionEnd").unwrap()];
        assert!(f.contains(r#"Services\${SERVICE}" "ImagePath""#), "{f}");
        assert!(f.contains(r#"" --data ""#), "{f}");
        assert!(f.contains("StrCpy $ServiceData"), "{f}");
        // ... before the service (and so its command line) is removed.
        let read = un.find("Call un.ServiceDataDir").unwrap();
        let delete = un.find(r#"sc.exe" delete"#).unwrap();
        assert!(read < delete);
        // The question names the folder; default No.
        assert!(un.contains(
            "MB_DEFBUTTON2 \"Also remove the capture server's data (certificates and paired phones) in $0?\" /SD IDNO"
        ));
        assert!(un.contains("StrCpy $0 $ServiceData"));
        // security-review-1 A1: without --data, the folder under %ProgramData%; the
        // old copy in the current user's folder goes with the same answer.
        assert!(nsi.contains(r#"!define CAPTURE_MACHINE_DATA "MoneyKeep\capture-server""#));
        assert!(un.contains(r#"StrCpy $0 "$ProgramDataDir\${CAPTURE_MACHINE_DATA}""#));
        // upgrade-resilience-1 L5: the original user's folder, from the stub.
        assert!(un.contains(r#"StrCpy $4 "$UserLocal\${CAPTURE_DATA}""#));
        // Removed only with the server's store file; otherwise said in the details.
        assert!(nsi.contains(r#"!define CAPTURE_MARKER "inbox.sqlite""#));
        assert_eq!(moneykeep_server_store_file(), "inbox.sqlite");
        let f = &nsi[nsi.find("Function un.RemoveCaptureData").unwrap()..];
        let f = &f[..f.find("FunctionEnd").unwrap()];
        let safe = f.find("Call un.SafeFolder").unwrap();
        let marker = f.find(r#"${FileExists} "$0\${CAPTURE_MARKER}""#).unwrap();
        let remove = f.find(r#"RMDir /r "$0""#).unwrap();
        assert!(safe < marker && marker < remove, "{f}");
        assert!(f.contains("DetailPrint \"Kept $0: it has no ${CAPTURE_MARKER}"));
        assert!(f.contains("DetailPrint \"Kept $0: it is a link or junction"));
        // L8: neither the folder nor its parent is a reparse point, and it is one of
        // the expected places.
        let f = &nsi[nsi.find("Function un.SafeFolder").unwrap()..];
        let f = &f[..f.find("FunctionEnd").unwrap()];
        assert!(
            f.contains(r#"${GetFileAttributes} "$0" "REPARSE_POINT" $1"#),
            "{f}"
        );
        assert!(f.contains(r#"${GetParent} "$0" $2"#), "{f}");
        assert!(
            f.contains(r#"${GetFileAttributes} "$2" "REPARSE_POINT" $1"#),
            "{f}"
        );
        assert!(
            f.contains(r#"$0 == "$ProgramDataDir\${CAPTURE_MACHINE_DATA}""#),
            "{f}"
        );
        assert!(f.contains(r#"$0 == "$UserLocal\${CAPTURE_DATA}""#), "{f}");
        // Both folders go through the checks.
        assert_eq!(un.matches("Call un.RemoveCaptureData").count(), 2);
        // The publisher (release-prep.md §6).
        assert!(nsi.contains(r#"!define PUBLISHER "MoneyKeep Project""#));
    }

    /// The capture server's store file name, from its source.
    fn moneykeep_server_store_file() -> String {
        let src = std::fs::read_to_string(
            workspace_root()
                .unwrap()
                .join("apps/moneykeep-server/src/store.rs"),
        )
        .unwrap();
        let line = src
            .lines()
            .find(|l| l.starts_with("pub const DB_FILE: &str = "))
            .unwrap();
        line.split('"').nth(1).unwrap().to_owned()
    }

    /// release-prep.md §4: the signing command never carries a password: a
    /// password with a .pfx is refused (and not repeated), the thumbprint signs from
    /// the user's store, and the dry run's printed command has no password.
    #[test]
    fn rp_signing_never_puts_the_password_on_a_command_line() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        let file = Path::new("moneykeep-0.1.0-alpha.1-windows-x64-setup.exe");
        let secret = "s3cret-Pa55";
        // A .pfx with its password: refused, telling what to do instead.
        let r = signing_from(env(&[
            ("MK_SIGN_CERT", "C:\\keys\\code.pfx"),
            ("MK_SIGN_PASSWORD", "s3cret-Pa55"),
        ]));
        let e = r.clone().unwrap_err();
        assert!(
            e.contains("MK_SIGN_THUMBPRINT") && !e.contains(secret),
            "{e}"
        );
        let line = sign_line(r, file);
        assert!(!line.contains(secret) && !line.contains("/p "), "{line}");
        // The thumbprint wins, and the password is not used.
        let r = signing_from(env(&[
            ("MK_SIGN_THUMBPRINT", "ab12 CD34"),
            ("MK_SIGN_CERT", "C:\\keys\\code.pfx"),
            ("MK_SIGN_PASSWORD", "s3cret-Pa55"),
        ]));
        let line = sign_line(r, file);
        assert!(line.contains("/sha1 ab12CD34"), "{line}");
        assert!(!line.contains(secret) && !line.contains("/p "), "{line}");
        assert!(
            line.ends_with("moneykeep-0.1.0-alpha.1-windows-x64-setup.exe"),
            "{line}"
        );
        // A .pfx without a password (a file only the user can read).
        let line = sign_line(signing_from(env(&[("MK_SIGN_CERT", "code.pfx")])), file);
        assert!(
            line.contains("/f code.pfx") && !line.contains("/p "),
            "{line}"
        );
        // Nothing set: not signed.
        assert!(sign_line(signing_from(env(&[])), file).starts_with("not signed"));
        assert!(signing_from(env(&[("MK_SIGN_THUMBPRINT", "not-hex!")])).is_err());
    }

    /// RP6: makensis compiles installer/moneykeep.nsi (with placeholder programs and
    /// the real icon and licence text); skipped with a note without makensis.
    #[test]
    fn rp6_installer_script_compiles() {
        let Some(makensis) = find_makensis() else {
            println!(
                "SKIPPED: makensis (NSIS) is not installed; installer/moneykeep.nsi was not compiled"
            );
            return;
        };
        let root = workspace_root().unwrap();
        let d = tempfile::tempdir().unwrap();
        let version = env!("CARGO_PKG_VERSION");
        let src = d.path().join(format!("MoneyKeep-{version}"));
        std::fs::create_dir_all(&src).unwrap();
        // The script names the Windows programs, whatever this system is.
        for name in ["moneykeep-app", "moneykeep-server", "moneykeep", "keep-mcp"] {
            std::fs::write(src.join(format!("{name}.exe")), b"placeholder").unwrap();
        }
        for doc in DOCUMENTS {
            std::fs::copy(root.join(doc), src.join(doc)).unwrap();
        }
        std::fs::write(src.join(crate::licences::FILE), b"crates").unwrap();
        std::fs::write(src.join("version.json"), b"{}").unwrap();
        std::fs::write(src.join("licence.txt"), licence_text(&root).unwrap()).unwrap();
        std::fs::write(src.join("moneykeep.ico"), mk_ui::app_icon_ico()).unwrap();
        let setup = d.path().join(setup_name(version));
        let out = Command::new(&makensis)
            .args(
                makensis_args(
                    version,
                    &src,
                    &setup,
                    &root.join("installer").join("moneykeep.nsi"),
                )
                .unwrap(),
            )
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "makensis failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let bytes = std::fs::read(&setup).unwrap();
        assert_eq!(bytes.get(..2), Some(&b"MZ"[..]), "a Windows program");
        // No warnings: they usually mean a mistake (an unused function, a page
        // macro in the wrong place).
        let log = String::from_utf8_lossy(&out.stdout).to_lowercase();
        assert!(
            !log.contains("warning:"),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// RP7: the installer's outputs, as the dry run lists them: the setup under the
    /// name keep-launcher installs, and the checksum list.
    #[test]
    fn rp7_dry_run_lists_the_launcher_setup_name_and_the_checksums() {
        assert_eq!(
            installer_outputs(env!("CARGO_PKG_VERSION")),
            ["moneykeep-0.1.0-windows-x64-setup.exe", "SHA256SUMS.txt"]
        );
        // The launcher's asset name: <app>-<version>-<os>-<arch>-setup.exe, lower case.
        let name = setup_name("1.2.3-rc.4");
        let rest = name.strip_suffix("-windows-x64-setup.exe").unwrap();
        assert_eq!(rest, "moneykeep-1.2.3-rc.4");
        assert_eq!(name, name.to_lowercase());
    }

    /// RP8: SHA256SUMS.txt has one `<sha256>  <name>` line per file, with the
    /// files' own hashes.
    #[test]
    fn rp8_sha256sums_lines_match_the_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a-setup.exe"), b"abc").unwrap();
        std::fs::write(d.path().join("empty.txt"), b"").unwrap();
        // Bigger than one read, so the chunks are put together right.
        let big = vec![b'x'; 200_000];
        std::fs::write(d.path().join("big.bin"), &big).unwrap();
        let names: Vec<String> = ["a-setup.exe", "empty.txt", "big.bin"]
            .iter()
            .map(|n| (*n).to_owned())
            .collect();
        let sums = write_sums(d.path(), &names).unwrap();
        assert_eq!(sums, d.path().join("SHA256SUMS.txt"));
        let text = std::fs::read_to_string(&sums).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // The published SHA-256 test vectors for "abc" and the empty message.
        assert_eq!(
            lines.first().copied(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a-setup.exe")
        );
        assert_eq!(
            lines.get(1).copied(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  empty.txt")
        );
        use sha2::{Digest, Sha256};
        let want: String = Sha256::digest(&big)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            lines.get(2).copied(),
            Some(format!("{want}  big.bin").as_str())
        );
        assert_eq!(lines.len(), 3);
        assert!(text.ends_with('\n') && !text.contains('\r'));
        assert!(write_sums(d.path(), &["missing.exe".to_owned()]).is_err());
    }

    /// RP9 (static part; the makensis part is RP6): the script's silent mode. Every
    /// question has a silent default, the running-app wait and the exit codes are
    /// there, and the pages' questions are behind `${Silent}`.
    /// mobile-capture.md test 107: Setup puts Wi-Fi capture's firewall rule back
    /// when Wi-Fi capture is set (static check of the script): after the files, in
    /// the main section (not the uninstaller), with the capture server's
    /// `services repair-firewall` on the data folder that holds server.toml, and no
    /// question asked.
    #[test]
    fn t107_installer_repairs_the_wifi_firewall_rule() {
        let root = workspace_root().unwrap();
        let script = std::fs::read_to_string(root.join("installer").join("moneykeep.nsi")).unwrap();
        let code: String = script
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n");
        let f = code
            .split("Function RepairWifiFirewall")
            .nth(1)
            .and_then(|r| r.split("FunctionEnd").next())
            .unwrap();
        assert!(f.contains("Call ServiceDataDir"), "{f}");
        assert!(f.contains("${FileExists} \"$0\\server.toml\""), "{f}");
        assert!(
            f.contains("\"$INSTDIR\\${SERVER_EXE}\" services repair-firewall --data \"$0\""),
            "{f}"
        );
        // upgrade-resilience-1 L3: its JSON report is read; a failure is shown
        // (CheckReport), never a question.
        assert!(f.contains("nsExec::ExecToStack"), "{f}");
        assert!(f.contains("Call CheckReport"), "{f}");
        assert!(!f.contains("MessageBox"), "no question: {f}");
        // Called in the main section after the files and the service.
        let main = code
            .split("Section \"MoneyKeep (required)\" SecMain")
            .nth(1)
            .and_then(|r| r.split("SectionEnd").next())
            .unwrap();
        let files = main.find("File \"${SOURCE_DIR}\\${SERVER_EXE}\"").unwrap();
        let start = main.find("Call StartCaptureService").unwrap();
        let repair = main.find("Call RepairWifiFirewall").unwrap();
        assert!(files < start && start < repair, "{main}");
        // security-review-1 A1: the data is moved to its protected folder first.
        let moved = main.find("Call MoveCaptureData").unwrap();
        assert!(start < moved && moved < repair, "{main}");
        let mv = code
            .split("Function MoveCaptureData")
            .nth(1)
            .and_then(|r| r.split("FunctionEnd").next())
            .unwrap();
        assert!(
            mv.contains("\"$INSTDIR\\${SERVER_EXE}\" services move-data"),
            "{mv}"
        );
        assert!(!mv.contains("MessageBox"), "no question: {mv}");
        assert!(mv.contains("Call CheckReport"), "{mv}");
        let uninstall = code.split("Section \"Uninstall\"").nth(1).unwrap();
        assert!(!uninstall.contains("RepairWifiFirewall"));
        // The installer's ServiceDataDir exists for Setup, not only the uninstaller.
        assert!(code.contains("!insertmacro SERVICE_DATA_FN \"\""));
        assert!(code.contains("!define FIREWALL_RULE \"MoneyKeep capture (Wi-Fi)\""));
        assert_eq!(mk_capture_api::FIREWALL_RULE, "MoneyKeep capture (Wi-Fi)");
        // The server program has the command.
        let server =
            std::fs::read_to_string(root.join("apps/moneykeep-server/src/main.rs")).unwrap();
        assert!(
            server.contains("RepairFirewall {"),
            "services repair-firewall"
        );
    }

    /// upgrade-resilience-1 UR-9 (static part) and U1.1: the service's own copy is
    /// stopped and refreshed whatever $INSTDIR is (H1); the helper's reports are
    /// read and a failure shown with /SD (L3); removing the data removes the tunnel
    /// token file and the cloudflared service MoneyKeep installed (L4); Installed
    /// apps runs the uninstaller through the stub that passes the original user's
    /// %LOCALAPPDATA% (L5); `.prev` copies go too (L2).
    #[test]
    fn ur9_installer_upgrade_resilience() {
        let root = workspace_root().unwrap();
        let script = std::fs::read_to_string(root.join("installer").join("moneykeep.nsi")).unwrap();
        let code: String = script
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n");
        let func = |name: &str| -> String {
            code.split(&format!("Function {name}\n"))
                .nth(1)
                .and_then(|r| r.split("FunctionEnd").next())
                .unwrap_or_else(|| panic!("no {name}"))
                .to_owned()
        };
        // H1.
        let stop = func("StopCaptureService");
        assert!(
            stop.contains(r#"StrLen $0 "${CAPTURE_COPY_DIR}\""#),
            "{stop}"
        );
        assert!(
            stop.contains(r#"${If} $1 == "${CAPTURE_COPY_DIR}\""#),
            "{stop}"
        );
        // L3.
        let check = func("CheckReport");
        assert!(check.contains(r#"StrCpy $Needle '"ok":true'"#), "{check}");
        assert!(check.contains(r#"StrCpy $Needle '"error":"'"#), "{check}");
        assert!(check.contains("Phone capture needs attention in MoneyKeep"));
        assert!(check.contains("/SD IDOK") && check.contains("DetailPrint"));
        // L4: only with Yes ($3 = 1), the token folder checked for links first.
        let un = code.split("Section \"Uninstall\"").nth(1).unwrap();
        let yes = un.find("${If} $3 == 1").unwrap();
        let token = un
            .find(r#"StrCpy $0 "$ProgramDataDir\${TOKEN_DIR}""#)
            .unwrap();
        let reparse = un[token..]
            .find(r#"${GetFileAttributes} "$0" "REPARSE_POINT" $1"#)
            .unwrap();
        let delete_svc = un[token..]
            .find("sc.exe\" delete ${CLOUDFLARED_SERVICE}")
            .unwrap();
        assert!(yes < token && reparse < delete_svc);
        assert!(un.contains(r#"${If} ${FileExists} "$0\${CLOUDFLARED_MARKER}""#));
        assert!(un.contains(r#"Delete "$0\*.token""#));
        assert!(code.contains(r#"!define TOKEN_DIR "MoneyKeep\cloudflared""#));
        assert!(code.contains(r#"!define CLOUDFLARED_MARKER "service-installed-by-moneykeep""#));
        let server =
            std::fs::read_to_string(root.join("apps/moneykeep-server/src/services.rs")).unwrap();
        assert!(
            server.contains(
                r#"pub const CLOUDFLARED_MARKER: &str = "service-installed-by-moneykeep";"#
            )
        );
        assert!(server.contains("machine.join(\"cloudflared\")"));
        // L5: the stub, and the uninstaller reading it.
        assert!(code.contains(
            r#""UninstallString" '"$SYSDIR\cmd.exe" /d /c start "" "$INSTDIR\uninstall.exe" /LOCALAPPDATA="%LOCALAPPDATA%"'"#
        ));
        assert!(code.contains(r#""QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'"#));
        let init = func("un.onInit");
        assert!(
            init.contains(r#"${GetOptions} $R0 "/LOCALAPPDATA=" $R1"#),
            "{init}"
        );
        assert!(init.contains(r#"${GetFileAttributes} "$R1" "REPARSE_POINT" $R2"#));
        assert!(init.contains("StrCpy $UserLocal $R1"));
        // L2.
        assert!(code.contains(r#"Delete "${dir}\${SERVER_EXE}.prev""#));
        // L6: the helper's refusal, which Setup shows through CheckReport.
        assert!(server.contains("(it was not made by MoneyKeep's setup) and try again."));
    }

    #[test]
    fn rp9_installer_script_has_the_silent_mode_paths() {
        let root = workspace_root().unwrap();
        let script = std::fs::read_to_string(root.join("installer").join("moneykeep.nsi")).unwrap();
        let code: Vec<&str> = script
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with(';'))
            .collect();
        for line in code.iter().filter(|l| l.starts_with("MessageBox")) {
            assert!(
                line.contains("/SD "),
                "a question without a silent default: {line}"
            );
        }
        // The data question's silent answer is No.
        assert!(code.iter().any(|l| l.starts_with("MessageBox")
            && l.contains("capture server's data")
            && l.contains("/SD IDNO")));
        let has = |needle: &str| code.iter().any(|l| l.contains(needle));
        assert!(has("!define EXIT_APP_OPEN 2"));
        assert!(has("!define EXIT_FAILED 3"));
        assert!(has("SetErrorLevel ${EXIT_APP_OPEN}"));
        assert!(has("Function ${un}SilentCloseApp"));
        assert!(has("${If} $R5 >= 30"), "waits up to 30 s");
        assert!(
            has("taskkill.exe\" /IM ${APP_EXE}"),
            "asks MoneyKeep to close"
        );
        assert!(!has("taskkill.exe\" /F"), "never forces MoneyKeep closed");
        assert!(has("QuietUninstallString"));
        assert!(has("\"InstallLocation\" \"$INSTDIR\""));
        // Both .onInit and un.onInit wait for MoneyKeep in silent mode.
        let body = |name: &str| -> String {
            let (_, from) = script.split_once(&format!("Function {name}\n")).unwrap();
            from.split_once("FunctionEnd").unwrap().0.to_owned()
        };
        for (init, call) in [
            (".onInit", "Call SilentCloseApp"),
            ("un.onInit", "Call un.SilentCloseApp"),
        ] {
            let b = body(init);
            assert!(b.contains("${If} ${Silent}"), "{init}");
            assert!(b.contains(call), "{init}");
            assert!(b.contains("SetErrorLevel ${EXIT_FAILED}"), "{init}");
        }
        // A silent update leaves the shortcuts alone, and success is 0.
        let (_, off) = script
            .split_once("Section \"-Shortcuts turned off\"\n")
            .unwrap();
        assert!(off.trim_start().starts_with("${IfNot} ${Silent}"));
        assert!(script.contains("Section \"-Done\"\n  SetErrorLevel 0\nSectionEnd"));
    }

    /// security-review-1 SR-B4: the installer script installs THIRD-PARTY-LICENSES.txt
    /// beside the programs (About opens it there) and the uninstaller removes it.
    #[test]
    fn sr_b4_the_installer_installs_the_third_party_licences() {
        let nsi =
            std::fs::read_to_string(workspace_root().unwrap().join("installer/moneykeep.nsi"))
                .unwrap();
        assert!(nsi.contains(r#"File "${SOURCE_DIR}\THIRD-PARTY-LICENSES.txt""#));
        assert!(nsi.contains(r#"Delete "$INSTDIR\THIRD-PARTY-LICENSES.txt""#));
        // In the same section as the programs.
        let file = nsi
            .find(r#"File "${SOURCE_DIR}\THIRD-PARTY-LICENSES.txt""#)
            .unwrap();
        let notice = nsi.find(r#"File "${SOURCE_DIR}\NOTICE""#).unwrap();
        let before = nsi.get(notice..file).unwrap();
        assert!(
            !before.contains("SectionEnd"),
            "installed with the documents"
        );
    }

    #[test]
    fn licence_page_has_windows_line_ends() {
        let t = licence_text(&workspace_root().unwrap()).unwrap();
        assert!(t.contains("MIT license"));
        assert!(!t.replace("\r\n", "").contains('\n'));
    }

    /// The package's version (the release tag is `v` and this).
    #[cfg(unix)]
    fn workspace_version() -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }

    /// Runs `packaging/macos/package.sh` with `args` and `env`; (success, stdout + stderr).
    #[cfg(unix)]
    fn package_sh(args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
        let root = workspace_root().unwrap();
        let mut cmd = Command::new("bash");
        cmd.arg(root.join(MAC_PACKAGE))
            .args(args)
            .current_dir(&root);
        for k in [
            "MACOS_SIGN_IDENTITY",
            "MACOS_KEYCHAIN",
            "MACOS_NOTARY_PROFILE",
            "MACOS_NOTARY_KEY",
            "MACOS_NOTARY_KEY_ID",
            "MACOS_NOTARY_ISSUER",
            "MK_PHONE_APP",
            "GITHUB_ACTIONS",
        ] {
            cmd.env_remove(k);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    }

    /// macos-release.md MAC-3: `package.sh --dry-run` prints the planned steps (both
    /// targets, lipo, the bundle's layout, the signing identity or "ad hoc", the
    /// notarization profile, the disk image's name) without running Apple's tools or
    /// building anything; the bundle identifier and file name are P2's and P3's.
    #[cfg(unix)]
    #[test]
    fn mac3_package_dry_run_prints_the_plan() {
        let v = workspace_version();
        let (ok, out) = package_sh(&["--dry-run"], &[]);
        assert!(ok, "{out}");
        for want in [
            format!("==> MoneyKeep {v} for macOS (universal), dry run"),
            "--target aarch64-apple-darwin --target x86_64-apple-darwin".to_owned(),
            "-p moneykeep-app -p moneykeep-server -p keep-mcp".to_owned(),
            "MACOSX_DEPLOYMENT_TARGET=11.0".to_owned(),
            "lipo -create moneykeep-app from aarch64-apple-darwin x86_64-apple-darwin".to_owned(),
            "lipo -create moneykeep-server from".to_owned(),
            "lipo -create keep-mcp from".to_owned(),
            "identifier com.moneykeep.moneykeep".to_owned(),
            "MoneyKeep.app/Contents/MacOS/MoneyKeep\n".to_owned(),
            "MoneyKeep.app/Contents/MacOS/moneykeep-server\n".to_owned(),
            "MoneyKeep.app/Contents/MacOS/keep-mcp\n".to_owned(),
            "MoneyKeep.app/Contents/Resources/MoneyKeep.icns\n".to_owned(),
            "MoneyKeep.app/Contents/Resources/THIRD-PARTY-LICENSES.txt\n".to_owned(),
            "MoneyKeep.app/Contents/Resources/NOTICE\n".to_owned(),
            "sign: ad hoc".to_owned(),
            "inside out: moneykeep-server, keep-mcp, MoneyKeep, then MoneyKeep.app".to_owned(),
            "notarize MoneyKeep.app: no (ad hoc".to_owned(),
            format!("disk image: moneykeep-{v}-macos-universal.dmg (volume MoneyKeep"),
            format!("SHA256SUMS.txt with moneykeep-{v}-macos-universal.dmg"),
            "cargo xtask phone-app --out".to_owned(),
        ] {
            assert!(out.contains(&want), "{want:?} not in:\n{out}");
        }
        for apple in ["codesign --", "hdiutil create", "xcrun ", "plutil "] {
            assert!(!out.contains(apple), "{apple} in:\n{out}");
        }
        // With the project's Mac's keychain (what runner-keychain.sh sets).
        let (ok, out) = package_sh(
            &["--dry-run", "--arch", "aarch64", "--skip-build"],
            &[
                (
                    "MACOS_SIGN_IDENTITY",
                    "0123456789ABCDEF0123456789ABCDEF01234567",
                ),
                (
                    "MACOS_KEYCHAIN",
                    "/Users/mkbuild/Library/Keychains/mk-signing.keychain-db",
                ),
                ("MACOS_NOTARY_PROFILE", "mk-notary"),
                ("MK_PHONE_APP", "/tmp/phone"),
            ],
        );
        assert!(ok, "{out}");
        for want in [
            "sign: identity 0123456789ABCDEF0123456789ABCDEF01234567 (keychain /Users/mkbuild/Library/Keychains/mk-signing.keychain-db)",
            "xcrun notarytool submit --keychain-profile mk-notary --wait",
            "phone app: prebuilt in /tmp/phone",
            "build: skipped (--skip-build)",
            "lipo -create moneykeep-app from aarch64-apple-darwin\n",
        ] {
            assert!(out.contains(want), "{want:?} not in:\n{out}");
        }
        assert!(
            out.contains(&format!("moneykeep-{v}-macos-aarch64.dmg")),
            "{out}"
        );
        assert!(!out.contains("x86_64-apple-darwin"), "{out}");
        // A real identity without a notarization profile: signed, not notarized.
        let (_, out) = package_sh(&["--dry-run"], &[("MACOS_SIGN_IDENTITY", "ABC")]);
        assert!(out.contains("signed but not notarized"), "{out}");
        // Bad options fail.
        assert!(!package_sh(&["--dry-run", "--arch", "ppc"], &[]).0);
        assert!(!package_sh(&["--bogus"], &[]).0);
    }

    /// P2: the bundle's Info.plist template (identifier, minimum macOS, the executable
    /// and icon names the script uses) and empty entitlements.
    #[test]
    fn mac_p2_bundle_template() {
        let root = workspace_root().unwrap();
        let plist = std::fs::read_to_string(root.join("packaging/macos/Info.plist.in")).unwrap();
        let script = std::fs::read_to_string(root.join(MAC_PACKAGE)).unwrap();
        let value = |key: &str| {
            let at = plist.find(&format!("<key>{key}</key>")).unwrap();
            let rest = plist.get(at..).unwrap();
            let s = rest.find("<string>").unwrap() + "<string>".len();
            let e = rest.find("</string>").unwrap();
            rest.get(s..e).unwrap().to_owned()
        };
        assert_eq!(value("CFBundleIdentifier"), "com.moneykeep.moneykeep");
        assert!(script.contains("BUNDLE_ID=com.moneykeep.moneykeep\n"));
        assert_eq!(value("CFBundleExecutable"), "MoneyKeep");
        assert_eq!(value("CFBundleIconFile"), "MoneyKeep");
        assert!(script.contains("APP_NAME=MoneyKeep "));
        assert_eq!(value("LSMinimumSystemVersion"), "11.0");
        assert!(script.contains("export MACOSX_DEPLOYMENT_TARGET=11.0\n"));
        assert_eq!(value("CFBundleShortVersionString"), "@SHORT_VERSION@");
        assert!(!plist.contains("LSUIElement"));
        let ent = std::fs::read_to_string(root.join("packaging/macos/entitlements.plist")).unwrap();
        assert!(ent.contains("<dict>\n</dict>"), "no entitlements");
        // The executables the script bundles are the release's programs (the CLI aside).
        for (pkg, _) in PROGRAMS.iter().filter(|(p, _)| *p != "moneykeep-cli") {
            assert!(script.contains(pkg), "{pkg}");
        }
    }

    /// The packaging scripts are stored executable in git (keep-launcher once shipped
    /// them without the bit, so `./package.sh` failed).
    #[test]
    fn mac_scripts_are_executable_in_git() {
        let root = workspace_root().unwrap();
        let Ok(out) = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["ls-files", "-s", "--", "packaging/macos/*.sh"])
            .output()
        else {
            return;
        };
        let listed = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || listed.trim().is_empty() {
            // Not a git checkout (or not yet added): nothing to check.
            return;
        }
        for line in listed.lines() {
            assert!(line.starts_with("100755 "), "not executable in git: {line}");
        }
        for name in [
            "package.sh",
            "verify.sh",
            "runner-keychain.sh",
            "import-cert.sh",
        ] {
            assert!(
                listed.contains(&format!("packaging/macos/{name}")),
                "{name}"
            );
        }
    }
}
