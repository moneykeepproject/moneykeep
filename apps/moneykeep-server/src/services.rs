//! The capture server and `cloudflared` as background services (mobile-capture.md
//! revision 2, section B5).
//!
//! The desktop runs `moneykeep-server services apply --plan FILE --report FILE`,
//! elevated once on Windows (UAC), and this module carries the plan out:
//!
//! - the capture server as a service that starts with the computer (a Windows
//!   service, a systemd user unit on Linux, a launchd agent on macOS);
//! - for a new tunnel, `cloudflared service install <token>` (Cloudflare's
//!   "Create a tunnel (API)" page,
//!   <https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/>),
//!   using an installed `cloudflared`, or on Windows the official installer from
//!   Cloudflare's GitHub releases (download links on
//!   <https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/>),
//!   refused unless its Authenticode signature is valid and issued to Cloudflare;
//! - an existing `cloudflared` service running a different tunnel is never replaced,
//!   unless the desktop found that its tunnel no longer exists in the account and
//!   names that tunnel in the plan (mobile-capture.md revision 16: "Replace it"):
//!   then it is removed (`cloudflared service uninstall`) in the same elevated run,
//!   before `cloudflared` is installed for the new tunnel.
//!
//! On macOS (macos-release.md Part C) both run as per-user launchd agents with no
//! administrator rights: the server from the app bundle, and `cloudflared` (only
//! Cloudflare's own, from `/usr/local/bin` or `/opt/homebrew/bin`) with its token in
//! a file in the data folder.
//!
//! Update server also moves an installation made before the product was renamed
//! (branding.md step 6): the old-name service, install folder and data folder.
//!
//! Everything that touches the operating system sits behind [`ServiceManager`],
//! [`SignatureChecker`] and [`Downloader`], so the rules are tested with fakes. The
//! Windows, Linux and macOS service managers all compile on every system; only the
//! few calls into the Windows API (`windows::WindowsApi`) have a Windows-only
//! implementation, with a stand-in elsewhere.

use std::path::{Path, PathBuf};

use mk_core::{brand, legacy};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The server's Windows service name.
pub const SERVER_SERVICE: &str = "MoneyKeepCapture";
/// The server's service display name.
pub const SERVER_DISPLAY: &str = "MoneyKeep phone capture server";
/// The server's systemd user unit.
pub const SERVER_UNIT: &str = "moneykeep-capture.service";
/// The server's launchd agent label.
pub const SERVER_LABEL: &str = "com.moneykeep.capture-server";
/// The official Windows installer (64-bit).
pub const CLOUDFLARED_MSI_URL: &str = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.msi";
/// Hosts the installer download may visit (GitHub redirects release downloads to
/// its asset hosts).
pub const DOWNLOAD_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// The signer the installer must carry.
pub const CLOUDFLARE_SIGNER: &str = "Cloudflare, Inc.";
const MAX_INSTALLER: usize = 200 * 1024 * 1024;

/// A service's state (same names as the desktop's `ServiceState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    NotInstalled,
    Stopped,
    Running,
}

/// `services status`: what the desktop's setup dialog needs to know (same shape as
/// the desktop's `HostReport`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub os: String,
    pub server_program: bool,
    pub server: State,
    pub cloudflared_installed: bool,
    pub cloudflared_service: Option<State>,
    pub can_download_cloudflared: bool,
    pub lan_addresses: Vec<String>,
    pub notes: Vec<String>,
    /// This program's build version (revision 4: Update server compares it).
    #[serde(default)]
    pub program_version: String,
    /// This program has the phone app built in.
    #[serde(default)]
    pub program_has_app: bool,
    /// This program's capture version (release-packaging.md): the desktop offers
    /// Update now only when it equals its own.
    #[serde(default)]
    pub program_capture_version: String,
    /// The program the server's service runs, when installed.
    #[serde(default)]
    pub server_path: Option<String>,
    /// The server is installed under its name from before the rename (branding.md
    /// step 6): Update server moves it.
    #[serde(default)]
    pub old_name: bool,
    /// The tunnel the `cloudflared` service runs (its id), when its command line
    /// shows it (mobile-capture.md revision 16). Never the token itself.
    #[serde(default)]
    pub cloudflared_tunnel: Option<String>,
    /// The account that tunnel belongs to (the account id in its token), when known.
    #[serde(default)]
    pub cloudflared_account: Option<String>,
    /// The capture server's data folder (security-review-1 A1).
    #[serde(default)]
    pub data_dir: Option<String>,
    /// security-review-1 A6: the server's service is not installed but its data is
    /// still here (`server.toml` in the data folder or in the user's old folder):
    /// the desktop offers "Install the server again".
    #[serde(default)]
    pub data_found: bool,
    /// upgrade-resilience-1 U1.4: the server refused to start (a newer data folder
    /// or inbox, settings this version can't read, an inbox key that can't be
    /// opened): what the desktop shows.
    #[serde(default)]
    pub refusal: Option<String>,
    /// Which refusal ([`crate::upgrade::Refusal`], snake case).
    #[serde(default)]
    pub refusal_kind: Option<crate::upgrade::Refusal>,
}

/// The operating system's service control.
pub trait ServiceManager {
    fn server_state(&self) -> State;
    /// Create (or update) the server's service with this command line and start it.
    fn install_server(&self, exe: &Path, args: &[String]) -> Result<(), String>;
    fn uninstall_server(&self) -> Result<(), String>;
    fn start_server(&self) -> Result<(), String>;
    /// Stop the server's service and wait until its process has ended, ending it
    /// after `wait` (revision 4: the program file must be free to replace).
    fn stop_server(&self, wait: std::time::Duration) -> Result<(), String>;
    /// The program file the server's service runs, when installed.
    fn server_path(&self) -> Option<PathBuf>;
    /// An installed `cloudflared` program.
    fn cloudflared_path(&self) -> Option<PathBuf>;
    /// The `cloudflared` service, if one exists: its state and command line (which
    /// holds the tunnel token it runs).
    fn cloudflared_service(&self) -> Option<(State, String)>;
    /// Run the downloaded installer (Windows).
    fn install_cloudflared_package(&self, installer: &Path) -> Result<(), String>;
    /// `cloudflared service install <token>`.
    fn install_cloudflared_service(&self, exe: &Path, token: &str) -> Result<(), String>;
    /// `cloudflared service uninstall`.
    fn uninstall_cloudflared_service(&self, exe: &Path) -> Result<(), String>;
    /// Stop and delete the `cloudflared` service without the `cloudflared` program
    /// (Windows: `sc.exe stop` and `sc.exe delete`), when `service uninstall` failed.
    fn delete_cloudflared_service(&self) -> Result<(), String> {
        Err("the cloudflared service can only be removed with cloudflared here".into())
    }
    /// Revision 16b: delete `cloudflared`'s Windows event-log source key
    /// ([`CLOUDFLARED_EVENT_LOG_KEY`]), which `sc.exe delete` leaves behind and which
    /// makes the next `cloudflared service install` fail. `Ok(true)` when it was
    /// there and was deleted; other systems have none.
    fn delete_cloudflared_event_log(&self) -> Result<bool, String> {
        Ok(false)
    }
    /// Add Wi-Fi capture's inbound firewall rule (mobile-capture.md revision 10):
    /// `Ok(true)` when added, `Ok(false)` when this system has no firewall MoneyKeep
    /// manages (the setup explains what to do instead).
    fn add_firewall_rule(&self, rule: &FirewallRule) -> Result<bool, String> {
        let _ = rule;
        Ok(false)
    }
    /// Remove the rule by name: `Ok(true)` when this system has such rules.
    fn remove_firewall_rule(&self, name: &str) -> Result<bool, String> {
        let _ = name;
        Ok(false)
    }
    /// Revision 18: the inbound firewall rules named `rule_name`, and the rules
    /// that name the program `program` (a file name, `moneykeep-server.exe`).
    /// `Ok(None)` when this system has no firewall MoneyKeep manages.
    fn firewall_scan(
        &self,
        rule_name: &str,
        program: &str,
    ) -> Result<Option<FirewallScan>, String> {
        let _ = (rule_name, program);
        Ok(None)
    }
    /// Revision 18: the networks this computer is on and how Windows treats each.
    fn network_profiles(&self) -> Result<Vec<NetProfile>, String> {
        Ok(Vec::new())
    }
    /// Revision 18: remove one firewall rule by its id (Windows' rule `Name`).
    fn remove_firewall_rule_id(&self, id: &str) -> Result<(), String> {
        let _ = id;
        Err("this system's firewall is not managed by MoneyKeep".into())
    }
    /// security-review-1 A1: the server's service command line as its words
    /// (program first), when installed.
    fn server_command(&self) -> Option<Vec<String>> {
        None
    }
    /// A1: a folder's owner as a security identifier (`S-1-5-18` …), where MoneyKeep
    /// checks owners (Windows); `Ok(None)` elsewhere.
    fn folder_owner(&self, dir: &Path) -> Result<Option<String>, String> {
        let _ = dir;
        Ok(None)
    }
    /// A1: set a folder's permissions for its use, with inheritance turned off
    /// (Windows); elsewhere owner-only.
    fn protect_folder(&self, dir: &Path, use_: FolderUse) -> Result<(), String> {
        let _ = use_;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("cannot protect {}: {e}", dir.display()))?;
        }
        #[cfg(not(unix))]
        let _ = dir;
        Ok(())
    }
    /// A2: keep `file` open, refusing others write and delete access, until the
    /// returned guard is dropped.
    fn hold_file(&self, file: &Path) -> Result<Box<dyn std::any::Any>, String> {
        hold_read_only(file).map(|f| Box::new(f) as Box<dyn std::any::Any>)
    }
    /// upgrade-resilience-1 U1.4: the server's last exit code when it stopped by
    /// itself (Windows: the service-specific exit code), when known.
    fn server_exit_code(&self) -> Option<u32> {
        None
    }
    /// macos-release.md C5: whether this system's application firewall is on
    /// (macOS: `socketfilterfw --getglobalstate`); `None` where MoneyKeep doesn't
    /// look.
    fn app_firewall(&self) -> Option<bool> {
        None
    }
    /// L1: `cloudflared --version` (run only after its signature was checked).
    fn cloudflared_version(&self, exe: &Path) -> Option<String> {
        let _ = exe;
        None
    }
    /// L1: run the `cloudflared` service with this command line instead of the one
    /// `service install` wrote, and restart it.
    fn set_cloudflared_command(&self, exe: &Path, args: &[String]) -> Result<(), String> {
        let _ = (exe, args);
        Err("the cloudflared service's command line is not changed on this system".into())
    }
}

/// What a folder MoneyKeep protects is for (security-review-1 A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderUse {
    /// `%ProgramData%\MoneyKeep`: SYSTEM and Administrators; Users may read it.
    Machine,
    /// The capture server's data folder: SYSTEM and Administrators, and modify for
    /// the service's own account.
    Data,
    /// SYSTEM and Administrators only (downloads, the tunnel token).
    AdminOnly,
}

/// The share mode the downloaded installer is held with (A2): others may read it,
/// never write or delete it (`FILE_SHARE_READ`).
pub const HOLD_SHARE_MODE: u32 = 0x1;

/// Open `file` for reading, letting others only read it (Windows share mode
/// [`HOLD_SHARE_MODE`]).
pub fn hold_read_only(file: &Path) -> Result<std::fs::File, String> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(HOLD_SHARE_MODE);
    }
    o.open(file)
        .map_err(|e| format!("cannot open {}: {e}", file.display()))
}

/// The owners a protected folder may already have (A1): SYSTEM, Administrators.
pub const TRUSTED_OWNERS: [&str; 2] = ["S-1-5-18", "S-1-5-32-544"];

/// A1's refusal.
pub fn unsafe_folder(dir: &Path) -> String {
    format!(
        "The capture server's folder isn't safe to use: {}",
        dir.display()
    )
}

/// L6 (upgrade-resilience-1): MoneyKeep's machine folder exists but wasn't made by
/// its setup (another owner).
pub fn not_ours(machine: &Path) -> String {
    format!(
        "{} Remove the folder {} (it was not made by MoneyKeep's setup) and try again.",
        unsafe_folder(machine),
        machine.display()
    )
}

/// A real folder: not a link, junction or other reparse point.
pub fn is_real_folder(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                // FILE_ATTRIBUTE_REPARSE_POINT: links and junctions.
                if m.file_attributes() & 0x400 != 0 {
                    return false;
                }
            }
            m.is_dir() && !m.file_type().is_symlink()
        }
        Err(_) => false,
    }
}

/// Make or check one protected folder (A1): an existing one must be a real folder
/// owned by SYSTEM or Administrators; then its permissions are set.
pub fn ensure_protected(
    sm: &dyn ServiceManager,
    dir: &Path,
    use_: FolderUse,
) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => check_folder(sm, dir)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        Err(e) => return Err(format!("cannot check {}: {e}", dir.display())),
    }
    sm.protect_folder(dir, use_)
}

/// An existing folder is safe to use (A1): real, and owned by SYSTEM or
/// Administrators where owners are checked.
pub fn check_folder(sm: &dyn ServiceManager, dir: &Path) -> Result<(), String> {
    if !is_real_folder(dir) {
        return Err(unsafe_folder(dir));
    }
    match sm.folder_owner(dir) {
        Ok(None) => Ok(()),
        Ok(Some(owner)) if TRUSTED_OWNERS.contains(&owner.as_str()) => Ok(()),
        Ok(Some(_)) | Err(_) => Err(unsafe_folder(dir)),
    }
}

/// A firewall rule as the system reports it (revision 18).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundRule {
    /// The rule's id (Windows: its `Name`, often a GUID).
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    /// `Allow` or `Block`.
    pub action: String,
    /// `Inbound` or `Outbound`.
    pub direction: String,
    /// The program the rule names, when it names one.
    #[serde(default)]
    pub program: String,
}

impl FoundRule {
    fn inbound(&self) -> bool {
        self.direction.eq_ignore_ascii_case("inbound")
    }
    fn is(&self, action: &str) -> bool {
        self.action.eq_ignore_ascii_case(action)
    }
    /// The rule names this program (by file name, any folder, any case).
    fn names_program(&self, program: &str) -> bool {
        let p = self.program.trim().trim_matches('"');
        let file = p.rsplit(['\\', '/']).next().unwrap_or(p);
        !file.is_empty() && file.eq_ignore_ascii_case(program)
    }
}

/// What [`ServiceManager::firewall_scan`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallScan {
    /// The rules with the rule's display name.
    #[serde(default)]
    pub named: Vec<FoundRule>,
    /// The rules that name the program.
    #[serde(default)]
    pub program_rules: Vec<FoundRule>,
}

/// A network the computer is on (revision 18).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetProfile {
    pub name: String,
    #[serde(default)]
    pub interface: String,
    /// `Public`, `Private` or `DomainAuthenticated`.
    pub category: String,
}

/// The state of Wi-Fi capture's firewall rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    Ok,
    #[default]
    Missing,
    Disabled,
}

/// `services firewall-check` (revision 18): MoneyKeep's rule, Windows' own Block
/// rules for the capture server, and the networks Windows treats as Public.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallCheck {
    /// This system's firewall is one MoneyKeep manages (Windows).
    pub managed: bool,
    pub rule: RuleState,
    /// Inbound Block rules naming `moneykeep-server.exe` (Windows makes these when
    /// its "allow access" pop-up is dismissed). Allow rules are never listed.
    #[serde(default)]
    pub block_rules: Vec<FoundRule>,
    /// The networks Windows treats as Public.
    #[serde(default)]
    pub public_networks: Vec<String>,
    /// A check that could not be made, in words.
    #[serde(default)]
    pub error: Option<String>,
    /// security-review-1 A6: the capture server's service state (while it is not
    /// installed the desktop shows no firewall problem: installing it again
    /// recreates the rule).
    #[serde(default)]
    pub server: Option<State>,
    /// macos-release.md C5: macOS's application firewall is on (`Some(false)`: off;
    /// `None`: not macOS, or unknown).
    #[serde(default)]
    pub app_firewall: Option<bool>,
}

/// The program Windows' own rules name.
pub const SERVER_EXE: &str = "moneykeep-server.exe";

/// Check the firewall (revision 18) through `sm`.
pub fn firewall_check(sm: &dyn ServiceManager, rule_name: &str) -> FirewallCheck {
    let mut c = FirewallCheck {
        server: Some(sm.server_state()),
        app_firewall: sm.app_firewall(),
        ..FirewallCheck::default()
    };
    match sm.firewall_scan(rule_name, SERVER_EXE) {
        Ok(None) => {}
        Ok(Some(scan)) => {
            c.managed = true;
            let ours: Vec<&FoundRule> = scan
                .named
                .iter()
                .filter(|r| r.display_name == rule_name && r.inbound() && r.is("Allow"))
                .collect();
            c.rule = if ours.iter().any(|r| r.enabled) {
                RuleState::Ok
            } else if ours.is_empty() {
                RuleState::Missing
            } else {
                RuleState::Disabled
            };
            c.block_rules = block_rules(&scan);
        }
        Err(e) => c.error = Some(e),
    }
    match sm.network_profiles() {
        Ok(list) => {
            c.public_networks = list
                .into_iter()
                .filter(|n| n.category.eq_ignore_ascii_case("public"))
                .map(|n| {
                    if n.name.is_empty() {
                        n.interface
                    } else {
                        n.name
                    }
                })
                .collect();
        }
        Err(e) => {
            c.error.get_or_insert(e);
        }
    }
    c
}

/// The inbound Block rules naming the capture server (never an Allow rule).
fn block_rules(scan: &FirewallScan) -> Vec<FoundRule> {
    scan.program_rules
        .iter()
        .filter(|r| r.inbound() && r.is("Block") && r.names_program(SERVER_EXE))
        .cloned()
        .collect()
}

/// `services repair-firewall --data DIR` (revision 18, the installer, as
/// administrator): when `server.toml` has Wi-Fi capture on and the rule is missing
/// or disabled, put it back (the same rule Wi-Fi setup makes). Returns what was done.
pub fn repair_firewall(
    sm: &dyn ServiceManager,
    config: &crate::Config,
) -> Result<Vec<String>, String> {
    if config.wifi_address.trim().is_empty() {
        return Ok(vec!["Wi-Fi capture is off: nothing to repair".into()]);
    }
    let check = firewall_check(sm, mk_capture_api::FIREWALL_RULE);
    if !check.managed {
        return Ok(vec![
            "this system's firewall is not managed by MoneyKeep".into(),
        ]);
    }
    if check.rule == RuleState::Ok {
        return Ok(vec![format!(
            "firewall rule \"{}\" is in place",
            mk_capture_api::FIREWALL_RULE
        )]);
    }
    let rule = FirewallRule {
        name: mk_capture_api::FIREWALL_RULE.to_owned(),
        ports: vec![config.wifi_https_port, config.wifi_http_port],
        profiles: vec!["Private".into(), "Domain".into()],
        program: sm.server_path(),
    };
    sm.add_firewall_rule(&rule)?;
    Ok(vec![format!(
        "firewall rule \"{}\" put back for ports {} and {} (Private and Domain networks only)",
        rule.name, config.wifi_https_port, config.wifi_http_port
    )])
}

/// An inbound rule for Wi-Fi capture's two ports (revision 10). Only the Private
/// and Domain network profiles are ever used: never Public.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallRule {
    pub name: String,
    pub ports: Vec<u16>,
    pub profiles: Vec<String>,
    /// The program the rule is limited to (the installed server), when known.
    #[serde(default)]
    pub program: Option<PathBuf>,
}

impl FirewallRule {
    /// The profiles kept: Private and Domain only, in that order.
    pub fn safe_profiles(&self) -> Vec<&'static str> {
        ["Private", "Domain"]
            .into_iter()
            .filter(|p| self.profiles.iter().any(|q| q.eq_ignore_ascii_case(p)))
            .collect()
    }
}

/// The PowerShell commands (Windows Defender Firewall, `New-NetFirewallRule`) that
/// replace the rule: an earlier rule of the same name is removed first.
pub fn firewall_script(rule: &FirewallRule) -> Result<String, String> {
    let profiles = rule.safe_profiles();
    if profiles.is_empty() || rule.ports.is_empty() || rule.name.trim().is_empty() {
        return Err(
            "the firewall rule needs a name, ports and the Private or Domain profile".into(),
        );
    }
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let ports: Vec<String> = rule.ports.iter().map(u16::to_string).collect();
    let program = rule
        .program
        .as_ref()
        .map(|p| format!(" -Program {}", q(&p.display().to_string())))
        .unwrap_or_default();
    Ok(format!(
        "Remove-NetFirewallRule -DisplayName {name} -ErrorAction SilentlyContinue; \
         New-NetFirewallRule -DisplayName {name} -Description {desc} -Direction Inbound -Action Allow \
         -Protocol TCP -LocalPort {ports} -Profile {profiles}{program} | Out-Null",
        name = q(&rule.name),
        desc = q("Lets phones on your home or office network reach MoneyKeep phone capture."),
        ports = ports.join(","),
        profiles = profiles.join(","),
    ))
}

fn ps_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Revision 18: the PowerShell script that lists the rules named `rule_name` and
/// the rules naming `program` (by file name) as one JSON object
/// `{"named": [...], "program_rules": [...]}`; reads only (no administrator rights).
pub fn firewall_scan_script(rule_name: &str, program: &str) -> String {
    let row = "ForEach-Object { $f = $_ | Get-NetFirewallApplicationFilter; [pscustomobject]@{ id = [string]$_.Name; display_name = [string]$_.DisplayName; enabled = ([string]$_.Enabled -eq 'True'); action = [string]$_.Action; direction = [string]$_.Direction; program = [string]$f.Program } }";
    format!(
        "$ErrorActionPreference = 'SilentlyContinue'; \
         $named = @(Get-NetFirewallRule -DisplayName {name} | {row}); \
         $prog = @(Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -and ((Split-Path -Leaf $_.Program) -ieq {program}) }} | Get-NetFirewallRule | {row}); \
         ConvertTo-Json -Depth 4 -Compress -InputObject ([pscustomobject]@{{ named = $named; program_rules = $prog }})",
        name = ps_literal(rule_name),
        program = ps_literal(program),
    )
}

/// One object or a list of them (PowerShell's ConvertTo-Json writes a single
/// element without the list).
fn one_or_many<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Vec<T> {
    match v {
        serde_json::Value::Array(a) => a
            .into_iter()
            .filter_map(|x| serde_json::from_value(x).ok())
            .collect(),
        serde_json::Value::Null => Vec::new(),
        other => serde_json::from_value(other).ok().into_iter().collect(),
    }
}

/// Read [`firewall_scan_script`]'s output.
pub fn parse_firewall_scan(json: &str) -> Result<FirewallScan, String> {
    let text = json.trim();
    if text.is_empty() {
        return Ok(FirewallScan::default());
    }
    let mut v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("the firewall list was not understood: {e}"))?;
    Ok(FirewallScan {
        named: one_or_many(
            v.get_mut("named")
                .map(serde_json::Value::take)
                .unwrap_or_default(),
        ),
        program_rules: one_or_many(
            v.get_mut("program_rules")
                .map(serde_json::Value::take)
                .unwrap_or_default(),
        ),
    })
}

/// Revision 18: the networks and their category, as JSON.
pub const NETWORK_PROFILES_SCRIPT: &str = "$ErrorActionPreference = 'SilentlyContinue'; ConvertTo-Json -Compress -InputObject @(Get-NetConnectionProfile | ForEach-Object { [pscustomobject]@{ name = [string]$_.Name; interface = [string]$_.InterfaceAlias; category = [string]$_.NetworkCategory } })";

/// Read [`NETWORK_PROFILES_SCRIPT`]'s output.
pub fn parse_network_profiles(json: &str) -> Result<Vec<NetProfile>, String> {
    let text = json.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("the network list was not understood: {e}"))?;
    Ok(one_or_many(v))
}

/// Revision 18: remove one rule by its id (`Name`).
pub fn firewall_remove_id_script(id: &str) -> String {
    format!(
        "Remove-NetFirewallRule -Name {} -ErrorAction Stop",
        ps_literal(id)
    )
}

/// The PowerShell command removing the rule by name.
pub fn firewall_remove_script(name: &str) -> String {
    format!(
        "Remove-NetFirewallRule -DisplayName '{}' -ErrorAction SilentlyContinue",
        name.replace('\'', "''")
    )
}

/// Checks a downloaded file's code signature; `Ok` carries the signer's subject.
pub trait SignatureChecker {
    fn signer(&self, file: &Path) -> Result<String, String>;
}

/// Downloads over HTTPS.
pub trait Downloader {
    fn download(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String>;
}

/// Whether the server in `data_dir` answers its health check (waiting a little
/// for it to start).
pub trait Probe {
    fn healthy(&self, data_dir: &Path) -> bool;
}

/// The installation from before the rename (branding.md step 6): the old-name
/// service's controls and its install folder.
pub struct Legacy<'a> {
    pub sm: &'a dyn ServiceManager,
    pub install_dir: &'a Path,
}

impl std::fmt::Debug for Legacy<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Legacy")
            .field("install_dir", &self.install_dir)
            .finish_non_exhaustive()
    }
}

/// What `apply` works with: the operating system's pieces (fakes in tests) and the
/// folders.
pub struct Env<'a> {
    pub sm: &'a dyn ServiceManager,
    pub sig: &'a dyn SignatureChecker,
    pub dl: &'a dyn Downloader,
    pub probe: &'a dyn Probe,
    pub can_download: bool,
    /// Where downloads go: the helper makes a private folder inside it.
    pub tmp: &'a Path,
    /// Where the server program is installed (the service runs it from here).
    pub install_dir: &'a Path,
    /// security-review-1 A1: the capture server's data folder ([`data_dir`]). The
    /// plan cannot name another.
    pub data_dir: &'a Path,
    /// A1: MoneyKeep's machine folder (`%ProgramData%\MoneyKeep`), whose folders the
    /// helper makes and protects; `None` where the server runs as the user.
    pub machine_dir: Option<&'a Path>,
    /// A2: run `cloudflared` only after its Authenticode signature was checked.
    pub check_cloudflared: bool,
    /// The installation from before the rename, moved by Update server.
    pub legacy: Option<Legacy<'a>>,
    /// security-review-1 A6: the only folders a plan may name to copy the data
    /// from ([`source_dirs`]: the helper's own user's old data folders).
    pub source_dirs: &'a [PathBuf],
    /// L5: seals a copied plain inbox key ([`crate::store::platform_sealer`]; a
    /// fake in tests).
    pub sealer: Option<&'a dyn crate::store::KeySealer>,
}

impl std::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("install_dir", &self.install_dir)
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

/// security-review-1 A1: MoneyKeep's machine folder on Windows,
/// `%ProgramData%\MoneyKeep`; `None` on other systems (the server runs as the user).
pub fn machine_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    Some(
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
            .join(brand::FOLDER),
    )
}

/// The capture server's data folder: `%ProgramData%\MoneyKeep\capture-server` on
/// Windows (A1); the user's application data elsewhere (as before).
pub fn data_dir() -> PathBuf {
    match machine_dir() {
        Some(m) => m.join("capture-server"),
        None => user_data_dir(brand::FOLDER).unwrap_or_else(|| PathBuf::from("capture-server")),
    }
}

/// The data folder in the user's application data (Windows: the folder used before
/// A1, `%LOCALAPPDATA%\MoneyKeep\data\capture-server`).
pub fn user_data_dir(folder: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", folder)
        .map(|d| d.data_local_dir().join("capture-server"))
}

/// security-review-1 A6: the folders the data may be copied from when the server
/// is installed again: the user's own data folder from before A1
/// (`%LOCALAPPDATA%\MoneyKeep\data\capture-server`) and the one from before the
/// rename. The helper runs elevated as the same user, so these are that user's.
pub fn source_dirs() -> Vec<PathBuf> {
    [brand::FOLDER, legacy::FOLDER]
        .into_iter()
        .filter_map(user_data_dir)
        .collect()
}

/// A6: capture-server data is still on this computer: `server.toml` in the data
/// folder or in one of `sources` (a real folder, not a link). A data folder that
/// exists but that this user may not open (A1: only the service and administrators
/// can) counts as found.
pub fn data_found(data_dir: &Path, sources: &[PathBuf]) -> bool {
    let has = |d: &Path| is_real_folder(d) && d.join(crate::config::CONFIG_FILE).is_file();
    has(data_dir)
        || (is_real_folder(data_dir)
            && std::fs::read_dir(data_dir)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied))
        || sources.iter().any(|s| has(s) && !was_moved(s))
}

/// The server program's file name.
pub fn program_name() -> &'static str {
    if cfg!(windows) {
        "moneykeep-server.exe"
    } else {
        "moneykeep-server"
    }
}

/// The install folder (revision 4): `%ProgramFiles%\MoneyKeep\capture` on Windows,
/// `~/.local/share/moneykeep/capture` on Linux, `~/Library/Application
/// Support/MoneyKeep/capture` on macOS.
///
/// macos-release.md C1: on macOS, when this program runs from inside an app bundle
/// (`/Applications/MoneyKeep.app/Contents/MacOS`), that folder: the agent runs the
/// bundle's own copy, nothing is copied out of it.
pub fn install_dir() -> PathBuf {
    if cfg!(target_os = "macos")
        && let Some(d) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            .filter(|d| bundle_program_dir(d))
    {
        return d;
    }
    install_dir_named(brand::FOLDER, &brand::FOLDER.to_lowercase())
}

/// C1: `dir` is an app bundle's program folder (`<Name>.app/Contents/MacOS`).
pub fn bundle_program_dir(dir: &Path) -> bool {
    let mut parts = dir.components().rev();
    parts.next().is_some_and(|c| c.as_os_str() == "MacOS")
        && parts.next().is_some_and(|c| c.as_os_str() == "Contents")
        && parts
            .next()
            .is_some_and(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
}

/// The install folder from before the rename (branding.md step 6).
pub fn legacy_install_dir() -> PathBuf {
    install_dir_named(legacy::FOLDER, legacy::FOLDER_LOWER)
}

fn install_dir_named(folder: &str, lower: &str) -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
    };
    if cfg!(windows) {
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Program Files"))
            .join(folder)
            .join("capture")
    } else if cfg!(target_os = "macos") {
        home()
            .join("Library/Application Support")
            .join(folder)
            .join("capture")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join(lower)
            .join("capture")
    }
}

/// `cloudflared`'s Windows event-log source (mobile-capture.md revision 16b).
pub const CLOUDFLARED_EVENT_LOG_KEY: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared";

/// `cloudflared service install` failed only because its event-log source already
/// exists (revision 16b: "Cannot install event logger: … registry key already
/// exists").
pub fn event_logger_only(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("cannot install event logger") && e.contains("already exists")
}

/// Install the `cloudflared` service (revision 16b): a stale event-log source is
/// removed first when no `cloudflared` service exists, and an install that reports
/// only the event-logger error while the service now exists counts as installed.
fn install_cloudflared(
    sm: &dyn ServiceManager,
    cf: &Path,
    token: &str,
    report: &mut Report,
) -> Result<(), String> {
    if sm.cloudflared_service().is_none() && sm.delete_cloudflared_event_log()? {
        report
            .actions
            .push("stale cloudflared event-log entry removed".into());
    }
    match sm.install_cloudflared_service(cf, token) {
        Ok(()) => Ok(()),
        Err(e) if event_logger_only(&e) && sm.cloudflared_service().is_some() => {
            report.actions.push(
                "note: cloudflared could not register its event logger (the entry already exists); the service is installed"
                    .into(),
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// What to do (written by the desktop, read once and deleted). Unknown fields are
/// refused (security-review-1 A3: a plan asking for anything else is refused whole).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// The data folder the desktop expects. The helper always uses its own
    /// ([`data_dir`], A1) and refuses a plan naming another.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// Install (or update) the server's service.
    #[serde(default)]
    pub install_server: bool,
    /// Let the server listen on a LAN address (`--allow-lan`). Left out (`None`),
    /// the service's present command line decides (upgrade-resilience-1 L1).
    #[serde(default)]
    pub allow_lan: Option<bool>,
    /// Install `cloudflared` as a service with this tunnel token.
    #[serde(default)]
    pub tunnel_token: Option<String>,
    #[serde(default)]
    pub remove_server: bool,
    #[serde(default)]
    pub remove_cloudflared: bool,
    #[serde(default)]
    pub start_server: bool,
    /// Replace the installed server program with this one and restart it
    /// (revision 4, Update server).
    #[serde(default)]
    pub update_server: bool,
    /// Add Wi-Fi capture's inbound firewall rule (revision 10).
    #[serde(default)]
    pub firewall_rule: Option<FirewallRule>,
    /// Remove the firewall rule of this name (Remove Wi-Fi capture).
    #[serde(default)]
    pub remove_firewall_rule: Option<String>,
    /// Revision 18: remove these firewall rules (by id) — only those that are
    /// inbound Block rules naming the capture server when the plan runs.
    #[serde(default)]
    pub remove_block_rules: Vec<String>,
    /// Restart the server's service (Wi-Fi settings changed) when it is installed.
    #[serde(default)]
    pub restart_server: bool,
    /// Revision 16: the `cloudflared` service found here runs this tunnel, which no
    /// longer exists in the account; replace it with the one for `tunnel_token`.
    /// Only a service running exactly this tunnel is replaced.
    #[serde(default)]
    pub replace_cloudflared_tunnel: Option<String>,
    /// Revision 17 (Start over): remove the `cloudflared` service (with
    /// `remove_cloudflared`) only while it runs this tunnel.
    #[serde(default)]
    pub remove_cloudflared_tunnel: Option<String>,
    /// Revision 17: empty the capture server's data folder (certificates, paired
    /// phones, the inbox), after its service was removed.
    #[serde(default)]
    pub clear_data_dir: bool,
    /// Revision 17 with A1: remove the emptied data folder itself.
    #[serde(default)]
    pub remove_data_dir: bool,
    /// A1: set the desktop token, given only as its SHA-256 (64 hexadecimal digits);
    /// a new data folder is made first.
    #[serde(default)]
    pub desktop_token_sha256: Option<String>,
    /// Set that desktop token only when the data folder has none yet (Wi-Fi setup
    /// keeps the desktop's token otherwise); the report says whether it was set.
    #[serde(default)]
    pub desktop_token_only_if_new: bool,
    /// A1: `configure` (Access and address settings in server.toml).
    #[serde(default)]
    pub configure: Option<crate::setup::ConfigureStep>,
    /// A1: `set-aud`; the report says whether server.toml changed.
    #[serde(default)]
    pub set_aud: Option<String>,
    /// A1: `wifi configure`; the report carries what was made.
    #[serde(default)]
    pub wifi_configure: Option<crate::setup::WifiStep>,
    /// A1: `wifi remove`.
    #[serde(default)]
    pub wifi_remove: bool,
    /// A1: `wifi check` at the end, waiting up to this many seconds.
    #[serde(default)]
    pub wifi_check_wait: Option<u64>,
    /// security-review-1 A6: the service is missing; install it again with the data
    /// folder (copied from `source_data_dir` when the data folder has no
    /// `server.toml`), start it and check its health; on failure remove it again.
    #[serde(default)]
    pub reinstall_server: bool,
    /// A6: the user's old data folder to copy from: only one of [`source_dirs`].
    #[serde(default)]
    pub source_data_dir: Option<PathBuf>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("data_dir", &self.data_dir)
            .field("install_server", &self.install_server)
            .field(
                "tunnel_token",
                &self.tunnel_token.as_ref().map(|_| "(hidden)"),
            )
            .finish_non_exhaustive()
    }
}

/// A3: the refusal when the plan's bytes are not the approved ones.
pub const PLAN_CHANGED: &str = "The setup plan changed after you approved it. Nothing was changed.";

/// The SHA-256 of `bytes` as 64 lower-case hexadecimal digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    crate::store::hex(&sha2::Sha256::digest(bytes))
}

/// A3: read the plan file once, letting no one write to it meanwhile, and check its
/// SHA-256 against the one the desktop passed.
pub fn read_plan(file: &Path, sha256: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = hold_read_only(file).map_err(|e| format!("cannot read the plan: {e}"))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the plan: {e}"))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(sha256.trim()) {
        return Err(PLAN_CHANGED.into());
    }
    Ok(bytes)
}

/// Write the report as a new owner-only file: anything already at that name (a
/// link left there, say) is removed first and never written through.
///
/// upgrade-resilience-1 U1.2: written to a new temporary file, flushed and renamed
/// over the name ([`crate::store::write_private`]), so a cut-short write leaves the
/// old file.
pub fn write_report(file: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::store::write_private(file, bytes)
}

/// A3: what refuses a plan, before anything is done.
fn refused(what: &str) -> String {
    format!(
        "The setup plan asks for something MoneyKeep's helper doesn't do ({what}). Nothing was changed."
    )
}

/// A3: the plan only does what the helper does for MoneyKeep: its own firewall rule
/// for the Wi-Fi ports and the capture server's program, Block rules for that
/// program, its own data folder, and the tunnel token it carries.
pub fn check_plan(plan: &Plan, env: &Env<'_>) -> Result<(), String> {
    if let Some(d) = &plan.data_dir
        && d != env.data_dir
    {
        return Err(refused(&format!("another data folder, {}", d.display())));
    }
    // A6: data is copied only from the helper's own user's old folders, and only
    // when installing the server again.
    if let Some(d) = &plan.source_data_dir {
        if !plan.reinstall_server {
            return Err(refused(
                "a folder to copy from without installing the server again",
            ));
        }
        if d != env.data_dir && !env.source_dirs.iter().any(|s| s == d) {
            return Err(refused(&format!(
                "copying from another folder, {}",
                d.display()
            )));
        }
        // upgrade-resilience-1 U1.3: a folder the data was moved out of is stale.
        if d != env.data_dir && was_moved(d) {
            return Err(refused(&format!(
                "copying from {}, which the data was moved out of before",
                d.display()
            )));
        }
    }
    if plan.reinstall_server && (plan.install_server || plan.update_server || plan.remove_server) {
        return Err(refused(
            "installing the server again together with another server step",
        ));
    }
    let rule_name = mk_capture_api::FIREWALL_RULE;
    if let Some(r) = &plan.firewall_rule {
        if r.name != rule_name {
            return Err(refused(&format!("another firewall rule, \"{}\"", r.name)));
        }
        if r.program.is_some() {
            return Err(refused("a firewall rule for another program"));
        }
        if r.profiles.is_empty() || r.safe_profiles().len() != r.profiles.len() {
            return Err(refused("a firewall rule for other networks"));
        }
        // A6: from the plan's Wi-Fi step, server.toml in the data folder, then in
        // the allowed folder the data is (still) in.
        let want = match &plan.wifi_configure {
            Some(w) => Some((w.https_port, w.http_port)),
            None => crate::setup::wifi_ports(env.data_dir).or_else(|| {
                plan.source_data_dir
                    .iter()
                    .chain(env.source_dirs)
                    .filter(|d| is_real_folder(d) && !was_moved(d))
                    .find_map(|d| crate::setup::wifi_ports(d))
            }),
        };
        let mut have = r.ports.clone();
        have.sort_unstable();
        let ok = want.is_some_and(|(a, b)| {
            let mut w = vec![a, b];
            w.sort_unstable();
            w == have
        });
        if !ok {
            return Err(refused("a firewall rule for other ports"));
        }
    }
    if let Some(n) = &plan.remove_firewall_rule
        && n != rule_name
    {
        return Err(refused(&format!("removing another firewall rule, \"{n}\"")));
    }
    if let Some(t) = &plan.tunnel_token
        && (t.is_empty() || t.len() > 4096 || !t.chars().all(|c| c.is_ascii_graphic()))
    {
        return Err(refused("a tunnel token that is not one"));
    }
    for id in [
        &plan.replace_cloudflared_tunnel,
        &plan.remove_cloudflared_tunnel,
    ]
    .into_iter()
    .flatten()
    {
        if !is_uuid(id) {
            return Err(refused("a tunnel that is not a tunnel id"));
        }
    }
    if plan
        .desktop_token_sha256
        .as_deref()
        .is_some_and(|h| crate::setup::parse_hash(h).is_none())
    {
        return Err(refused("a desktop token hash that is not one"));
    }
    Ok(())
}

/// Read a plan's bytes, check it and carry it out (`services apply`).
pub fn apply_bytes(bytes: &[u8], exe: &Path, env: &Env<'_>) -> Report {
    let plan: Plan = match serde_json::from_slice(bytes) {
        Ok(p) => p,
        Err(e) => {
            return Report {
                error: Some(refused(&format!("the plan is not valid: {e}"))),
                ..Report::default()
            };
        }
    };
    apply(&plan, exe, env)
}

/// What was done (never a token).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub ok: bool,
    pub actions: Vec<String>,
    /// The data folder used (A1).
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// `set_aud`: whether server.toml changed.
    #[serde(default)]
    pub aud_changed: Option<bool>,
    /// `wifi_configure`: what was made (no key).
    #[serde(default)]
    pub wifi: Option<serde_json::Value>,
    /// `wifi_check_wait`: `{"ok": bool, "error": …}`.
    #[serde(default)]
    pub wifi_check: Option<serde_json::Value>,
    /// The data folder had no settings before this run (a new setup).
    #[serde(default)]
    pub fresh: bool,
    /// The plan's desktop token was set.
    #[serde(default)]
    pub desktop_token_set: bool,
    /// `cloudflared` was installed as a service by this run.
    pub cloudflared_service: bool,
    /// The `cloudflared` service of a deleted tunnel was removed first (revision 16).
    #[serde(default)]
    pub cloudflared_replaced: bool,
    pub error: Option<String>,
}

/// Revision 18: Windows' own Block rules for the capture server, removed by id —
/// checked again first, so an Allow rule (or any rule not naming the server) is
/// never removed whatever the plan says.
fn remove_block_rules(
    plan: &Plan,
    sm: &dyn ServiceManager,
    report: &mut Report,
) -> Result<(), String> {
    let found = sm
        .firewall_scan(mk_capture_api::FIREWALL_RULE, SERVER_EXE)?
        .map(|s| block_rules(&s))
        .unwrap_or_default();
    for id in &plan.remove_block_rules {
        match found.iter().find(|r| r.id == *id) {
            Some(r) => {
                sm.remove_firewall_rule_id(id)?;
                report.actions.push(format!(
                    "Windows' Block rule \"{}\" for {SERVER_EXE} removed",
                    r.display_name
                ));
            }
            None => report.actions.push(format!(
                "firewall rule {id} was left alone: it is not a Block rule for {SERVER_EXE}"
            )),
        }
    }
    Ok(())
}

/// The server's service command line.
pub fn server_args(data_dir: &Path, allow_lan: bool) -> Vec<String> {
    let mut args = vec![
        // macos-release.md C1: the macOS agent logs to `server.log` like the
        // Windows service.
        if cfg!(windows) || cfg!(target_os = "macos") {
            "service-run"
        } else {
            "run"
        }
        .to_owned(),
        "--data".to_owned(),
        data_dir.display().to_string(),
        "--key-file".to_owned(),
    ];
    if allow_lan {
        args.push("--allow-lan".to_owned());
    }
    args
}

/// L1 (upgrade-resilience-1): `--allow-lan` as the plan says, else as the service's
/// present command line has it.
fn allow_lan_of(plan: &Plan, sm: &dyn ServiceManager) -> bool {
    plan.allow_lan.unwrap_or_else(|| {
        sm.server_command()
            .is_some_and(|a| a.iter().any(|w| w == "--allow-lan"))
    })
}

/// Carry out a plan after [`check_plan`]. Stops at the first failure.
pub fn apply(plan: &Plan, exe: &Path, env: &Env<'_>) -> Report {
    let mut report = Report {
        data_dir: Some(env.data_dir.to_path_buf()),
        ..Report::default()
    };
    if let Err(e) = check_plan(plan, env) {
        report.error = Some(e);
        return report;
    }
    match apply_inner(plan, exe, env, &mut report) {
        Ok(()) => report.ok = true,
        Err(e) => report.error = Some(e),
    }
    if report.ok
        && let Some(wait) = plan.wifi_check_wait
    {
        report.wifi_check = Some(
            crate::setup::wifi_check(env.data_dir, wait.min(60))
                .unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e})),
        );
    }
    report
}

/// Copy `exe` into the install folder (unless it already runs from there); returns
/// the installed path.
fn install_copy(exe: &Path, dir: &Path) -> Result<PathBuf, String> {
    refresh_program(exe, dir).map(|c| c.target)
}

/// The previous program kept beside the new one until it passes its health check
/// (upgrade-resilience-1 L2).
pub fn prev_program(dir: &Path) -> PathBuf {
    dir.join(format!("{}.prev", program_name()))
}

/// The service's copy of the program, refreshed.
struct Refreshed {
    target: PathBuf,
    /// The previous program was kept as [`prev_program`].
    had_prev: bool,
}

impl Refreshed {
    /// Put the previous program back (L2).
    fn restore(&self, dir: &Path) {
        if self.had_prev {
            let _ = std::fs::remove_file(&self.target);
            let _ = std::fs::rename(prev_program(dir), &self.target);
        }
    }
    /// The new one passed: the previous one goes.
    fn keep(&self, dir: &Path) {
        if self.had_prev {
            let _ = std::fs::remove_file(prev_program(dir));
        }
    }
}

/// Copy `exe` into the install folder as the service's program (H1, L2), keeping
/// the program that was there as `.prev`; nothing changes when the service's copy
/// is `exe` itself. The service must be stopped first.
fn refresh_program(exe: &Path, dir: &Path) -> Result<Refreshed, String> {
    let target = dir.join(program_name());
    if same_file(exe, &target) {
        return Ok(Refreshed {
            target,
            had_prev: false,
        });
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let had_prev = target.is_file();
    if had_prev {
        let prev = prev_program(dir);
        let _ = std::fs::remove_file(&prev);
        std::fs::rename(&target, &prev)
            .map_err(|e| format!("cannot set the previous server program aside: {e}"))?;
    }
    let r = Refreshed { target, had_prev };
    if let Err(e) = std::fs::copy(exe, &r.target) {
        r.restore(dir);
        return Err(format!(
            "cannot copy the server program to {}: {e}",
            r.target.display()
        ));
    }
    Ok(r)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The data folder a service command line names (`--data DIR`).
pub fn data_of(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

/// Split a Windows command line into its words (the rules of
/// `CommandLineToArgvW`: quotes group, backslashes escape only before a quote).
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any, mut slashes) = (false, false, 0usize);
    for c in line.chars() {
        match c {
            '\\' => {
                slashes += 1;
                any = true;
            }
            '"' => {
                cur.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    cur.push('"');
                } else {
                    quoted = !quoted;
                }
                slashes = 0;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                cur.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                if any {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => {
                cur.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                cur.push(c);
                any = true;
            }
        }
    }
    cur.extend(std::iter::repeat_n('\\', slashes));
    if any {
        out.push(cur);
    }
    out
}

/// The data folder the server's service uses now, from its command line.
pub fn service_data_dir(sm: &dyn ServiceManager) -> Option<PathBuf> {
    sm.server_command().as_deref().and_then(data_of)
}

/// A1: the files of a data folder that are moved with it (the store, the inbox
/// key, the Wi-Fi certificates and their sealed keys, `wifi.json`, the settings).
pub const KNOWN_FILES: [&str; 10] = [
    crate::config::CONFIG_FILE,
    crate::store::DB_FILE,
    "inbox.sqlite-journal",
    crate::store::KEY_FILE,
    crate::store::SEALED_KEY_FILE,
    crate::wifi::CA_CERT_FILE,
    crate::wifi::SERVER_CERT_FILE,
    crate::wifi::INFO_FILE,
    "wifi-ca-key.sealed",
    "wifi-tls-key.sealed",
];

/// A1: the marker left in a data folder the server moved out of: where to and
/// when (`{"to": …, "at": unix seconds}`). The desktop offers "Remove the old copy"
/// for [`OLD_COPY_DAYS`] days after.
pub const MOVED_MARKER: &str = "moved.json";
/// How long "Remove the old copy" is offered.
pub const OLD_COPY_DAYS: i64 = 30;

/// U1.3: the server's data was moved out of `dir` (it holds [`MOVED_MARKER`]): a
/// stale copy, never used as a source again.
pub fn was_moved(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(MOVED_MARKER)).is_ok()
}

/// U1.3: what the old folder's `server.toml` is renamed to once the move passed, so
/// nothing takes it for a server's data folder again.
pub const RETIRED_CONFIG: &str = "server.toml.moved";

/// After a move passed its health check: the marker, then `server.toml` renamed.
fn retire_old(old: &Path, new: &Path, report: &mut Report) {
    if let Err(e) = mark_moved(old, new) {
        report
            .actions
            .push(format!("note: the old folder could not be marked: {e}"));
        return;
    }
    let cfg = old.join(crate::config::CONFIG_FILE);
    if is_plain_file(&cfg)
        && let Err(e) = std::fs::rename(&cfg, old.join(RETIRED_CONFIG))
    {
        report
            .actions
            .push(format!("note: {} could not be renamed: {e}", cfg.display()));
    }
}

/// What [`copy_known`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Copied {
    pub files: Vec<String>,
    /// Entries that were links (or not plain files), left alone.
    pub skipped: Vec<String>,
    /// `format.json` was written by the copy (removed with the copies).
    pub format_written: bool,
}

/// A plain file: not a link or other reparse point.
fn is_plain_file(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return false;
                }
            }
            m.file_type().is_file()
        }
        Err(_) => false,
    }
}

/// A1: copy the known files of `from` into `to`, their contents only; an entry
/// that is a link is never followed (it is skipped and reported).
pub fn copy_known(from: &Path, to: &Path) -> Result<Copied, String> {
    let mut done = Copied::default();
    copy_known_into(from, to, &mut done).map(|()| done)
}

/// [`copy_known`], recording each copy in `done` as it is made (so a failure can
/// remove exactly them).
fn copy_known_into(from: &Path, to: &Path, done: &mut Copied) -> Result<(), String> {
    if !is_real_folder(from) {
        return Err(format!(
            "{} is not a folder (or is a link); nothing was moved",
            from.display()
        ));
    }
    // upgrade-resilience-1 U1.4: never a folder of a newer format, from or to.
    crate::upgrade::check_format(from)?;
    crate::upgrade::check_format(to)?;
    // M3: a sealed file in the destination (from an earlier, unfinished move) whose
    // source has only the plain file would win over it when opened: it goes first.
    for name in KNOWN_FILES {
        let Some(plain) = name.strip_suffix(".sealed") else {
            continue;
        };
        if is_plain_file(&from.join(plain))
            && std::fs::symlink_metadata(from.join(name)).is_err()
            && std::fs::symlink_metadata(to.join(name)).is_ok()
        {
            std::fs::remove_file(to.join(name))
                .map_err(|e| format!("cannot remove {}: {e}", to.join(name).display()))?;
        }
    }
    for name in KNOWN_FILES {
        let src = from.join(name);
        if std::fs::symlink_metadata(&src).is_err() {
            continue;
        }
        if !is_plain_file(&src) {
            done.skipped.push(name.to_owned());
            continue;
        }
        let bytes = Zeroizing::new(
            std::fs::read(&src).map_err(|e| format!("cannot read {}: {e}", src.display()))?,
        );
        // The folder the service names is the one in use: its files win over any
        // left from an earlier, unfinished move.
        crate::store::write_private(&to.join(name), &bytes)?;
        done.files.push(name.to_owned());
    }
    done.format_written = crate::upgrade::read_format(to).is_none();
    crate::upgrade::write_format(to)
}

/// Remove the copies [`copy_known`] made, and the folder when that leaves it empty.
fn remove_copies(to: &Path, copied: &Copied) {
    for f in &copied.files {
        let _ = std::fs::remove_file(to.join(f));
    }
    let _ = std::fs::remove_file(to.join(crate::store::SEALED_KEY_FILE));
    if copied.format_written {
        let _ = std::fs::remove_file(to.join(crate::upgrade::FORMAT_FILE));
    }
    let _ = std::fs::remove_dir(to);
}

/// L5: seal a copied plain inbox key where this system can (Windows). When a
/// sealed key came along too, it is the one used and the plain copy is removed
/// (once the sealed one opens): no plain key is left beside it.
fn seal_key(env: &Env<'_>) -> Result<(), String> {
    let dir = env.data_dir;
    let Some(sealer) = env.sealer else {
        return Ok(());
    };
    let plain = dir.join(crate::store::KEY_FILE);
    if !plain.exists() {
        return Ok(());
    }
    crate::store::load_key_with(dir, crate::KeyHome::File, Some(sealer))?;
    if plain.exists() {
        std::fs::remove_file(&plain)
            .map_err(|e| format!("cannot remove {}: {e}", plain.display()))?;
    }
    Ok(())
}

/// Leave the marker in the folder moved out of (written as a new file, never
/// through a link).
fn mark_moved(old: &Path, new: &Path) -> Result<(), String> {
    let at = crate::Clock::default().now();
    let v = serde_json::json!({"to": new, "at": at});
    write_report(&old.join(MOVED_MARKER), v.to_string().as_bytes())
}

/// A1: when the server's service still uses another data folder (the one in the
/// user's profile from before), move it into the protected data folder.
///
/// upgrade-resilience-1 U1.1 (H1, M1, M3): the service is stopped and its program
/// in the install folder is refreshed with this build's first (the previous one kept
/// as `.prev`): an older program never meets the moved data. Then the known files
/// are copied (a stale sealed key in the destination never wins over the source's
/// plain one), the inbox key is sealed there, and the service is registered with
/// this build's program and the new folder, started and health-checked. On any
/// failure the copies are removed, the program and the service's old command line
/// are put back and the old server is started; only after a passing health check is
/// the old folder marked ([`MOVED_MARKER`]) and its `server.toml` renamed
/// ([`RETIRED_CONFIG`]).
fn move_data(exe: &Path, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let sm = env.sm;
    if env.machine_dir.is_none() || sm.server_state() == State::NotInstalled {
        return Ok(());
    }
    let Some(args) = sm.server_command() else {
        return Ok(());
    };
    let Some(old) = data_of(&args) else {
        return Ok(());
    };
    if old == env.data_dir {
        return Ok(());
    }
    // U1.3: a folder the data was moved out of is never a source again.
    if was_moved(&old) {
        return Err(format!(
            "The capture server's service uses {}, which its data was moved out of before; it was not moved again. Open MoneyKeep: Settings → Phone capture.",
            old.display()
        ));
    }
    let old_program = args
        .first()
        .map(PathBuf::from)
        .or_else(|| sm.server_path())
        .unwrap_or_else(|| env.install_dir.join(program_name()));
    let old_args: Vec<String> = args.iter().skip(1).cloned().collect();
    let keeps = format!("The capture server keeps using {}.", old.display());
    sm.stop_server(std::time::Duration::from_secs(30))?;
    // H1: this build's program first.
    let program = match refresh_program(exe, env.install_dir) {
        Ok(p) => p,
        Err(e) => {
            let _ = sm.start_server();
            return Err(format!(
                "The capture server's program could not be updated, so its data was not moved: {e}. {keeps}"
            ));
        }
    };
    let mut copied = Copied::default();
    let back = |why: String, copied: &Copied, registered: bool| -> Result<(), String> {
        if registered {
            let _ = sm.stop_server(std::time::Duration::from_secs(30));
        }
        remove_copies(env.data_dir, copied);
        program.restore(env.install_dir);
        let _ = sm.install_server(&old_program, &old_args);
        let _ = sm.start_server();
        Err(format!("{why} {keeps}"))
    };
    if let Err(e) = copy_known_into(&old, env.data_dir, &mut copied) {
        return back(
            format!("The data folder could not be moved: {e}."),
            &copied,
            false,
        );
    }
    for s in &copied.skipped {
        report.actions.push(format!(
            "{s} in {} was skipped: it is a link, not a file",
            old.display()
        ));
    }
    // L5: the inbox key is sealed in its new place.
    if let Err(e) = seal_key(env) {
        return back(
            format!("The inbox key could not be moved: {e}."),
            &copied,
            false,
        );
    }
    let allow_lan = args.iter().any(|a| a == "--allow-lan");
    if let Err(e) = sm.install_server(&program.target, &server_args(env.data_dir, allow_lan)) {
        return back(
            format!("The capture server could not be moved to its protected folder: {e}."),
            &copied,
            true,
        );
    }
    if !env.probe.healthy(env.data_dir) {
        return back(
            "The capture server did not answer its health check in its protected folder.".into(),
            &copied,
            true,
        );
    }
    program.keep(env.install_dir);
    retire_old(&old, env.data_dir, report);
    report.actions.push(format!(
        "capture server data moved to {} ({} files); the old copy in {} is kept for now",
        env.data_dir.display(),
        copied.files.len(),
        old.display()
    ));
    Ok(())
}

/// `services move-data` (the installer, as administrator): only the A1 move.
pub fn move_data_only(exe: &Path, env: &Env<'_>) -> Report {
    let mut report = Report {
        data_dir: Some(env.data_dir.to_path_buf()),
        ..Report::default()
    };
    let r = crate::upgrade::check_format(env.data_dir)
        .and_then(|()| prepare_folders(env, true))
        .and_then(|()| move_data(exe, env, &mut report));
    match r {
        Ok(()) => report.ok = true,
        Err(e) => report.error = Some(e),
    }
    report
}

/// A1: make and protect MoneyKeep's machine folder and the data folder (Windows),
/// or the user's data folder elsewhere. With `make` false an existing data folder
/// is only checked (before it is emptied).
fn prepare_folders(env: &Env<'_>, make: bool) -> Result<(), String> {
    let sm = env.sm;
    if !make {
        if std::fs::symlink_metadata(env.data_dir).is_ok() {
            check_folder(sm, env.data_dir)?;
        }
        return Ok(());
    }
    match env.machine_dir {
        Some(machine) => {
            ensure_protected(sm, machine, FolderUse::Machine).map_err(|e| {
                // L6: a folder MoneyKeep's setup didn't make is in the way.
                if e == unsafe_folder(machine) {
                    not_ours(machine)
                } else {
                    e
                }
            })?;
            ensure_protected(sm, env.data_dir, FolderUse::Data)
        }
        None => {
            if let Some(parent) = env.data_dir.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            ensure_protected(sm, env.data_dir, FolderUse::Data)
        }
    }
}

/// A1: the data folder's own steps (the desktop used to run them itself).
fn data_steps(plan: &Plan, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let dir = env.data_dir;
    let home = crate::KeyHome::File;
    if plan.desktop_token_sha256.is_some()
        || plan.configure.is_some()
        || plan.wifi_configure.is_some()
    {
        // U1.4: setup writes the format number.
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        crate::upgrade::write_format(dir)?;
    }
    if let Some(hash) = &plan.desktop_token_sha256 {
        report.fresh = !dir.join(crate::config::CONFIG_FILE).is_file();
        if report.fresh || !plan.desktop_token_only_if_new {
            crate::setup::set_desktop_token_hash(dir, home, hash, crate::Clock::default().now())?;
            report.desktop_token_set = true;
            report.actions.push(if report.fresh {
                "capture server data folder made, with a desktop token".to_owned()
            } else {
                "new desktop token set (the old one no longer works)".to_owned()
            });
        }
    }
    if let Some(c) = &plan.configure {
        crate::setup::configure(dir, c)?;
        report
            .actions
            .push("server.toml Access settings written".into());
    }
    if let Some(aud) = &plan.set_aud {
        let changed = crate::setup::set_aud(dir, aud)?;
        report.aud_changed = Some(changed);
        if changed {
            report
                .actions
                .push(format!("server.toml access_aud set to {aud}"));
            // The server reads server.toml when it starts.
            if env.sm.server_state() != State::NotInstalled {
                env.sm.stop_server(std::time::Duration::from_secs(30))?;
                env.sm.start_server()?;
                report.actions.push("capture server restarted".into());
            }
        }
    }
    if plan.wifi_remove {
        report.actions.extend(crate::setup::wifi_remove(dir, home)?);
    }
    if let Some(w) = &plan.wifi_configure {
        report.wifi = Some(crate::setup::wifi_configure(dir, w, home)?);
    }
    Ok(())
}

/// Update server (revision 4): stop the service and wait for its process, keep the
/// installed program as `.prev` (upgrade-resilience-1 L2), copy the new one in,
/// register the service with the install-folder path, start it and check its
/// health; on failure put the previous program back and start it. `--allow-lan`
/// stays as the service has it unless the plan says (L1).
fn update(plan: &Plan, exe: &Path, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    if let Some(old) = &env.legacy
        && old.sm.server_state() != State::NotInstalled
    {
        return migrate(plan, exe, env, old, report);
    }
    let sm = env.sm;
    let target = env.install_dir.join(program_name());
    let args = server_args(env.data_dir, allow_lan_of(plan, sm));
    if sm.server_state() != State::NotInstalled {
        sm.stop_server(std::time::Duration::from_secs(30))?;
        report.actions.push("capture server stopped".into());
    }
    let moved = sm.server_path().is_some_and(|p| !same_file(&p, &target));
    let program = refresh_program(exe, env.install_dir).map_err(|e| {
        let _ = sm.start_server();
        format!("The new server program could not be copied: {e}.")
    })?;
    let restore = |why: String| -> Result<(), String> {
        let _ = sm.stop_server(std::time::Duration::from_secs(30));
        if program.had_prev {
            program.restore(env.install_dir);
            let _ = sm.install_server(&target, &args);
        }
        Err(format!("{why} The previous server program was restored."))
    };
    if let Err(e) = sm.install_server(&target, &args) {
        return restore(format!("The new server could not be started: {e}."));
    }
    if !env.probe.healthy(env.data_dir) {
        return restore("The new server did not answer its health check.".into());
    }
    program.keep(env.install_dir);
    if moved {
        report
            .actions
            .push(format!("the service now runs {}", target.display()));
    }
    report
        .actions
        .push("capture server updated and running".into());
    Ok(())
}

/// The data folder the old-name service uses (A3: the one old folder its command
/// line names), when it holds a server and is not the data folder.
fn old_data(env: &Env<'_>, old: &Legacy<'_>) -> Option<PathBuf> {
    service_data_dir(old.sm).filter(|d| {
        d != env.data_dir && d.join(crate::config::CONFIG_FILE).is_file() && !was_moved(d)
    })
}

/// branding.md step 6: Update server over an installation from before the rename.
/// The old-name service is stopped, the known files of its data folder copied (A1:
/// contents only, never through a link), the new service installed from the new
/// install folder with the new data folder, started and health-checked; only then
/// are the old service and install folder removed. The old data folder is left in
/// place with [`MOVED_MARKER`] (the desktop offers to remove it). On failure the
/// new service is removed and the old service is started again.
fn migrate(
    plan: &Plan,
    exe: &Path,
    env: &Env<'_>,
    old: &Legacy<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let sm = env.sm;
    let old_installed = old.sm.server_state() != State::NotInstalled;
    let old_data = old_data(env, old);
    let args = server_args(
        env.data_dir,
        plan.allow_lan.unwrap_or_else(|| {
            old.sm
                .server_command()
                .is_some_and(|a| a.iter().any(|w| w == "--allow-lan"))
        }),
    );
    if old_installed {
        old.sm.stop_server(std::time::Duration::from_secs(30))?;
        report
            .actions
            .push("the capture server under its old name stopped".into());
    }
    let mut copied = Copied::default();
    let restore = |why: String, copied: &Copied| -> Result<(), String> {
        let _ = sm.uninstall_server();
        remove_copies(env.data_dir, copied);
        if old_installed {
            let _ = old.sm.start_server();
            return Err(format!(
                "{why} The capture server under its old name was started again."
            ));
        }
        Err(why)
    };
    if sm.server_state() != State::NotInstalled
        && let Err(e) = sm.stop_server(std::time::Duration::from_secs(30))
    {
        return restore(
            format!("The capture server could not be stopped: {e}."),
            &copied,
        );
    }
    if let Some(from) = &old_data {
        if let Err(e) = copy_known_into(from, env.data_dir, &mut copied) {
            return restore(
                format!("The data folder could not be copied: {e}."),
                &copied,
            );
        }
        for s in &copied.skipped {
            report.actions.push(format!(
                "{s} in {} was skipped: it is a link, not a file",
                from.display()
            ));
        }
        if let Err(e) = seal_key(env) {
            return restore(format!("The inbox key could not be moved: {e}."), &copied);
        }
        report
            .actions
            .push(format!("data folder copied to {}", env.data_dir.display()));
    }
    let target = match install_copy(exe, env.install_dir) {
        Ok(t) => t,
        Err(e) => return restore(format!("{e}."), &copied),
    };
    if let Err(e) = sm.install_server(&target, &args) {
        return restore(
            format!("The new server could not be started: {e}."),
            &copied,
        );
    }
    if !env.probe.healthy(env.data_dir) {
        return restore(
            "The new server did not answer its health check.".into(),
            &copied,
        );
    }
    // The new installation works: now the old one goes.
    if old_installed {
        match old.sm.uninstall_server() {
            Ok(()) => report
                .actions
                .push("the service with the old name removed".into()),
            Err(e) => report.actions.push(format!(
                "the service with the old name could not be removed: {e}"
            )),
        }
    }
    if let Some(from) = &old_data {
        retire_old(from, env.data_dir, report);
        report.actions.push(format!(
            "the old data folder {} is kept for now",
            from.display()
        ));
    }
    if old.install_dir != env.install_dir && is_real_folder(old.install_dir) {
        match remove_with_empty_parent(old.install_dir) {
            Ok(()) => report.actions.push(format!(
                "old install folder {} deleted",
                old.install_dir.display()
            )),
            Err(e) => report.actions.push(format!(
                "old install folder {} could not be deleted: {e}",
                old.install_dir.display()
            )),
        }
    }
    report.actions.push(format!(
        "capture server moved to the {SERVER_SERVICE} service and running"
    ));
    Ok(())
}

/// Remove `dir` and then its parent folder when that is left empty.
fn remove_with_empty_parent(dir: &Path) -> std::io::Result<()> {
    std::fs::remove_dir_all(dir)?;
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

/// A2: the refusal for a `cloudflared` not signed by Cloudflare.
pub const NOT_CLOUDFLARE_SIGNED: &str = "The cloudflared program on this computer isn't signed by Cloudflare, so MoneyKeep won't run it.";

/// A2: a signer's subject names Cloudflare's organisation exactly: `O=Cloudflare,
/// Inc.` and `C=US` (the subject Windows reports, e.g. `CN="Cloudflare, Inc.",
/// O="Cloudflare, Inc.", L=San Francisco, S=California, C=US`).
pub fn is_cloudflare_signer(subject: &str) -> bool {
    let parts = dn_parts(subject);
    let get = |k: &str| {
        parts
            .iter()
            .filter(|(a, _)| a.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
    };
    get("O") == [CLOUDFLARE_SIGNER] && get("C") == ["US"]
}

/// The attribute-value pairs of a distinguished name as Windows writes it
/// (comma-separated, values with commas in double quotes).
fn dn_parts(dn: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut items = Vec::new();
    for c in dn.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => items.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    items.push(cur);
    for it in items {
        if let Some((k, v)) = it.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .unwrap_or(v)
                .replace("\"\"", "\"");
            out.push((k.trim().to_owned(), v));
        }
    }
    out
}

/// A2: check `cloudflared`'s signature before running it as administrator.
///
/// macos-release.md C6: on macOS the signer is a Team ID (`TeamIdentifier=…`, from
/// [`macos::Codesign`], which reports one only for a valid signature anchored at
/// Apple): it must be Cloudflare's, and the program must be in one of
/// [`macos::CLOUDFLARED_DIRS`].
pub fn verify_cloudflared(env: &Env<'_>, cf: &Path) -> Result<(), String> {
    if !env.check_cloudflared {
        return Ok(());
    }
    match env.sig.signer(cf) {
        Ok(subject) if is_cloudflare_signer(&subject) => Ok(()),
        Ok(subject) if macos::team_of(&subject).is_some() => macos::check_cloudflared(cf, &subject),
        _ => Err(NOT_CLOUDFLARE_SIGNED.into()),
    }
}

/// L1: `cloudflared --version` is at least 2025.4.0, which reads the token from a
/// file (`--token-file`, Cloudflare's "Tunnel run parameters").
pub fn supports_token_file(version: &str) -> bool {
    let Some(v) = version
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
    else {
        return false;
    };
    let n: Vec<u32> = v
        .split(['.', '-'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let at = |i: usize| n.get(i).copied().unwrap_or(0);
    (at(0), at(1), at(2)) >= (2025, 4, 0)
}

/// L1: the token file's name: the tunnel id and the account id (neither secret), so
/// the service's command line still shows which tunnel it runs (revision 16).
pub fn token_file_name(tunnel: Option<&str>, account: Option<&str>) -> String {
    match (tunnel, account) {
        (Some(t), Some(a)) => format!("{t}.{a}.token"),
        (Some(t), None) => format!("{t}.token"),
        _ => "tunnel.token".to_owned(),
    }
}

/// The folder for the tunnel token (administrators only).
fn token_dir(machine: &Path) -> PathBuf {
    machine.join("cloudflared")
}

/// L4 (upgrade-resilience-1): the file that says MoneyKeep installed the
/// `cloudflared` service (in the token folder); the uninstaller removes that service
/// with the data.
pub const CLOUDFLARED_MARKER: &str = "service-installed-by-moneykeep";

/// Leave [`CLOUDFLARED_MARKER`] (Windows: in `%ProgramData%\MoneyKeep\cloudflared`).
fn mark_cloudflared(env: &Env<'_>, report: &mut Report) {
    let Some(machine) = env.machine_dir else {
        return;
    };
    let dir = token_dir(machine);
    let done = ensure_protected(env.sm, &dir, FolderUse::AdminOnly)
        .and_then(|()| write_report(&dir.join(CLOUDFLARED_MARKER), b"MoneyKeep\n"));
    if let Err(e) = done {
        report.actions.push(format!(
            "note: the uninstaller won't know MoneyKeep installed cloudflared's service: {e}"
        ));
    }
}

/// Remove the token files MoneyKeep wrote.
fn remove_token_files(env: &Env<'_>) {
    let Some(machine) = env.machine_dir else {
        return;
    };
    let dir = token_dir(machine);
    if !is_real_folder(&dir) {
        return;
    }
    if let Ok(list) = std::fs::read_dir(&dir) {
        for e in list.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "token") && is_plain_file(&p) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// L1: keep the tunnel token off `cloudflared`'s command line when it can read it
/// from a file: the token goes in a file only administrators can open, and the
/// service runs `cloudflared tunnel run --token-file FILE`. An older `cloudflared`
/// keeps the token on its command line (the help says administrators can see it).
fn token_off_command_line(
    env: &Env<'_>,
    cf: &Path,
    token: &str,
    report: &mut Report,
) -> Result<(), String> {
    let Some(machine) = env.machine_dir else {
        return Ok(());
    };
    let sm = env.sm;
    if !sm
        .cloudflared_version(cf)
        .as_deref()
        .is_some_and(supports_token_file)
    {
        report.actions.push(
            "note: this cloudflared is older than 2025.4.0, so the tunnel token stays on its service's command line, where administrators of this computer can see it"
                .into(),
        );
        return Ok(());
    }
    let dir = token_dir(machine);
    ensure_protected(sm, &dir, FolderUse::AdminOnly)?;
    remove_token_files(env);
    let (tunnel, account) = tunnel_of(&format!("--token {token}"));
    let file = dir.join(token_file_name(tunnel.as_deref(), account.as_deref()));
    write_report(&file, token.as_bytes())?;
    let args = vec![
        "tunnel".to_owned(),
        "run".to_owned(),
        "--token-file".to_owned(),
        file.display().to_string(),
    ];
    match sm.set_cloudflared_command(cf, &args) {
        Ok(()) => report.actions.push(
            "the tunnel token is kept in a file only administrators can open, not on cloudflared's command line"
                .into(),
        ),
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            report.actions.push(format!(
                "note: the tunnel token stays on cloudflared's command line ({e})"
            ));
        }
    }
    Ok(())
}

/// The `cloudflared` service runs this token: on its command line, or in the token
/// file its command line names (L1).
fn runs_token(command_line: &str, token: &str) -> bool {
    if command_line.contains(token) {
        return true;
    }
    let words = split_command_line(command_line);
    words
        .iter()
        .position(|w| w == "--token-file")
        .and_then(|i| words.get(i + 1))
        .and_then(|f| std::fs::read_to_string(f).ok())
        .is_some_and(|t| t.trim() == token)
}

fn apply_inner(plan: &Plan, exe: &Path, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let (sm, sig, dl, can_download, tmp) = (env.sm, env.sig, env.dl, env.can_download, env.tmp);
    let emptying = plan.clear_data_dir || plan.remove_data_dir;
    prepare_folders(env, !emptying)?;
    // U1.4: a data folder of a newer format is not touched.
    crate::upgrade::check_format(env.data_dir)?;
    if !plan.remove_server && !emptying {
        move_data(exe, env, report)?;
    }
    data_steps(plan, env, report)?;
    if plan.update_server {
        update(plan, exe, env, report)?;
    }
    if plan.remove_cloudflared {
        remove_cloudflared(plan, env, report)?;
    }
    if let Some(name) = &plan.remove_firewall_rule
        && sm.remove_firewall_rule(name)?
    {
        report
            .actions
            .push(format!("firewall rule \"{name}\" removed"));
    }
    if !plan.remove_block_rules.is_empty() {
        remove_block_rules(plan, sm, report)?;
    }
    // macos-release.md C1, C8: an agent whose program was moved or deleted reads as
    // not installed but is still removed.
    if plan.remove_server
        && (sm.server_state() != State::NotInstalled || sm.server_command().is_some())
    {
        sm.uninstall_server()?;
        report.actions.push("capture server service removed".into());
    }
    if plan.clear_data_dir {
        let n = clear_data_dir(env.data_dir)?;
        report.actions.push(format!(
            "capture server data folder emptied ({n} items removed)"
        ));
    }
    if plan.remove_data_dir
        && is_real_folder(env.data_dir)
        && std::fs::remove_dir(env.data_dir).is_ok()
    {
        report
            .actions
            .push("capture server data folder removed".into());
    }
    if plan.reinstall_server {
        reinstall(plan, exe, env, report)?;
    }
    if plan.install_server {
        // The service runs its own copy in the install folder, never the build
        // folder (revision 4), so rebuilding never touches the running program.
        let before = sm.server_command();
        if sm.server_state() != State::NotInstalled {
            sm.stop_server(std::time::Duration::from_secs(30))?;
        }
        let program = refresh_program(exe, env.install_dir)?;
        let args = server_args(env.data_dir, allow_lan_of(plan, sm));
        if let Err(e) = sm.install_server(&program.target, &args) {
            program.restore(env.install_dir);
            return Err(e);
        }
        // L2: a program that replaced another must pass its health check, or the
        // previous one comes back with the service's previous command line.
        if program.had_prev && !env.probe.healthy(env.data_dir) {
            let _ = sm.stop_server(std::time::Duration::from_secs(30));
            program.restore(env.install_dir);
            if let Some(b) = &before {
                let _ = sm.install_server(&program.target, b.get(1..).unwrap_or(&[]));
            }
            return Err(
                "The new server did not answer its health check. The previous server program was restored."
                    .into(),
            );
        }
        program.keep(env.install_dir);
        report
            .actions
            .push("capture server installed as a service that starts with the computer".into());
    } else if plan.restart_server && sm.server_state() != State::NotInstalled {
        sm.stop_server(std::time::Duration::from_secs(30))?;
        sm.start_server()?;
        report.actions.push("capture server restarted".into());
    }
    if let Some(rule) = &plan.firewall_rule
        && !plan.reinstall_server
    {
        add_rule(rule, env, report)?;
    }
    if let Some(token) = &plan.tunnel_token {
        let mut replace = false;
        if let Some((_, command_line)) = sm.cloudflared_service() {
            if runs_token(&command_line, token) {
                report
                    .actions
                    .push("cloudflared already runs this tunnel".into());
                return start_if(plan, sm, report);
            }
            let runs = tunnel_of(&command_line).0;
            match (&plan.replace_cloudflared_tunnel, runs) {
                (Some(want), Some(have)) if want.eq_ignore_ascii_case(&have) => replace = true,
                _ => {
                    return Err(
                        "A cloudflared service for another tunnel is already installed on this computer; it was not replaced. Choose \"Add to my existing tunnel\" in MoneyKeep."
                            .into(),
                    );
                }
            }
        }
        let cf = match sm.cloudflared_path() {
            Some(p) => p,
            None if can_download => {
                download_installer(sm, sig, dl, tmp, env.machine_dir.is_some())?;
                report.actions.push(format!(
                    "cloudflared downloaded from Cloudflare's GitHub releases (signed by {CLOUDFLARE_SIGNER}) and installed"
                ));
                sm.cloudflared_path().ok_or_else(|| {
                    "cloudflared was installed but could not be found afterwards".to_owned()
                })?
            }
            None => {
                return Err(
                    "cloudflared is not installed: install it (https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/) and try again."
                        .into(),
                );
            }
        };
        // A2: never run a cloudflared that is not Cloudflare's.
        verify_cloudflared(env, &cf)?;
        if replace {
            // Revision 16: `cloudflared service uninstall` removes the Windows
            // service (Cloudflare's "Run as a service on Windows" page,
            // <https://developers.cloudflare.com/tunnel/advanced/local-management/as-a-service/windows/>);
            // stop and delete it directly when that fails.
            if let Err(first) = sm.uninstall_cloudflared_service(&cf) {
                sm.delete_cloudflared_service().map_err(|e| {
                    format!("The cloudflared service of the deleted tunnel could not be removed: {first}; {e}")
                })?;
            }
            remove_token_files(env);
            report.cloudflared_replaced = true;
            report
                .actions
                .push("cloudflared service of a deleted tunnel removed".into());
        }
        install_cloudflared(sm, &cf, token, report)?;
        report.cloudflared_service = true;
        mark_cloudflared(env, report);
        report
            .actions
            .push("cloudflared installed as a service for the tunnel".into());
        token_off_command_line(env, &cf, token, report)?;
    }
    start_if(plan, sm, report)
}

/// Remove the `cloudflared` service: with `cloudflared service uninstall` (only a
/// `cloudflared` signed by Cloudflare is run, A2), else stopped and deleted
/// directly (revision 16); with `remove_cloudflared_tunnel` only while it runs that
/// tunnel (Start over, revision 17).
fn remove_cloudflared(plan: &Plan, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let sm = env.sm;
    let Some((_, line)) = sm.cloudflared_service() else {
        return Ok(());
    };
    if let Some(want) = &plan.remove_cloudflared_tunnel
        && tunnel_of(&line).0.as_deref() != Some(want.to_ascii_lowercase().as_str())
    {
        report
            .actions
            .push("cloudflared service kept: it runs another tunnel".into());
        return Ok(());
    }
    let first = match sm.cloudflared_path() {
        Some(cf) => {
            verify_cloudflared(env, &cf).and_then(|()| sm.uninstall_cloudflared_service(&cf))
        }
        None => Err("cloudflared was not found".to_owned()),
    };
    if let Err(first) = first {
        sm.delete_cloudflared_service()
            .map_err(|e| format!("The cloudflared service could not be removed: {first}; {e}"))?;
    }
    remove_token_files(env);
    if let Some(m) = env.machine_dir {
        let _ = std::fs::remove_file(token_dir(m).join(CLOUDFLARED_MARKER));
    }
    report.actions.push("cloudflared service removed".into());
    // Revision 16b: its event-log source goes with it.
    if sm.delete_cloudflared_event_log()? {
        report
            .actions
            .push("cloudflared event-log entry removed".into());
    }
    Ok(())
}

/// The plan and report files of the running helper stay (the desktop deletes them).
const HELPER_FILES: [&str; 2] = ["setup-plan.json", "setup-report.json"];

/// Empty the capture server's data folder (Start over, revision 17). Only a folder
/// that is the capture server's (named `capture-server`, or holding its
/// `server.toml`) is emptied; a missing one counts as empty. Links inside are
/// removed themselves, never followed (`remove_dir_all` does not follow them).
pub fn clear_data_dir(dir: &Path) -> Result<usize, String> {
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(0);
    }
    if !is_real_folder(dir) {
        return Err(unsafe_folder(dir));
    }
    let named = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("capture-server"));
    if !named && !dir.join("server.toml").is_file() {
        return Err(format!(
            "{} does not look like the capture server's data folder; it was not emptied",
            dir.display()
        ));
    }
    let mut n = 0;
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if HELPER_FILES.iter().any(|h| name.to_str() == Some(*h)) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let done = if kind.is_dir() && is_real_folder(&path) {
            std::fs::remove_dir_all(&path)
        } else if kind.is_symlink() && path.is_dir() {
            // A link to a folder: remove the link only.
            std::fs::remove_dir(&path).or_else(|_| std::fs::remove_file(&path))
        } else {
            std::fs::remove_file(&path)
        };
        done.map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        n += 1;
    }
    Ok(n)
}

/// Add Wi-Fi capture's firewall rule (A3: only ever for the capture server's
/// program: the installed one, or where installing puts it).
fn add_rule(rule: &FirewallRule, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let sm = env.sm;
    let mut rule = rule.clone();
    rule.program = Some(
        sm.server_path()
            .unwrap_or_else(|| env.install_dir.join(program_name())),
    );
    if sm.add_firewall_rule(&rule)? {
        let ports: Vec<String> = rule.ports.iter().map(u16::to_string).collect();
        report.actions.push(format!(
            "firewall rule \"{}\" added for ports {} ({} networks only)",
            rule.name,
            ports.join(" and "),
            rule.safe_profiles().join(" and ")
        ));
    } else {
        report.actions.push(
            "no firewall rule was added: this system's firewall is not managed by MoneyKeep (see the help)"
                .into(),
        );
    }
    Ok(())
}

/// The data folder's `server.toml` listens on a non-loopback address, so the
/// server needs `--allow-lan` to start.
fn listens_on_lan(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(crate::config::CONFIG_FILE))
        .ok()
        .and_then(|t| crate::Config::parse(&t).ok())
        .and_then(|c| c.listen.parse::<std::net::SocketAddr>().ok())
        .is_some_and(|a| !a.ip().is_loopback())
}

/// security-review-1 A6: Install the server again, when its service is missing
/// and its data is still here. When the data folder has no `server.toml` and the
/// allowed old folder has one, the old folder's known files are copied in (A1: no
/// links, inbox key sealed). The service is installed with its own account and the
/// data folder, started and health-checked, and the Wi-Fi firewall rule (when the
/// plan has one) recreated; only then is the old folder marked as moved (the
/// desktop offers "Remove the old copy"). On failure the service is removed again,
/// the copies are deleted and the old folder is left exactly as it was.
fn reinstall(plan: &Plan, exe: &Path, env: &Env<'_>, report: &mut Report) -> Result<(), String> {
    let sm = env.sm;
    if sm.server_state() != State::NotInstalled {
        return Err(
            "The capture server is already installed, so it was not installed again. Press Check now."
                .into(),
        );
    }
    let config = crate::config::CONFIG_FILE;
    let has_data = env.data_dir.join(config).is_file();
    let from = plan
        .source_data_dir
        .as_deref()
        .filter(|d| !has_data && *d != env.data_dir && d.join(config).is_file() && !was_moved(d));
    if !has_data && from.is_none() {
        return Err(
            "The capture server's data was not found, so it was not installed again. Set up phone capture again."
                .into(),
        );
    }
    let mut copied = Copied::default();
    let restore = |why: String, copied: &Copied| -> Result<(), String> {
        let _ = sm.uninstall_server();
        if !copied.files.is_empty() {
            remove_copies(env.data_dir, copied);
        }
        Err(match from {
            Some(d) => format!(
                "{why} The capture server was not installed; its data in {} was not changed.",
                d.display()
            ),
            None => format!("{why} The capture server was not installed."),
        })
    };
    if let Some(from) = from {
        if let Err(e) = copy_known_into(from, env.data_dir, &mut copied) {
            return restore(
                format!("The data folder could not be copied: {e}."),
                &copied,
            );
        }
        for s in &copied.skipped {
            report.actions.push(format!(
                "{s} in {} was skipped: it is a link, not a file",
                from.display()
            ));
        }
        // L5: the inbox key is sealed in its new place.
        if let Err(e) = seal_key(env) {
            return restore(format!("The inbox key could not be moved: {e}."), &copied);
        }
        report.actions.push(format!(
            "capture server data copied from {} to {} ({} files)",
            from.display(),
            env.data_dir.display(),
            copied.files.len()
        ));
    }
    let allow_lan = plan.allow_lan == Some(true) || listens_on_lan(env.data_dir);
    // U1.4: reinstall writes the format number.
    if let Err(e) = crate::upgrade::write_format(env.data_dir) {
        return restore(format!("{e}."), &copied);
    }
    let target = match install_copy(exe, env.install_dir) {
        Ok(t) => t,
        Err(e) => return restore(format!("{e}."), &copied),
    };
    if let Err(e) = sm.install_server(&target, &server_args(env.data_dir, allow_lan)) {
        return restore(
            format!("The capture server could not be installed: {e}."),
            &copied,
        );
    }
    if !env.probe.healthy(env.data_dir) {
        return restore(
            "The capture server did not answer its health check.".into(),
            &copied,
        );
    }
    if let Some(rule) = &plan.firewall_rule
        && let Err(e) = add_rule(rule, env, report)
    {
        return restore(
            format!("The firewall rule could not be added: {e}."),
            &copied,
        );
    }
    if let Some(from) = from {
        retire_old(from, env.data_dir, report);
        report.actions.push(format!(
            "the old copy in {} is kept for now",
            from.display()
        ));
    }
    report.actions.push(format!(
        "capture server installed again as the {SERVER_SERVICE} service and running"
    ));
    Ok(())
}

fn start_if(plan: &Plan, sm: &dyn ServiceManager, report: &mut Report) -> Result<(), String> {
    if plan.start_server {
        sm.start_server()?;
        report.actions.push("capture server started".into());
    }
    Ok(())
}

/// Download the official installer into a new folder only administrators can open
/// (A2: under `%ProgramData%\MoneyKeep\tmp`, never the user's temp folder), hold it
/// open so no one can change or replace it, check that Cloudflare signed it, run it,
/// and delete it whatever happens.
fn download_installer(
    sm: &dyn ServiceManager,
    sig: &dyn SignatureChecker,
    dl: &dyn Downloader,
    tmp: &Path,
    protect: bool,
) -> Result<(), String> {
    let bytes = dl.download(CLOUDFLARED_MSI_URL, MAX_INSTALLER)?;
    std::fs::create_dir_all(tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    if protect {
        check_folder(sm, tmp)?;
        sm.protect_folder(tmp, FolderUse::AdminOnly)?;
    }
    let dir = tmp.join(format!("cloudflared-{}", crate::store::new_id()?));
    std::fs::create_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    if protect {
        sm.protect_folder(&dir, FolderUse::AdminOnly)?;
    }
    let file = dir.join("cloudflared-windows-amd64.msi");
    let result = (|| {
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|e| format!("cannot save the installer: {e}"))?;
            f.write_all(&bytes)
                .and_then(|()| f.sync_all())
                .map_err(|e| format!("cannot save the installer: {e}"))?;
        }
        // Held from the signature check until msiexec has finished.
        let held = sm.hold_file(&file)?;
        let checked = (|| {
            let signer = sig.signer(&file).map_err(|e| {
                format!("The downloaded cloudflared installer was refused: its signature is not valid ({e}).")
            })?;
            if !is_cloudflare_signer(&signer) {
                return Err(format!(
                    "The downloaded cloudflared installer was refused: it is signed by {signer}, not {CLOUDFLARE_SIGNER}."
                ));
            }
            sm.install_cloudflared_package(&file)
        })();
        drop(held);
        checked
    })();
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&dir);
    result
}

/// This computer's private IPv4 address used for outgoing traffic (no packet is
/// sent: connecting a UDP socket only picks the route).
pub fn lan_addresses() -> Vec<String> {
    let probe = || -> Option<std::net::IpAddr> {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:9").ok()?;
        s.local_addr().ok().map(|a| a.ip())
    };
    match probe() {
        Some(std::net::IpAddr::V4(ip)) if ip.is_private() => vec![ip.to_string()],
        _ => Vec::new(),
    }
}

/// One of this computer's IPv4 addresses on a home or office network, with its
/// network adapter (Wi-Fi setup's address list, mobile-capture.md revision 10).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAddress {
    pub address: String,
    /// The adapter's name, e.g. `Wi-Fi` or `Ethernet` (`wlan0` on Linux).
    #[serde(default)]
    pub adapter: String,
    /// The adapter's MAC address (for a DHCP reservation).
    #[serde(default)]
    pub mac: String,
    /// Windows network profile: `Public`, `Private` or `DomainAuthenticated`.
    #[serde(default)]
    pub profile: String,
    /// The address came from DHCP (a reservation keeps it fixed).
    #[serde(default)]
    pub dhcp: bool,
}

/// The PowerShell script listing IPv4 addresses with adapter, MAC address and
/// network profile as JSON (Windows; `Get-NetIPAddress`, `Get-NetAdapter`,
/// `Get-NetConnectionProfile`).
pub const ADDRESSES_SCRIPT: &str = "ConvertTo-Json -Compress -InputObject @(Get-NetIPAddress -AddressFamily IPv4 | \
     ForEach-Object { $a = Get-NetAdapter -InterfaceIndex $_.InterfaceIndex -ErrorAction SilentlyContinue; \
     $p = Get-NetConnectionProfile -InterfaceIndex $_.InterfaceIndex -ErrorAction SilentlyContinue; \
     [pscustomobject]@{ address = $_.IPAddress; adapter = $_.InterfaceAlias; mac = [string]$a.MacAddress; \
     profile = [string]$p.NetworkCategory; dhcp = ([string]$_.PrefixOrigin -eq 'Dhcp') } })";

fn usable(ip: &str) -> bool {
    ip.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|a| a.is_private() || a.is_link_local())
}

/// Parse the Windows script's JSON.
pub fn parse_windows_addresses(json: &str) -> Vec<LocalAddress> {
    serde_json::from_str::<Vec<LocalAddress>>(json.trim())
        .unwrap_or_default()
        .into_iter()
        .filter(|a| usable(&a.address))
        .map(|mut a| {
            a.mac = a.mac.replace('-', ":").to_ascii_lowercase();
            a
        })
        .collect()
}

/// Parse `ip -o -4 addr show` (Linux); `mac` reads an adapter's MAC address.
pub fn parse_ip_addr(text: &str, mac: impl Fn(&str) -> String) -> Vec<LocalAddress> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let adapter = f.get(1)?.trim_end_matches(':').to_owned();
            let pos = f.iter().position(|w| *w == "inet")?;
            let address = f.get(pos + 1)?.split('/').next()?.to_owned();
            usable(&address).then(|| LocalAddress {
                mac: mac(&adapter),
                dhcp: f.contains(&"dynamic"),
                address,
                adapter,
                profile: String::new(),
            })
        })
        .collect()
}

/// Parse `ifconfig` (macOS).
pub fn parse_ifconfig(text: &str) -> Vec<LocalAddress> {
    let mut out = Vec::new();
    let mut adapter = String::new();
    let mut mac = String::new();
    let mut found: Vec<String> = Vec::new();
    let flush = |adapter: &str, mac: &str, found: &mut Vec<String>, out: &mut Vec<LocalAddress>| {
        for a in found.drain(..) {
            out.push(LocalAddress {
                address: a,
                adapter: adapter.to_owned(),
                mac: mac.to_owned(),
                profile: String::new(),
                dhcp: false,
            });
        }
    };
    for line in text.lines() {
        if !line.starts_with(['\t', ' ']) && line.contains(':') {
            flush(&adapter, &mac, &mut found, &mut out);
            adapter = line.split(':').next().unwrap_or("").to_owned();
            mac.clear();
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        match (f.first(), f.get(1)) {
            (Some(&"ether"), Some(m)) => mac = (*m).to_owned(),
            (Some(&"inet"), Some(a)) if usable(a) => found.push((*a).to_owned()),
            _ => {}
        }
    }
    flush(&adapter, &mac, &mut found, &mut out);
    out
}

/// This computer's addresses on home or office networks.
pub fn local_addresses() -> Vec<LocalAddress> {
    let found = if cfg!(windows) {
        windows::addresses().unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        run(Path::new("ifconfig"), &[])
            .map(|t| parse_ifconfig(&t))
            .unwrap_or_default()
    } else {
        run(Path::new("ip"), &["-o", "-4", "addr", "show"])
            .map(|t| {
                parse_ip_addr(&t, |a| {
                    std::fs::read_to_string(format!("/sys/class/net/{a}/address"))
                        .map(|m| m.trim().to_owned())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    };
    if found.is_empty() {
        return lan_addresses()
            .into_iter()
            .map(|address| LocalAddress {
                address,
                ..LocalAddress::default()
            })
            .collect();
    }
    found
}

/// The tunnel a `cloudflared` service runs and its account, from the service's
/// command line (a Windows service's command, a systemd unit or a launchd plist):
/// the `--token` value, which is base64 of a JSON object whose `t` is the tunnel id
/// and `a` the account id (the token that "Get a Cloudflare Tunnel token",
/// `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}/token`, returns; the spec's
/// sources: "a tunnel's token identifies its tunnel id"), or else a tunnel id
/// written out (`tunnel run <id>`). Never returns the token or its secret.
pub fn tunnel_of(command_line: &str) -> (Option<String>, Option<String>) {
    use base64::Engine as _;
    let text = command_line
        .replace("<string>", " ")
        .replace("</string>", " ");
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .collect();
    let token = words.iter().enumerate().find_map(|(i, w)| {
        if *w == "--token" {
            words.get(i + 1).copied()
        } else {
            w.strip_prefix("--token=")
        }
    });
    // L1: `--token-file DIR\<tunnel>.<account>.token` (see [`token_file_name`]);
    // the words as the command line quotes them (macOS: the data folder's path has
    // a space, macos-release.md C6).
    let quoted = split_command_line(&text);
    let token_file = quoted.iter().enumerate().find_map(|(i, w)| {
        if w == "--token-file" {
            quoted.get(i + 1).map(String::as_str)
        } else {
            w.strip_prefix("--token-file=")
        }
    });
    if let Some(f) = token_file {
        let file = f.rsplit(['\\', '/']).next().unwrap_or(f);
        let mut parts = file.trim_end_matches(".token").split('.');
        if let Some(t) = parts.next().filter(|t| is_uuid(t)) {
            let account = parts
                .next()
                .filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric()))
                .map(str::to_owned);
            return (Some(t.to_ascii_lowercase()), account);
        }
    }
    if let Some(t) = token {
        let engines = [
            base64::engine::general_purpose::STANDARD,
            base64::engine::general_purpose::STANDARD_NO_PAD,
            base64::engine::general_purpose::URL_SAFE,
            base64::engine::general_purpose::URL_SAFE_NO_PAD,
        ];
        let json = engines
            .iter()
            .find_map(|e| e.decode(t.as_bytes()).ok())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        if let Some(v) = json {
            let field = |k: &str| {
                v.get(k)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .filter(|s| !s.is_empty())
            };
            if let Some(id) = field("t").filter(|t| is_uuid(t)) {
                return (Some(id.to_ascii_lowercase()), field("a"));
            }
        }
    }
    let id = words
        .iter()
        .find(|w| is_uuid(w))
        .map(|w| w.to_ascii_lowercase());
    (id, None)
}

/// `8-4-4-4-12` hexadecimal digits.
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `services status` with these service controls (`old`: the controls of the
/// service from before the rename).
pub fn status(sm: &dyn ServiceManager, old: Option<&dyn ServiceManager>) -> Status {
    let (cloudflared_tunnel, cloudflared_account) = sm
        .cloudflared_service()
        .map(|(_, line)| tunnel_of(&line))
        .unwrap_or_default();
    let server = sm.server_state();
    let refusal = refusal_now(sm, server);
    Status {
        refusal: refusal.as_ref().map(|r| r.1.clone()),
        refusal_kind: refusal.map(|r| r.0),
        cloudflared_tunnel,
        cloudflared_account,
        data_found: server == State::NotInstalled && data_found(&data_dir(), &source_dirs()),
        data_dir: Some(data_dir().display().to_string()),
        old_name: old.is_some_and(|o| o.server_state() != State::NotInstalled),
        program_version: crate::VERSION.to_owned(),
        program_has_app: crate::PhoneApp::embedded().is_some(),
        program_capture_version: crate::CAPTURE_VERSION.to_owned(),
        server_path: sm.server_path().map(|p| p.display().to_string()),
        os: std::env::consts::OS.to_owned(),
        server_program: true,
        server,
        cloudflared_installed: sm.cloudflared_path().is_some(),
        cloudflared_service: sm.cloudflared_service().map(|s| s.0),
        can_download_cloudflared: cfg!(windows),
        lan_addresses: lan_addresses(),
        notes: Vec::new(),
    }
}

/// upgrade-resilience-1 U1.4: why the server isn't running, when it refused to
/// start: its exit code (Windows: the service-specific one), else — where this user
/// can read the data folder — the folder itself.
fn refusal_now(
    sm: &dyn ServiceManager,
    server: State,
) -> Option<(crate::upgrade::Refusal, String)> {
    if server == State::Running {
        return None;
    }
    if server == State::Stopped
        && let Some(r) = sm
            .server_exit_code()
            .and_then(crate::upgrade::Refusal::from_code)
    {
        return Some((r, r.message().to_owned()));
    }
    let dir = service_data_dir(sm).unwrap_or_else(data_dir);
    crate::upgrade::check_dir(&dir).err()
}

/// The program of a service command line: the quoted first part, or up to the
/// first space.
pub fn program_of(command_line: &str) -> Option<PathBuf> {
    let c = command_line.trim();
    let p = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => c.split(' ').next()?,
    };
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// Run a program and wait; `Err` with its output when it fails.
pub fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "{} failed: {}",
            program.file_name().map_or_else(
                || program.display().to_string(),
                |n| n.to_string_lossy().into_owned()
            ),
            err.trim().chars().take(400).collect::<String>()
        ))
    }
}

/// A program in one of these folders only (never `PATH`).
pub fn find_in(name: &str, folders: &[PathBuf]) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    folders.iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

/// A program on `PATH` or in one of `extra` folders.
pub fn find_program(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(extra.iter().cloned())
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// The health check over plain HTTP to the address in `server.toml`, retried for
/// up to 20 seconds while the server starts.
#[derive(Debug, Clone, Copy, Default)]
pub struct HttpProbe;

impl Probe for HttpProbe {
    fn healthy(&self, data_dir: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(data_dir.join(crate::config::CONFIG_FILE)) else {
            return false;
        };
        let Ok(config) = crate::Config::parse(&text) else {
            return false;
        };
        let url = format!("http://{}{}", config.listen, mk_capture_api::HEALTH_PATH);
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(3)))
            .build()
            .new_agent();
        for _ in 0..40 {
            if agent
                .get(&url)
                .call()
                .is_ok_and(|r| r.status().as_u16() == 200)
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        false
    }
}

/// HTTPS downloads restricted to [`DOWNLOAD_HOSTS`], following at most five
/// redirects, with the operating system's trusted roots.
#[derive(Debug, Clone, Copy, Default)]
pub struct HttpsDownloader;

impl Downloader for HttpsDownloader {
    fn download(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        use std::io::Read;
        use std::sync::Arc;
        let roots: Vec<ureq::tls::Certificate<'static>> = rustls_native_certs::load_native_certs()
            .certs
            .into_iter()
            .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
            .collect();
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::Specific(Arc::new(roots)))
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(600)))
            .build()
            .new_agent();
        let mut next = url.to_owned();
        for _ in 0..=5 {
            let host = next
                .strip_prefix("https://")
                .and_then(|r| r.split(['/', '?', '#']).next())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !DOWNLOAD_HOSTS.contains(&host.as_str()) {
                return Err(format!(
                    "the download went to {host:?}, which is not allowed"
                ));
            }
            let mut resp = agent.get(&next).call().map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                next = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                    .ok_or("a redirect without a target")?;
                continue;
            }
            if status != 200 {
                return Err(format!("the download answered HTTP {status}"));
            }
            let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX);
            let mut body = Vec::new();
            resp.body_mut()
                .with_config()
                .limit(limit)
                .reader()
                .take(limit)
                .read_to_end(&mut body)
                .map_err(|e| e.to_string())?;
            return Ok(body);
        }
        Err("too many redirects".into())
    }
}

// ----- the operating systems --------------------------------------------------------------

/// The real service controls of this operating system (every branch compiles on
/// every system).
pub fn system() -> Box<dyn ServiceManager> {
    if cfg!(windows) {
        Box::new(windows::WindowsServices::CURRENT)
    } else if cfg!(target_os = "linux") {
        Box::new(linux::SystemdUser::CURRENT)
    } else if cfg!(target_os = "macos") {
        Box::new(macos::Launchd::CURRENT)
    } else {
        Box::new(Unsupported)
    }
}

/// The controls of the server's service under its name from before the rename
/// (branding.md step 6).
pub fn legacy_system() -> Box<dyn ServiceManager> {
    if cfg!(windows) {
        Box::new(windows::WindowsServices::LEGACY)
    } else if cfg!(target_os = "linux") {
        Box::new(linux::SystemdUser::LEGACY)
    } else if cfg!(target_os = "macos") {
        Box::new(macos::Launchd::LEGACY)
    } else {
        Box::new(Unsupported)
    }
}

/// The real signature check: Authenticode on Windows, `codesign` on macOS
/// (macos-release.md C6); elsewhere every file is refused.
pub fn signature_checker() -> Box<dyn SignatureChecker> {
    if cfg!(windows) {
        Box::new(windows::Authenticode)
    } else if cfg!(target_os = "macos") {
        Box::new(macos::Codesign)
    } else {
        Box::new(NoSignatures)
    }
}

/// Refuses every file (downloads happen on Windows only).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSignatures;

impl SignatureChecker for NoSignatures {
    fn signer(&self, _: &Path) -> Result<String, String> {
        Err("code signatures are checked on Windows only".into())
    }
}

/// Systems without a supported service manager. Like the other systems' managers it
/// compiles everywhere, so every one is type-checked on every build.
#[derive(Debug, Clone, Copy)]
pub struct Unsupported;

impl ServiceManager for Unsupported {
    fn server_state(&self) -> State {
        State::NotInstalled
    }
    fn install_server(&self, _: &Path, _: &[String]) -> Result<(), String> {
        Err("services are not supported on this system; run moneykeep-server run yourself".into())
    }
    fn uninstall_server(&self) -> Result<(), String> {
        Ok(())
    }
    fn start_server(&self) -> Result<(), String> {
        Err("services are not supported on this system".into())
    }
    fn stop_server(&self, _: std::time::Duration) -> Result<(), String> {
        Ok(())
    }
    fn server_path(&self) -> Option<PathBuf> {
        None
    }
    fn cloudflared_path(&self) -> Option<PathBuf> {
        None
    }
    fn cloudflared_service(&self) -> Option<(State, String)> {
        None
    }
    fn install_cloudflared_package(&self, _: &Path) -> Result<(), String> {
        Err("not supported".into())
    }
    fn install_cloudflared_service(&self, _: &Path, _: &str) -> Result<(), String> {
        Err("not supported".into())
    }
    fn uninstall_cloudflared_service(&self, _: &Path) -> Result<(), String> {
        Err("not supported".into())
    }
}

// Compiled on every system (only `system()` picks by OS), so a missing trait method
// or type error fails every build, not just the matching system's.
pub mod linux {
    //! A systemd user unit (starts at login; at boot once lingering is enabled) and
    //! the `cloudflared` system service installed through `pkexec`.

    use super::*;

    fn unit_path(unit: &str) -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("systemd/user").join(unit))
    }

    fn systemctl(args: &[&str]) -> Result<String, String> {
        run(Path::new("systemctl"), args)
    }

    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// The server's user unit (by name: the current one, or the one from before the
    /// rename).
    #[derive(Debug, Clone, Copy)]
    pub struct SystemdUser {
        pub unit: &'static str,
    }

    impl SystemdUser {
        pub const CURRENT: Self = Self { unit: SERVER_UNIT };
        pub const LEGACY: Self = Self {
            unit: legacy::CAPTURE_UNIT,
        };
    }

    impl ServiceManager for SystemdUser {
        fn server_state(&self) -> State {
            let unit = self.unit;
            match systemctl(&["--user", "is-active", unit]) {
                Ok(s) if s.trim() == "active" => State::Running,
                _ if unit_path(unit).is_some_and(|p| p.is_file()) => State::Stopped,
                _ => State::NotInstalled,
            }
        }
        fn install_server(&self, exe: &Path, args: &[String]) -> Result<(), String> {
            let unit_name = self.unit;
            let path = unit_path(unit_name).ok_or("HOME is not set")?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let cmd: Vec<String> = std::iter::once(exe.display().to_string())
                .chain(args.iter().cloned())
                .map(|a| quote(&a))
                .collect();
            let unit = format!(
                "[Unit]\nDescription={SERVER_DISPLAY}\nAfter=network-online.target\n\n\
                 [Service]\nExecStart={}\nRestart=on-failure\nRestartSec=10\n\
                 RestartPreventExitStatus={}\n\n[Install]\nWantedBy=default.target\n",
                cmd.join(" "),
                // upgrade-resilience-1 U1.4: a refusal is not tried again.
                crate::upgrade::Refusal::ALL
                    .map(|r| r.code().to_string())
                    .join(" ")
            );
            std::fs::write(&path, unit).map_err(|e| e.to_string())?;
            systemctl(&["--user", "daemon-reload"])?;
            systemctl(&["--user", "enable", unit_name])?;
            systemctl(&["--user", "restart", unit_name])?;
            // Start at boot rather than at login (allowed for the user by default on
            // most systems; harmless when refused).
            let _ = run(Path::new("loginctl"), &["enable-linger"]);
            Ok(())
        }
        fn uninstall_server(&self) -> Result<(), String> {
            let _ = systemctl(&["--user", "disable", "--now", self.unit]);
            if let Some(p) = unit_path(self.unit) {
                let _ = std::fs::remove_file(p);
            }
            systemctl(&["--user", "daemon-reload"]).map(|_| ())
        }
        fn start_server(&self) -> Result<(), String> {
            systemctl(&["--user", "start", self.unit]).map(|_| ())
        }
        fn stop_server(&self, _: std::time::Duration) -> Result<(), String> {
            // `systemctl stop` waits for the process (and ends it after systemd's
            // own stop timeout).
            if self.server_state() == State::Running {
                systemctl(&["--user", "stop", self.unit])?;
            }
            Ok(())
        }
        fn server_path(&self) -> Option<PathBuf> {
            let unit = std::fs::read_to_string(unit_path(self.unit)?).ok()?;
            let line = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
            program_of(line)
        }
        fn cloudflared_path(&self) -> Option<PathBuf> {
            // security-review-1 A2: only where Cloudflare's packages install it
            // (folders only root can change), never PATH.
            find_in(
                "cloudflared",
                &[PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")],
            )
        }
        fn server_exit_code(&self) -> Option<u32> {
            systemctl(&[
                "--user",
                "show",
                self.unit,
                "-p",
                "ExecMainStatus",
                "--value",
            ])
            .ok()?
            .trim()
            .parse()
            .ok()
        }
        fn server_command(&self) -> Option<Vec<String>> {
            let unit = std::fs::read_to_string(unit_path(self.unit)?).ok()?;
            let line = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
            Some(split_command_line(line))
        }
        fn cloudflared_service(&self) -> Option<(State, String)> {
            let unit = std::fs::read_to_string("/etc/systemd/system/cloudflared.service").ok()?;
            let state = match systemctl(&["is-active", "cloudflared.service"]) {
                Ok(s) if s.trim() == "active" => State::Running,
                _ => State::Stopped,
            };
            Some((state, unit))
        }
        fn install_cloudflared_package(&self, _: &Path) -> Result<(), String> {
            Err("cloudflared is not downloaded on Linux; install it from Cloudflare's package repository".into())
        }
        fn install_cloudflared_service(&self, exe: &Path, token: &str) -> Result<(), String> {
            let exe = exe.display().to_string();
            run(Path::new("pkexec"), &[&exe, "service", "install", token]).map(|_| ())
        }
        fn uninstall_cloudflared_service(&self, exe: &Path) -> Result<(), String> {
            let exe = exe.display().to_string();
            run(Path::new("pkexec"), &[&exe, "service", "uninstall"]).map(|_| ())
        }
    }
}

pub mod macos {
    //! macos-release.md Part C: per-user launchd agents, no administrator rights.
    //!
    //! - The capture server: `~/Library/LaunchAgents/<label>.plist` running the
    //!   program inside the app bundle (C1), `RunAtLoad`, `KeepAlive`,
    //!   `ThrottleInterval` 10, its output to `server.log` in the data folder.
    //!   Install, Start, Stop, Update and Remove use `launchctl bootstrap
    //!   gui/<uid>`, `kickstart -k` and `bootout` (C3). A user agent runs while
    //!   that user is logged in (C2).
    //! - `cloudflared` (C6): only from [`CLOUDFLARED_DIRS`], only when signed by
    //!   Cloudflare's Team ID ([`Codesign`]), as the agent [`CLOUDFLARED_LABEL`]
    //!   running `cloudflared tunnel run --token-file FILE`, the token file in the
    //!   data folder (owner-only). MoneyKeep never runs `cloudflared service
    //!   install` as root here.

    use super::*;

    /// The `cloudflared` agent's label (C6).
    pub const CLOUDFLARED_LABEL: &str = "com.moneykeep.cloudflared";
    /// The only folders `cloudflared` is run from (C6): where Cloudflare's package
    /// and Homebrew install it. Never `PATH`.
    pub const CLOUDFLARED_DIRS: [&str; 2] = ["/usr/local/bin", "/opt/homebrew/bin"];
    /// Cloudflare's Apple Team ID (C6), as recorded in macos-release.md from
    /// Cloudflare's own signed release. If it were ever `None`, every `cloudflared`
    /// would be refused on macOS with [`TEAM_UNKNOWN`] (fail closed).
    pub const CLOUDFLARE_TEAM_ID: Option<&str> = Some("68WVV388M8");
    /// C6: the program is not in one of [`CLOUDFLARED_DIRS`].
    pub const NOT_CLOUDFLARED_PLACE: &str = "MoneyKeep runs cloudflared only from /usr/local/bin or /opt/homebrew/bin, where Cloudflare's package installs it, so it won't run this one.";
    /// C6: Cloudflare's Team ID is not known to this build.
    pub const TEAM_UNKNOWN: &str = "This MoneyKeep can't check who signed cloudflared on a Mac yet, so it won't run it. Use phone capture on Wi-Fi instead.";
    /// C6: `cloudflared` reads its token from a file from this version on.
    pub const CLOUDFLARED_TOO_OLD: &str = "This cloudflared is older than 2025.4.0 and can't read its token from a file. Install Cloudflare's current cloudflared package, then press Check again.";
    /// C1: restart after a crash after this many seconds.
    pub const THROTTLE_SECS: u32 = 10;
    /// The application firewall's command-line tool (C5).
    pub const FIREWALL_TOOL: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";
    /// C1: the agents' log in the data folder.
    pub const SERVER_LOG: &str = "server.log";
    /// C6: `cloudflared`'s log in the data folder.
    pub const CLOUDFLARED_LOG: &str = "cloudflared.log";

    fn plist_path(label: &str) -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist"))
        })
    }

    /// Text as XML character data.
    pub fn xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    fn unxml(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    /// C1: an agent's property list: `label` running `args` (program first), at
    /// load and kept alive (restarted after [`THROTTLE_SECS`]), its standard output
    /// and error appended to `log`.
    pub fn agent_plist(label: &str, args: &[String], log: &Path) -> String {
        let items: String = args
            .iter()
            .map(|a| format!("    <string>{}</string>\n", xml(a)))
            .collect();
        let log = xml(&log.display().to_string());
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \x20 <key>Label</key>\n  <string>{}</string>\n\
             \x20 <key>ProgramArguments</key>\n  <array>\n{items}  </array>\n\
             \x20 <key>RunAtLoad</key>\n  <true/>\n\
             \x20 <key>KeepAlive</key>\n  <true/>\n\
             \x20 <key>ThrottleInterval</key>\n  <integer>{THROTTLE_SECS}</integer>\n\
             \x20 <key>ProcessType</key>\n  <string>Background</string>\n\
             \x20 <key>StandardOutPath</key>\n  <string>{log}</string>\n\
             \x20 <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
             </dict>\n</plist>\n",
            xml(label)
        )
    }

    /// The `ProgramArguments` of a property list written by [`agent_plist`].
    pub fn plist_arguments(text: &str) -> Option<Vec<String>> {
        let after = text.split("<key>ProgramArguments</key>").nth(1)?;
        let array = after.split("</array>").next()?;
        Some(
            array
                .split("<string>")
                .skip(1)
                .filter_map(|x| x.split("</string>").next())
                .map(unxml)
                .collect(),
        )
    }

    /// Words as one command line, quoted where they hold spaces (read back with
    /// [`split_command_line`]).
    pub fn command_line(args: &[String]) -> String {
        args.iter()
            .map(|a| {
                if a.is_empty() || a.contains(char::is_whitespace) {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `launchctl print`: the service is running.
    pub fn print_running(text: &str) -> bool {
        text.lines().any(|l| l.trim() == "state = running")
    }

    /// `launchctl print`: the last exit code (U1.4), when the program has exited.
    pub fn print_last_exit(text: &str) -> Option<u32> {
        let v = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("last exit code = "))?;
        let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// `socketfilterfw --getglobalstate` (C5): on (`State = 1` or `2`, "enabled")
    /// or off.
    pub fn parse_firewall_state(text: &str) -> Option<bool> {
        let t = text.to_ascii_lowercase();
        if let Some(n) = t
            .split("state = ")
            .nth(1)
            .and_then(|r| r.chars().next())
            .and_then(|c| c.to_digit(10))
        {
            return Some(n > 0);
        }
        if t.contains("disabled") {
            Some(false)
        } else if t.contains("enabled") {
            Some(true)
        } else {
            None
        }
    }

    fn launchctl(args: &[&str]) -> Result<String, String> {
        run(Path::new("/bin/launchctl"), args)
    }

    /// `gui/<uid>`: this user's login session.
    fn domain() -> Result<String, String> {
        let uid = run(Path::new("/usr/bin/id"), &["-u"])?;
        Ok(format!("gui/{}", uid.trim()))
    }

    fn target(label: &str) -> Result<String, String> {
        Ok(format!("{}/{label}", domain()?))
    }

    fn print(label: &str) -> Option<String> {
        launchctl(&["print", &target(label).ok()?]).ok()
    }

    /// Unload the agent (it stops; `KeepAlive` no longer applies), waiting up to
    /// `wait` for it to be gone.
    fn bootout(label: &str, wait: std::time::Duration) {
        let Ok(t) = target(label) else {
            return;
        };
        let _ = launchctl(&["bootout", &t]);
        let started = std::time::Instant::now();
        while print(label).is_some() && started.elapsed() < wait {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// Load the agent from its property list (it starts: `RunAtLoad`). Retried
    /// briefly: launchd may still be letting go of the previous one.
    fn bootstrap(plist: &Path) -> Result<(), String> {
        let d = domain()?;
        let p = plist.display().to_string();
        let mut last = String::new();
        for _ in 0..10 {
            match launchctl(&["bootstrap", &d, &p]) {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Err(last)
    }

    /// Write an agent's property list (atomically, owner-only) and load it in
    /// place of the one loaded now.
    fn install_agent(label: &str, args: &[String], log: &Path) -> Result<(), String> {
        let path = plist_path(label).ok_or("HOME is not set")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        bootout(label, std::time::Duration::from_secs(30));
        crate::store::write_private(&path, agent_plist(label, args, log).as_bytes())?;
        bootstrap(&path)
    }

    /// Unload an agent and delete its property list (C8).
    fn remove_agent(label: &str) {
        bootout(label, std::time::Duration::from_secs(30));
        if let Some(p) = plist_path(label) {
            let _ = std::fs::remove_file(p);
        }
    }

    fn agent_args(label: &str) -> Option<Vec<String>> {
        plist_arguments(&std::fs::read_to_string(plist_path(label)?).ok()?)
    }

    fn state_of(label: &str) -> State {
        if !plist_path(label).is_some_and(|p| p.is_file()) {
            return State::NotInstalled;
        }
        match print(label) {
            Some(out) if print_running(&out) => State::Running,
            _ => State::Stopped,
        }
    }

    /// The token files (`*.token`) in `dir`, plain files only.
    fn remove_token_files_in(dir: &Path) {
        if let Ok(list) = std::fs::read_dir(dir) {
            for e in list.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "token")
                    && std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file())
                {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }

    /// The Team ID of a signer as [`Codesign`] reports it (`TeamIdentifier=…`).
    pub fn team_of(subject: &str) -> Option<&str> {
        subject
            .strip_prefix("TeamIdentifier=")
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric()))
    }

    /// C6: `cloudflared` at `cf`, signed as `subject`, may be run: it is in one of
    /// [`CLOUDFLARED_DIRS`] and its Team ID is Cloudflare's.
    pub fn check_cloudflared(cf: &Path, subject: &str) -> Result<(), String> {
        let place_ok = cf.file_name().is_some_and(|n| n == "cloudflared")
            && cf
                .parent()
                .is_some_and(|d| CLOUDFLARED_DIRS.iter().any(|x| d == Path::new(x)));
        if !place_ok {
            return Err(NOT_CLOUDFLARED_PLACE.into());
        }
        let Some(team) = CLOUDFLARE_TEAM_ID else {
            return Err(TEAM_UNKNOWN.into());
        };
        match team_of(subject) {
            Some(t) if t == team => Ok(()),
            _ => Err(NOT_CLOUDFLARE_SIGNED.into()),
        }
    }

    /// `codesign -dv` output: the `TeamIdentifier=` line's value (not "not set").
    pub fn parse_team_id(text: &str) -> Option<String> {
        text.lines()
            .find_map(|l| l.trim().strip_prefix("TeamIdentifier="))
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_owned)
    }

    /// C6: a file's signature with Apple's `codesign`. The signature must be
    /// valid (`--verify --strict`) and anchored at Apple (a Developer ID, not a
    /// self-made certificate, which could name any team); the signer is then
    /// reported as `TeamIdentifier=<id>`.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Codesign;

    impl SignatureChecker for Codesign {
        fn signer(&self, file: &Path) -> Result<String, String> {
            let f = file.display().to_string();
            let codesign = Path::new("/usr/bin/codesign");
            run(
                codesign,
                &["--verify", "--strict", "-R=anchor apple generic", &f],
            )?;
            // `-d` writes what it shows to standard error.
            let out = std::process::Command::new(codesign)
                .args(["-d", "--verbose=2", &f])
                .output()
                .map_err(|e| format!("cannot run codesign: {e}"))?;
            let text = String::from_utf8_lossy(&out.stderr);
            parse_team_id(&text)
                .map(|t| format!("TeamIdentifier={t}"))
                .ok_or_else(|| "cloudflared has no Team ID in its signature".to_owned())
        }
    }

    /// The server's launchd agent (by label: the current one, or the one from
    /// before the rename).
    #[derive(Debug, Clone, Copy)]
    pub struct Launchd {
        pub label: &'static str,
    }

    impl Launchd {
        pub const CURRENT: Self = Self {
            label: SERVER_LABEL,
        };
        pub const LEGACY: Self = Self {
            label: legacy::CAPTURE_LABEL,
        };
    }

    impl ServiceManager for Launchd {
        fn server_state(&self) -> State {
            // C1: the agent's program was moved or deleted (the app was moved or
            // dragged to the Trash): Status offers "Install the server again".
            if self.server_path().is_some_and(|p| !p.is_file()) {
                return State::NotInstalled;
            }
            state_of(self.label)
        }
        fn install_server(&self, exe: &Path, args: &[String]) -> Result<(), String> {
            let words: Vec<String> = std::iter::once(exe.display().to_string())
                .chain(args.iter().cloned())
                .collect();
            let log = data_of(args)
                .unwrap_or_else(super::data_dir)
                .join(SERVER_LOG);
            install_agent(self.label, &words, &log)
        }
        fn uninstall_server(&self) -> Result<(), String> {
            remove_agent(self.label);
            Ok(())
        }
        fn start_server(&self) -> Result<(), String> {
            // C3: loaded: `kickstart -k` (restart); not loaded (after Stop): load it.
            if print(self.label).is_some() {
                launchctl(&["kickstart", "-k", &target(self.label)?]).map(|_| ())
            } else {
                bootstrap(&plist_path(self.label).ok_or("HOME is not set")?)
            }
        }
        fn stop_server(&self, wait: std::time::Duration) -> Result<(), String> {
            // C3: `bootout` stops it for good (`KeepAlive` would restart a kill)
            // and waits for it; the property list stays for Start.
            if print(self.label).is_some() {
                bootout(self.label, wait);
            }
            Ok(())
        }
        fn server_path(&self) -> Option<PathBuf> {
            agent_args(self.label)?.first().map(PathBuf::from)
        }
        fn server_command(&self) -> Option<Vec<String>> {
            agent_args(self.label)
        }
        fn server_exit_code(&self) -> Option<u32> {
            print_last_exit(&print(self.label)?)
        }
        fn app_firewall(&self) -> Option<bool> {
            run(Path::new(FIREWALL_TOOL), &["--getglobalstate"])
                .ok()
                .and_then(|t| parse_firewall_state(&t))
        }
        fn cloudflared_path(&self) -> Option<PathBuf> {
            // security-review-1 A2, C6: only these folders, never PATH.
            find_in("cloudflared", &CLOUDFLARED_DIRS.map(PathBuf::from))
        }
        fn cloudflared_version(&self, exe: &Path) -> Option<String> {
            run(exe, &["--version"]).ok()
        }
        fn cloudflared_service(&self) -> Option<(State, String)> {
            let args = agent_args(CLOUDFLARED_LABEL)?;
            Some((state_of(CLOUDFLARED_LABEL), command_line(&args)))
        }
        fn install_cloudflared_package(&self, _: &Path) -> Result<(), String> {
            Err("cloudflared is not downloaded on macOS: install Cloudflare's own cloudflared package, then press Check again.".into())
        }
        fn install_cloudflared_service(&self, exe: &Path, token: &str) -> Result<(), String> {
            // C6: the token in an owner-only file in the data folder, never on the
            // command line; that needs cloudflared 2025.4.0 or later.
            if !self
                .cloudflared_version(exe)
                .as_deref()
                .is_some_and(supports_token_file)
            {
                return Err(CLOUDFLARED_TOO_OLD.into());
            }
            let dir = super::data_dir();
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            remove_token_files_in(&dir);
            let (tunnel, account) = tunnel_of(&format!("--token {token}"));
            let file = dir.join(token_file_name(tunnel.as_deref(), account.as_deref()));
            crate::store::write_private(&file, token.as_bytes())?;
            let args = vec![
                exe.display().to_string(),
                "tunnel".to_owned(),
                "run".to_owned(),
                "--token-file".to_owned(),
                file.display().to_string(),
            ];
            install_agent(CLOUDFLARED_LABEL, &args, &dir.join(CLOUDFLARED_LOG)).inspect_err(|_| {
                let _ = std::fs::remove_file(&file);
            })
        }
        fn uninstall_cloudflared_service(&self, _: &Path) -> Result<(), String> {
            self.delete_cloudflared_service()
        }
        fn delete_cloudflared_service(&self) -> Result<(), String> {
            remove_agent(CLOUDFLARED_LABEL);
            remove_token_files_in(&super::data_dir());
            Ok(())
        }
    }
}

pub mod windows {
    //! Windows: the server as a Windows service, `cloudflared` through its own
    //! `service install`, and the installer's Authenticode signature checked by
    //! Windows through PowerShell's `Get-AuthenticodeSignature` (no unsafe code: the
    //! workspace forbids it).
    //!
    //! The service logic here compiles on every operating system, so a missing trait
    //! method or a type error fails the Linux CI too. Only the calls into Windows sit
    //! in the small [`Os`] shim: the real one (`cfg(windows)`, the `windows-service`
    //! crate's Service Control Manager API) and a stand-in elsewhere that answers
    //! "Windows only". The real shim cannot be compiled outside Windows.

    use std::time::Duration;

    use super::*;

    const CLOUDFLARED_SERVICE: &str = "Cloudflared";
    const DESCRIPTION: &str =
        "Keeps receipts and expenses sent from your phone until MoneyKeep collects them.";

    /// A service as the Service Control Manager reports it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Queried {
        /// `Stopped`, or `Running` for every other state (starting, stopping, …).
        pub state: State,
        /// The registered command line (program and arguments).
        pub command: String,
        pub pid: Option<u32>,
        /// upgrade-resilience-1 U1.4: the service-specific exit code it stopped
        /// with (`ERROR_SERVICE_SPECIFIC_ERROR`), if any.
        pub exit_code: Option<u32>,
    }

    /// The calls into Windows. Every signature is checked on every system through
    /// the stand-in implementation.
    pub trait WindowsApi {
        fn query(name: &str) -> Option<Queried>;
        /// Create the service, or change an existing one's configuration, then start
        /// it.
        fn install(cfg: &InstallConfig) -> Result<(), String>;
        /// Ask the service to stop (no wait).
        fn stop(name: &str) -> Result<(), String>;
        fn start(name: &str) -> Result<(), String>;
        fn delete(name: &str) -> Result<(), String>;
        /// Run a program without a console window; its output.
        fn run_hidden(program: &Path, args: &[&str]) -> Result<std::process::Output, String>;
        /// The same, with `input` on its standard input (secrets never go on a
        /// command line).
        fn run_hidden_input(
            program: &Path,
            args: &[&str],
            input: &[u8],
        ) -> Result<std::process::Output, String>;
        /// `service-run`: hand control to the Service Control Manager.
        fn run_service(data: PathBuf, allow_lan: bool) -> Result<(), String>;
        /// upgrade-resilience-1 U1.6: DPAPI for this computer (`protect`) or opening
        /// with it, through the Windows API ([`mk_dpapi`]), never a process.
        fn dpapi(protect: bool, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String>;
    }

    #[cfg(windows)]
    pub use real::Real as Os;
    #[cfg(not(windows))]
    pub use stand_in::StandIn as Os;

    /// security-review-1 A1: the server's service runs as its own virtual account,
    /// not LocalSystem.
    pub const SERVICE_ACCOUNT: &str = "NT SERVICE\\MoneyKeepCapture";

    /// What the server's service is created or changed with (A1).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct InstallConfig {
        pub name: String,
        pub display: String,
        pub description: String,
        pub exe: PathBuf,
        pub args: Vec<String>,
        /// The account it runs as ([`SERVICE_ACCOUNT`]).
        pub account: String,
        /// Its service SID type is "unrestricted" (`SERVICE_SID_TYPE_UNRESTRICTED`),
        /// so `NT SERVICE\MoneyKeepCapture` can be given rights on its folder.
        pub sid_unrestricted: bool,
        /// upgrade-resilience-1 U1.5: restart after these many seconds on the first,
        /// second and later failures (`SERVICE_CONFIG_FAILURE_ACTIONS`) …
        pub restart_after_secs: Vec<u64>,
        /// … counting failures again after this many seconds without one …
        pub failure_reset_secs: u64,
        /// … also when it stops with an error (a non-zero exit code;
        /// `SERVICE_CONFIG_FAILURE_ACTIONS_FLAG`).
        pub failure_actions_on_exit_code: bool,
    }

    /// The server's service configuration for this program and command line.
    pub fn install_config(exe: &Path, args: &[String]) -> InstallConfig {
        InstallConfig {
            name: SERVER_SERVICE.to_owned(),
            display: SERVER_DISPLAY.to_owned(),
            description: DESCRIPTION.to_owned(),
            exe: exe.to_path_buf(),
            args: args.to_vec(),
            account: SERVICE_ACCOUNT.to_owned(),
            sid_unrestricted: true,
            restart_after_secs: vec![10, 30, 60],
            failure_reset_secs: 86_400,
            failure_actions_on_exit_code: true,
        }
    }

    /// A service's own security identifier (Microsoft: a service SID is
    /// `S-1-5-80-` and the SHA-1 of the service name in upper case, UTF-16LE, as
    /// five little-endian 32-bit numbers). The same as `sc.exe showsid NAME`.
    pub fn service_sid(name: &str) -> String {
        use sha1::Digest as _;
        let wide: Vec<u8> = name
            .to_uppercase()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let h = sha1::Sha1::digest(&wide);
        let parts: Vec<String> = h
            .chunks_exact(4)
            .map(|c| {
                let b: [u8; 4] = [
                    c.first().copied().unwrap_or(0),
                    c.get(1).copied().unwrap_or(0),
                    c.get(2).copied().unwrap_or(0),
                    c.get(3).copied().unwrap_or(0),
                ];
                u32::from_le_bytes(b).to_string()
            })
            .collect();
        format!("S-1-5-80-{}", parts.join("-"))
    }

    /// The PowerShell script that gives `dir` exactly its permissions (A1), with
    /// inheritance from its parent turned off and Administrators as owner:
    /// SYSTEM and Administrators full control; the service's account modify for the
    /// data folder; Users may only look into MoneyKeep's machine folder itself.
    pub fn folder_acl_script(dir: &Path, use_: FolderUse) -> String {
        let all =
            "[System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'";
        let here = "[System.Security.AccessControl.InheritanceFlags]::None";
        let mut rules: Vec<(String, &str, &str)> = vec![
            ("S-1-5-18".into(), "FullControl", all),
            ("S-1-5-32-544".into(), "FullControl", all),
        ];
        match use_ {
            FolderUse::Data => rules.push((service_sid(SERVER_SERVICE), "Modify", all)),
            FolderUse::Machine => rules.push(("S-1-5-32-545".into(), "ReadAndExecute", here)),
            FolderUse::AdminOnly => {}
        }
        let adds: String = rules
            .iter()
            .map(|(sid, right, inh)| {
                format!(
                    "$a.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule((New-Object System.Security.Principal.SecurityIdentifier '{sid}'), [System.Security.AccessControl.FileSystemRights]'{right}', {inh}, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow))); "
                )
            })
            .collect();
        format!(
            "$ErrorActionPreference = 'Stop'; \
             $a = New-Object System.Security.AccessControl.DirectorySecurity; \
             $a.SetAccessRuleProtection($true, $false); \
             $a.SetOwner((New-Object System.Security.Principal.SecurityIdentifier 'S-1-5-32-544')); \
             {adds}Set-Acl -LiteralPath {} -AclObject $a",
            ps_literal(&dir.display().to_string())
        )
    }

    /// The PowerShell script printing a folder's owner as a security identifier.
    pub fn owner_script(dir: &Path) -> String {
        format!(
            "$ErrorActionPreference = 'Stop'; (Get-Acl -LiteralPath {}).GetOwner([System.Security.Principal.SecurityIdentifier]).Value",
            ps_literal(&dir.display().to_string())
        )
    }

    /// The entropy the inbox key is sealed with (the same as the PowerShell sealing
    /// before upgrade-resilience-1 used, so keys sealed then still open).
    pub const DPAPI_ENTROPY: &[u8] = b"MoneyKeep capture inbox key";

    /// L5: the inbox key sealed with DPAPI for this computer. U1.6: through the
    /// Windows API (`CryptProtectData` / `CryptUnprotectData`), so a restricted
    /// PowerShell can't stop the server from opening its key. The folder's
    /// permissions keep everyone but the service and administrators out; DPAPI keeps
    /// a copy of the file useless on another computer.
    #[derive(Debug, Clone, Copy)]
    pub struct Dpapi;

    /// Seal (`protect`) or open with `A`'s DPAPI (tests: a fake that records every
    /// process it would start).
    pub fn dpapi_with<A: WindowsApi>(protect: bool, data: &[u8]) -> Result<Vec<u8>, String> {
        A::dpapi(protect, data, DPAPI_ENTROPY)
    }

    impl crate::store::KeySealer for Dpapi {
        fn seal(&self, plain: &[u8]) -> crate::store::StoreResult<Vec<u8>> {
            dpapi_with::<Os>(true, plain)
        }
        fn unseal(&self, sealed: &[u8]) -> crate::store::StoreResult<Zeroizing<Vec<u8>>> {
            dpapi_with::<Os>(false, sealed).map(Zeroizing::new)
        }
    }

    /// U1.6: a PowerShell step was blocked (Constrained Language Mode, script
    /// policy, or PowerShell not allowed to run).
    pub const POWERSHELL_BLOCKED: &str =
        "Windows blocked a PowerShell step (it may be restricted by your organisation).";

    /// The error text of a PowerShell step that was blocked rather than failed.
    pub fn powershell_blocked(error: &str) -> bool {
        let e = error.to_ascii_lowercase();
        [
            "language mode",
            "constrainedlanguage",
            "only on core types",
            "running scripts is disabled",
            "pssecurityexception",
            "unauthorizedaccess",
            "blocked by group policy",
            "blocked by your system administrator",
            "applocker",
            "cannot run powershell",
        ]
        .iter()
        .any(|w| e.contains(w))
    }

    /// Whether `program` is Windows PowerShell (`powershell` / `powershell.exe`).
    fn is_windows_powershell(program: &Path) -> bool {
        program
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|n| n == "powershell" || n == "powershell.exe")
    }

    /// Windows PowerShell starts without the caller's `PSModulePath`. Started from
    /// PowerShell 7 (a pwsh window, or anything run from one), it inherits PowerShell
    /// 7's module folders, can't load its own built-in modules and fails on
    /// `Set-Acl`/`Get-Acl` ("the module could not be loaded"); without the variable it
    /// uses its default folders.
    pub fn without_ps7_modules(cmd: &mut std::process::Command, program: &Path) {
        if is_windows_powershell(program) {
            cmd.env_remove("PSModulePath");
        }
    }

    /// A failed PowerShell step's error, saying so when Windows blocked it.
    fn ps_error(program: &Path, text: String) -> String {
        let is_ps = program
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|n| n.starts_with("powershell"));
        if is_ps && powershell_blocked(&text) {
            format!("{POWERSHELL_BLOCKED} ({})", text.trim())
        } else {
            text
        }
    }

    /// Quote a word for a Windows command line (`CommandLineToArgvW` rules).
    pub fn quote_arg(a: &str) -> String {
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            return a.to_owned();
        }
        let mut out = String::from('"');
        let mut slashes = 0usize;
        for c in a.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', slashes));
                    out.push(c);
                    slashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }

    /// The folders `cloudflared`'s Windows installer (MSI) puts it in (A2):
    /// `%ProgramFiles%\cloudflared` and `%ProgramFiles(x86)%\cloudflared`.
    pub fn cloudflared_folders() -> Vec<PathBuf> {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|p| PathBuf::from(p).join("cloudflared"))
            .collect()
    }

    /// Stop a service, wait for it (up to 30 seconds) and start it again.
    fn restart(name: &str) -> Result<(), String> {
        let _ = Os::stop(name);
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(30) {
            match Os::query(name) {
                Some(q) if q.state == State::Stopped => break,
                None => break,
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        Os::start(name)
    }

    fn hidden(program: &Path, args: &[&str]) -> Result<(), String> {
        let out = Os::run_hidden(program, args).map_err(|e| ps_error(program, e))?;
        if out.status.success() {
            Ok(())
        } else {
            // cloudflared writes some errors to standard output (revision 16b).
            let err = String::from_utf8_lossy(&out.stderr);
            let text = if err.trim().is_empty() {
                String::from_utf8_lossy(&out.stdout).into_owned()
            } else {
                err.into_owned()
            };
            Err(ps_error(
                program,
                format!("{} failed: {}", program.display(), text.trim()),
            ))
        }
    }

    /// The server's Windows service (by name: the current one, or the one from
    /// before the rename).
    #[derive(Debug, Clone, Copy)]
    pub struct WindowsServices {
        pub name: &'static str,
    }

    impl WindowsServices {
        pub const CURRENT: Self = Self {
            name: SERVER_SERVICE,
        };
        pub const LEGACY: Self = Self {
            name: legacy::CAPTURE_SERVICE,
        };
    }

    impl ServiceManager for WindowsServices {
        fn server_state(&self) -> State {
            Os::query(self.name).map_or(State::NotInstalled, |q| q.state)
        }
        fn install_server(&self, exe: &Path, args: &[String]) -> Result<(), String> {
            // Only the current name is ever installed; the old name is only
            // stopped, started again after a failed move, and removed.
            if self.name != SERVER_SERVICE {
                return Err("the service with the old name is not installed again".into());
            }
            Os::install(&install_config(exe, args))
        }
        fn server_command(&self) -> Option<Vec<String>> {
            Os::query(self.name).map(|q| split_command_line(&q.command))
        }
        fn server_exit_code(&self) -> Option<u32> {
            Os::query(self.name).and_then(|q| q.exit_code)
        }
        fn folder_owner(&self, dir: &Path) -> Result<Option<String>, String> {
            powershell_output(&owner_script(dir)).map(|o| Some(o.trim().to_owned()))
        }
        fn protect_folder(&self, dir: &Path, use_: FolderUse) -> Result<(), String> {
            hidden(
                Path::new("powershell"),
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &folder_acl_script(dir, use_),
                ],
            )
        }
        fn cloudflared_version(&self, exe: &Path) -> Option<String> {
            let out = Os::run_hidden(exe, &["--version"]).ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        }
        fn set_cloudflared_command(&self, exe: &Path, args: &[String]) -> Result<(), String> {
            // Cloudflare's "Tunnel run parameters": on Windows the service's
            // parameters are its ImagePath; `sc.exe config … binPath=` sets it.
            let line: Vec<String> = std::iter::once(exe.display().to_string())
                .chain(args.iter().cloned())
                .map(|a| quote_arg(&a))
                .collect();
            hidden(
                Path::new("sc.exe"),
                &["config", CLOUDFLARED_SERVICE, "binPath=", &line.join(" ")],
            )?;
            restart(CLOUDFLARED_SERVICE)
        }
        fn uninstall_server(&self) -> Result<(), String> {
            if Os::query(self.name).is_none() {
                return Ok(());
            }
            self.stop_server(Duration::from_secs(30))?;
            Os::delete(self.name)
        }
        fn start_server(&self) -> Result<(), String> {
            Os::start(self.name)
        }
        fn stop_server(&self, wait: Duration) -> Result<(), String> {
            let Some(first) = Os::query(self.name) else {
                return Ok(());
            };
            if first.state == State::Stopped {
                return Ok(());
            }
            // A service already stopping refuses a second stop; the wait follows.
            let _ = Os::stop(self.name);
            let started = std::time::Instant::now();
            while started.elapsed() < wait {
                match Os::query(self.name) {
                    Some(q) if q.state == State::Stopped => return Ok(()),
                    None => return Ok(()),
                    _ => std::thread::sleep(Duration::from_millis(500)),
                }
            }
            // Still running after the wait: end its process.
            let pid = Os::query(self.name)
                .and_then(|q| q.pid)
                .or(first.pid)
                .ok_or("the capture server did not stop and its process is unknown")?;
            hidden(Path::new("taskkill"), &["/F", "/PID", &pid.to_string()])
        }
        fn server_path(&self) -> Option<PathBuf> {
            Os::query(self.name).and_then(|q| program_of(&q.command))
        }
        fn cloudflared_path(&self) -> Option<PathBuf> {
            // A2: only where its installer puts it, never PATH or a user folder.
            find_in("cloudflared", &cloudflared_folders())
        }
        fn cloudflared_service(&self) -> Option<(State, String)> {
            Os::query(CLOUDFLARED_SERVICE).map(|q| (q.state, q.command))
        }
        fn install_cloudflared_package(&self, installer: &Path) -> Result<(), String> {
            let file = installer.display().to_string();
            hidden(Path::new("msiexec"), &["/i", &file, "/quiet", "/norestart"])
        }
        fn install_cloudflared_service(&self, exe: &Path, token: &str) -> Result<(), String> {
            hidden(exe, &["service", "install", token])
        }
        fn uninstall_cloudflared_service(&self, exe: &Path) -> Result<(), String> {
            hidden(exe, &["service", "uninstall"])
        }
        fn delete_cloudflared_service(&self) -> Result<(), String> {
            // A stopped service makes `stop` fail; `delete` decides.
            let _ = hidden(Path::new("sc.exe"), &["stop", CLOUDFLARED_SERVICE]);
            hidden(Path::new("sc.exe"), &["delete", CLOUDFLARED_SERVICE])
        }
        fn delete_cloudflared_event_log(&self) -> Result<bool, String> {
            // `reg query` fails when the key is not there: nothing to delete.
            let reg = Path::new("reg.exe");
            if hidden(reg, &["query", CLOUDFLARED_EVENT_LOG_KEY]).is_err() {
                return Ok(false);
            }
            hidden(reg, &["delete", CLOUDFLARED_EVENT_LOG_KEY, "/f"]).map(|()| true)
        }
        fn add_firewall_rule(&self, rule: &FirewallRule) -> Result<bool, String> {
            let script = firewall_script(rule)?;
            hidden(
                Path::new("powershell"),
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            )
            .map(|()| true)
        }
        fn remove_firewall_rule(&self, name: &str) -> Result<bool, String> {
            let script = firewall_remove_script(name);
            hidden(
                Path::new("powershell"),
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            )
            .map(|()| true)
        }
        fn firewall_scan(
            &self,
            rule_name: &str,
            program: &str,
        ) -> Result<Option<FirewallScan>, String> {
            let out = powershell_output(&firewall_scan_script(rule_name, program))?;
            parse_firewall_scan(&out).map(Some)
        }
        fn network_profiles(&self) -> Result<Vec<NetProfile>, String> {
            parse_network_profiles(&powershell_output(NETWORK_PROFILES_SCRIPT)?)
        }
        fn remove_firewall_rule_id(&self, id: &str) -> Result<(), String> {
            hidden(
                Path::new("powershell"),
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &firewall_remove_id_script(id),
                ],
            )
        }
    }

    /// A PowerShell script's standard output.
    fn powershell_output(script: &str) -> Result<String, String> {
        let ps = Path::new("powershell");
        let out = Os::run_hidden(ps, &["-NoProfile", "-NonInteractive", "-Command", script])
            .map_err(|e| ps_error(ps, e))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(ps_error(
                ps,
                format!(
                    "powershell failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ))
        }
    }

    /// The PowerShell script that reports a file's Authenticode signer only when the
    /// signature is `Valid` (exit code 2 and the status otherwise).
    pub fn authenticode_script(file: &Path) -> String {
        let path = file.display().to_string().replace('\'', "''");
        format!(
            "$s = Get-AuthenticodeSignature -LiteralPath '{path}'; \
             if ($s.Status -ne 'Valid') {{ Write-Output $s.Status; exit 2 }}; \
             Write-Output $s.SignerCertificate.Subject"
        )
    }

    /// Authenticode through Windows (`Get-AuthenticodeSignature`): `Ok` with the
    /// signer's subject only when the status is `Valid`.
    #[derive(Debug, Clone, Copy)]
    pub struct Authenticode;

    impl SignatureChecker for Authenticode {
        fn signer(&self, file: &Path) -> Result<String, String> {
            let script = authenticode_script(file);
            let out = Os::run_hidden(
                Path::new("powershell"),
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            )?;
            let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if out.status.success() {
                Ok(text)
            } else {
                Err(format!("signature status {text}"))
            }
        }
    }

    /// The IPv4 addresses with adapter, MAC address and network profile (as the user;
    /// no elevation needed).
    pub fn addresses() -> Result<Vec<LocalAddress>, String> {
        let out = Os::run_hidden(
            Path::new("powershell"),
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                ADDRESSES_SCRIPT,
            ],
        )?;
        Ok(parse_windows_addresses(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    /// `service-run`.
    pub fn run_service(data: PathBuf, allow_lan: bool) -> Result<(), String> {
        Os::run_service(data, allow_lan)
    }

    /// Start the capture server as the service: log to `server.log`, inbox key in
    /// its file. Shared by the real shim's service main.
    pub fn start_for_service(data: &Path, allow_lan: bool) -> Result<crate::Running, String> {
        crate::Options::for_dir(data, allow_lan).and_then(|mut o| {
            o.key_home = crate::KeyHome::File;
            o.log = crate::file_log(&data.join("server.log"));
            crate::start(o)
        })
    }

    #[cfg(not(windows))]
    mod stand_in {
        //! Outside Windows: every call answers "Windows only" (the service logic
        //! above still compiles and is type-checked here).
        use super::*;

        const WINDOWS_ONLY: &str = "Windows only";

        #[derive(Debug)]
        pub struct StandIn;

        impl WindowsApi for StandIn {
            fn query(_: &str) -> Option<Queried> {
                None
            }
            fn install(_: &InstallConfig) -> Result<(), String> {
                Err(WINDOWS_ONLY.into())
            }
            fn run_hidden_input(
                _: &Path,
                _: &[&str],
                _: &[u8],
            ) -> Result<std::process::Output, String> {
                Err(WINDOWS_ONLY.into())
            }
            fn stop(_: &str) -> Result<(), String> {
                Err(WINDOWS_ONLY.into())
            }
            fn start(_: &str) -> Result<(), String> {
                Err(WINDOWS_ONLY.into())
            }
            fn delete(_: &str) -> Result<(), String> {
                Err(WINDOWS_ONLY.into())
            }
            fn run_hidden(_: &Path, _: &[&str]) -> Result<std::process::Output, String> {
                Err(WINDOWS_ONLY.into())
            }
            fn run_service(_: PathBuf, _: bool) -> Result<(), String> {
                Err(WINDOWS_ONLY.into())
            }
            fn dpapi(_: bool, _: &[u8], _: &[u8]) -> Result<Vec<u8>, String> {
                Err(WINDOWS_ONLY.into())
            }
        }
    }

    #[cfg(windows)]
    mod real {
        //! The Service Control Manager through the `windows-service` crate (0.8),
        //! and processes without a console window. Compiled on Windows only.
        use std::ffi::OsString;
        use std::os::windows::process::CommandExt;
        use std::sync::OnceLock;
        use std::sync::mpsc;
        use std::time::Duration;

        use windows_service::service::{
            ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
            ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
            ServiceInfo, ServiceSidType, ServiceStartType, ServiceState, ServiceStatus,
            ServiceType,
        };
        use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
        use windows_service::service_manager::{ServiceManager as Scm, ServiceManagerAccess};
        use windows_service::{define_windows_service, service_dispatcher};

        use super::*;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        fn err(e: windows_service::Error) -> String {
            e.to_string()
        }

        fn scm(access: ServiceManagerAccess) -> Result<Scm, String> {
            Scm::local_computer(None::<&str>, access).map_err(err)
        }

        #[derive(Debug)]
        pub struct Real;

        impl WindowsApi for Real {
            fn query(name: &str) -> Option<Queried> {
                let svc = scm(ServiceManagerAccess::CONNECT)
                    .ok()?
                    .open_service(
                        name,
                        ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
                    )
                    .ok()?;
                let status = svc.query_status().ok()?;
                let state = match status.current_state {
                    ServiceState::Stopped => State::Stopped,
                    _ => State::Running,
                };
                let command = svc
                    .query_config()
                    .map(|c| c.executable_path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let exit_code = match status.exit_code {
                    ServiceExitCode::ServiceSpecific(n) => Some(n),
                    ServiceExitCode::Win32(_) => None,
                };
                Some(Queried {
                    state,
                    command,
                    pid: status.process_id,
                    exit_code,
                })
            }

            fn install(cfg: &InstallConfig) -> Result<(), String> {
                let manager =
                    scm(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
                let name = cfg.name.as_str();
                let info = ServiceInfo {
                    name: OsString::from(name),
                    display_name: OsString::from(&cfg.display),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::AutoStart,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: cfg.exe.clone(),
                    launch_arguments: cfg.args.iter().map(OsString::from).collect(),
                    dependencies: Vec::new(),
                    // security-review-1 A1: its own virtual account (no password).
                    account_name: Some(OsString::from(&cfg.account)),
                    account_password: None,
                };
                let access = ServiceAccess::QUERY_STATUS
                    | ServiceAccess::START
                    | ServiceAccess::STOP
                    | ServiceAccess::CHANGE_CONFIG;
                let svc = match manager.open_service(name, access | ServiceAccess::DELETE) {
                    Ok(svc) => match svc.change_config(&info) {
                        Ok(()) => svc,
                        Err(first) => {
                            // upgrade-resilience-1 L10: ChangeServiceConfig refused
                            // (changing a LocalSystem service to the virtual account,
                            // say): delete it and create it again with the same
                            // settings.
                            let first = err(first);
                            svc.delete()
                                .map_err(|e| format!("{first}; deleting it failed: {}", err(e)))?;
                            drop(svc);
                            create_again(&manager, &info, access)
                                .map_err(|e| format!("{first}; creating it again failed: {e}"))?
                        }
                    },
                    Err(_) => manager.create_service(&info, access).map_err(err)?,
                };
                if cfg.sid_unrestricted {
                    svc.set_config_service_sid_info(ServiceSidType::Unrestricted)
                        .map_err(err)?;
                }
                // U1.5: the service looks after itself: restarts after 10 s, 30 s,
                // then 60 s, counted again after a day, also when it stops with an
                // error.
                svc.update_failure_actions(ServiceFailureActions {
                    reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(
                        cfg.failure_reset_secs,
                    )),
                    reboot_msg: None,
                    command: None,
                    actions: Some(
                        cfg.restart_after_secs
                            .iter()
                            .map(|s| ServiceAction {
                                action_type: ServiceActionType::Restart,
                                delay: Duration::from_secs(*s),
                            })
                            .collect(),
                    ),
                })
                .map_err(err)?;
                svc.set_failure_actions_on_non_crash_failures(cfg.failure_actions_on_exit_code)
                    .map_err(err)?;
                let _ = svc.set_description(&cfg.description);
                let running = svc
                    .query_status()
                    .is_ok_and(|s| s.current_state != ServiceState::Stopped);
                if running {
                    return Ok(());
                }
                svc.start::<&str>(&[]).map_err(err)
            }

            fn stop(name: &str) -> Result<(), String> {
                let svc = scm(ServiceManagerAccess::CONNECT)?
                    .open_service(name, ServiceAccess::STOP)
                    .map_err(err)?;
                svc.stop().map(|_| ()).map_err(err)
            }

            fn start(name: &str) -> Result<(), String> {
                let svc = scm(ServiceManagerAccess::CONNECT)?
                    .open_service(name, ServiceAccess::START)
                    .map_err(err)?;
                svc.start::<&str>(&[]).map_err(err)
            }

            fn delete(name: &str) -> Result<(), String> {
                let svc = scm(ServiceManagerAccess::CONNECT)?
                    .open_service(name, ServiceAccess::DELETE)
                    .map_err(err)?;
                svc.delete().map_err(err)
            }

            fn run_hidden(program: &Path, args: &[&str]) -> Result<std::process::Output, String> {
                let mut cmd = std::process::Command::new(program);
                without_ps7_modules(&mut cmd, program);
                cmd.args(args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .map_err(|e| format!("cannot run {}: {e}", program.display()))
            }

            fn run_hidden_input(
                program: &Path,
                args: &[&str],
                input: &[u8],
            ) -> Result<std::process::Output, String> {
                use std::io::Write;
                let mut cmd = std::process::Command::new(program);
                without_ps7_modules(&mut cmd, program);
                let mut child = cmd
                    .args(args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin
                        .write_all(input)
                        .map_err(|e| format!("cannot write to {}: {e}", program.display()))?;
                }
                child
                    .wait_with_output()
                    .map_err(|e| format!("cannot run {}: {e}", program.display()))
            }

            fn run_service(data: PathBuf, allow_lan: bool) -> Result<(), String> {
                let _ = ARGS.set((data, allow_lan));
                service_dispatcher::start(SERVER_SERVICE, ffi_service_main).map_err(err)
            }

            fn dpapi(protect: bool, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
                if protect {
                    mk_dpapi::protect(data, entropy)
                } else {
                    mk_dpapi::unprotect(data, entropy)
                }
            }
        }

        /// L10: create the service again after deleting it; the Service Control
        /// Manager finishes a deletion once every handle is closed, so creating is
        /// tried for up to 10 seconds.
        fn create_again(
            manager: &Scm,
            info: &ServiceInfo,
            access: ServiceAccess,
        ) -> Result<windows_service::service::Service, String> {
            let started = std::time::Instant::now();
            loop {
                match manager.create_service(info, access) {
                    Ok(svc) => return Ok(svc),
                    Err(e) if started.elapsed() >= Duration::from_secs(10) => return Err(err(e)),
                    Err(_) => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        }

        static ARGS: OnceLock<(PathBuf, bool)> = OnceLock::new();

        define_windows_service!(ffi_service_main, service_main);

        fn service_main(_: Vec<OsString>) {
            let _ = serve();
        }

        fn serve() -> Result<(), String> {
            let (tx, rx) = mpsc::channel();
            let handler = move |event| match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            };
            let handle = service_control_handler::register(SERVER_SERVICE, handler).map_err(err)?;
            let status =
                |state: ServiceState, accept: ServiceControlAccept, code: u32| ServiceStatus {
                    service_type: ServiceType::OWN_PROCESS,
                    current_state: state,
                    controls_accepted: accept,
                    exit_code: ServiceExitCode::Win32(code),
                    checkpoint: 0,
                    wait_hint: Duration::default(),
                    process_id: None,
                };
            let (data, allow_lan) = ARGS.get().cloned().ok_or("no service arguments")?;
            match start_for_service(&data, allow_lan) {
                Ok(running) => {
                    let _ = handle.set_service_status(status(
                        ServiceState::Running,
                        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                        0,
                    ));
                    let _ = rx.recv();
                    drop(running);
                    let _ = handle.set_service_status(status(
                        ServiceState::Stopped,
                        ServiceControlAccept::empty(),
                        0,
                    ));
                }
                Err(e) => {
                    crate::file_log(&data.join("server.log"))(&format!("could not start: {e}"));
                    // U1.4: each refusal has its own service-specific exit code,
                    // which `services status` reads without opening the folder.
                    let exit_code = match crate::refusal_for(&data, &e) {
                        Some(r) => ServiceExitCode::ServiceSpecific(u32::from(r.code())),
                        None => ServiceExitCode::Win32(1),
                    };
                    let _ = handle.set_service_status(ServiceStatus {
                        exit_code,
                        ..status(ServiceState::Stopped, ServiceControlAccept::empty(), 1)
                    });
                }
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used)]
        use super::*;

        /// Windows PowerShell never inherits PowerShell 7's module path; other
        /// programs keep their environment.
        #[test]
        fn windows_powershell_starts_without_ps7_modules() {
            for name in ["powershell", "powershell.exe", "PowerShell.EXE"] {
                let mut cmd = std::process::Command::new(name);
                without_ps7_modules(&mut cmd, Path::new(name));
                let cleared: Vec<_> = cmd.get_envs().collect();
                assert_eq!(
                    cleared,
                    [(std::ffi::OsStr::new("PSModulePath"), None)],
                    "{name}"
                );
            }
            for name in ["sc.exe", "pwsh", "cloudflared.exe"] {
                let mut cmd = std::process::Command::new(name);
                without_ps7_modules(&mut cmd, Path::new(name));
                assert_eq!(cmd.get_envs().count(), 0, "{name}");
            }
        }

        /// Outside Windows the service logic is compiled and answers safely: no
        /// service, and every action says "Windows only".
        #[cfg(not(windows))]
        #[test]
        fn stand_in_answers_windows_only() {
            let w = WindowsServices::CURRENT;
            assert_eq!(w.server_state(), State::NotInstalled);
            let old = WindowsServices::LEGACY;
            assert_eq!(old.server_state(), State::NotInstalled);
            assert!(old.install_server(Path::new("x"), &[]).is_err());
            assert!(old.uninstall_server().is_ok(), "nothing to remove");
            assert!(w.server_path().is_none());
            assert!(
                w.stop_server(Duration::from_millis(1)).is_ok(),
                "nothing to stop"
            );
            assert_eq!(w.start_server().unwrap_err(), "Windows only");
            assert_eq!(
                w.install_server(Path::new("x"), &[]).unwrap_err(),
                "Windows only"
            );
            assert!(w.cloudflared_service().is_none());
            assert_eq!(
                Authenticode.signer(Path::new("x")).unwrap_err(),
                "Windows only"
            );
            assert_eq!(
                run_service(PathBuf::from("x"), false).unwrap_err(),
                "Windows only"
            );
        }

        #[test]
        fn authenticode_script_quotes_the_path() {
            let s = authenticode_script(Path::new("C:\\Users\\O'Neil\\x.msi"));
            assert!(
                s.contains("-LiteralPath 'C:\\Users\\O''Neil\\x.msi'"),
                "{s}"
            );
            assert!(s.contains("if ($s.Status -ne 'Valid')"));
        }
    }
}
