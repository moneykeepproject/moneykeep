//! The real [`CaptureHost`] (mobile-capture.md revision 2, section B5): the capture
//! server and `cloudflared` on this computer, through the `moneykeep-server`
//! program installed next to MoneyKeep.
//!
//! - Everything that touches the capture server's data folder goes through one
//!   `services apply --plan FILE --report FILE --plan-sha256 HASH` run
//!   (security-review-1 A1, A3). On Windows the data folder belongs to the service's
//!   own account (`%ProgramData%\MoneyKeep\capture-server`) and this program never
//!   opens it; the helper runs elevated (Windows asks the user for permission once:
//!   UAC, through PowerShell's `Start-Process -Verb RunAs`).
//! - The plan is written to MoneyKeep's own folder in the user's application data,
//!   and its SHA-256 goes on the helper's command line, so the helper refuses a plan
//!   that changed after the user approved it.
//! - The desktop token is made here and only its SHA-256 goes in the plan; the
//!   plan holds the tunnel token only until the helper reads and deletes it; the
//!   report holds no token.

use std::path::{Path, PathBuf};

use mk_commands::capture_setup::{
    CaptureHost, FirewallFix, FirewallState, HostReport, InstallOutcome, InstallRequest,
    LocalAddress, OldCopy, PERMISSION_DECLINED, ReinstallRequest, StartOverRequest,
    UninstallRequest, UpdateRequest, WifiOutcome, WifiRemove, WifiRequest,
};
use mk_core::{brand, legacy};
use serde_json::{Value, json};

/// The capture server program and the user's own folders.
#[derive(Debug, Clone)]
pub struct SystemHost {
    exe: Option<PathBuf>,
    /// Where the plan and report files are handed over (MoneyKeep's folder in the
    /// user's application data).
    handover: Option<PathBuf>,
    /// The data folders in the user's application data from before A1 (and from
    /// before the rename): an old copy is looked for there.
    old_dirs: Vec<PathBuf>,
}

fn user_dir(folder: &str, leaf: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", folder).map(|d| d.data_local_dir().join(leaf))
}

/// The marker the helper leaves in a folder it moved the data out of.
const MOVED_MARKER: &str = "moved.json";

impl Default for SystemHost {
    fn default() -> Self {
        Self::new()
    }
}

fn program_name() -> &'static str {
    if cfg!(windows) {
        "moneykeep-server.exe"
    } else {
        "moneykeep-server"
    }
}

/// `moneykeep-server` next to this program, and only there (upgrade-resilience-1
/// U1.4: the helper ships beside the desktop app, so a server program elsewhere on
/// `PATH`, of another version, is never used).
fn find_server() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(program_name())))
        .filter(|p| p.is_file())
}

/// A string as a single-quoted PowerShell literal.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The exit code [`elevation_script`] gives when the permission prompt was declined
/// (Windows' `ERROR_CANCELLED`).
pub const ERROR_CANCELLED: i32 = 1223;

/// The PowerShell command that runs `exe args…` elevated and returns its exit code;
/// [`ERROR_CANCELLED`] when the user declined the permission prompt (Start-Process
/// then fails before the program runs). Each argument is wrapped in double quotes
/// because `Start-Process` joins `-ArgumentList` with spaces.
pub fn elevation_script(exe: &Path, args: &[String]) -> String {
    let list: Vec<String> = args
        .iter()
        .map(|a| ps_quote(&format!("\"{}\"", a.replace('"', ""))))
        .collect();
    format!(
        "try {{ $p = Start-Process -FilePath {} -ArgumentList @({}) -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ErrorAction Stop }} catch {{ exit {ERROR_CANCELLED} }}; exit $p.ExitCode",
        ps_quote(&exe.display().to_string()),
        list.join(",")
    )
}

/// Write the plan as a new owner-only file (upgrade-resilience-1 U1.2): to a new
/// temporary name in the same folder, flushed, then renamed over the plan's name,
/// so a cut-short write never leaves half a plan. A link at the name is replaced,
/// never written through.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let err = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    let mut r = [0u8; 6];
    getrandom::fill(&mut r).map_err(|e| format!("no random source: {e}"))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.{}.tmp",
        sha256_hex(&r).get(..12).unwrap_or("0")
    ));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp).map_err(err)?;
    let done = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = done.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(e));
    }
    Ok(())
}

/// SHA-256 as 64 lower-case hexadecimal digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A new desktop token (32 random bytes, Base64URL without padding, as the server
/// makes them) and its SHA-256: only the hash goes to the helper.
pub fn new_desktop_token() -> Result<(String, String), String> {
    use base64::Engine as _;
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| format!("no random source: {e}"))?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    b.fill(0);
    let hash = sha256_hex(token.as_bytes());
    Ok((token, hash))
}

/// The old copy in `dir`, when the helper moved the data out of it (A1): a real
/// folder (not a link) holding the marker.
pub fn old_copy_in(dir: &Path) -> Option<OldCopy> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return None;
    }
    let text = std::fs::read(dir.join(MOVED_MARKER)).ok()?;
    let v: Value = serde_json::from_slice(&text).ok()?;
    let to = v.get("to").and_then(Value::as_str).unwrap_or("");
    if Path::new(to) == dir {
        return None;
    }
    Some(OldCopy {
        path: dir.display().to_string(),
        moved_at: v.get("at").and_then(Value::as_i64)?,
    })
}

/// security-review-1 A6: the plan for Install the server again. The source is the
/// first of the user's old data folders (a real folder, not a link) that holds
/// `server.toml`; the helper copies from it only when its own data folder has none,
/// and refuses any folder that isn't one of these.
///
/// upgrade-resilience-1 U1.3: a folder holding the helper's marker (the data was
/// moved out of it) is never named.
pub fn reinstall_plan(old_dirs: &[PathBuf], req: &ReinstallRequest) -> Value {
    let source = old_dirs.iter().find(|d| {
        std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            && d.join("server.toml").is_file()
            && std::fs::symlink_metadata(d.join(MOVED_MARKER)).is_err()
    });
    json!({
        "reinstall_server": true,
        "source_data_dir": source,
        "allow_lan": req.allow_lan,
        "firewall_rule": req.firewall_rule.as_ref().map(|r| json!({
            "name": r.name,
            "ports": r.ports,
            "profiles": r.profiles,
        })),
    })
}

#[cfg(test)]
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl SystemHost {
    /// The installed server program and the standard folders.
    pub fn new() -> Self {
        Self {
            exe: find_server(),
            handover: user_dir(brand::FOLDER, "capture-setup"),
            old_dirs: [brand::FOLDER, legacy::FOLDER]
                .into_iter()
                .filter_map(|f| user_dir(f, "capture-server"))
                .collect(),
        }
    }

    /// A given program and hand-over folder (tests).
    pub fn with(exe: Option<PathBuf>, handover: PathBuf) -> Self {
        Self {
            exe,
            handover: Some(handover),
            old_dirs: Vec::new(),
        }
    }

    /// The same, looking for an old copy in these folders (tests).
    pub fn with_old_dirs(exe: Option<PathBuf>, handover: PathBuf, old_dirs: Vec<PathBuf>) -> Self {
        Self {
            exe,
            handover: Some(handover),
            old_dirs,
        }
    }

    fn exe(&self) -> Result<&Path, String> {
        self.exe.as_deref().ok_or_else(|| {
            "the capture server program (moneykeep-server) is missing: install MoneyKeep with its installer, or (developers) build with cargo xtask release"
                .to_owned()
        })
    }

    /// The folder for the plan and report files.
    fn dir(&self) -> Result<&Path, String> {
        self.handover
            .as_deref()
            .ok_or_else(|| "no application data folder on this computer".to_owned())
    }

    /// The old copy of the data folder while it exists (upgrade-resilience-1 U1.3:
    /// on the Status card for `OLD_COPY_DAYS` days, then under Clean up).
    pub fn old_copy(&self) -> Option<OldCopy> {
        self.old_dirs.iter().find_map(|d| old_copy_in(d))
    }

    fn command(&self, exe: &Path) -> std::process::Command {
        // Changed only on Windows.
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut c = std::process::Command::new(exe);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c
    }

    /// Run the server program as the user; its standard output.
    fn output(&self, args: &[&str]) -> Result<String, String> {
        let exe = self.exe()?;
        let out = self
            .command(exe)
            .args(args)
            .output()
            .map_err(|e| format!("cannot run moneykeep-server: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr)
                .trim()
                .trim_start_matches("moneykeep-server: ")
                .chars()
                .take(500)
                .collect())
        }
    }

    /// Run the service helper with administrator rights where needed.
    fn elevated(&self, args: &[String]) -> Result<(), String> {
        let exe = self.exe()?;
        if cfg!(windows) {
            let mut cmd = self.command(Path::new("powershell"));
            // Windows PowerShell can't load its own modules with PowerShell 7's
            // PSModulePath (MoneyKeep started from a pwsh window); it has its defaults
            // without the variable.
            cmd.env_remove("PSModulePath");
            let out = cmd
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &elevation_script(exe, args),
                ])
                .output()
                .map_err(|e| format!("cannot run PowerShell: {e}"))?;
            if out.status.success() {
                Ok(())
            } else if out.status.code() == Some(ERROR_CANCELLED) {
                // The user answered No to Windows' permission prompt.
                Err(PERMISSION_DECLINED.into())
            } else {
                Err("Windows did not give permission, or the installation stopped".into())
            }
        } else {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            self.output(&refs).map(|_| ())
        }
    }

    /// Hand a plan to `services apply` and read its report.
    fn apply(&self, plan: &Value) -> Result<Value, String> {
        let dir = self.dir()?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let plan_file = dir.join("setup-plan.json");
        let report_file = dir.join("setup-report.json");
        let _ = std::fs::remove_file(&report_file);
        let bytes = serde_json::to_vec(plan).map_err(|e| e.to_string())?;
        write_private(&plan_file, &bytes)?;
        let ran = self.elevated(&apply_args(&plan_file, &report_file, &bytes));
        let _ = std::fs::remove_file(&plan_file);
        let report: Option<Value> = std::fs::read(&report_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let _ = std::fs::remove_file(&report_file);
        match (report, ran) {
            (Some(r), _) if r.get("ok").and_then(Value::as_bool) == Some(true) => Ok(r),
            (Some(r), _) => Err(r
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the installation failed")
                .to_owned()),
            (None, Err(e)) => Err(e),
            (None, Ok(())) => Err("the installation gave no report".into()),
        }
    }
}

/// The helper's command line for a plan (security-review-1 A3: with the plan's
/// SHA-256).
pub fn apply_args(plan_file: &Path, report_file: &Path, plan: &[u8]) -> Vec<String> {
    vec![
        "services".to_owned(),
        "apply".to_owned(),
        "--plan".to_owned(),
        plan_file.display().to_string(),
        "--report".to_owned(),
        report_file.display().to_string(),
        "--plan-sha256".to_owned(),
        sha256_hex(plan),
    ]
}

fn actions_of(report: &Value) -> Vec<String> {
    report
        .get("actions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

impl CaptureHost for SystemHost {
    fn inspect(&self) -> HostReport {
        let os = std::env::consts::OS.to_owned();
        if self.exe.is_none() {
            return HostReport {
                os,
                notes: vec!["The capture server program (moneykeep-server) was not found next to MoneyKeep.".into()],
                ..HostReport::default()
            };
        }
        let old_copy = self.old_copy();
        match self
            .output(&["services", "status"])
            .and_then(|s| serde_json::from_str::<HostReport>(&s).map_err(|e| e.to_string()))
        {
            Ok(mut r) => {
                r.old_copy = old_copy;
                r
            }
            Err(e) => HostReport {
                os,
                server_program: true,
                notes: vec![format!("The services could not be checked: {e}")],
                old_copy,
                ..HostReport::default()
            },
        }
    }

    fn install(&self, req: &InstallRequest) -> Result<InstallOutcome, String> {
        // A1: the helper makes the data folder, sets the desktop token (only its
        // hash travels), writes the Access settings and installs the services, in
        // one run.
        let (token, hash) = new_desktop_token()?;
        let report = self.apply(&json!({
            "desktop_token_sha256": hash,
            "configure": {
                "team_domain": req.team_domain,
                "aud": req.aud,
                "listen": req.listen,
                "client_url": req.client_url,
                "service_client_id": req.service_client_id,
            },
            "install_server": true,
            "allow_lan": req.allow_lan,
            "tunnel_token": req.tunnel_token,
            // mobile-capture.md revision 16: replaced in this same elevated run.
            "replace_cloudflared_tunnel": req.replace_cloudflared_tunnel,
        }))?;
        Ok(InstallOutcome {
            desktop_token: token,
            local_url: format!("http://{}", req.listen),
            actions: actions_of(&report),
            cloudflared_service: report
                .get("cloudflared_service")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cloudflared_replaced: report
                .get("cloudflared_replaced")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn uninstall(&self, req: &UninstallRequest) -> Result<Vec<String>, String> {
        let report = self.apply(&json!({
            "remove_server": req.server,
            "remove_cloudflared": req.cloudflared,
        }))?;
        Ok(actions_of(&report))
    }

    fn update_server(&self, req: &UpdateRequest) -> Result<Vec<String>, String> {
        // The helper is the newly built program (next to MoneyKeep): it copies
        // itself into the install folder, and moves an installation from before the
        // rename, data folder included (branding.md step 6).
        // upgrade-resilience-1 L1: `--allow-lan` is kept as the service has it
        // unless this desktop knows it is needed.
        let report = self.apply(&json!({
            "update_server": true,
            "allow_lan": req.allow_lan.then_some(true),
        }))?;
        Ok(actions_of(&report))
    }

    fn set_access_aud(
        &self,
        aud: &str,
        update: &UpdateRequest,
    ) -> Result<(bool, Vec<String>), String> {
        // mobile-capture.md revision 8: `set_aud` rewrites only `access_aud` and
        // says whether anything changed; the helper then restarts the server so it
        // reads server.toml again (A1: in the same run, since only the helper may
        // open the data folder). Its program is not replaced: only Update now does
        // that (release-packaging.md).
        let report = self.apply(&json!({
            "set_aud": aud,
            "allow_lan": update.allow_lan.then_some(true),
        }))?;
        let changed = report
            .get("aud_changed")
            .and_then(Value::as_bool)
            .ok_or("moneykeep-server did not say whether server.toml changed")?;
        if !changed {
            return Ok((false, Vec::new()));
        }
        Ok((true, actions_of(&report)))
    }

    fn start_server(&self) -> Result<(), String> {
        self.apply(&json!({"start_server": true})).map(|_| ())
    }

    /// security-review-1 A6: one `services apply` (one permission prompt on
    /// Windows): the data copied in from the user's old folder when needed, the
    /// service installed, started and health-checked, the Wi-Fi firewall rule.
    fn reinstall_server(&self, req: &ReinstallRequest) -> Result<Vec<String>, String> {
        let report = self.apply(&reinstall_plan(&self.old_dirs, req))?;
        Ok(actions_of(&report))
    }

    // ----- Wi-Fi capture (mobile-capture.md revision 10) -----------------------------

    fn wifi_addresses(&self) -> Result<Vec<LocalAddress>, String> {
        let out = self.output(&["wifi", "addresses"])?;
        serde_json::from_str(out.trim())
            .map_err(|e| format!("the address list was not understood: {e}"))
    }

    fn wifi_install(&self, req: &WifiRequest) -> Result<WifiOutcome, String> {
        // One helper run (one permission prompt on Windows): the data folder and a
        // desktop token MoneyKeep keeps (set when the folder is new, or a new one
        // was asked for), server.toml and the certificates (keys in sealed files:
        // the service runs under its own account), the service installed or
        // restarted, the firewall rule, then whether the address answers.
        let (token, hash) = new_desktop_token()?;
        let installed =
            self.inspect().server != mk_commands::capture_setup::ServiceState::NotInstalled;
        let report = self.apply(&json!({
            "desktop_token_sha256": hash,
            "desktop_token_only_if_new": !req.new_desktop_token,
            "wifi_configure": {
                "address": req.address,
                "name": req.name,
                "https_port": req.https_port,
                "http_port": req.http_port,
                "computer": (!req.computer.is_empty()).then_some(&req.computer),
            },
            "install_server": !installed,
            "restart_server": installed,
            "allow_lan": req.allow_lan.then_some(true),
            "firewall_rule": {
                "name": req.firewall.name,
                "ports": req.firewall.ports,
                "profiles": req.firewall.profiles,
            },
            "wifi_check_wait": 20,
        }))?;
        let made = report.get("wifi").cloned().unwrap_or(Value::Null);
        let token_set = report.get("desktop_token_set").and_then(Value::as_bool) == Some(true);
        let mut actions = vec![
            if made.get("ca_created").and_then(Value::as_bool) == Some(true) {
                format!(
                    "certificate authority \"{}\" made",
                    str_of(&made, "ca_name")
                )
            } else {
                "certificate authority kept".to_owned()
            },
        ];
        actions.push(format!(
            "server certificate for {} made or kept",
            req.address
        ));
        let more = actions_of(&report);
        let firewall_added = more.iter().any(|a| a.starts_with("firewall rule"));
        actions.extend(more);
        let check = report.get("wifi_check").cloned().unwrap_or(Value::Null);
        let num = |k: &str| made.get(k).and_then(Value::as_i64).unwrap_or(0);
        Ok(WifiOutcome {
            desktop_token: token_set.then_some(token),
            local_url: format!(
                "http://127.0.0.1:{}",
                mk_commands::capture_setup::SERVER_PORT
            ),
            ca_name: str_of(&made, "ca_name"),
            fingerprint: str_of(&made, "fingerprint"),
            ca_created: made.get("ca_created").and_then(Value::as_bool) == Some(true),
            ca_not_after: num("ca_not_after"),
            cert_not_before: num("cert_not_before"),
            cert_not_after: num("cert_not_after"),
            actions,
            firewall_added,
            answered: check.get("ok").and_then(Value::as_bool) == Some(true),
            check_error: check
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    fn wifi_remove(&self, req: &WifiRemove) -> Result<Vec<String>, String> {
        let report = self.apply(&json!({
            "wifi_remove": true,
            "restart_server": true,
            "allow_lan": req.allow_lan.then_some(true),
            "remove_firewall_rule": req.firewall_rule,
        }))?;
        Ok(actions_of(&report))
    }

    /// Start over (mobile-capture.md revision 17): the services, the firewall rule
    /// and the data folder in one `services apply` (one permission prompt on
    /// Windows: the data folder belongs to the service's account, A1); then the old
    /// copy in the user's folder, as the user.
    fn start_over(&self, req: &StartOverRequest) -> Result<Vec<String>, String> {
        let mut done = Vec::new();
        if req.needs_permission() {
            let report = self.apply(&json!({
                "remove_server": req.server,
                "remove_cloudflared": req.cloudflared,
                "remove_cloudflared_tunnel": req.cloudflared_tunnel,
                "remove_firewall_rule": req.firewall_rule,
                "clear_data_dir": req.data_folder,
                "remove_data_dir": req.data_folder,
            }))?;
            done.extend(actions_of(&report));
        }
        if req.data_folder && self.old_dirs.iter().any(|d| old_copy_in(d).is_some()) {
            done.extend(self.remove_old_copy()?);
        }
        Ok(done)
    }

    /// Revision 18: `services firewall-check` (reads only, no permission prompt).
    fn firewall_check(&self, rule_name: &str) -> Result<FirewallState, String> {
        let out = self.output(&["services", "firewall-check", "--rule", rule_name])?;
        serde_json::from_str(&out)
            .map_err(|e| format!("the firewall check was not understood: {e}"))
    }

    /// Revision 18: the rule put back and Windows' Block rules removed in one
    /// `services apply` (one permission prompt on Windows).
    fn firewall_fix(&self, req: &FirewallFix) -> Result<Vec<String>, String> {
        let report = self.apply(&json!({
            "firewall_rule": req.add_rule.as_ref().map(|r| json!({
                "name": r.name,
                "ports": r.ports,
                "profiles": r.profiles,
            })),
            "remove_block_rules": req.remove_block_rules,
        }))?;
        Ok(actions_of(&report))
    }

    /// A1: the old copy is the user's own folder: removed as the user (no
    /// permission prompt). Only a real folder holding the helper's marker is
    /// removed; links inside are removed, never followed.
    fn remove_old_copy(&self) -> Result<Vec<String>, String> {
        let mut done = Vec::new();
        for d in &self.old_dirs {
            if old_copy_in(d).is_none() {
                continue;
            }
            std::fs::remove_dir_all(d).map_err(|e| {
                format!("The old copy in {} could not be removed: {e}", d.display())
            })?;
            done.push(format!("old copy {} removed", d.display()));
        }
        if done.is_empty() {
            return Err("There is no old copy to remove.".into());
        }
        Ok(done)
    }

    /// The operating system's resolver (hosts file, the router's or company's DNS),
    /// as a phone on the same network would ask it. Not found is an empty list.
    fn resolve_name(&self, name: &str) -> Result<Vec<String>, String> {
        use std::net::ToSocketAddrs;
        let mut out: Vec<String> = Vec::new();
        if let Ok(list) = (name, 0u16).to_socket_addrs() {
            for a in list {
                let ip = a.ip().to_string();
                if !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn elevation_quotes_paths() {
        let s = elevation_script(
            Path::new("C:\\Program Files\\MoneyKeep\\moneykeep-server.exe"),
            &[
                "services".into(),
                "--plan".into(),
                "C:\\Users\\O'Neil\\plan.json".into(),
            ],
        );
        assert!(s.starts_with(
            "try { $p = Start-Process -FilePath 'C:\\Program Files\\MoneyKeep\\moneykeep-server.exe'"
        ));
        // release-packaging.md: a declined permission prompt gives its own exit code.
        assert!(
            s.contains("-ErrorAction Stop } catch { exit 1223 }; exit $p.ExitCode"),
            "{s}"
        );
        assert!(
            s.contains("@('\"services\"','\"--plan\"','\"C:\\Users\\O''Neil\\plan.json\"')"),
            "{s}"
        );
        assert!(s.contains("-Verb RunAs -Wait -PassThru"));
    }

    /// security-review-1 A3: the helper is given the plan's SHA-256; A1: an old
    /// copy is found by the helper's marker in a real folder, offered for 30 days
    /// and removed as the user.
    #[test]
    fn sr_a3_plan_hash_and_sr_a1_old_copy() {
        let args = apply_args(Path::new("p.json"), Path::new("r.json"), b"{}");
        assert_eq!(args[6], "--plan-sha256");
        assert_eq!(
            args[7],
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        let (token, hash) = new_desktop_token().unwrap();
        assert_eq!(token.len(), 43);
        assert_eq!(hash, sha256_hex(token.as_bytes()));
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("capture-server");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("server.toml"), "").unwrap();
        let h = SystemHost::with_old_dirs(None, root.path().join("setup"), vec![old.clone()]);
        assert!(h.old_copy().is_none(), "not moved: no marker");
        assert!(h.remove_old_copy().is_err());
        std::fs::write(
            old.join(MOVED_MARKER),
            json!({"to": "C:/ProgramData/MoneyKeep/capture-server", "at": now_secs() - 86_400})
                .to_string(),
        )
        .unwrap();
        let c = h.old_copy().unwrap();
        assert_eq!(c.path, old.display().to_string());
        // upgrade-resilience-1 U1.3: still offered after 30 days, while it exists
        // (the Status card then moves it to Clean up).
        std::fs::write(
            old.join(MOVED_MARKER),
            json!({"to": "x", "at": now_secs() - 31 * 86_400}).to_string(),
        )
        .unwrap();
        assert!(h.old_copy().is_some());
        std::fs::write(
            old.join(MOVED_MARKER),
            json!({"to": "x", "at": now_secs()}).to_string(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            // A link inside is removed, not followed.
            let outside = root.path().join("outside.txt");
            std::fs::write(&outside, "keep").unwrap();
            std::os::unix::fs::symlink(&outside, old.join("link")).unwrap();
        }
        let done = h.remove_old_copy().unwrap();
        assert!(done[0].contains("removed"), "{done:?}");
        assert!(!old.exists());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_to_string(root.path().join("outside.txt")).unwrap(),
            "keep"
        );
    }

    /// SR-A6: Install the server again names the user's old folder that holds
    /// `server.toml` (never a link), and carries the Wi-Fi firewall rule.
    #[test]
    fn sr_a6_reinstall_plan() {
        use mk_commands::capture_setup::FirewallRule;
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("MoneyKeep/data/capture-server");
        let old = root.path().join("Old/data/capture-server");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("server.toml"), "").unwrap();
        let req = ReinstallRequest {
            allow_lan: false,
            firewall_rule: Some(FirewallRule {
                name: "MoneyKeep capture (Wi-Fi)".into(),
                ports: vec![15443, 15080],
                profiles: vec!["Private".into(), "Domain".into()],
            }),
        };
        let p = reinstall_plan(&[empty.clone(), old.clone()], &req);
        assert_eq!(p["reinstall_server"], true);
        assert_eq!(p["source_data_dir"], old.display().to_string());
        assert_eq!(p["firewall_rule"]["ports"], json!([15443, 15080]));
        assert_eq!(p["allow_lan"], false);
        let p = reinstall_plan(std::slice::from_ref(&empty), &ReinstallRequest::default());
        assert!(p["source_data_dir"].is_null() && p["firewall_rule"].is_null());
        // UR-3: a folder the data was moved out of is never named.
        std::fs::write(old.join(MOVED_MARKER), "{}").unwrap();
        let p = reinstall_plan(&[empty.clone(), old.clone()], &req);
        assert!(p["source_data_dir"].is_null());
        std::fs::remove_file(old.join(MOVED_MARKER)).unwrap();
        #[cfg(unix)]
        {
            let link = root.path().join("Link/data/capture-server");
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&old, &link).unwrap();
            let p = reinstall_plan(&[link], &ReinstallRequest::default());
            assert!(p["source_data_dir"].is_null(), "a link is not named");
        }
    }

    #[test]
    fn missing_program_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let h = SystemHost::with(None, dir.path().to_path_buf());
        let r = h.inspect();
        assert!(!r.server_program);
        assert!(r.notes[0].contains("moneykeep-server"));
        let e = h.install(&InstallRequest::default()).unwrap_err();
        assert!(e.contains("cargo xtask release"), "{e}");
    }
}
