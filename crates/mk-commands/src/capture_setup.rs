//! Phone capture guided setup and status light (mobile-capture.md revision 2,
//! sections B and C).
//!
//! Cloudflare is configured through its v4 API at `https://api.cloudflare.com` with
//! the user's API token, through the injected [`crate::connect::Fetcher`] (tests fake
//! it). Endpoints and fields are taken from Cloudflare's documentation:
//!
//! - Remotely-managed tunnels, ingress and the proxied CNAME:
//!   <https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/>
//!   (`POST /accounts/{account}/cfd_tunnel` with `config_src: cloudflare` returns `id`
//!   and `token`; `PUT …/cfd_tunnel/{id}/configurations` takes `config.ingress`, whose
//!   last rule is the catch-all `http_status:404`; `POST /zones/{zone}/dns_records`
//!   with `type: CNAME`, `proxied: true`, `content: <id>.cfargotunnel.com`).
//! - Access service tokens (`POST /accounts/{account}/access/service_tokens` returns
//!   `id`, `client_id`, `client_secret`; clients send `CF-Access-Client-Id` and
//!   `CF-Access-Client-Secret`):
//!   <https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/>
//! - Reusable Access policies (`/accounts/{account}/access/policies`, `decision`
//!   `allow` or `non_identity`, `include` rules `{email: {email}}` and
//!   `{service_token: {token_id}}`):
//!   <https://developers.cloudflare.com/api/resources/zero_trust/subresources/access/subresources/policies/methods/get>
//! - Access applications (`/accounts/{account}/access/apps`, `type: self_hosted`,
//!   `domain`, `policies: [{id, precedence}]`; the answer carries the `aud` tag), the
//!   organization's `auth_domain` (`/accounts/{account}/access/organizations`), token
//!   verification (`/user/tokens/verify`), accounts and zones: the Cloudflare API
//!   reference <https://developers.cloudflare.com/api/resources/zero_trust/>.
//!
//! What runs on this computer (the capture server and `cloudflared` as services) is
//! behind [`CaptureHost`]; `mk-connect` supplies the real one and tests a fake.
//! Network calls and service installation never happen inside a book write: each step
//! is a request (or a host action) with no transaction open, followed by a short write
//! that records what was made, so a stopped setup resumes where it stopped and
//! removal deletes exactly what was made.

use mk_capture_api as api;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::capture::MODULE;
use crate::connect::{
    Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, NetJob, NetOp, NetStep, SecretRef,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

/// The only host the setup talks to.
pub const CLOUDFLARE_HOST: &str = "api.cloudflare.com";
const CF_BASE: &str = "https://api.cloudflare.com/client/v4";
pub(crate) const SETUP_KEY: &str = "capture.setup";
/// The capture server's port.
pub const SERVER_PORT: u16 = 8787;
pub const TUNNEL_NAME: &str = "moneykeep-capture";
pub const DNS_COMMENT: &str = "Made by MoneyKeep phone capture";
pub const APP_NAME: &str = "MoneyKeep phone capture";
pub const ALLOW_POLICY_NAME: &str = "MoneyKeep phone capture - allowed people";
pub const SERVICE_TOKEN_NAME: &str = "MoneyKeep desktop status";
pub const SERVICE_POLICY_NAME: &str = "MoneyKeep desktop status - service token";
/// Where people create (and revoke) API tokens.
pub const TOKEN_PAGE: &str = "https://dash.cloudflare.com/profile/api-tokens";
const MAX_DETAILS: usize = 200;
/// The Access application's session length: 30 days (revision 3).
pub const SESSION_DURATION: &str = "720h";
/// No account from either list (app-desktop.md revision 30).
pub const NO_ACCOUNT: &str = "This token can't see your domain. Check that it includes Zone · Zone · Read and Zone · DNS · Edit for your domain, and the Account permissions for your account.";
const MAX_JSON: usize = 4 * 1024 * 1024;

/// The API token's permissions, as Cloudflare's token page names them (section B1).
pub const PERMISSIONS: [&str; 6] = [
    PERM_TUNNEL,
    PERM_ACCESS_APPS,
    PERM_SERVICE_TOKENS,
    PERM_ORGANIZATIONS,
    PERM_DNS,
    PERM_ZONE,
];
const PERM_TUNNEL: &str = "Account · Cloudflare Tunnel · Edit";
const PERM_ACCESS_APPS: &str = "Account · Access: Apps and Policies · Edit";
const PERM_SERVICE_TOKENS: &str = "Account · Access: Service Tokens · Edit";
const PERM_ORGANIZATIONS: &str =
    "Account · Access: Organizations, Identity Providers, and Groups · Read";
const PERM_DNS: &str = "Zone · DNS · Edit";
const PERM_ZONE: &str = "Zone · Zone · Read";

pub mod text {
    /// macos-release.md C6: no `cloudflared` on the Mac (never downloaded there).
    pub const MAC_NO_CLOUDFLARED: &str = "cloudflared is not installed: install Cloudflare's own cloudflared package for macOS (https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/), then press Check again.";
    /// macos-release.md C2: the Status card and the help.
    pub const MAC_LOGGED_IN: &str = "On a Mac, phone capture works while you're logged in. Expenses sent while you're logged out wait on your phone.";
    /// macos-release.md C8: the action.
    pub const MAC_REMOVE: &str = "Remove phone capture from this Mac…";
    /// C8: done.
    pub const MAC_REMOVED: &str = "Phone capture was removed from this Mac.";
    /// C8: asked on another system.
    pub const MAC_ONLY: &str =
        "Remove phone capture from this Mac is only for Macs. Use Start over on this computer.";
    /// Revision 16: the `cloudflared` service here runs a tunnel that is gone.
    pub const REPLACE_MESSAGE: &str = "A cloudflared service on this computer runs a tunnel that no longer exists in your Cloudflare account. MoneyKeep can replace it with the new tunnel.";
    /// The plan's and the Details list's step.
    pub const REPLACE_STEP: &str = "Replace the cloudflared service of a deleted tunnel";
    /// Why Install is unavailable after Leave it.
    pub const LEAVE_REASON: &str = "Install is unavailable while the cloudflared service of the deleted tunnel stays on this computer: choose \"Replace it\", or remove that service yourself (see the help: Remove phone capture) and plan again.";
    /// Revision 16a: the `cloudflared` service here runs a tunnel of another
    /// Cloudflare account (maybe live, serving something else): left alone unless
    /// the owner chooses to replace it.
    pub const OTHER_ACCOUNT_MESSAGE: &str = "A cloudflared service on this computer runs a tunnel in another Cloudflare account. MoneyKeep leaves it alone unless you choose to replace it.";
    /// The warning beside Replace it for that service.
    pub const OTHER_ACCOUNT_WARNING: &str = "Anything that tunnel serves will stop working.";
    /// Why Install is unavailable while that service is left alone.
    pub const OTHER_ACCOUNT_LEAVE_REASON: &str = "Install is unavailable while the cloudflared service for a tunnel in another Cloudflare account stays on this computer: choose \"Replace it\" (anything that tunnel serves will stop working), or set up phone capture in that account instead.";
    /// A service whose tunnel exists (or could not be looked up): as before.
    pub const OTHER_TUNNEL: &str = "A cloudflared service for another tunnel already runs on this computer; it was not replaced. Choose \"Add to my existing tunnel\".";
    /// Revision 16: Remove phone capture did not finish.
    pub const UNDO_INCOMPLETE: &str = "Phone capture was not completely removed.";
    pub const GREEN: &str = "Phone capture is working";
    pub const GREY: &str = "Phone capture is off";
    pub const RED: &str = "Phone capture isn't working: the server isn't running";
    /// security-review-1 A6: the service is gone but its data is still here.
    pub const NOT_INSTALLED: &str =
        "Phone capture isn't working: the capture server isn't installed";
    /// A6: Install the server again's answers.
    pub const REINSTALLED: &str = "The capture server was installed again and is running.";
    pub const REINSTALL_DECLINED: &str = "The capture server was not installed — Windows permission was not given. Press Install the server again when ready.";
    pub const TUNNEL: &str = "The internet address isn't reaching your computer (tunnel down?)";
    pub const VERSION: &str = "Server version differs from the app";
    pub const OLDER: &str = "The capture server is older than this app";
    pub const NEWER: &str = "The capture server is newer than this app";
    pub const BUILD_FIRST: &str =
        "Install MoneyKeep with its installer, or (developers) build with cargo xtask release.";
    /// release-packaging.md: Update now's answers.
    pub fn updated_to(version: &str) -> String {
        format!(
            "Capture server updated to {}.",
            mk_capture_api::package_version(version)
        )
    }
    pub const NOT_UPDATED: &str = "Not updated — Windows permission was not given. Phones can keep sending to the current server; press Update now when ready.";
    /// branding.md step 6.
    pub const OLD_NAME: &str = "The capture server uses the old name — press Update server";
    pub const ALREADY_THIS_BUILD: &str = "The running server is already this build.";
    pub const RUNNING_IS_NEWER: &str = "The running server is newer than the built one.";
    pub const UPDATE_ANYWAY: &str =
        "The built server can't be compared with the running one. Update anyway?";
    pub const COLLECT: &str = "Collecting from the server failed";
    /// upgrade-resilience-1 U1.9: the collection error with when it happened
    /// ("Collecting from the server failed at 18:12: …").
    pub fn collect_failed(when: &str, error: &str) -> String {
        if when.is_empty() {
            format!("{COLLECT}: {error}")
        } else {
            format!("{COLLECT} {when}: {error}")
        }
    }
    /// U1.4: the capture server's API level is lower than this desktop needs.
    pub const UPDATE_SERVER: &str = "Update the capture server";
    pub const UPDATE_SERVER_DETAIL: &str = "The capture server on this computer is older than this MoneyKeep needs. Press Update now, or install MoneyKeep again with its installer.";
    /// U1.2: Wi-Fi capture's certificate files are damaged; the server runs
    /// without its Wi-Fi listener.
    pub const WIFI_DAMAGED: &str = "Wi-Fi capture can't start: its certificate files are damaged";
    pub const NEW_CERTIFICATE: &str = "Make a new certificate";
    pub const NEW_CERTIFICATE_DETAIL: &str = "MoneyKeep makes a new certificate for Wi-Fi capture. Every phone will then need the certificate installed again (Certificate QR code…); pairings and waiting expenses are kept.";
    /// U1.4: the server refused to start (its message comes from the server); the
    /// help says downgrades aren't supported.
    pub const REFUSED: &str = "Phone capture isn't working: the capture server refused to start";
    /// U1.7: after Change address, the phones that haven't connected since.
    pub const STALE_PHONES: &str = "These phones haven't connected since the address changed. Install the new certificate on each (Certificate QR code…), then open the new address:";
    /// Revision 8: the capture server's latest Access refusal, in words.
    pub fn access_problem(reason: &str) -> String {
        format!(
            "Phone requests are being refused by the capture server: {}.",
            reason.trim_end_matches('.')
        )
    }
}

// ----- this computer ---------------------------------------------------------------------

/// A background service's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    #[default]
    NotInstalled,
    Stopped,
    Running,
}

/// What this computer has (section B4 and B5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReport {
    /// `windows`, `linux` or `macos`.
    #[serde(default)]
    pub os: String,
    /// The `moneykeep-server` program was found.
    #[serde(default)]
    pub server_program: bool,
    #[serde(default)]
    pub server: ServiceState,
    /// A `cloudflared` program is installed.
    #[serde(default)]
    pub cloudflared_installed: bool,
    /// A `cloudflared` service exists (for whichever tunnel).
    #[serde(default)]
    pub cloudflared_service: Option<ServiceState>,
    /// The official Windows installer may be downloaded (Windows only).
    #[serde(default)]
    pub can_download_cloudflared: bool,
    /// This computer's private network addresses (IPv4).
    #[serde(default)]
    pub lan_addresses: Vec<String>,
    /// Problems finding things out, in words.
    #[serde(default)]
    pub notes: Vec<String>,
    /// The found server program's build version (revision 4).
    #[serde(default)]
    pub program_version: String,
    /// The found server program has the phone app built in.
    #[serde(default)]
    pub program_has_app: bool,
    /// The found server program's capture version (release-packaging.md; empty
    /// for programs built before it).
    #[serde(default)]
    pub program_capture_version: String,
    /// The program the server's service runs, when installed.
    #[serde(default)]
    pub server_path: Option<String>,
    /// The server is still installed under the name from before the rename (its
    /// service or its data folder; branding.md step 6): Update server moves it.
    #[serde(default)]
    pub old_name: bool,
    /// The tunnel (id) the `cloudflared` service runs, when its command line shows
    /// it (revision 16).
    #[serde(default)]
    pub cloudflared_tunnel: Option<String>,
    /// The account (id) that tunnel's token belongs to, when known.
    #[serde(default)]
    pub cloudflared_account: Option<String>,
    /// security-review-1 A1: the capture server's data was moved to its protected
    /// folder and the old copy in the user's folder is still there.
    #[serde(default)]
    pub old_copy: Option<OldCopy>,
    /// security-review-1 A6: the server's service is not installed but its data
    /// (`server.toml`) is still on this computer: "Install the server again".
    #[serde(default)]
    pub data_found: bool,
    /// upgrade-resilience-1 U1.4: the server refused to start, in words (a newer
    /// data folder or inbox, settings this version can't read, an inbox key that
    /// can't be opened).
    #[serde(default)]
    pub refusal: Option<String>,
    /// Which refusal (`newer_format`, `newer_database`, `settings`, `key`).
    #[serde(default)]
    pub refusal_kind: Option<String>,
}

/// security-review-1 A6: Install the server again (one permission prompt on
/// Windows). The data folder is the computer's own; the old copy in the user's
/// folder is copied in when it is the only one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReinstallRequest {
    /// The server listens on a LAN address (`--allow-lan`).
    pub allow_lan: bool,
    /// Wi-Fi capture's firewall rule, recreated in the same step (Wi-Fi capture
    /// set up).
    #[serde(default)]
    pub firewall_rule: Option<FirewallRule>,
}

/// The data folder the capture server moved out of (security-review-1 A1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldCopy {
    pub path: String,
    /// When it moved (Unix seconds); "Remove the old copy" is offered for
    /// [`OLD_COPY_DAYS`] days after.
    pub moved_at: i64,
}

/// How long the Status card offers "Remove the old copy" (A1).
pub const OLD_COPY_DAYS: i64 = 30;

/// The error a [`CaptureHost`] returns when the user declined the operating
/// system's permission prompt (release-packaging.md: "Not updated — Windows
/// permission was not given …").
pub const PERMISSION_DECLINED: &str = "Windows permission was not given";

/// Update server (revision 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRequest {
    /// The server listens on a LAN address (`--allow-lan`).
    pub allow_lan: bool,
}

/// What to install (one permission prompt on Windows).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InstallRequest {
    /// `127.0.0.1:8787`, or a LAN address with `allow_lan`.
    pub listen: String,
    pub allow_lan: bool,
    pub team_domain: String,
    pub aud: String,
    /// The status service token's Client ID (not a secret).
    pub service_client_id: String,
    /// `https://<address>`, shown with pairing codes.
    pub client_url: String,
    /// Install `cloudflared` as a service for a new tunnel with this token.
    pub tunnel_token: Option<String>,
    /// Revision 16 ("Replace it"): the `cloudflared` service here runs this
    /// tunnel, which no longer exists; remove it in the same elevated step first.
    #[serde(default)]
    pub replace_cloudflared_tunnel: Option<String>,
}

impl std::fmt::Debug for InstallRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallRequest")
            .field("listen", &self.listen)
            .field("allow_lan", &self.allow_lan)
            .field("team_domain", &self.team_domain)
            .field("client_url", &self.client_url)
            .field(
                "tunnel_token",
                &self.tunnel_token.as_ref().map(|_| "(hidden)"),
            )
            .field(
                "replace_cloudflared_tunnel",
                &self.replace_cloudflared_tunnel,
            )
            .finish_non_exhaustive()
    }
}

/// What an installation did.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InstallOutcome {
    /// The server's new desktop token (kept in MoneyKeep's secrets).
    pub desktop_token: String,
    /// The address MoneyKeep uses on this computer (`http://127.0.0.1:8787`).
    pub local_url: String,
    /// What was done, in words.
    pub actions: Vec<String>,
    /// `cloudflared` was installed as a service by this run.
    pub cloudflared_service: bool,
    /// The `cloudflared` service of a deleted tunnel was replaced (revision 16).
    #[serde(default)]
    pub cloudflared_replaced: bool,
}

impl std::fmt::Debug for InstallOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallOutcome")
            .field("local_url", &self.local_url)
            .field("actions", &self.actions)
            .field("desktop_token", &"(hidden)")
            .finish_non_exhaustive()
    }
}

/// What to remove.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallRequest {
    pub server: bool,
    pub cloudflared: bool,
}

/// Start over's part on this computer (mobile-capture.md revision 17), in one
/// elevated step: what to remove. Nothing else is touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOverRequest {
    /// The capture server's service.
    #[serde(default)]
    pub server: bool,
    /// The `cloudflared` service (MoneyKeep installed it, or its tunnel is gone).
    #[serde(default)]
    pub cloudflared: bool,
    /// Remove that service only while it runs this tunnel (the one the preview
    /// showed); `None` when its command line does not show a tunnel.
    #[serde(default)]
    pub cloudflared_tunnel: Option<String>,
    /// Wi-Fi capture's firewall rule, by name.
    #[serde(default)]
    pub firewall_rule: Option<String>,
    /// The capture server's data folder (certificates, paired phones, the inbox).
    #[serde(default)]
    pub data_folder: bool,
}

impl StartOverRequest {
    /// Anything that needs administrator rights (the services, the firewall rule,
    /// and the data folder, which belongs to the service's own account:
    /// security-review-1 A1).
    pub fn needs_permission(&self) -> bool {
        self.server || self.cloudflared || self.firewall_rule.is_some() || self.data_folder
    }
    /// Nothing to do on this computer.
    pub fn is_empty(&self) -> bool {
        !self.needs_permission() && !self.data_folder
    }
}

// ----- Wi-Fi capture on this computer (mobile-capture.md revision 10) ----------------------

/// One of this computer's IPv4 addresses with its network adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAddress {
    pub address: String,
    #[serde(default)]
    pub adapter: String,
    #[serde(default)]
    pub mac: String,
    /// Windows network profile (`Public`, `Private`, `DomainAuthenticated`).
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub dhcp: bool,
}

/// The inbound firewall rule Wi-Fi setup asks for (Windows Defender Firewall):
/// the two ports, the Private and Domain profiles only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallRule {
    pub name: String,
    pub ports: Vec<u16>,
    pub profiles: Vec<String>,
}

/// Turn Wi-Fi capture on (or move it to another address).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiRequest {
    pub address: String,
    #[serde(default)]
    pub name: String,
    pub https_port: u16,
    pub http_port: u16,
    /// The computer's name in the certificate authority's name.
    #[serde(default)]
    pub computer: String,
    pub firewall: FirewallRule,
    /// MoneyKeep has no desktop token for this server yet: make one.
    #[serde(default)]
    pub new_desktop_token: bool,
    /// The server also listens on a LAN address for a tunnel on another device.
    #[serde(default)]
    pub allow_lan: bool,
}

/// What turning Wi-Fi capture on did.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WifiOutcome {
    /// A new desktop token (when asked for one).
    #[serde(default)]
    pub desktop_token: Option<String>,
    /// The address MoneyKeep uses on this computer (`http://127.0.0.1:8787`).
    pub local_url: String,
    pub ca_name: String,
    pub fingerprint: String,
    #[serde(default)]
    pub ca_created: bool,
    pub ca_not_after: i64,
    pub cert_not_before: i64,
    pub cert_not_after: i64,
    pub actions: Vec<String>,
    /// The firewall rule was added (Windows).
    #[serde(default)]
    pub firewall_added: bool,
    /// The server answered over HTTPS on the chosen address.
    #[serde(default)]
    pub answered: bool,
    #[serde(default)]
    pub check_error: Option<String>,
}

impl std::fmt::Debug for WifiOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WifiOutcome")
            .field("fingerprint", &self.fingerprint)
            .field("actions", &self.actions)
            .field(
                "desktop_token",
                &self.desktop_token.as_ref().map(|_| "(hidden)"),
            )
            .finish_non_exhaustive()
    }
}

/// Remove Wi-Fi capture from this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiRemove {
    pub firewall_rule: String,
    #[serde(default)]
    pub allow_lan: bool,
}

/// A firewall rule as the computer reports it (revision 18).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub program: String,
}

/// The firewall check (revision 18; `moneykeep-server services firewall-check`):
/// Wi-Fi capture's rule (`ok`, `missing` or `disabled`), Windows' own inbound Block
/// rules for the capture server, and the networks Windows treats as Public.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallState {
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub block_rules: Vec<FoundRule>,
    #[serde(default)]
    pub public_networks: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// security-review-1 A6: the capture server's service state (unknown from an
    /// older helper).
    #[serde(default)]
    pub server: Option<ServiceState>,
    /// macos-release.md C5: macOS's application firewall is on (`None`: not a Mac,
    /// or unknown).
    #[serde(default)]
    pub app_firewall: Option<bool>,
}

/// Fix the firewall (revision 18; one permission prompt on Windows): put the rule
/// back, and remove Windows' Block rules for the capture server (by id; the
/// computer checks again that each is one).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallFix {
    #[serde(default)]
    pub add_rule: Option<FirewallRule>,
    #[serde(default)]
    pub remove_block_rules: Vec<String>,
}

const NO_WIFI_HOST: &str = "This computer cannot set up Wi-Fi capture from this MoneyKeep.";

/// The capture server and `cloudflared` on this computer (section B5). The real one
/// (in `mk-connect`) runs `moneykeep-server`'s service helper, asking Windows for
/// permission once; tests use a fake.
pub trait CaptureHost: Send + Sync {
    fn inspect(&self) -> HostReport;
    fn install(&self, req: &InstallRequest) -> Result<InstallOutcome, String>;
    /// Returns what was done, in words.
    fn uninstall(&self, req: &UninstallRequest) -> Result<Vec<String>, String>;
    fn start_server(&self) -> Result<(), String>;
    /// Replace the installed server program with the newly built one (one
    /// permission prompt): stop, copy (old kept until the new one answers), register
    /// the install-folder path, start, check health; restore the old one on failure.
    fn update_server(&self, req: &UpdateRequest) -> Result<Vec<String>, String>;
    /// Make `access_aud` in the server's `server.toml` this AUD tag (revision 8,
    /// Check). Returns whether it changed and what was done; when it changed, the
    /// server's service is restarted (its program is never replaced here: only
    /// Update now replaces it, release-packaging.md).
    fn set_access_aud(
        &self,
        aud: &str,
        update: &UpdateRequest,
    ) -> Result<(bool, Vec<String>), String>;
    /// This computer's addresses on home or office networks (revision 10).
    fn wifi_addresses(&self) -> Result<Vec<LocalAddress>, String> {
        Err(NO_WIFI_HOST.into())
    }
    /// Turn Wi-Fi capture on: the server (installed or restarted), its certificates,
    /// the firewall rule (one permission prompt on Windows), then check that the
    /// address answers.
    fn wifi_install(&self, req: &WifiRequest) -> Result<WifiOutcome, String> {
        let _ = req;
        Err(NO_WIFI_HOST.into())
    }
    /// Turn Wi-Fi capture off: the firewall rule, the certificates and keys; the
    /// server keeps running for Cloudflare, if set up. Returns what was done.
    fn wifi_remove(&self, req: &WifiRemove) -> Result<Vec<String>, String> {
        let _ = req;
        Err(NO_WIFI_HOST.into())
    }
    /// The addresses a local name leads to on this computer (revision 12: Start
    /// checks a typed name against the chosen address). A name that does not
    /// resolve gives an empty list, not an error.
    fn resolve_name(&self, name: &str) -> Result<Vec<String>, String> {
        let _ = name;
        Err(NO_WIFI_HOST.into())
    }
    /// Start over (revision 17): remove what `req` names in one step (one
    /// permission prompt on Windows; [`PERMISSION_DECLINED`] when it was declined).
    /// Returns what was done, in words.
    fn start_over(&self, req: &StartOverRequest) -> Result<Vec<String>, String> {
        let _ = req;
        Err("This MoneyKeep cannot remove services from this computer.".into())
    }
    /// Revision 18: check the firewall for Wi-Fi capture (reads only).
    fn firewall_check(&self, rule_name: &str) -> Result<FirewallState, String> {
        let _ = rule_name;
        Err(NO_WIFI_HOST.into())
    }
    /// Revision 18: fix it ([`PERMISSION_DECLINED`] when the prompt was declined).
    /// Returns what was done, in words.
    fn firewall_fix(&self, req: &FirewallFix) -> Result<Vec<String>, String> {
        let _ = req;
        Err(NO_WIFI_HOST.into())
    }
    /// security-review-1 A1: remove the old copy of the data folder in the user's
    /// folder ([`HostReport::old_copy`]). Returns what was done, in words.
    fn remove_old_copy(&self) -> Result<Vec<String>, String> {
        Err("There is no old copy to remove.".into())
    }
    /// security-review-1 A6: install the capture server's service again with its
    /// data ([`PERMISSION_DECLINED`] when the prompt was declined). Returns what was
    /// done, in words.
    fn reinstall_server(&self, req: &ReinstallRequest) -> Result<Vec<String>, String> {
        let _ = req;
        Err("This MoneyKeep cannot install services on this computer.".into())
    }
}

/// A host action run with the requests of a [`NetJob`] (no book open).
#[derive(Debug, Clone)]
pub(crate) enum HostOp {
    Inspect,
    Install(InstallRequest),
    Uninstall(UninstallRequest),
    Start,
    Update(UpdateRequest),
    SetAud(String, UpdateRequest),
    WifiAddresses,
    WifiInstall(WifiRequest),
    WifiRemove(WifiRemove),
    /// Revision 12: what a local name leads to on this computer.
    WifiResolve(String),
    /// Revision 17: Start over's part on this computer.
    StartOver(StartOverRequest),
    /// Revision 18: the firewall check (by the rule's name), and its fix.
    FirewallCheck(String),
    FirewallFix(FirewallFix),
    /// security-review-1 A1: remove the old copy of the data folder.
    RemoveOldCopy,
    /// security-review-1 A6: install the server again.
    Reinstall(ReinstallRequest),
}

#[derive(Debug, Clone)]
pub(crate) enum HostOut {
    Report(HostReport),
    Installed(InstallOutcome),
    Removed(Vec<String>),
    Started,
    /// `server.toml`'s AUD tag: changed (and the server restarted), what was done.
    AudSet(bool, Vec<String>),
    Addresses(Vec<LocalAddress>),
    WifiInstalled(WifiOutcome),
    /// The addresses a local name leads to (empty: it does not resolve).
    Resolved(Vec<String>),
    /// The firewall check (revision 18).
    Firewall(FirewallState),
    Failed(String),
}

pub(crate) fn run_host(h: &dyn CaptureHost, op: &HostOp) -> HostOut {
    match op {
        HostOp::Inspect => HostOut::Report(h.inspect()),
        HostOp::Install(r) => h
            .install(r)
            .map_or_else(HostOut::Failed, HostOut::Installed),
        HostOp::Uninstall(r) => h
            .uninstall(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::Start => h
            .start_server()
            .map_or_else(HostOut::Failed, |()| HostOut::Started),
        HostOp::Update(r) => h
            .update_server(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::SetAud(aud, r) => h
            .set_access_aud(aud, r)
            .map_or_else(HostOut::Failed, |(c, done)| HostOut::AudSet(c, done)),
        HostOp::WifiAddresses => h
            .wifi_addresses()
            .map_or_else(HostOut::Failed, HostOut::Addresses),
        HostOp::WifiInstall(r) => h
            .wifi_install(r)
            .map_or_else(HostOut::Failed, HostOut::WifiInstalled),
        HostOp::WifiRemove(r) => h
            .wifi_remove(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::WifiResolve(n) => h
            .resolve_name(n)
            .map_or_else(HostOut::Failed, HostOut::Resolved),
        HostOp::StartOver(r) => h
            .start_over(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::FirewallCheck(n) => h
            .firewall_check(n)
            .map_or_else(HostOut::Failed, HostOut::Firewall),
        HostOp::FirewallFix(r) => h
            .firewall_fix(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::RemoveOldCopy => h
            .remove_old_copy()
            .map_or_else(HostOut::Failed, HostOut::Removed),
        HostOp::Reinstall(r) => h
            .reinstall_server(r)
            .map_or_else(HostOut::Failed, HostOut::Removed),
    }
}

// ----- records ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Named {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TunnelRec {
    pub id: String,
    pub name: String,
    /// Made by this setup (removal deletes the tunnel; else only our ingress rule).
    pub created: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ServiceTokenRec {
    pub id: String,
    pub client_id: String,
    pub secret_ref: Option<SecretRef>,
}

/// One API call in the Details list: never a token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Detail {
    pub method: String,
    pub path: String,
    pub result: String,
}

/// What setup chose and made (book setting `capture.setup`; secrets by reference).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SetupState {
    #[serde(default)]
    pub token_ref: Option<SecretRef>,
    #[serde(default)]
    pub account: Option<Named>,
    #[serde(default)]
    pub zone: Option<Named>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub tunnel: Option<TunnelRec>,
    /// `this` or `lan` (another device runs the tunnel).
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub lan_address: String,
    /// The ingress service (`http://127.0.0.1:8787` or `http://<LAN>:8787`).
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub ingress: bool,
    #[serde(default)]
    pub dns_record: Option<String>,
    #[serde(default)]
    pub team_domain: String,
    #[serde(default)]
    pub service_token: Option<ServiceTokenRec>,
    #[serde(default)]
    pub allow_policy: Option<String>,
    #[serde(default)]
    pub policy_emails: Vec<String>,
    #[serde(default)]
    pub service_policy: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub aud: String,
    #[serde(default)]
    pub installed_server: bool,
    #[serde(default)]
    pub installed_cloudflared: bool,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub details: Vec<Detail>,
    /// Revision 16: the deleted tunnel whose `cloudflared` service Install replaces.
    #[serde(default)]
    pub replace_service: Option<String>,
}

impl SetupState {
    pub(crate) fn account_id(&self) -> &str {
        self.account.as_ref().map_or("", |a| a.id.as_str())
    }
    fn zone_id(&self) -> &str {
        self.zone.as_ref().map_or("", |z| z.id.as_str())
    }
    fn tunnel_id(&self) -> &str {
        self.tunnel.as_ref().map_or("", |t| t.id.as_str())
    }
    fn detail(&mut self, d: Detail) {
        self.details.push(d);
        if self.details.len() > MAX_DETAILS {
            let cut = self.details.len() - MAX_DETAILS;
            self.details.drain(..cut);
        }
    }
    /// Anything still made at Cloudflare or installed here.
    fn has_resources(&self) -> bool {
        self.tunnel.as_ref().is_some_and(|t| t.created)
            || self.ingress
            || self.dns_record.is_some()
            || self.service_token.is_some()
            || self.allow_policy.is_some()
            || self.service_policy.is_some()
            || self.app.is_some()
            || self.installed_server
            || self.installed_cloudflared
    }
}

pub(crate) fn load(ctx: &Ctx<'_>) -> CommandResult<SetupState> {
    Ok(ctx
        .tx
        .get_setting(MODULE, SETUP_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn save(ctx: &Ctx<'_>, st: &SetupState) -> CommandResult<()> {
    let v = serde_json::to_value(st).map_err(|e| CommandError::Integrity(e.to_string()))?;
    ctx.save_setting(MODULE, SETUP_KEY, Some(&v))
}

/// The setup summary for `capture.settings` (no secrets).
pub(crate) fn summary(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let st = load(ctx)?;
    let has_token = match (&st.token_ref, ctx.connector) {
        (Some(r), Some(c)) => crate::connect::get_secret(ctx, c, r)
            .ok()
            .flatten()
            .is_some(),
        (Some(r), None) => r.store == "book" && ctx.tx.get_secret(&r.id)?.is_some(),
        _ => false,
    };
    Ok(json!({
        "complete": st.complete,
        "started": st.has_resources() || st.account.is_some(),
        "address": st.hostname,
        "emails": st.emails,
        "account": st.account.as_ref().map(|a| a.name.clone()),
        "zone": st.zone.as_ref().map(|z| z.name.clone()),
        "tunnel": st.tunnel.as_ref().map(|t| json!({"id": t.id, "name": t.name, "created": t.created})),
        "device": st.device,
        "lan_address": st.lan_address,
        "has_cloudflare_token": has_token,
        "installed_server": st.installed_server,
        "installed_cloudflared": st.installed_cloudflared,
        "details": st.details,
        "resources": resources(&st),
    }))
}

/// What setup made, in words (the removal confirmation lists them).
fn resources(st: &SetupState) -> Vec<String> {
    let mut out = Vec::new();
    if st.installed_server {
        out.push(undo_what(st, RStep::Uninstall, Some(false)));
    }
    if st.installed_cloudflared {
        out.push(undo_what(st, RStep::Uninstall, Some(true)));
    }
    if st.app.is_some() {
        out.push(undo_what(st, RStep::App, None));
    }
    if st.allow_policy.is_some() {
        out.push(undo_what(st, RStep::AllowPolicy, None));
    }
    if st.service_policy.is_some() {
        out.push(undo_what(st, RStep::ServicePolicy, None));
    }
    if st.service_token.is_some() {
        out.push(undo_what(st, RStep::ServiceToken, None));
    }
    if st.dns_record.is_some() {
        out.push(undo_what(st, RStep::Dns, None));
    }
    match &st.tunnel {
        Some(t) if t.created => out.push(undo_what(st, RStep::Tunnel, None)),
        Some(_) if st.ingress => out.push(undo_what(st, RStep::WriteIngress, None)),
        _ => {}
    }
    out
}

/// What a removal step removes, in words (`cloudflared`: which service, for the
/// services on this computer; `None` names both).
pub(crate) fn undo_what(st: &SetupState, step: RStep, cloudflared: Option<bool>) -> String {
    let tunnel = st.tunnel.as_ref().map_or("", |t| t.name.as_str());
    match step {
        RStep::Uninstall => match cloudflared {
            Some(false) => "The MoneyKeep capture server service on this computer".to_owned(),
            Some(true) => "The cloudflared service on this computer".to_owned(),
            None => "The services on this computer".to_owned(),
        },
        RStep::App => format!("The Access application for {}", st.hostname),
        RStep::AllowPolicy => format!("The Access policy \"{ALLOW_POLICY_NAME}\""),
        RStep::ServicePolicy => format!("The Access policy \"{SERVICE_POLICY_NAME}\""),
        RStep::ServiceToken => format!("The Access service token \"{SERVICE_TOKEN_NAME}\""),
        RStep::Dns => format!("The DNS record {}", st.hostname),
        RStep::Tunnel => format!("The tunnel \"{tunnel}\""),
        RStep::ReadIngress | RStep::WriteIngress => {
            format!("The rule for {} in the tunnel \"{tunnel}\"", st.hostname)
        }
    }
}

/// Where to remove what a removal step removes by hand (revision 16).
pub(crate) fn undo_where(step: RStep) -> &'static str {
    match step {
        RStep::Uninstall => {
            "This computer: see the help, Remove phone capture (cloudflared service uninstall in an administrator terminal)"
        }
        RStep::App => "Cloudflare dashboard: Zero Trust > Access > Applications",
        RStep::AllowPolicy | RStep::ServicePolicy => {
            "Cloudflare dashboard: Zero Trust > Access > Policies"
        }
        RStep::ServiceToken => "Cloudflare dashboard: Zero Trust > Access > Service auth",
        RStep::Dns => "Cloudflare dashboard: your domain > DNS > Records",
        RStep::Tunnel | RStep::ReadIngress | RStep::WriteIngress => {
            "Cloudflare dashboard: Zero Trust > Networks > Tunnels"
        }
    }
}

/// What is still left after a removal that did not finish, with where to remove it.
fn leftovers(st: &SetupState) -> Vec<Value> {
    let mut steps: Vec<(RStep, Option<bool>)> = Vec::new();
    if st.installed_server {
        steps.push((RStep::Uninstall, Some(false)));
    }
    if st.installed_cloudflared {
        steps.push((RStep::Uninstall, Some(true)));
    }
    for (on, step) in [
        (st.app.is_some(), RStep::App),
        (st.allow_policy.is_some(), RStep::AllowPolicy),
        (st.service_policy.is_some(), RStep::ServicePolicy),
        (st.service_token.is_some(), RStep::ServiceToken),
        (st.dns_record.is_some(), RStep::Dns),
        (st.tunnel.as_ref().is_some_and(|t| t.created), RStep::Tunnel),
        (
            st.tunnel.as_ref().is_some_and(|t| !t.created) && st.ingress,
            RStep::WriteIngress,
        ),
    ] {
        if on {
            steps.push((step, None));
        }
    }
    steps
        .into_iter()
        .map(|(step, cf)| json!({"what": undo_what(st, step, cf), "where": undo_where(step)}))
        .collect()
}

// ----- commands ----------------------------------------------------------------------------

pub(crate) fn is_network(name: &str) -> bool {
    matches!(
        name,
        "capture.setup_check_token"
            | "capture.setup_plan"
            | "capture.setup_apply"
            | "capture.setup_remove"
            | "capture.status"
            | "capture.start_server"
            | "capture.setup_repair"
            | "capture.update_server"
            | "capture.check_access"
            | "capture.start_over_preview"
            | "capture.start_over"
            | "capture.remove_old_copy"
            | "capture.reinstall_server"
            | "capture.remove_from_computer"
    )
}

fn network_only<A>(_: &Ctx<'_>, _: A) -> CommandResult<Value> {
    Err(no_network())
}

fn no_network() -> CommandError {
    CommandError::Rejected("this MoneyKeep front end cannot reach the network; use the app".into())
}

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "capture.setup_check_token",
        "Check a Cloudflare API token for phone capture setup: accounts, zones and missing permissions (handles a secret; not offered over MCP)",
        true,
        network_only::<CheckArgs>,
    );
    r.add_in(
        MODULE,
        "capture.setup_plan",
        "Dry run of phone capture setup: what would be created at Cloudflare and installed on this computer",
        false,
        network_only::<PlanArgs>,
    );
    r.add_in(
        MODULE,
        "capture.setup_apply",
        "Set up phone capture: tunnel, DNS record, Access application, service token and services (not offered over MCP)",
        true,
        network_only::<PlanArgs>,
    );
    r.add_in(
        MODULE,
        "capture.setup_remove",
        "Remove phone capture: delete what setup created at Cloudflare and uninstall the services (not offered over MCP)",
        true,
        network_only::<RemoveArgs>,
    );
    r.add_in(
        MODULE,
        "capture.status",
        "Phone capture status light: whether the server runs, the internet address reaches it and collecting works",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.remove_old_copy",
        "Remove the old copy of the phone capture server's data that stayed in your user folder after it moved to its protected folder (not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.reinstall_server",
        "Install the phone capture server's service again when it is missing but its data is still on this computer, with the Wi-Fi firewall rule (asks Windows for permission once; not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.remove_from_computer",
        "Remove phone capture from this Mac: stop and remove the capture server's and cloudflared's launchd agents, and delete the capture server's data folder only when asked (macOS only; not offered over MCP)",
        false,
        network_only::<RemoveFromComputerArgs>,
    );
    r.add_in(
        MODULE,
        "capture.start_server",
        "Start the phone capture server service on this computer (not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.update_server",
        "Replace the phone capture server with the newly built one and restart it (asks Windows for permission; not offered over MCP)",
        true,
        network_only::<UpdateArgs>,
    );
    r.add_in(
        MODULE,
        "capture.setup_repair",
        "Repair phone capture's Cloudflare settings: the Access application's 30-day sign-in and its policies (not offered over MCP)",
        true,
        network_only::<RemoveArgs>,
    );
    r.add_in(
        MODULE,
        "capture.check_access",
        "Check phone capture's Access application against the capture server: the AUD tag in server.toml (rewritten and the server restarted when it differs) and the address (not offered over MCP)",
        true,
        network_only::<AccessArgs>,
    );
    r.add_in(
        MODULE,
        "capture.start_over_preview",
        "Start over (preview): what removing everything phone capture set up would remove in Cloudflare, on this computer and in MoneyKeep, and what is kept; changes nothing (not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.start_over",
        "Start over: remove everything phone capture set up, in Cloudflare, on this computer and in MoneyKeep; books and backups are never touched (asks Windows for permission once; not offered over MCP)",
        true,
        network_only::<crate::capture_start_over::StartOverArgs>,
    );
    r.add_in(
        MODULE,
        "capture.forget_cloudflare_token",
        "Erase MoneyKeep's stored copy of the Cloudflare API token (not offered over MCP)",
        true,
        forget_token,
    );
}

/// macos-release.md C8: Remove phone capture from this Mac.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoveFromComputerArgs {
    /// Also delete the capture server's data folder (paired phones, the Wi-Fi
    /// certificate, expenses not yet collected). Default: keep it.
    #[serde(default)]
    delete_data: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    /// The Cloudflare API token (stored in MoneyKeep's secrets once it works); the
    /// stored one is used when omitted.
    #[serde(default)]
    token: Option<String>,
    /// The account to use (id), when the token sees several.
    #[serde(default)]
    account: Option<String>,
    /// The zone (domain) to use (id), when the account has several.
    #[serde(default)]
    zone: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanArgs {
    /// The subdomain (default `capture`); the address is `<subdomain>.<zone>`.
    #[serde(default)]
    subdomain: Option<String>,
    /// Email addresses allowed to sign in on the phone.
    emails: Vec<String>,
    /// An existing tunnel's id to add the address to; empty or omitted creates a new
    /// tunnel on this computer.
    #[serde(default)]
    tunnel: Option<String>,
    /// Where the existing tunnel runs: `this` (default) or `lan` (another device on
    /// this network).
    #[serde(default)]
    device: Option<String>,
    /// This computer's LAN address for `lan` (detected when omitted).
    #[serde(default)]
    lan_address: Option<String>,
    /// A `cloudflared` service here runs a tunnel that no longer exists: replace it
    /// with the new tunnel's (true, the default: "Replace it") or leave it (false:
    /// Install is unavailable). For a tunnel in another Cloudflare account the
    /// default is false (revision 16a: "Leave it").
    #[serde(default)]
    replace_service: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    /// Update even when the built server can't be compared with the running one
    /// (the user confirmed "Update anyway?").
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveArgs {
    /// A Cloudflare API token, when none is stored.
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccessArgs {
    /// A Cloudflare API token, when none is stored.
    #[serde(default)]
    token: Option<String>,
    /// Create the Access application again (Check found it gone).
    #[serde(default)]
    recreate: bool,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

pub(crate) fn coded(code: &'static str, m: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code,
        message: m.into(),
    }
}

fn forget_token(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let mut st = load(ctx)?;
    let had = st.token_ref.is_some();
    if let Some(r) = st.token_ref.take() {
        delete_secret(ctx, &r)?;
    }
    save(ctx, &st)?;
    Ok(json!({"forgotten": had, "token_page": TOKEN_PAGE}))
}

fn put_secret(ctx: &Ctx<'_>, id: &str, value: &str) -> CommandResult<SecretRef> {
    let home = if ctx.encrypted {
        "book"
    } else {
        match ctx.connector {
            Some(c) => crate::connect::secret_home(ctx, c)?,
            None => {
                return Err(coded(
                    "no_secure_storage",
                    "No secure place to keep the secret here; use the desktop app.",
                ));
            }
        }
    };
    if home == "book" {
        ctx.tx.put_secret(id, value)?;
    } else if let Some(c) = ctx.connector {
        c.secrets
            .put(id, value)
            .map_err(|e| CommandError::Rejected(format!("the credential store refused: {e}")))?;
    }
    Ok(SecretRef {
        store: home.to_owned(),
        id: id.to_owned(),
    })
}

pub(crate) fn delete_secret(ctx: &Ctx<'_>, r: &SecretRef) -> CommandResult<()> {
    if r.store == "book" {
        ctx.tx.delete_secret(&r.id)?;
    } else if let Some(c) = ctx.connector {
        let _ = c.secrets.delete(&r.id);
    }
    Ok(())
}

pub(crate) fn get_secret(ctx: &Ctx<'_>, r: &SecretRef) -> CommandResult<Option<String>> {
    match ctx.connector {
        Some(c) => crate::connect::get_secret(ctx, c, r),
        None if r.store == "book" => Ok(ctx.tx.get_secret(&r.id)?),
        None => Ok(None),
    }
}

fn token_shape(t: &str) -> CommandResult<String> {
    let t = t.trim();
    if t.is_empty() || t.len() > 400 || !t.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(invalid(
            "Paste the Cloudflare API token (a single line of letters, digits and symbols).",
        ));
    }
    Ok(t.to_owned())
}

fn check_subdomain(s: &str) -> CommandResult<String> {
    let s = s.trim().to_ascii_lowercase();
    let ok = !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err(invalid(
            "The subdomain is one word of letters, digits and hyphens, such as capture.",
        ));
    }
    Ok(s)
}

fn check_emails(list: &[String]) -> CommandResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for e in list {
        let e = e.trim().to_ascii_lowercase();
        if e.is_empty() {
            continue;
        }
        let valid = e.len() <= 254
            && e.split_once('@').is_some_and(|(l, d)| {
                !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
            })
            && !e.contains([' ', ',', ';', '<', '>']);
        if !valid {
            return Err(invalid(format!("{e} is not an email address.")));
        }
        if !out.contains(&e) {
            out.push(e);
        }
    }
    if out.is_empty() {
        return Err(invalid(
            "Enter at least one email address that may sign in on the phone.",
        ));
    }
    if out.len() > 20 {
        return Err(invalid("Enter at most 20 email addresses."));
    }
    Ok(out)
}

fn check_lan(a: &str) -> CommandResult<String> {
    let ip: std::net::Ipv4Addr = a
        .trim()
        .parse()
        .map_err(|_| invalid("The LAN address is an IPv4 address such as 192.168.1.20."))?;
    if !ip.is_private() {
        return Err(invalid(
            "The LAN address must be a private address such as 192.168.1.20.",
        ));
    }
    Ok(ip.to_string())
}

// ----- Cloudflare requests -----------------------------------------------------------------

/// A Cloudflare API request and its path template for the Details list.
#[derive(Debug, Clone)]
pub(crate) struct CfCall {
    pub(crate) req: HttpRequest,
    method: &'static str,
    template: String,
}

pub(crate) fn cf(
    token: &str,
    method: HttpMethod,
    template: &str,
    vars: &[(&str, &str)],
    query: &str,
    body: Option<&Value>,
) -> CfCall {
    let mut path = template.to_owned();
    for (k, v) in vars {
        let clean: String = v
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        path = path.replace(&format!("{{{k}}}"), &clean);
    }
    let mut headers = vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    if body.is_some() {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    let method_name = match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
    };
    CfCall {
        req: HttpRequest {
            method,
            url: format!("{CF_BASE}{path}{query}"),
            basic: None,
            authority: CLOUDFLARE_HOST.to_owned(),
            max_bytes: MAX_JSON,
            headers,
            body: body.map(|b| serde_json::to_vec(b).unwrap_or_default()),
            allow_http: false,
        },
        method: method_name,
        template: template.to_owned(),
    }
}

/// A Cloudflare answer: `{success, errors[{code, message}], result}`.
#[derive(Debug, Clone)]
pub(crate) struct CfAnswer {
    pub ok: bool,
    pub status: u16,
    pub result: Value,
    pub message: String,
}

pub(crate) fn cf_answer(r: Option<&Result<HttpResponse, FetchError>>) -> CfAnswer {
    match r {
        None => CfAnswer {
            ok: false,
            status: 0,
            result: Value::Null,
            message: "no answer".into(),
        },
        Some(Err(e)) => CfAnswer {
            ok: false,
            status: 0,
            result: Value::Null,
            message: e.to_string(),
        },
        Some(Ok(resp)) => {
            let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
            let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
            let message = v
                .get("errors")
                .and_then(Value::as_array)
                .map(|errs| {
                    errs.iter()
                        .filter_map(|e| e.get("message").and_then(Value::as_str))
                        .map(|m| mk_core::text::clean(m, 300).text)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| format!("HTTP {}", resp.status));
            CfAnswer {
                ok: success && (200..300).contains(&resp.status),
                status: resp.status,
                result: v.get("result").cloned().unwrap_or(Value::Null),
                message,
            }
        }
    }
}

impl CfAnswer {
    fn detail(&self, call: &CfCall) -> Detail {
        Detail {
            method: call.method.to_owned(),
            path: call.template.clone(),
            result: if self.ok {
                "ok".to_owned()
            } else if self.status == 0 {
                format!("failed: {}", self.message)
            } else {
                format!("HTTP {}: {}", self.status, self.message)
            },
        }
    }
    fn error(&self, call: &CfCall) -> CommandError {
        coded(
            "cloudflare",
            format!(
                "Cloudflare: {} ({} {})",
                self.message, call.method, call.template
            ),
        )
    }
}

pub(crate) fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn list_of(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

// ----- the network steps ---------------------------------------------------------------------

/// Answers kept in memory between the steps of one run (never written).
#[derive(Clone, Default)]
pub(crate) struct Mem {
    pub(crate) token: String,
    tunnel_token: Option<String>,
    /// The existing tunnel's configuration (`config` object).
    config: Option<Value>,
    /// An existing DNS record for the address that MoneyKeep made.
    dns_ours: Option<String>,
    host: HostReport,
    /// Removal (revision 16): the steps that failed in this run (skipped when the
    /// next step is chosen), with their reasons.
    pub(crate) skip: Vec<RStep>,
    pub(crate) failed: Vec<UndoFailure>,
    /// Start over (revision 17): the run's plan, while its Cloudflare part runs
    /// through the removal steps; the computer and MoneyKeep parts follow.
    pub(crate) start_over: Option<Box<crate::capture_start_over::Run>>,
}

/// A removal step that failed, in words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct UndoFailure {
    /// What is left.
    pub what: String,
    pub reason: String,
    /// Where to remove it by hand.
    #[serde(rename = "where")]
    pub place: String,
}

impl std::fmt::Debug for Mem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mem")
            .field("token", &"(hidden)")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Op {
    /// security-review-1 A1: the old copy of the data folder was removed.
    RemoveOldCopy,
    /// macos-release.md C8: this computer was looked at (it must be a Mac).
    RemoveFromComputerCheck {
        delete_data: bool,
    },
    /// C8: the agents (and the data folder, when asked) were removed.
    RemovedFromComputer {
        delete_data: bool,
    },
    /// Verify the token; list accounts and zones.
    Check {
        mem: Mem,
        from_arg: bool,
        account: Option<String>,
        zone: Option<String>,
    },
    /// Probe permissions in the chosen account and zone.
    Probe {
        mem: Mem,
        from_arg: bool,
        accounts: Vec<Named>,
        zones: Vec<(Named, String)>,
        account: Named,
        zone: Option<Named>,
        verify: String,
        details: Vec<Detail>,
    },
    /// The plan's first read: this computer (revision 16: the tunnel a
    /// `cloudflared` service runs decides the next reads).
    PrepareHost {
        mem: Mem,
        args: PlanArgs,
        apply: bool,
    },
    /// The plan's reads (DNS, existing tunnel, the service's tunnel).
    Prepare {
        mem: Mem,
        args: PlanArgs,
        apply: bool,
    },
    /// One setup step: its request (or host action) answered.
    Step {
        mem: Mem,
        step: Step,
        call: Option<CfCall>,
    },
    /// One removal step.
    Remove {
        mem: Mem,
        step: RStep,
        call: Option<CfCall>,
    },
    /// Update server: the running version and the built program.
    UpdateCheck {
        allow_lan: bool,
        force: bool,
    },
    /// Update server: the host replaced the program.
    UpdateDone {
        version: String,
    },
    /// The Access application rewritten with the current settings.
    Repair {
        call: CfCall,
    },
    /// Check (revision 8): the Access application as Cloudflare has it.
    AccessRead {
        mem: Mem,
        call: CfCall,
    },
    /// Check: the application's domain set to the address.
    AccessDomain {
        aud: String,
        changes: Vec<String>,
        call: CfCall,
    },
    /// Check: the application made again.
    AccessRecreate {
        call: CfCall,
    },
    /// Check: `server.toml` given the application's AUD tag.
    AccessAud {
        aud: String,
        changes: Vec<String>,
    },
    Status {
        st: Box<StatusPlan>,
    },
    /// Start over (revision 17): this computer inspected.
    StartOverHost {
        mem: Mem,
        preview: bool,
    },
    /// Start over: the reads answered (Cloudflare, the capture server).
    StartOverRead {
        mem: Mem,
        preview: bool,
        reads: crate::capture_start_over::Reads,
    },
    /// Start over: the part on this computer done (or not).
    StartOverComputer {
        mem: Mem,
    },
    StatusHost {
        partial: Value,
    },
    Start,
    /// security-review-1 A6: Install the server again.
    Reinstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    CreateTunnel,
    TunnelToken,
    Ingress,
    Dns,
    Org,
    ServiceToken,
    AllowPolicy,
    UpdateAllowPolicy,
    ServicePolicy,
    App,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RStep {
    Uninstall,
    App,
    AllowPolicy,
    ServicePolicy,
    ServiceToken,
    Dns,
    Tunnel,
    ReadIngress,
    WriteIngress,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusPlan {
    public: bool,
    desktop: bool,
    /// The stored collection error and when it happened (U1.9).
    collect_error: Option<(String, Option<i64>)>,
}

pub(crate) fn job(
    c: &Connector,
    op: Op,
    calls: Vec<HttpRequest>,
    host: Option<HostOp>,
) -> CommandResult<NetStep> {
    let host = match host {
        Some(h) => Some((
            c.host.clone().ok_or_else(|| {
                coded(
                    "no_host",
                    "This MoneyKeep front end cannot install or check services; use the desktop app.",
                )
            })?,
            h,
        )),
        None => None,
    };
    Ok(NetStep::Fetch(NetJob::with_host(
        NetOp::CaptureSetup(Box::new(op)),
        calls,
        c.fetcher.clone(),
        host,
    )))
}

pub(crate) fn host_report(out: Option<HostOut>) -> HostReport {
    match out {
        Some(HostOut::Report(r)) => r,
        Some(HostOut::Failed(e)) => HostReport {
            notes: vec![e],
            ..HostReport::default()
        },
        _ => HostReport::default(),
    }
}

/// The address `<subdomain>.<zone>`.
fn fqdn(sub: &str, zone: &str) -> String {
    format!("{sub}.{zone}")
}

/// The result of the plan's reads.
struct Plan {
    hostname: String,
    emails: Vec<String>,
    tunnel: Option<TunnelRec>,
    device: String,
    lan: String,
    origin: String,
    actions: Vec<String>,
    problems: Vec<String>,
    /// Revision 16: the `cloudflared` service here, when it runs a deleted tunnel
    /// (`{state, message, replace, tunnel}`), else null.
    service: Value,
    /// The deleted tunnel whose service Install replaces.
    replace: Option<String>,
}

impl Registry {
    pub(crate) fn setup_spec(&self, name: &str) -> CommandResult<&crate::CommandSpec> {
        self.spec(name)
            .ok_or_else(|| CommandError::UnknownCommand(name.to_owned()))
    }

    /// A short read transaction (module `expenses` on).
    pub(crate) fn setup_read<T>(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        f: impl FnOnce(&Ctx<'_>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let encrypted = store.is_encrypted();
        let connector = self.connector.as_ref();
        store.read(|tx| {
            if !crate::is_module_enabled(tx, MODULE)? {
                return Err(CommandError::ModuleDisabled {
                    command: "capture".into(),
                    module: MODULE.into(),
                });
            }
            let mut ctx = Ctx::new(tx, actor);
            ctx.connector = connector;
            ctx.encrypted = encrypted;
            f(&ctx)
        })
    }

    /// A short audited write of the setup state under `command`.
    fn setup_write(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        command: &str,
        audit: Value,
        f: &dyn Fn(&Ctx<'_>, &mut SetupState) -> CommandResult<()>,
    ) -> CommandResult<SetupState> {
        let spec = self.setup_spec(command)?;
        let v = self.execute(store, actor, spec, audit, None, &|ctx, _| {
            let mut st = load(ctx)?;
            f(ctx, &mut st)?;
            save(ctx, &st)?;
            serde_json::to_value(&st).map_err(|e| CommandError::Integrity(e.to_string()))
        })?;
        serde_json::from_value(v).map_err(|e| CommandError::Integrity(e.to_string()))
    }

    /// Start a setup or status command.
    pub(crate) fn setup_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<NetStep> {
        if name == "capture.status" {
            return self.status_start(store, actor);
        }
        self.setup_read(store, actor, |_| Ok(()))?;
        let c = self.connector.as_ref().ok_or_else(no_network)?;
        match name {
            "capture.setup_check_token" => {
                let a: CheckArgs = crate::parse_args(args)?;
                let (token, from_arg) = match a.token.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) => (token_shape(t)?, true),
                    None => (self.stored_token(store, actor)?, false),
                };
                let reqs = vec![
                    cf(
                        &token,
                        HttpMethod::Get,
                        "/user/tokens/verify",
                        &[],
                        "",
                        None,
                    )
                    .req,
                    cf(
                        &token,
                        HttpMethod::Get,
                        "/accounts",
                        &[],
                        "?per_page=50",
                        None,
                    )
                    .req,
                    cf(&token, HttpMethod::Get, "/zones", &[], "?per_page=50", None).req,
                ];
                job(
                    c,
                    Op::Check {
                        mem: Mem {
                            token,
                            ..Mem::default()
                        },
                        from_arg,
                        account: a.account,
                        zone: a.zone,
                    },
                    reqs,
                    None,
                )
            }
            "capture.setup_plan" | "capture.setup_apply" => {
                let a: PlanArgs = crate::parse_args(args)?;
                let token = self.stored_token(store, actor)?;
                let st = self.setup_read(store, actor, load)?;
                if st.account.is_none() || st.zone.is_none() {
                    return Err(coded(
                        "not_checked",
                        "Check the Cloudflare token first (step 2: Connect to Cloudflare).",
                    ));
                }
                check_subdomain(a.subdomain.as_deref().unwrap_or("capture"))?;
                check_emails(&a.emails)?;
                // Revision 16: this computer first, so the tunnel a cloudflared
                // service runs can be looked up with the plan's reads.
                job(
                    c,
                    Op::PrepareHost {
                        mem: Mem {
                            token,
                            ..Mem::default()
                        },
                        args: a,
                        apply: name == "capture.setup_apply",
                    },
                    Vec::new(),
                    Some(HostOp::Inspect),
                )
            }
            "capture.setup_remove" => {
                let a: RemoveArgs = crate::parse_args(args)?;
                let token = match a.token.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) => token_shape(t)?,
                    None => self.stored_token(store, actor)?,
                };
                let st = self.setup_read(store, actor, load)?;
                self.remove_next(
                    store,
                    actor,
                    c,
                    Mem {
                        token,
                        ..Mem::default()
                    },
                    &st,
                )
            }
            "capture.start_over_preview" | "capture.start_over" => {
                self.start_over_start(store, actor, c, name, args)
            }
            "capture.start_server" => job(c, Op::Start, Vec::new(), Some(HostOp::Start)),
            "capture.reinstall_server" => {
                // A6: the same listener setting as setup chose, and Wi-Fi capture's
                // firewall rule with the ports it was set up with.
                let (lan, w) = self.setup_read(store, actor, |ctx| {
                    Ok((load(ctx)?.device == "lan", crate::capture_wifi::load(ctx)?))
                })?;
                let req = ReinstallRequest {
                    allow_lan: lan,
                    firewall_rule: (w.installed && w.https_port != 0 && w.http_port != 0)
                        .then(|| crate::capture_wifi::firewall_rule(w.https_port, w.http_port)),
                };
                job(c, Op::Reinstall, Vec::new(), Some(HostOp::Reinstall(req)))
            }
            "capture.remove_from_computer" => {
                let a: RemoveFromComputerArgs = crate::parse_args(args)?;
                job(
                    c,
                    Op::RemoveFromComputerCheck {
                        delete_data: a.delete_data,
                    },
                    Vec::new(),
                    Some(HostOp::Inspect),
                )
            }
            "capture.remove_old_copy" => job(
                c,
                Op::RemoveOldCopy,
                Vec::new(),
                Some(HostOp::RemoveOldCopy),
            ),
            "capture.update_server" => {
                let a: UpdateArgs = crate::parse_args(args)?;
                let (conf, st) = self.setup_read(store, actor, |ctx| {
                    Ok((crate::capture::conf(ctx)?, load(ctx)?))
                })?;
                let conf =
                    conf.ok_or_else(|| coded("not_set_up", "Phone capture is not set up."))?;
                let (base, authority, http) = crate::capture::check_url(&conf.url, conf.trusted)?;
                let health = HttpRequest {
                    method: HttpMethod::Get,
                    url: format!("{base}{}", api::HEALTH_PATH),
                    authority,
                    max_bytes: 64 * 1024,
                    allow_http: http,
                    ..HttpRequest::default()
                };
                job(
                    c,
                    Op::UpdateCheck {
                        allow_lan: st.device == "lan",
                        force: a.force,
                    },
                    vec![health],
                    Some(HostOp::Inspect),
                )
            }
            "capture.setup_repair" => {
                let a: RemoveArgs = crate::parse_args(args)?;
                let token = match a.token.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) => token_shape(t)?,
                    None => self.stored_token(store, actor)?,
                };
                let st = self.setup_read(store, actor, load)?;
                let Some(app) = st.app.clone().filter(|_| st.complete) else {
                    return Err(coded(
                        "not_set_up",
                        "Phone capture is not set up with the guided setup, so there is nothing to repair.",
                    ));
                };
                let call = cf(
                    &token,
                    HttpMethod::Put,
                    "/accounts/{account}/access/apps/{app}",
                    &[("account", st.account_id()), ("app", &app)],
                    "",
                    Some(&app_body(&st)),
                );
                let req = call.req.clone();
                job(c, Op::Repair { call }, vec![req], None)
            }
            "capture.check_access" => {
                let a: AccessArgs = crate::parse_args(args)?;
                let token = match a.token.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) => token_shape(t)?,
                    None => self.stored_token(store, actor)?,
                };
                let st = self.setup_read(store, actor, load)?;
                let Some(app) = st.app.clone().filter(|_| st.complete) else {
                    return Err(coded(
                        "not_set_up",
                        "Phone capture is not set up with the guided setup, so there is nothing to check.",
                    ));
                };
                let acc = [("account", st.account_id()), ("app", app.as_str())];
                if a.recreate {
                    let call = cf(
                        &token,
                        HttpMethod::Post,
                        "/accounts/{account}/access/apps",
                        &acc[..1],
                        "",
                        Some(&app_body(&st)),
                    );
                    let req = call.req.clone();
                    return job(c, Op::AccessRecreate { call }, vec![req], None);
                }
                let call = cf(
                    &token,
                    HttpMethod::Get,
                    "/accounts/{account}/access/apps/{app}",
                    &acc,
                    "",
                    None,
                );
                let req = call.req.clone();
                job(
                    c,
                    Op::AccessRead {
                        mem: Mem {
                            token,
                            ..Mem::default()
                        },
                        call,
                    },
                    vec![req],
                    None,
                )
            }
            other => Err(CommandError::UnknownCommand(other.to_owned())),
        }
    }

    /// Check, after the Access application's address is right (revision 8): record
    /// its AUD tag when it differs and have the host bring `server.toml` in line
    /// (which restarts the server only when the file changes).
    fn access_aud(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        aud: String,
        mut changes: Vec<String>,
    ) -> CommandResult<NetStep> {
        let st = self.setup_read(store, actor, load)?;
        if !aud.is_empty() && aud != st.aud {
            let line = format!(
                "AUD tag recorded: {aud} (was {})",
                if st.aud.is_empty() {
                    "none"
                } else {
                    st.aud.as_str()
                }
            );
            let d = Detail {
                method: "RECORD".into(),
                path: "access_aud".into(),
                result: line.clone(),
            };
            self.setup_write(
                store,
                actor,
                "capture.check_access",
                json!({"aud": aud}),
                &|_, s| {
                    s.aud.clone_from(&aud);
                    s.detail(d.clone());
                    Ok(())
                },
            )?;
            changes.push(line);
        }
        if !aud.is_empty() && st.installed_server && c.host.is_some() {
            let update = UpdateRequest {
                allow_lan: st.device == "lan",
            };
            return job(
                c,
                Op::AccessAud {
                    aud: aud.clone(),
                    changes,
                },
                Vec::new(),
                Some(HostOp::SetAud(aud, update)),
            );
        }
        Ok(NetStep::Done(access_done(&st.hostname, changes)))
    }

    fn stored_token(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
    ) -> CommandResult<String> {
        self.setup_read(store, actor, |ctx| {
            let st = load(ctx)?;
            let t = match &st.token_ref {
                Some(r) => get_secret(ctx, r)?,
                None => None,
            };
            t.ok_or_else(|| {
                coded(
                    "token_needed",
                    "A Cloudflare API token is needed for this change: create one and paste it.",
                )
            })
        })
    }

    /// Continue a setup or status command.
    pub(crate) fn setup_resume(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        op: Op,
        responses: Vec<Result<HttpResponse, FetchError>>,
        host_out: Option<HostOut>,
    ) -> CommandResult<NetStep> {
        match op {
            Op::Check {
                mem,
                from_arg,
                account,
                zone,
            } => {
                let verify = cf_answer(responses.first());
                if verify.status == 401
                    || verify.status == 403
                    || (verify.status == 0 && !verify.ok)
                {
                    return Err(coded(
                        "bad_cloudflare_token",
                        format!("Cloudflare did not accept this token: {}", verify.message),
                    ));
                }
                let accounts_a = cf_answer(responses.get(1));
                let zones_a = cf_answer(responses.get(2));
                let mut accounts: Vec<Named> = list_of(&accounts_a.result)
                    .iter()
                    .map(|a| Named {
                        id: str_of(a, "id").to_owned(),
                        name: str_of(a, "name").to_owned(),
                    })
                    .filter(|a| !a.id.is_empty())
                    .collect();
                let zone_list = list_of(&zones_a.result);
                let zones: Vec<(Named, String)> = zone_list
                    .iter()
                    .map(|z| {
                        (
                            Named {
                                id: str_of(z, "id").to_owned(),
                                name: str_of(z, "name").to_owned(),
                            },
                            z.get("account")
                                .map(|a| str_of(a, "id").to_owned())
                                .unwrap_or_default(),
                        )
                    })
                    .filter(|z| !z.0.id.is_empty())
                    .collect();
                // app-desktop.md revision 30: `GET /accounts` can be empty for a token
                // without account-settings read access; zones carry their account.
                let from_accounts = accounts.len();
                for z in &zone_list {
                    if let Some(a) = z.get("account") {
                        let id = str_of(a, "id");
                        if !id.is_empty() && !accounts.iter().any(|x| x.id == id) {
                            accounts.push(Named {
                                id: id.to_owned(),
                                name: str_of(a, "name").to_owned(),
                            });
                        }
                    }
                }
                let count = |a: &CfAnswer, n: usize, one: &str, many: &str| {
                    let what = if n == 1 { one } else { many };
                    if a.ok {
                        format!("ok: {n} {what}")
                    } else if a.status == 0 {
                        format!("failed: {}", a.message)
                    } else {
                        format!("HTTP {}: {}", a.status, a.message)
                    }
                };
                let details = vec![
                    Detail {
                        method: "GET".into(),
                        path: "/accounts".into(),
                        result: count(&accounts_a, from_accounts, "account", "accounts"),
                    },
                    Detail {
                        method: "GET".into(),
                        path: "/zones".into(),
                        result: count(&zones_a, zones.len(), "zone", "zones"),
                    },
                ];
                let Some(chosen) = account
                    .as_deref()
                    .and_then(|id| accounts.iter().find(|a| a.id == id))
                    .or_else(|| accounts.first())
                    .cloned()
                else {
                    self.setup_write(
                        store,
                        actor,
                        "capture.setup_check_token",
                        json!({"token": "(hidden)", "accounts": 0}),
                        &|_, st| {
                            for d in &details {
                                st.detail(d.clone());
                            }
                            Ok(())
                        },
                    )?;
                    return Err(coded("cloudflare", NO_ACCOUNT));
                };
                let in_account: Vec<&Named> = zones
                    .iter()
                    .filter(|(_, acc)| acc.is_empty() || *acc == chosen.id)
                    .map(|(z, _)| z)
                    .collect();
                let zone_pick = zone
                    .as_deref()
                    .and_then(|id| in_account.iter().find(|z| z.id == id).copied())
                    .or_else(|| in_account.first().copied())
                    .cloned();
                let t = mem.token.clone();
                let acc = [("account", chosen.id.as_str())];
                let mut reqs = vec![
                    cf(
                        &t,
                        HttpMethod::Get,
                        "/accounts/{account}/cfd_tunnel",
                        &acc,
                        "?is_deleted=false&per_page=50",
                        None,
                    )
                    .req,
                    cf(
                        &t,
                        HttpMethod::Get,
                        "/accounts/{account}/access/apps",
                        &acc,
                        "",
                        None,
                    )
                    .req,
                    cf(
                        &t,
                        HttpMethod::Get,
                        "/accounts/{account}/access/service_tokens",
                        &acc,
                        "",
                        None,
                    )
                    .req,
                    cf(
                        &t,
                        HttpMethod::Get,
                        "/accounts/{account}/access/organizations",
                        &acc,
                        "",
                        None,
                    )
                    .req,
                ];
                if let Some(z) = &zone_pick {
                    reqs.push(
                        cf(
                            &t,
                            HttpMethod::Get,
                            "/zones/{zone}/dns_records",
                            &[("zone", z.id.as_str())],
                            "?per_page=5",
                            None,
                        )
                        .req,
                    );
                }
                job(
                    c,
                    Op::Probe {
                        mem,
                        from_arg,
                        accounts,
                        zones,
                        account: chosen,
                        zone: zone_pick,
                        details,
                        verify: if verify.ok {
                            "active".into()
                        } else {
                            verify.message
                        },
                    },
                    reqs,
                    Some(HostOp::Inspect),
                )
            }
            Op::Probe {
                mem,
                from_arg,
                accounts,
                zones,
                account,
                zone,
                verify,
                details,
            } => {
                let host = host_report(host_out);
                let a = |i: usize| cf_answer(responses.get(i));
                let mut missing: Vec<&str> = Vec::new();
                let tunnels_a = a(0);
                if !tunnels_a.ok {
                    missing.push(PERM_TUNNEL);
                }
                if !a(1).ok {
                    missing.push(PERM_ACCESS_APPS);
                }
                if !a(2).ok {
                    missing.push(PERM_SERVICE_TOKENS);
                }
                let org = a(3);
                if !org.ok {
                    missing.push(PERM_ORGANIZATIONS);
                }
                match &zone {
                    Some(_) => {
                        if !a(4).ok {
                            missing.push(PERM_DNS);
                        }
                    }
                    None => {
                        missing.push(PERM_ZONE);
                        missing.push(PERM_DNS);
                    }
                }
                let tunnels: Vec<Value> = list_of(&tunnels_a.result)
                    .iter()
                    .map(|t| {
                        json!({"id": str_of(t, "id"), "name": str_of(t, "name"),
                               "status": str_of(t, "status"),
                               "remote_config": t.get("remote_config").and_then(Value::as_bool).unwrap_or(false)})
                    })
                    .collect();
                let team = str_of(&org.result, "auth_domain").to_owned();
                let token = mem.token.clone();
                let (acc2, zone2) = (account.clone(), zone.clone());
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.setup_check_token",
                    json!({"token": "(hidden)", "account": account.id, "zone": zone.as_ref().map(|z| z.id.clone())}),
                    &|ctx, st| {
                        if from_arg {
                            let book = ctx.tx.info()?.id;
                            if let Some(old) = st.token_ref.take() {
                                delete_secret(ctx, &old)?;
                            }
                            st.token_ref = Some(put_secret(ctx, &format!("{book}:cloudflare-api"), &token)?);
                        }
                        for d in &details {
                            st.detail(d.clone());
                        }
                        st.account = Some(acc2.clone());
                        st.zone = zone2.clone();
                        if !team.is_empty() {
                            st.team_domain.clone_from(&team);
                        }
                        Ok(())
                    },
                )?;
                let here = host.cloudflared_service.is_some() && !st.installed_cloudflared;
                // Revision 16: suggest the tunnel the service runs when it is listed;
                // a service whose tunnel is gone suggests a new tunnel (Install then
                // offers to replace it).
                let runs = host.cloudflared_tunnel.as_deref().filter(|t| !t.is_empty());
                let other_account = host
                    .cloudflared_account
                    .as_deref()
                    .is_some_and(|a| !a.is_empty() && a != account.id);
                let listed = runs
                    .filter(|id| !other_account && tunnels.iter().any(|t| str_of(t, "id") == *id));
                let suggest = match (here, runs, listed) {
                    (false, _, _) | (true, Some(_), None) => "new",
                    _ => "existing",
                };
                Ok(NetStep::Done(json!({
                    "suggest_tunnel": listed,
                    "ok": missing.is_empty(),
                    "verify": verify,
                    "accounts": accounts,
                    "zones": zones.iter().filter(|(_, acc)| acc.is_empty() || *acc == account.id).map(|(z, _)| z).collect::<Vec<_>>(),
                    "account": account,
                    "zone": zone,
                    "missing": missing,
                    "tunnels": tunnels,
                    "team_domain": st.team_domain,
                    "host": host,
                    "suggest": suggest,
                    "token_stored": st.token_ref.is_some(),
                })))
            }
            Op::PrepareHost {
                mut mem,
                args,
                apply,
            } => {
                mem.host = host_report(host_out);
                let st = self.setup_read(store, actor, load)?;
                let reqs = plan_reads(&st, &args, &mem.token, &mem.host)?;
                job(c, Op::Prepare { mem, args, apply }, reqs, None)
            }
            Op::Prepare {
                mut mem,
                args,
                apply,
            } => {
                if host_out.is_some() {
                    mem.host = host_report(host_out);
                }
                let st = self.setup_read(store, actor, load)?;
                let plan = plan_of(&st, &args, &responses, &mut mem)?;
                if !apply {
                    return Ok(NetStep::Done(json!({
                        "address": plan.hostname,
                        "url": format!("https://{}", plan.hostname),
                        "actions": plan.actions,
                        "problems": plan.problems,
                        "service": plan.service,
                        "host": mem.host,
                    })));
                }
                if !plan.problems.is_empty() {
                    return Err(coded("setup_blocked", plan.problems.join(" ")));
                }
                if st.complete && !st.hostname.is_empty() && st.hostname != plan.hostname {
                    return Err(coded(
                        "address_change",
                        "To use a different address, remove phone capture first (Settings > Phone capture > Remove phone capture), then set it up again.",
                    ));
                }
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.setup_apply",
                    json!({"address": plan.hostname, "emails": plan.emails, "tunnel": plan.tunnel.as_ref().map(|t| t.id.clone()), "device": plan.device}),
                    &|_, st| {
                        st.hostname.clone_from(&plan.hostname);
                        st.emails.clone_from(&plan.emails);
                        st.device.clone_from(&plan.device);
                        st.lan_address.clone_from(&plan.lan);
                        if st.origin != plan.origin {
                            st.ingress = false;
                        }
                        st.origin.clone_from(&plan.origin);
                        st.replace_service.clone_from(&plan.replace);
                        if let Some(t) = &plan.tunnel {
                            if st.tunnel.as_ref().is_none_or(|old| old.id != t.id) {
                                st.tunnel = Some(t.clone());
                                st.ingress = false;
                            }
                        } else if st.tunnel.as_ref().is_some_and(|t| !t.created) {
                            st.tunnel = None;
                            st.ingress = false;
                        }
                        st.complete = false;
                        Ok(())
                    },
                )?;
                self.setup_next(store, actor, c, mem, &st)
            }
            Op::Step {
                mut mem,
                step,
                call,
            } => {
                let answer = call.as_ref().map(|_| cf_answer(responses.first()));
                if let (Some(a), Some(call)) = (&answer, &call)
                    && !a.ok
                {
                    let d = a.detail(call);
                    self.setup_write(
                        store,
                        actor,
                        "capture.setup_apply",
                        json!({"step": format!("{step:?}"), "failed": true}),
                        &|_, st| {
                            st.detail(d.clone());
                            Ok(())
                        },
                    )?;
                    return Err(a.error(call));
                }
                let detail = match (&answer, &call) {
                    (Some(a), Some(call)) => Some(a.detail(call)),
                    _ => None,
                };
                let result = answer.map(|a| a.result).unwrap_or(Value::Null);
                if step == Step::TunnelToken || step == Step::CreateTunnel {
                    let token = match &result {
                        Value::String(s) => s.clone(),
                        v => str_of(v, "token").to_owned(),
                    };
                    if !token.is_empty() {
                        mem.tunnel_token = Some(token);
                    }
                }
                let installed = match (step, host_out) {
                    (Step::Install, Some(HostOut::Installed(o))) => Some(o),
                    (Step::Install, Some(HostOut::Failed(e))) => {
                        let d = Detail {
                            method: "INSTALL".into(),
                            path: "this computer".into(),
                            result: format!("failed: {e}"),
                        };
                        self.setup_write(
                            store,
                            actor,
                            "capture.setup_apply",
                            json!({"step": "Install", "failed": true}),
                            &|_, st| {
                                st.detail(d.clone());
                                Ok(())
                            },
                        )?;
                        return Err(coded(
                            "install_failed",
                            format!("Installing on this computer failed: {e}"),
                        ));
                    }
                    (Step::Install, _) => {
                        return Err(coded(
                            "install_failed",
                            "Installing on this computer did not finish.",
                        ));
                    }
                    _ => None,
                };
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.setup_apply",
                    json!({"step": format!("{step:?}")}),
                    &|ctx, st| {
                        if let Some(d) = &detail {
                            st.detail(d.clone());
                        }
                        absorb(ctx, st, step, &result, installed.as_ref())?;
                        Ok(())
                    },
                )?;
                self.setup_next(store, actor, c, mem, &st)
            }
            Op::Remove {
                mut mem,
                step,
                call,
            } => {
                if step == RStep::Uninstall {
                    match host_out {
                        Some(HostOut::Removed(done)) => {
                            let d = Detail {
                                method: "UNINSTALL".into(),
                                path: "this computer".into(),
                                result: if done.is_empty() {
                                    "ok".into()
                                } else {
                                    done.join("; ")
                                },
                            };
                            let st = self.setup_write(
                                store,
                                actor,
                                "capture.setup_remove",
                                json!({"step": "Uninstall"}),
                                &|_, st| {
                                    st.detail(d.clone());
                                    st.installed_server = false;
                                    st.installed_cloudflared = false;
                                    Ok(())
                                },
                            )?;
                            return self.remove_next(store, actor, c, mem, &st);
                        }
                        // Revision 16: a failed step is listed and the others go on.
                        other => {
                            let reason = match other {
                                Some(HostOut::Failed(e)) => e,
                                _ => "removing the services did not finish".to_owned(),
                            };
                            let st =
                                self.undo_failed(store, actor, &mut mem, step, reason, None)?;
                            return self.remove_next(store, actor, c, mem, &st);
                        }
                    }
                }
                let Some(call) = call else {
                    return Err(CommandError::Integrity(
                        "a removal step without a request".into(),
                    ));
                };
                let a = cf_answer(responses.first());
                // Already gone at Cloudflare counts as removed.
                let gone = a.status == 404;
                if !a.ok && !gone {
                    // Revision 16: listed with its reason; the other steps go on.
                    let reason = format!(
                        "Cloudflare: {} ({} {})",
                        a.message, call.method, call.template
                    );
                    let st = self.undo_failed(
                        store,
                        actor,
                        &mut mem,
                        step,
                        reason,
                        Some(a.detail(&call)),
                    )?;
                    return self.remove_next(store, actor, c, mem, &st);
                }
                if step == RStep::ReadIngress && !gone {
                    let st = self.setup_read(store, actor, load)?;
                    let config = a.result.get("config").cloned().unwrap_or_else(|| json!({}));
                    let next = without_host(&config, &st.hostname);
                    let t = mem.token.clone();
                    let vars = [("account", st.account_id()), ("tunnel", st.tunnel_id())];
                    let body = json!({"config": next});
                    let call = cf(
                        &t,
                        HttpMethod::Put,
                        "/accounts/{account}/cfd_tunnel/{tunnel}/configurations",
                        &vars,
                        "",
                        Some(&body),
                    );
                    let req = call.req.clone();
                    return job(
                        c,
                        Op::Remove {
                            mem,
                            step: RStep::WriteIngress,
                            call: Some(call),
                        },
                        vec![req],
                        None,
                    );
                }
                let d = a.detail(&call);
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.setup_remove",
                    json!({"step": format!("{step:?}")}),
                    &|ctx, st| {
                        st.detail(d.clone());
                        match step {
                            RStep::App => st.app = None,
                            RStep::AllowPolicy => {
                                st.allow_policy = None;
                                st.policy_emails.clear();
                            }
                            RStep::ServicePolicy => st.service_policy = None,
                            RStep::ServiceToken => {
                                if let Some(r) = st.service_token.take().and_then(|s| s.secret_ref)
                                {
                                    delete_secret(ctx, &r)?;
                                }
                            }
                            RStep::Dns => st.dns_record = None,
                            RStep::Tunnel => {
                                st.tunnel = None;
                                st.ingress = false;
                            }
                            // A tunnel already gone has no rule left either.
                            RStep::WriteIngress | RStep::ReadIngress => st.ingress = false,
                            RStep::Uninstall => {}
                        }
                        Ok(())
                    },
                )?;
                self.remove_next(store, actor, c, mem, &st)
            }
            Op::UpdateCheck { allow_lan, force } => {
                let host = host_report(host_out);
                if !host.server_program {
                    return Err(coded(
                        "build_first",
                        format!(
                            "The new server program was not found. {}",
                            text::BUILD_FIRST
                        ),
                    ));
                }
                if !host.program_has_app {
                    return Err(coded(
                        "build_first",
                        format!(
                            "The server program has no phone app built in. {}",
                            text::BUILD_FIRST
                        ),
                    ));
                }
                let running = match responses.first() {
                    Some(Ok(r)) if r.status == 200 => {
                        serde_json::from_slice::<api::Health>(&r.body).ok()
                    }
                    _ => None,
                };
                // An installation under the old name is always moved (branding.md
                // step 6), whichever build it runs.
                let running = running.filter(|_| !host.old_name);
                let by_capture = running.as_ref().and_then(|h| {
                    h.capture_version
                        .as_deref()
                        .filter(|_| !host.program_capture_version.is_empty())
                });
                if let (Some(h), Some(rc)) = (&running, by_capture) {
                    // release-packaging.md: the same sources are already running;
                    // different sources go ahead unless the running build is newer.
                    if rc == host.program_capture_version {
                        return Err(coded("already_this_build", text::ALREADY_THIS_BUILD));
                    }
                    if api::compare_versions(&host.program_version, &h.version)
                        == Some(std::cmp::Ordering::Less)
                    {
                        return Err(coded(
                            "running_is_newer",
                            format!(
                                "{} (running {}, built {})",
                                text::RUNNING_IS_NEWER,
                                h.version,
                                host.program_version
                            ),
                        ));
                    }
                } else if let Some(v) = running.as_ref().map(|h| &h.version) {
                    // Revision 4a (servers without a capture version): newer goes
                    // ahead; equal or older is refused; undecidable asks first.
                    match api::compare_versions(&host.program_version, v) {
                        Some(std::cmp::Ordering::Greater) => {}
                        Some(std::cmp::Ordering::Equal) => {
                            return Err(coded("already_this_build", text::ALREADY_THIS_BUILD));
                        }
                        Some(std::cmp::Ordering::Less) => {
                            return Err(coded(
                                "running_is_newer",
                                format!(
                                    "{} (running {v}, built {})",
                                    text::RUNNING_IS_NEWER,
                                    host.program_version
                                ),
                            ));
                        }
                        None if force => {}
                        None => return Err(coded("confirm_needed", text::UPDATE_ANYWAY)),
                    }
                }
                job(
                    c,
                    Op::UpdateDone {
                        version: host.program_version,
                    },
                    Vec::new(),
                    Some(HostOp::Update(UpdateRequest { allow_lan })),
                )
            }
            Op::UpdateDone { version } => {
                let (result, actions) = match host_out {
                    Some(HostOut::Removed(done)) => (Ok(()), done.join("; ")),
                    Some(HostOut::Failed(e)) => (Err(e.clone()), format!("failed: {e}")),
                    _ => (
                        Err("the update did not finish".to_owned()),
                        "failed".to_owned(),
                    ),
                };
                let d = Detail {
                    method: "UPDATE".into(),
                    path: "this computer".into(),
                    result: if actions.is_empty() {
                        "ok".into()
                    } else {
                        actions
                    },
                };
                self.setup_write(
                    store,
                    actor,
                    "capture.update_server",
                    json!({"version": version}),
                    &|_, st| {
                        st.detail(d.clone());
                        Ok(())
                    },
                )?;
                match result {
                    Ok(()) => Ok(NetStep::Done(
                        json!({"updated": true, "version": version, "text": text::updated_to(&version)}),
                    )),
                    // release-packaging.md: the permission prompt was declined.
                    Err(e) if e == PERMISSION_DECLINED => {
                        Err(coded("permission_declined", text::NOT_UPDATED))
                    }
                    Err(e) => Err(coded(
                        "update_failed",
                        format!("Updating the capture server failed: {e}"),
                    )),
                }
            }
            Op::Repair { call } => {
                let a = cf_answer(responses.first());
                let d = a.detail(&call);
                self.setup_write(
                    store,
                    actor,
                    "capture.setup_repair",
                    json!({"session_duration": SESSION_DURATION}),
                    &|_, st| {
                        st.detail(d.clone());
                        Ok(())
                    },
                )?;
                if !a.ok {
                    return Err(a.error(&call));
                }
                Ok(NetStep::Done(json!({
                    "repaired": true,
                    "session_duration": SESSION_DURATION,
                })))
            }
            Op::AccessRead { mem, call } => {
                let a = cf_answer(responses.first());
                let d = a.detail(&call);
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.check_access",
                    json!({"read": "access application"}),
                    &|_, st| {
                        st.detail(d.clone());
                        Ok(())
                    },
                )?;
                if a.status == 404 {
                    let text = format!(
                        "The Access application for {} no longer exists at Cloudflare. Create it again?",
                        st.hostname
                    );
                    return Ok(NetStep::Done(json!({
                        "ok": false,
                        "app_missing": true,
                        "text": text,
                        "changes": [],
                    })));
                }
                if !a.ok {
                    return Err(a.error(&call));
                }
                let aud = str_of(&a.result, "aud").to_owned();
                let domain = str_of(&a.result, "domain").trim().to_ascii_lowercase();
                if !st.hostname.is_empty() && domain != st.hostname.to_ascii_lowercase() {
                    // The address, the other fields as Cloudflare has them.
                    let mut body = a.result.clone();
                    if let Some(o) = body.as_object_mut() {
                        for k in ["id", "aud", "created_at", "updated_at"] {
                            o.remove(k);
                        }
                        o.insert("domain".into(), json!(st.hostname));
                        if o.contains_key("self_hosted_domains") {
                            o.insert("self_hosted_domains".into(), json!([st.hostname]));
                        }
                    }
                    let app = st.app.clone().unwrap_or_default();
                    let put = cf(
                        &mem.token,
                        HttpMethod::Put,
                        "/accounts/{account}/access/apps/{app}",
                        &[("account", st.account_id()), ("app", &app)],
                        "",
                        Some(&body),
                    );
                    let req = put.req.clone();
                    let was = if domain.is_empty() {
                        "none".to_owned()
                    } else {
                        domain
                    };
                    return job(
                        c,
                        Op::AccessDomain {
                            aud,
                            changes: vec![format!(
                                "Access application address set to {} (was {was})",
                                st.hostname
                            )],
                            call: put,
                        },
                        vec![req],
                        None,
                    );
                }
                self.access_aud(store, actor, c, aud, Vec::new())
            }
            Op::AccessDomain { aud, changes, call } => {
                let a = cf_answer(responses.first());
                let d = a.detail(&call);
                self.setup_write(
                    store,
                    actor,
                    "capture.check_access",
                    json!({"domain": "set"}),
                    &|_, st| {
                        st.detail(d.clone());
                        Ok(())
                    },
                )?;
                if !a.ok {
                    return Err(a.error(&call));
                }
                self.access_aud(store, actor, c, aud, changes)
            }
            Op::AccessRecreate { call } => {
                let a = cf_answer(responses.first());
                let d = a.detail(&call);
                let id = str_of(&a.result, "id").to_owned();
                let ok = a.ok && !id.is_empty();
                let st = self.setup_write(
                    store,
                    actor,
                    "capture.check_access",
                    json!({"recreate": true}),
                    &|_, st| {
                        st.detail(d.clone());
                        if ok {
                            st.app = Some(id.clone());
                        }
                        Ok(())
                    },
                )?;
                if !ok {
                    return Err(a.error(&call));
                }
                let aud = str_of(&a.result, "aud").to_owned();
                let changes = vec![format!(
                    "Access application created again for {}",
                    st.hostname
                )];
                self.access_aud(store, actor, c, aud, changes)
            }
            Op::AccessAud { aud, mut changes } => {
                let (changed, done) = match host_out {
                    Some(HostOut::AudSet(changed, done)) => (changed, done),
                    Some(HostOut::Failed(e)) => {
                        return Err(coded(
                            "check_failed",
                            format!("The capture server's settings could not be changed: {e}"),
                        ));
                    }
                    _ => {
                        return Err(coded(
                            "check_failed",
                            "The capture server's settings could not be checked.",
                        ));
                    }
                };
                let st = if changed {
                    let line = format!(
                        "server.toml access_aud set to {aud}; the capture server was restarted"
                    );
                    let d = Detail {
                        method: "CONFIGURE".into(),
                        path: "server.toml".into(),
                        result: if done.is_empty() {
                            line.clone()
                        } else {
                            format!("{line} ({})", done.join("; "))
                        },
                    };
                    changes.push(line);
                    self.setup_write(
                        store,
                        actor,
                        "capture.check_access",
                        json!({"server_aud": aud}),
                        &|_, st| {
                            st.detail(d.clone());
                            Ok(())
                        },
                    )?
                } else {
                    self.setup_read(store, actor, load)?
                };
                Ok(NetStep::Done(access_done(&st.hostname, changes)))
            }
            Op::Status { st } => self.status_resume(store, actor, c, *st, &responses),
            Op::StatusHost { partial, .. } => {
                let host = host_report(host_out);
                let mut v = partial;
                if let Some(o) = v.as_object_mut() {
                    // release-packaging.md: Update now only when the program beside
                    // the app came from the same build (its capture version is the
                    // desktop's, with the phone app built in).
                    let ready = o.get("update_needed").and_then(Value::as_bool) == Some(true)
                        && host.server_program
                        && host.program_has_app
                        && host.program_capture_version == api::CAPTURE_VERSION;
                    o.insert("update_ready".into(), json!(ready));
                    o.insert("os".into(), json!(host.os));
                    // upgrade-resilience-1 U1.4: the server refused to start; its
                    // reason leads, and starting it again would only refuse again.
                    let refused = host.server != ServiceState::Running
                        && host.refusal.is_some()
                        && o.get("state").and_then(Value::as_str) == Some("red");
                    o.insert(
                        "can_start".into(),
                        json!(host.server == ServiceState::Stopped && !host.old_name && !refused),
                    );
                    if refused && let Some(why) = &host.refusal {
                        o.insert("text".into(), json!(text::REFUSED));
                        o.insert(
                            "accessible_name".into(),
                            json!(format!("Phone capture status: {}", text::REFUSED)),
                        );
                        o.insert("refusal".into(), json!(why));
                        o.insert("refusal_kind".into(), json!(host.refusal_kind));
                        if let Some(Value::Array(p)) = o.get_mut("problems") {
                            p.retain(|x| x.as_str() != Some(text::RED));
                            p.insert(0, json!(text::REFUSED));
                            p.insert(1, json!(why));
                        }
                    }
                    o.insert("server_service".into(), json!(host.server));
                    o.insert("old_name".into(), json!(host.old_name));
                    // security-review-1 A6: the service is gone but its data is
                    // here: "the capture server isn't installed", with Install the
                    // server again.
                    let reinstall = !refused
                        && host.server == ServiceState::NotInstalled
                        && host.data_found
                        && !host.old_name
                        && o.get("state").and_then(Value::as_str) == Some("red");
                    o.insert("can_reinstall".into(), json!(reinstall));
                    if reinstall {
                        o.insert("text".into(), json!(text::NOT_INSTALLED));
                        o.insert(
                            "accessible_name".into(),
                            json!(format!("Phone capture status: {}", text::NOT_INSTALLED)),
                        );
                        if let Some(Value::Array(p)) = o.get_mut("problems") {
                            p.retain(|x| x.as_str() != Some(text::RED));
                            p.insert(0, json!(text::NOT_INSTALLED));
                        }
                    }
                    // security-review-1 A1, upgrade-resilience-1 U1.3: "Remove the
                    // old copy" while the old folder exists: on the Status card for
                    // 30 days, then under Clean up (the help menu).
                    if let Some(oc) = &host.old_copy {
                        let until = oc.moved_at.saturating_add(OLD_COPY_DAYS * 86_400);
                        o.insert(
                            "old_copy".into(),
                            json!({"path": oc.path, "until": until, "on_card": c.now() < until}),
                        );
                    }
                    if host.old_name {
                        // branding.md step 6: first among the problems; the light
                        // is yellow unless the server is down.
                        if let Some(Value::Array(p)) = o.get_mut("problems") {
                            p.insert(0, json!(text::OLD_NAME));
                        }
                        if o.get("state").and_then(Value::as_str) != Some("red") {
                            o.insert("state".into(), json!("yellow"));
                            o.insert("text".into(), json!(text::OLD_NAME));
                            o.insert(
                                "accessible_name".into(),
                                json!(format!("Phone capture status: {}", text::OLD_NAME)),
                            );
                        }
                        o.insert("can_update".into(), json!(true));
                    }
                }
                Ok(NetStep::Done(v))
            }
            Op::RemoveFromComputerCheck { delete_data } => {
                let host = host_report(host_out);
                if host.os != "macos" {
                    return Err(coded("not_mac", text::MAC_ONLY));
                }
                // C8: the agents MoneyKeep made (cloudflared's is MoneyKeep's own
                // on a Mac); the data folder only when asked.
                let req = StartOverRequest {
                    server: true,
                    cloudflared: host.cloudflared_service.is_some(),
                    cloudflared_tunnel: None,
                    firewall_rule: None,
                    data_folder: delete_data,
                };
                job(
                    c,
                    Op::RemovedFromComputer { delete_data },
                    Vec::new(),
                    Some(HostOp::StartOver(req)),
                )
            }
            Op::RemovedFromComputer { delete_data } => match host_out {
                Some(HostOut::Removed(done)) => Ok(NetStep::Done(json!({
                    "removed": true,
                    "text": text::MAC_REMOVED,
                    "data_deleted": delete_data,
                    "actions": done,
                }))),
                Some(HostOut::Failed(e)) => Err(coded(
                    "remove_failed",
                    format!("Phone capture could not be removed from this Mac: {e}"),
                )),
                _ => Err(coded(
                    "remove_failed",
                    "Phone capture could not be removed from this Mac.",
                )),
            },
            Op::RemoveOldCopy => match host_out {
                Some(HostOut::Removed(done)) => {
                    Ok(NetStep::Done(json!({"removed": true, "actions": done})))
                }
                Some(HostOut::Failed(e)) => Err(coded("remove_failed", e)),
                _ => Err(coded("remove_failed", "The old copy could not be removed.")),
            },
            Op::Reinstall => match host_out {
                Some(HostOut::Removed(done)) => Ok(NetStep::Done(
                    json!({"installed": true, "text": text::REINSTALLED, "actions": done}),
                )),
                Some(HostOut::Failed(e)) if e == PERMISSION_DECLINED => {
                    Err(coded("permission_declined", text::REINSTALL_DECLINED))
                }
                Some(HostOut::Failed(e)) => Err(coded(
                    "reinstall_failed",
                    format!("The capture server could not be installed again: {e}"),
                )),
                _ => Err(coded(
                    "reinstall_failed",
                    "The capture server could not be installed again.",
                )),
            },
            Op::Start => match host_out {
                Some(HostOut::Started) => Ok(NetStep::Done(json!({"started": true}))),
                Some(HostOut::Failed(e)) => Err(coded(
                    "start_failed",
                    format!("The capture server could not be started: {e}"),
                )),
                _ => Err(coded(
                    "start_failed",
                    "The capture server could not be started.",
                )),
            },
            Op::StartOverHost { mem, preview } => {
                self.start_over_reads(store, actor, c, mem, preview, host_report(host_out))
            }
            Op::StartOverRead {
                mem,
                preview,
                reads,
            } => self.start_over_assessed(store, actor, c, mem, preview, reads, &responses),
            Op::StartOverComputer { mem } => {
                let outcome = match host_out {
                    Some(HostOut::Removed(done)) => Ok(done),
                    Some(HostOut::Failed(e)) => Err(e),
                    _ => Err("the part on this computer did not finish".to_owned()),
                };
                self.start_over_finish(store, actor, mem, outcome)
            }
        }
    }

    /// The next setup step for what is still missing, or the end.
    fn setup_next(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        mem: Mem,
        st: &SetupState,
    ) -> CommandResult<NetStep> {
        let t = mem.token.clone();
        let acc = st.account_id().to_owned();
        let step_call = |step: Step, call: CfCall, mem: Mem| -> CommandResult<NetStep> {
            let req = call.req.clone();
            job(
                c,
                Op::Step {
                    mem,
                    step,
                    call: Some(call),
                },
                vec![req],
                None,
            )
        };
        let new_tunnel = st.tunnel.as_ref().is_none_or(|t| t.created);
        if st.tunnel.is_none() {
            let body = json!({"name": TUNNEL_NAME, "config_src": "cloudflare"});
            return step_call(
                Step::CreateTunnel,
                cf(
                    &t,
                    HttpMethod::Post,
                    "/accounts/{account}/cfd_tunnel",
                    &[("account", &acc)],
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        let tid = st.tunnel_id().to_owned();
        let vars = [("account", acc.as_str()), ("tunnel", tid.as_str())];
        if new_tunnel && !st.installed_cloudflared && mem.tunnel_token.is_none() {
            return step_call(
                Step::TunnelToken,
                cf(
                    &t,
                    HttpMethod::Get,
                    "/accounts/{account}/cfd_tunnel/{tunnel}/token",
                    &vars,
                    "",
                    None,
                ),
                mem,
            );
        }
        if !st.ingress {
            let base = if new_tunnel {
                json!({})
            } else {
                mem.config.clone().unwrap_or_else(|| json!({}))
            };
            let config = with_host(&base, &st.hostname, &st.origin);
            let body = json!({"config": config});
            return step_call(
                Step::Ingress,
                cf(
                    &t,
                    HttpMethod::Put,
                    "/accounts/{account}/cfd_tunnel/{tunnel}/configurations",
                    &vars,
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        if st.dns_record.is_none() {
            if let Some(id) = mem.dns_ours.clone() {
                let st2 = self.setup_write(
                    store,
                    actor,
                    "capture.setup_apply",
                    json!({"step": "Dns", "reused": true}),
                    &|_, s| {
                        s.dns_record = Some(id.clone());
                        Ok(())
                    },
                )?;
                return self.setup_next(store, actor, c, mem, &st2);
            }
            let body = json!({"type": "CNAME", "name": st.hostname, "content": format!("{tid}.cfargotunnel.com"),
                              "proxied": true, "comment": DNS_COMMENT});
            return step_call(
                Step::Dns,
                cf(
                    &t,
                    HttpMethod::Post,
                    "/zones/{zone}/dns_records",
                    &[("zone", st.zone_id())],
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        if st.team_domain.is_empty() {
            return step_call(
                Step::Org,
                cf(
                    &t,
                    HttpMethod::Get,
                    "/accounts/{account}/access/organizations",
                    &[("account", &acc)],
                    "",
                    None,
                ),
                mem,
            );
        }
        if st.service_token.is_none() {
            let body = json!({"name": SERVICE_TOKEN_NAME, "duration": "8760h"});
            return step_call(
                Step::ServiceToken,
                cf(
                    &t,
                    HttpMethod::Post,
                    "/accounts/{account}/access/service_tokens",
                    &[("account", &acc)],
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        let include: Vec<Value> = st
            .emails
            .iter()
            .map(|e| json!({"email": {"email": e}}))
            .collect();
        let allow_body =
            json!({"name": ALLOW_POLICY_NAME, "decision": "allow", "include": include});
        match &st.allow_policy {
            None => {
                return step_call(
                    Step::AllowPolicy,
                    cf(
                        &t,
                        HttpMethod::Post,
                        "/accounts/{account}/access/policies",
                        &[("account", &acc)],
                        "",
                        Some(&allow_body),
                    ),
                    mem,
                );
            }
            Some(id) if st.policy_emails != st.emails => {
                return step_call(
                    Step::UpdateAllowPolicy,
                    cf(
                        &t,
                        HttpMethod::Put,
                        "/accounts/{account}/access/policies/{policy}",
                        &[("account", &acc), ("policy", id)],
                        "",
                        Some(&allow_body),
                    ),
                    mem,
                );
            }
            Some(_) => {}
        }
        if st.service_policy.is_none() {
            let token_id = st.service_token.as_ref().map_or("", |s| s.id.as_str());
            let body = json!({"name": SERVICE_POLICY_NAME, "decision": "non_identity",
                              "include": [{"service_token": {"token_id": token_id}}]});
            return step_call(
                Step::ServicePolicy,
                cf(
                    &t,
                    HttpMethod::Post,
                    "/accounts/{account}/access/policies",
                    &[("account", &acc)],
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        if st.app.is_none() {
            let body = app_body(st);
            return step_call(
                Step::App,
                cf(
                    &t,
                    HttpMethod::Post,
                    "/accounts/{account}/access/apps",
                    &[("account", &acc)],
                    "",
                    Some(&body),
                ),
                mem,
            );
        }
        if !st.installed_server {
            let lan = st.device == "lan";
            let listen = if lan {
                format!("{}:{SERVER_PORT}", st.lan_address)
            } else {
                format!("127.0.0.1:{SERVER_PORT}")
            };
            let req = InstallRequest {
                listen,
                allow_lan: lan,
                team_domain: st.team_domain.clone(),
                aud: st.aud.clone(),
                service_client_id: st
                    .service_token
                    .as_ref()
                    .map(|s| s.client_id.clone())
                    .unwrap_or_default(),
                client_url: format!("https://{}", st.hostname),
                tunnel_token: if new_tunnel && !st.installed_cloudflared {
                    mem.tunnel_token.clone()
                } else {
                    None
                },
                replace_cloudflared_tunnel: if new_tunnel && !st.installed_cloudflared {
                    st.replace_service.clone()
                } else {
                    None
                },
            };
            return job(
                c,
                Op::Step {
                    mem,
                    step: Step::Install,
                    call: None,
                },
                Vec::new(),
                Some(HostOp::Install(req)),
            );
        }
        let st = self.setup_write(
            store,
            actor,
            "capture.setup_apply",
            json!({"step": "Finish"}),
            &|_, s| {
                s.complete = true;
                Ok(())
            },
        )?;
        Ok(NetStep::Done(json!({
            "complete": true,
            "address": st.hostname,
            "url": format!("https://{}", st.hostname),
            "details": st.details,
            "resources": resources(&st),
        })))
    }

    /// A removal step failed (revision 16): its Details line, and the failure kept
    /// for the end of this run; the step is skipped for the rest of it.
    fn undo_failed(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        mem: &mut Mem,
        step: RStep,
        reason: String,
        detail: Option<Detail>,
    ) -> CommandResult<SetupState> {
        let d = detail.unwrap_or_else(|| Detail {
            method: "UNINSTALL".into(),
            path: "this computer".into(),
            result: format!("failed: {reason}"),
        });
        let st = self.setup_write(
            store,
            actor,
            "capture.setup_remove",
            json!({"step": format!("{step:?}"), "failed": true}),
            &|_, st| {
                st.detail(d.clone());
                Ok(())
            },
        )?;
        // Reading and writing the tunnel's rules are one step.
        let key = if step == RStep::WriteIngress {
            RStep::ReadIngress
        } else {
            step
        };
        mem.skip.push(key);
        let what = match step {
            RStep::Uninstall => undo_what(
                &st,
                step,
                match (st.installed_server, st.installed_cloudflared) {
                    (true, false) => Some(false),
                    (false, true) => Some(true),
                    _ => None,
                },
            ),
            _ => undo_what(&st, step, None),
        };
        mem.failed.push(UndoFailure {
            what,
            reason: mk_core::text::clean(&reason, 400).text,
            place: undo_where(step).to_owned(),
        });
        Ok(st)
    }

    /// The next removal step, or the end (the setup record is cleared).
    pub(crate) fn remove_next(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        mem: Mem,
        st: &SetupState,
    ) -> CommandResult<NetStep> {
        let t = mem.token.clone();
        let acc = st.account_id().to_owned();
        let call_step = |step: RStep, call: CfCall, mem: Mem| -> CommandResult<NetStep> {
            let req = call.req.clone();
            job(
                c,
                Op::Remove {
                    mem,
                    step,
                    call: Some(call),
                },
                vec![req],
                None,
            )
        };
        let skip = mem.skip.clone();
        let todo = |step: RStep| !skip.contains(&step);
        if (st.installed_server || st.installed_cloudflared) && todo(RStep::Uninstall) {
            let req = UninstallRequest {
                server: st.installed_server,
                cloudflared: st.installed_cloudflared,
            };
            return job(
                c,
                Op::Remove {
                    mem,
                    step: RStep::Uninstall,
                    call: None,
                },
                Vec::new(),
                Some(HostOp::Uninstall(req)),
            );
        }
        if let Some(id) = st.app.as_ref().filter(|_| todo(RStep::App)) {
            return call_step(
                RStep::App,
                cf(
                    &t,
                    HttpMethod::Delete,
                    "/accounts/{account}/access/apps/{app}",
                    &[("account", &acc), ("app", id)],
                    "",
                    None,
                ),
                mem,
            );
        }
        for (step, id) in [
            (RStep::AllowPolicy, &st.allow_policy),
            (RStep::ServicePolicy, &st.service_policy),
        ] {
            if let Some(id) = id.as_ref().filter(|_| todo(step)) {
                return call_step(
                    step,
                    cf(
                        &t,
                        HttpMethod::Delete,
                        "/accounts/{account}/access/policies/{policy}",
                        &[("account", &acc), ("policy", id)],
                        "",
                        None,
                    ),
                    mem,
                );
            }
        }
        if let Some(s) = st
            .service_token
            .as_ref()
            .filter(|_| todo(RStep::ServiceToken))
        {
            return call_step(
                RStep::ServiceToken,
                cf(
                    &t,
                    HttpMethod::Delete,
                    "/accounts/{account}/access/service_tokens/{token}",
                    &[("account", &acc), ("token", &s.id)],
                    "",
                    None,
                ),
                mem,
            );
        }
        if let Some(id) = st.dns_record.as_ref().filter(|_| todo(RStep::Dns)) {
            return call_step(
                RStep::Dns,
                cf(
                    &t,
                    HttpMethod::Delete,
                    "/zones/{zone}/dns_records/{record}",
                    &[("zone", st.zone_id()), ("record", id)],
                    "",
                    None,
                ),
                mem,
            );
        }
        if let Some(tun) = &st.tunnel {
            let vars = [("account", acc.as_str()), ("tunnel", tun.id.as_str())];
            if tun.created && todo(RStep::Tunnel) {
                return call_step(
                    RStep::Tunnel,
                    cf(
                        &t,
                        HttpMethod::Delete,
                        "/accounts/{account}/cfd_tunnel/{tunnel}",
                        &vars,
                        "",
                        None,
                    ),
                    mem,
                );
            }
            if !tun.created && st.ingress && todo(RStep::ReadIngress) {
                return call_step(
                    RStep::ReadIngress,
                    cf(
                        &t,
                        HttpMethod::Get,
                        "/accounts/{account}/cfd_tunnel/{tunnel}/configurations",
                        &vars,
                        "",
                        None,
                    ),
                    mem,
                );
            }
        }
        // Revision 17: Start over goes on with this computer and MoneyKeep whatever
        // was left in Cloudflare (the result lists it with where to finish by hand).
        if mem.start_over.is_some() {
            return self.start_over_computer(store, actor, c, mem);
        }
        // Revision 16: something is left; the setup is kept so Retry undo can
        // finish it, and the list says what is left and where to remove it by hand.
        if !mem.failed.is_empty() {
            return Ok(NetStep::Done(json!({
                "removed": false,
                "text": text::UNDO_INCOMPLETE,
                "failed": mem.failed,
                "left": leftovers(st),
                "details": st.details,
            })));
        }
        // Everything is gone: forget the setup, the server address and its secrets.
        let spec = self.setup_spec("capture.setup_remove")?;
        let details = st.details.clone();
        self.execute(
            store,
            actor,
            spec,
            json!({"step": "Finish"}),
            None,
            &|ctx, _| {
                let st = load(ctx)?;
                if let Some(r) = &st.token_ref {
                    delete_secret(ctx, r)?;
                }
                if let Some(r) = st
                    .service_token
                    .as_ref()
                    .and_then(|s| s.secret_ref.as_ref())
                {
                    delete_secret(ctx, r)?;
                }
                if let Some(conf) = crate::capture::conf(ctx)? {
                    if let Some(r) = conf.secret_ref {
                        delete_secret(ctx, &r)?;
                    }
                    ctx.save_setting(MODULE, crate::capture::SERVER_KEY, None)?;
                }
                ctx.save_setting(MODULE, SETUP_KEY, None)?;
                Ok(Value::Null)
            },
        )?;
        Ok(NetStep::Done(json!({"removed": true, "details": details})))
    }

    // ----- status -------------------------------------------------------------------------

    fn status_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
    ) -> CommandResult<NetStep> {
        let grey = || NetStep::Done(status_json("grey", text::GREY, Vec::new(), json!({})));
        let Some(c) = self.connector.as_ref() else {
            return Ok(grey());
        };
        let plan = match self.setup_read(store, actor, |ctx| {
            let st = load(ctx)?;
            let Some(conf) = crate::capture::conf(ctx)? else {
                return Ok(None);
            };
            let ep = crate::capture::endpoint(ctx, c).ok();
            let service = match st.service_token.as_ref() {
                Some(s) if st.complete && !st.hostname.is_empty() => s
                    .secret_ref
                    .as_ref()
                    .and_then(|r| get_secret(ctx, r).ok().flatten())
                    .map(|secret| (st.hostname.clone(), s.client_id.clone(), secret)),
                _ => None,
            };
            let state = crate::capture::collect_error(ctx)?;
            Ok(Some((conf, ep, service, state)))
        }) {
            Ok(Some(p)) => p,
            Ok(None) | Err(CommandError::ModuleDisabled { .. }) => return Ok(grey()),
            Err(e) => return Err(e),
        };
        let (conf, ep, service, collect_error) = plan;
        let Ok((base, authority, http)) = crate::capture::check_url(&conf.url, conf.trusted) else {
            return Ok(grey());
        };
        let mut reqs = vec![HttpRequest {
            method: HttpMethod::Get,
            url: format!("{base}{}", api::HEALTH_PATH),
            authority: authority.clone(),
            max_bytes: 64 * 1024,
            allow_http: http,
            ..HttpRequest::default()
        }];
        let desktop = ep.is_some();
        if let Some(ep) = ep {
            reqs.push(ep.req(HttpMethod::Get, "/status", None, 64 * 1024));
        }
        let public = service.is_some();
        if let Some((host, id, secret)) = service {
            reqs.push(HttpRequest {
                method: HttpMethod::Get,
                url: format!("https://{host}{}", api::HEALTH_PATH),
                authority: host.clone(),
                max_bytes: 64 * 1024,
                headers: vec![
                    ("CF-Access-Client-Id".to_owned(), id),
                    ("CF-Access-Client-Secret".to_owned(), secret),
                ],
                ..HttpRequest::default()
            });
        }
        job(
            c,
            Op::Status {
                st: Box::new(StatusPlan {
                    public,
                    desktop,
                    collect_error,
                }),
            },
            reqs,
            None,
        )
    }

    fn status_resume(
        &self,
        _store: &mut mk_store::Store,
        _actor: &crate::Actor,
        c: &Connector,
        plan: StatusPlan,
        responses: &[Result<HttpResponse, FetchError>],
    ) -> CommandResult<NetStep> {
        let health = |r: Option<&Result<HttpResponse, FetchError>>| -> Option<api::Health> {
            match r {
                Some(Ok(resp)) if resp.status == 200 => {
                    serde_json::from_slice::<api::Health>(&resp.body)
                        .ok()
                        .filter(|h| h.ok)
                }
                _ => None,
            }
        };
        let local = health(responses.first());
        let mut i = 1;
        let desktop = if plan.desktop {
            let d = match responses.get(i) {
                Some(Ok(resp)) if resp.status == 200 => {
                    serde_json::from_slice::<api::DesktopStatus>(&resp.body).ok()
                }
                _ => None,
            };
            i += 1;
            d
        } else {
            None
        };
        let public_ok = if plan.public {
            Some(health(responses.get(i)).is_some())
        } else {
            None
        };
        let extra = json!({
            "local_ok": local.is_some(),
            "public_ok": public_ok,
            "server_version": local.as_ref().map(|h| h.version.clone()),
            "app_version": api::BUILD_VERSION,
            "server_capture_version": local.as_ref().and_then(|h| h.capture_version.clone()),
            "app_capture_version": api::CAPTURE_VERSION,
            "waiting": desktop.as_ref().map(|d| d.waiting),
            "last_activity": desktop.as_ref().and_then(|d| d.last_activity),
            "access_problem": desktop.as_ref().and_then(|d| d.access_problem.clone()),
            "checked_at": c.now(),
            "utc_offset_secs": c.utc_offset_secs,
            "can_start": false,
        });
        // U1.9: a collection error from before this passing health check is a note
        // under the status, not the status; with its time.
        let collect_line = plan
            .collect_error
            .as_ref()
            .map(|(e, at)| text::collect_failed(&at.map_or_else(String::new, |at| when(at, c)), e));
        let Some(local) = local else {
            let partial = status_json("red", text::RED, vec![text::RED.to_owned()], extra);
            return match c.host {
                Some(_) => job(
                    c,
                    Op::StatusHost { partial },
                    Vec::new(),
                    Some(HostOp::Inspect),
                ),
                None => Ok(NetStep::Done(partial)),
            };
        };
        let mut problems: Vec<String> = Vec::new();
        // Revision 8: why phones are refused comes first; Check fixes it.
        if let Some(p) = desktop.as_ref().and_then(|d| d.access_problem.as_ref()) {
            problems.push(text::access_problem(&p.reason));
        }
        // Revision 10: the Wi-Fi address went away (adapter off, another network).
        if let Some(w) = desktop.as_ref().and_then(|d| d.wifi.as_ref())
            && w.enabled
            && !w.address_present
        {
            problems.push(crate::capture_wifi::text::address_gone(&w.address));
        }
        if public_ok == Some(false) {
            problems.push(text::TUNNEL.to_owned());
        }
        // U1.2: Wi-Fi capture is set up but couldn't start.
        let wifi_damaged = local.wifi_error.is_some();
        if wifi_damaged {
            problems.push(text::WIFI_DAMAGED.to_owned());
        }
        // U1.4: a server whose API level is lower than this desktop needs.
        let level_low = local.api_level < api::DESKTOP_NEEDS_API_LEVEL;
        // release-packaging.md: the capture versions decide; a server without one
        // (built before it) is compared by build version.
        let differs = match &local.capture_version {
            Some(cv) => cv != api::CAPTURE_VERSION,
            None => local.version != api::BUILD_VERSION,
        };
        if level_low {
            problems.push(text::UPDATE_SERVER.to_owned());
        } else if differs {
            problems.push(
                match api::compare_versions(&local.version, api::BUILD_VERSION) {
                    Some(std::cmp::Ordering::Less) => text::OLDER,
                    Some(std::cmp::Ordering::Greater) => text::NEWER,
                    _ => text::VERSION,
                }
                .to_owned(),
            );
        }
        let mut extra = extra;
        if let Some(o) = extra.as_object_mut() {
            // The update notice (release-packaging.md); whether Update now is
            // offered is decided once this computer's server program is known.
            o.insert("update_needed".into(), json!(differs || level_low));
            o.insert("update_ready".into(), json!(false));
            o.insert("can_update".into(), json!(false));
            o.insert("server_api_level".into(), json!(local.api_level));
            o.insert("level_low".into(), json!(level_low));
            o.insert("wifi_damaged".into(), json!(wifi_damaged));
            o.insert("wifi_listening".into(), json!(local.wifi_listening));
            if let Some(line) = &collect_line {
                o.insert("collect_note".into(), json!(line));
            }
        }
        let v = match problems.first() {
            None => status_json("green", text::GREEN, problems.clone(), extra),
            Some(first) => {
                let t = first.clone();
                status_json("yellow", &t, problems.clone(), extra)
            }
        };
        // This computer's services: an installation under the old name shows here
        // (branding.md step 6).
        match c.host {
            Some(_) => job(
                c,
                Op::StatusHost { partial: v },
                Vec::new(),
                Some(HostOp::Inspect),
            ),
            None => Ok(NetStep::Done(v)),
        }
    }
}

/// U1.9: when a collection error happened, in this computer's time: "at 18:12"
/// today, else "on 2026-11-08 at 18:12".
fn when(at: i64, c: &Connector) -> String {
    let local = |t: i64| t.saturating_add(i64::from(c.utc_offset_secs));
    let (day, now_day) = (
        local(at).div_euclid(86_400),
        local(c.now()).div_euclid(86_400),
    );
    let secs = local(at).rem_euclid(86_400);
    let hm = format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60);
    if day == now_day {
        format!("at {hm}")
    } else {
        let (y, m, d) = api::civil_from_days(day);
        format!("on {y:04}-{m:02}-{d:02} at {hm}")
    }
}

/// Check's answer: what changed, in words.
fn access_done(host: &str, changes: Vec<String>) -> Value {
    let text = if changes.is_empty() {
        format!("The Access application for {host} matches the capture server's settings.")
    } else {
        format!("Fixed: {}.", changes.join("; "))
    };
    json!({"ok": true, "app_missing": false, "text": text, "changes": changes})
}

fn status_json(state: &str, text: &str, problems: Vec<String>, extra: Value) -> Value {
    let mut v = json!({
        "state": state,
        "text": text,
        "accessible_name": format!("Phone capture status: {text}"),
        "problems": problems,
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

/// The Access application: self-hosted at the address, a 30-day session so the
/// phone rarely signs in again (revision 3), the Allow and Service Auth policies.
fn app_body(st: &SetupState) -> Value {
    json!({
        "name": APP_NAME,
        "domain": st.hostname,
        "type": "self_hosted",
        "session_duration": SESSION_DURATION,
        "policies": [
            {"id": st.allow_policy, "precedence": 1},
            {"id": st.service_policy, "precedence": 2},
        ],
    })
}

/// The tunnel configuration with `host` routed to `service` before the catch-all,
/// keeping every other rule and setting.
pub(crate) fn with_host(config: &Value, host: &str, service: &str) -> Value {
    let mut out = config.clone();
    if !out.is_object() {
        out = json!({});
    }
    let rules = list_of(config.get("ingress").unwrap_or(&Value::Null));
    let is_catch_all = |r: &Value| r.get("hostname").is_none() && r.get("path").is_none();
    let catch_all = rules
        .iter()
        .rev()
        .find(|r| is_catch_all(r))
        .cloned()
        .unwrap_or_else(|| json!({"service": "http_status:404"}));
    let mut next: Vec<Value> = rules
        .into_iter()
        .filter(|r| !is_catch_all(r) && str_of(r, "hostname") != host)
        .collect();
    next.push(json!({"hostname": host, "service": service, "originRequest": {}}));
    next.push(catch_all);
    if let Some(o) = out.as_object_mut() {
        o.insert("ingress".into(), Value::Array(next));
    }
    out
}

/// The tunnel configuration without `host`'s rules, the other rules kept in order
/// and always ending with a catch-all (revision 16: Cloudflare refuses a
/// configuration without ingress rules, and its last rule must match everything,
/// such as `{"service": "http_status:404"}` — Cloudflare Tunnel documentation,
/// <https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/>).
pub(crate) fn without_host(config: &Value, host: &str) -> Value {
    let mut out = config.clone();
    if !out.is_object() {
        out = json!({});
    }
    let is_catch_all = |r: &Value| r.get("hostname").is_none() && r.get("path").is_none();
    let rules = list_of(config.get("ingress").unwrap_or(&Value::Null));
    let catch_all = rules
        .iter()
        .rev()
        .find(|r| is_catch_all(r))
        .cloned()
        .unwrap_or_else(|| json!({"service": "http_status:404"}));
    let mut next: Vec<Value> = rules
        .into_iter()
        .filter(|r| !is_catch_all(r) && str_of(r, "hostname") != host)
        .collect();
    next.push(catch_all);
    if let Some(o) = out.as_object_mut() {
        o.insert("ingress".into(), Value::Array(next));
    }
    out
}

/// The plan's reads: the DNS records for the address; the existing tunnel and its
/// configuration, when one was chosen; the tunnel a `cloudflared` service here runs,
/// when it is not this setup's (revision 16).
fn plan_reads(
    st: &SetupState,
    a: &PlanArgs,
    token: &str,
    host: &HostReport,
) -> CommandResult<Vec<HttpRequest>> {
    let sub = check_subdomain(a.subdomain.as_deref().unwrap_or("capture"))?;
    let zone = st.zone.as_ref().map_or("", |z| z.name.as_str());
    let name = fqdn(&sub, zone);
    let mut reqs = vec![
        cf(
            token,
            HttpMethod::Get,
            "/zones/{zone}/dns_records",
            &[("zone", st.zone_id())],
            &format!("?name={name}"),
            None,
        )
        .req,
    ];
    if let Some(t) = a.tunnel.as_deref().filter(|t| !t.trim().is_empty()) {
        let vars = [("account", st.account_id()), ("tunnel", t.trim())];
        reqs.push(
            cf(
                token,
                HttpMethod::Get,
                "/accounts/{account}/cfd_tunnel/{tunnel}",
                &vars,
                "",
                None,
            )
            .req,
        );
        reqs.push(
            cf(
                token,
                HttpMethod::Get,
                "/accounts/{account}/cfd_tunnel/{tunnel}/configurations",
                &vars,
                "",
                None,
            )
            .req,
        );
    } else if let Some(id) = service_tunnel(st, a, host) {
        // "Get a Cloudflare Tunnel" (Cloudflare API reference,
        // <https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/subresources/cloudflared/methods/get/>):
        // `deleted_at` is null unless the tunnel was deleted.
        reqs.push(
            cf(
                token,
                HttpMethod::Get,
                "/accounts/{account}/cfd_tunnel/{tunnel}",
                &[("account", st.account_id()), ("tunnel", &id)],
                "",
                None,
            )
            .req,
        );
    }
    Ok(reqs)
}

/// The tunnel a `cloudflared` service here runs, when that service is not this
/// setup's and a new tunnel is planned (revision 16: it is looked up).
fn service_tunnel(st: &SetupState, a: &PlanArgs, host: &HostReport) -> Option<String> {
    let existing = a.tunnel.as_deref().is_some_and(|t| !t.trim().is_empty());
    if existing || host.cloudflared_service.is_none() || st.installed_cloudflared {
        return None;
    }
    host.cloudflared_tunnel.clone().filter(|t| !t.is_empty())
}

/// What became of the tunnel a `cloudflared` service here runs (revision 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceTunnel {
    /// Deleted or missing from the account (a 404 or `deleted_at`): its id.
    Gone(String),
    /// Revision 16a: the service's token belongs to another Cloudflare account; the
    /// tunnel may be live there (its id). Not treated as deleted.
    OtherAccount(String),
    /// Still in the account: its name.
    Exists(String),
    /// The service's tunnel is not known, or the lookup failed.
    Unknown,
}

/// Judge the lookup of the service's tunnel: a 404 or a `deleted_at` time in this
/// account means gone; a token from another account is that account's tunnel
/// (revision 16a: not "deleted").
pub(crate) fn service_tunnel_state(
    account: &str,
    host: &HostReport,
    answer: Option<&Result<HttpResponse, FetchError>>,
) -> ServiceTunnel {
    let Some(id) = host.cloudflared_tunnel.clone().filter(|t| !t.is_empty()) else {
        return ServiceTunnel::Unknown;
    };
    if host
        .cloudflared_account
        .as_deref()
        .is_some_and(|a| !a.is_empty() && !account.is_empty() && a != account)
    {
        return ServiceTunnel::OtherAccount(id);
    }
    let a = cf_answer(answer);
    if a.ok {
        if a.result.get("deleted_at").is_some_and(|d| !d.is_null()) {
            ServiceTunnel::Gone(id)
        } else {
            ServiceTunnel::Exists(str_of(&a.result, "name").to_owned())
        }
    } else if a.status == 404 {
        ServiceTunnel::Gone(id)
    } else {
        ServiceTunnel::Unknown
    }
}

/// The plan from the reads: what will be made and what stands in the way.
fn plan_of(
    st: &SetupState,
    a: &PlanArgs,
    responses: &[Result<HttpResponse, FetchError>],
    mem: &mut Mem,
) -> CommandResult<Plan> {
    let sub = check_subdomain(a.subdomain.as_deref().unwrap_or("capture"))?;
    let emails = check_emails(&a.emails)?;
    let zone = st.zone.as_ref().map_or("", |z| z.name.as_str());
    let hostname = fqdn(&sub, zone);
    let mut actions: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut service = Value::Null;
    let mut replace: Option<String> = None;
    let host = mem.host.clone();
    if !host.server_program {
        problems.push(
            "The capture server program (moneykeep-server) is missing: install MoneyKeep with its installer, or (developers) build with cargo xtask release, then try again."
                .into(),
        );
    }
    let existing = a.tunnel.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let device = match a.device.as_deref().map(str::trim) {
        Some("lan") if existing.is_some() => "lan",
        Some("lan") => {
            return Err(invalid(
                "\"Another device on my network\" is for an existing tunnel.",
            ));
        }
        None | Some("" | "this") => "this",
        Some(_) => return Err(invalid("device is this or lan")),
    };
    let lan = if device == "lan" {
        match a.lan_address.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(l) => check_lan(l)?,
            None => host.lan_addresses.first().cloned().ok_or_else(|| {
                invalid("This computer's network address could not be detected; enter it.")
            })?,
        }
    } else {
        String::new()
    };
    let origin = if device == "lan" {
        format!("http://{lan}:{SERVER_PORT}")
    } else {
        format!("http://127.0.0.1:{SERVER_PORT}")
    };
    // DNS: the address must be free unless MoneyKeep made it.
    let dns = cf_answer(responses.first());
    if !dns.ok {
        problems.push(format!(
            "The DNS records could not be read: {}",
            dns.message
        ));
    }
    let records = list_of(&dns.result);
    let tunnel = match existing {
        Some(id) => {
            let t = cf_answer(responses.get(1));
            let cfg = cf_answer(responses.get(2));
            if !t.ok {
                problems.push(format!("The tunnel could not be read: {}", t.message));
                None
            } else {
                let remote = t
                    .result
                    .get("remote_config")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let config = cfg.result.get("config").cloned().filter(Value::is_object);
                if !remote && config.is_none() {
                    problems.push(format!(
                        "The tunnel \"{}\" is managed by a configuration file on its device, so MoneyKeep can't add the address to it: add a rule for {hostname} with the service {origin} there (see the guide), or create a new tunnel.",
                        str_of(&t.result, "name")
                    ));
                }
                mem.config = config;
                Some(TunnelRec {
                    id: id.to_owned(),
                    name: str_of(&t.result, "name").to_owned(),
                    created: false,
                })
            }
        }
        None => None,
    };
    let target_tunnel = tunnel.as_ref().map(|t| t.id.clone()).or_else(|| {
        st.tunnel
            .as_ref()
            .filter(|t| t.created)
            .map(|t| t.id.clone())
    });
    for r in &records {
        // Records made before the product was renamed are ours too (branding.md).
        let comment = str_of(r, "comment");
        let ours = comment == DNS_COMMENT
            || comment == mk_core::legacy::DNS_COMMENT
            || st.dns_record.as_deref() == Some(str_of(r, "id"));
        let points = target_tunnel
            .as_ref()
            .is_some_and(|t| str_of(r, "content") == format!("{t}.cfargotunnel.com"));
        if ours && (points || target_tunnel.is_none()) {
            mem.dns_ours = Some(str_of(r, "id").to_owned());
        } else {
            problems.push(format!(
                "The address {hostname} already exists in your DNS; choose another subdomain."
            ));
            break;
        }
    }
    // This computer.
    match &tunnel {
        None => {
            let ours = st.installed_cloudflared;
            if host.cloudflared_service.is_some() && !ours {
                // Revision 16: which tunnel that service runs, and whether it still
                // exists (looked up with the plan's reads, after the DNS read).
                let looked_up = service_tunnel(st, a, &host).and(responses.get(1));
                match service_tunnel_state(st.account_id(), &host, looked_up) {
                    ServiceTunnel::Gone(id) => {
                        let yes = a.replace_service.unwrap_or(true);
                        service = json!({"state": "deleted", "tunnel": id,
                                         "message": text::REPLACE_MESSAGE, "replace": yes});
                        if yes {
                            replace = Some(id);
                        } else {
                            problems.push(text::LEAVE_REASON.into());
                        }
                    }
                    // Revision 16a: another account's tunnel is left alone unless
                    // the owner chooses Replace it (with the warning).
                    ServiceTunnel::OtherAccount(id) => {
                        let yes = a.replace_service.unwrap_or(false);
                        service = json!({"state": "other_account", "tunnel": id,
                                         "message": text::OTHER_ACCOUNT_MESSAGE,
                                         "warning": text::OTHER_ACCOUNT_WARNING, "replace": yes});
                        if yes {
                            replace = Some(id);
                        } else {
                            problems.push(text::OTHER_ACCOUNT_LEAVE_REASON.into());
                        }
                    }
                    ServiceTunnel::Exists(name) if !name.is_empty() => problems.push(format!(
                        "A cloudflared service for your tunnel \"{name}\" already runs on this computer; it was not replaced. Choose \"Add to my existing tunnel {name}\"."
                    )),
                    ServiceTunnel::Exists(_) | ServiceTunnel::Unknown => {
                        problems.push(text::OTHER_TUNNEL.into());
                    }
                }
            }
            if st.tunnel.as_ref().is_none_or(|t| !t.created) {
                actions.push(format!("Create the tunnel \"{TUNNEL_NAME}\""));
            }
            if replace.is_some() {
                actions.push(text::REPLACE_STEP.into());
            }
            if !ours {
                if host.cloudflared_installed {
                    actions.push("Install cloudflared (already on this computer) as a service for the new tunnel".into());
                } else if host.can_download_cloudflared {
                    actions.push(
                        "Download the official cloudflared installer from Cloudflare's GitHub releases (refused unless signed by Cloudflare) and install it as a service"
                            .into(),
                    );
                } else if host.os == "macos" {
                    // macos-release.md C6: never downloaded on a Mac.
                    problems.push(text::MAC_NO_CLOUDFLARED.into());
                } else if host.os != "windows" {
                    problems.push(
                        "cloudflared is not installed: install it (https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/), then press Try again."
                            .into(),
                    );
                }
            }
        }
        Some(t) => {
            if device == "lan" {
                actions.push(format!(
                    "Let the capture server listen on this computer's network address {lan} (only that address)"
                ));
            }
            actions.push(format!(
                "Send {hostname} to {origin} through the tunnel \"{}\", keeping its rules (before the catch-all)",
                t.name
            ));
        }
    }
    if tunnel.is_none() {
        actions.push(format!(
            "Route {hostname} to {origin} in the tunnel's settings"
        ));
    }
    if mem.dns_ours.is_none() {
        actions.push(format!("Create the proxied DNS record {hostname}"));
    }
    if st.app.is_none() {
        actions.push(format!(
            "Create the Access application for {hostname}, allowing {}",
            emails.join(", ")
        ));
    } else if st.policy_emails != emails {
        actions.push(format!("Allow {} to sign in", emails.join(", ")));
    }
    if st.service_token.is_none() {
        actions.push(format!(
            "Create the service token \"{SERVICE_TOKEN_NAME}\" with a Service Auth policy"
        ));
    }
    if !st.installed_server {
        actions.push("Install the MoneyKeep capture server as a background service that starts with the computer".into());
    }
    Ok(Plan {
        hostname,
        emails,
        tunnel,
        device: device.to_owned(),
        lan,
        origin,
        actions,
        problems,
        service,
        replace,
    })
}

/// Record what a step made (inside the short write after it).
fn absorb(
    ctx: &Ctx<'_>,
    st: &mut SetupState,
    step: Step,
    result: &Value,
    installed: Option<&InstallOutcome>,
) -> CommandResult<()> {
    let id = || str_of(result, "id").to_owned();
    match step {
        Step::CreateTunnel => {
            st.tunnel = Some(TunnelRec {
                id: id(),
                name: str_of(result, "name").to_owned(),
                created: true,
            });
            st.ingress = false;
        }
        Step::TunnelToken => {}
        Step::Ingress => st.ingress = true,
        Step::Dns => st.dns_record = Some(id()),
        Step::Org => st.team_domain = str_of(result, "auth_domain").to_owned(),
        Step::ServiceToken => {
            let book = ctx.tx.info()?.id;
            let r = put_secret(
                ctx,
                &format!("{book}:capture-service-token"),
                str_of(result, "client_secret"),
            )?;
            st.service_token = Some(ServiceTokenRec {
                id: id(),
                client_id: str_of(result, "client_id").to_owned(),
                secret_ref: Some(r),
            });
        }
        Step::AllowPolicy => {
            st.allow_policy = Some(id());
            st.policy_emails.clone_from(&st.emails);
        }
        Step::UpdateAllowPolicy => st.policy_emails.clone_from(&st.emails),
        Step::ServicePolicy => st.service_policy = Some(id()),
        Step::App => {
            st.app = Some(id());
            st.aud = str_of(result, "aud").to_owned();
        }
        Step::Install => {
            let Some(o) = installed else {
                return Err(CommandError::Integrity(
                    "installation result missing".into(),
                ));
            };
            st.installed_server = true;
            if o.cloudflared_service {
                st.installed_cloudflared = true;
            }
            if o.cloudflared_replaced {
                st.detail(Detail {
                    method: "REPLACE".into(),
                    path: "this computer".into(),
                    result: format!("{}: ok", text::REPLACE_STEP),
                });
            }
            st.replace_service = None;
            let lan = st.device == "lan";
            crate::capture::store_server(ctx, &o.local_url, lan, &o.desktop_token)?;
            st.detail(Detail {
                method: "INSTALL".into(),
                path: "this computer".into(),
                result: if o.actions.is_empty() {
                    "ok".into()
                } else {
                    o.actions.join("; ")
                },
            });
        }
    }
    Ok(())
}

// ----- test doubles ----------------------------------------------------------------------------

pub mod testing {
    //! A scripted Cloudflare API, capture server and computer (tests and screen
    //! reviews; no network, nothing installed).

    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::connect::{FetchError, Fetcher, HttpMethod, HttpRequest, HttpResponse};

    /// One request the fake saw.
    #[derive(Debug, Clone)]
    pub struct Call {
        pub method: &'static str,
        /// The URL without the API prefix for Cloudflare (`/accounts/a1/cfd_tunnel`).
        pub path: String,
        pub authority: String,
        pub body: Option<Value>,
        /// The request's header values (to check that secrets go only where they belong).
        pub headers: Vec<(String, String)>,
    }

    #[derive(Debug)]
    pub struct CfState {
        pub token: String,
        /// Permissions the token has (names from [`PERMISSIONS`]).
        pub granted: BTreeSet<&'static str>,
        pub accounts: Vec<(String, String)>,
        /// (id, name, account id)
        pub zones: Vec<(String, String, String)>,
        /// id to (name, config, remote_config)
        pub tunnels: BTreeMap<String, (String, Value, bool)>,
        pub dns: BTreeMap<String, Value>,
        pub apps: BTreeMap<String, Value>,
        pub policies: BTreeMap<String, Value>,
        pub service_tokens: BTreeMap<String, Value>,
        pub team_domain: String,
        pub calls: Vec<Call>,
        pub next: u64,
        /// The capture server's answers.
        pub local_health: bool,
        pub public_health: bool,
        pub server_version: String,
        /// The health answer's capture version (release-packaging.md); `None`
        /// for a server built before it.
        pub server_capture_version: Option<String>,
        pub waiting: usize,
        pub pairing_code: String,
        /// `GET /accounts` answers an empty list (a token without account-settings
        /// read access, app-desktop.md revision 30).
        pub hide_accounts: bool,
        /// `desktop/status`'s `access_problem` (revision 8).
        pub access_problem: Option<Value>,
        /// Revision 10: the paired phones (`desktop/devices`), the PINs set
        /// (`desktop/pin`, kept by this fake only to check what was sent) and
        /// `desktop/status`'s `wifi`.
        pub devices: Vec<Value>,
        pub pins: Vec<String>,
        pub wifi: Option<Value>,
        /// Revision 17: Cloudflare can't be reached (the capture server still can).
        pub cloudflare_down: bool,
        /// Waiting phone submissions (`desktop/inbox` items; acknowledging one
        /// removes it, and `desktop/status` counts them).
        pub inbox: Vec<Value>,
        /// Revision 16c: this computer's loopback address can't be reached; the
        /// server refuses the desktop token (a JSON 401).
        pub loopback_down: bool,
        pub refuse_desktop_token: bool,
        /// upgrade-resilience-1: health's `api_level` (0: a server from before it)
        /// and `wifi_error`.
        pub server_api_level: u32,
        pub wifi_error: Option<String>,
    }

    /// A Cloudflare API and capture server answering through [`Fetcher`].
    #[derive(Debug, Clone)]
    pub struct FakeCloudflare {
        pub state: Arc<Mutex<CfState>>,
    }

    pub const TEST_TOKEN: &str = "cf-test-token-0123456789abcdefSECRET";

    impl Default for FakeCloudflare {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeCloudflare {
        pub fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(CfState {
                    token: TEST_TOKEN.into(),
                    granted: PERMISSIONS.iter().copied().collect(),
                    accounts: vec![("acc1".into(), "Owner's account".into())],
                    zones: vec![("zone1".into(), "example.com".into(), "acc1".into())],
                    tunnels: BTreeMap::new(),
                    dns: BTreeMap::new(),
                    apps: BTreeMap::new(),
                    policies: BTreeMap::new(),
                    service_tokens: BTreeMap::new(),
                    team_domain: "owner.cloudflareaccess.com".into(),
                    calls: Vec::new(),
                    next: 0,
                    local_health: true,
                    public_health: true,
                    server_version: api::BUILD_VERSION.into(),
                    server_capture_version: Some(api::CAPTURE_VERSION.into()),
                    server_api_level: api::API_LEVEL,
                    wifi_error: None,
                    waiting: 0,
                    pairing_code: "ABCD2345".into(),
                    hide_accounts: false,
                    access_problem: None,
                    devices: Vec::new(),
                    pins: Vec::new(),
                    wifi: None,
                    cloudflare_down: false,
                    inbox: Vec::new(),
                    loopback_down: false,
                    refuse_desktop_token: false,
                })),
            }
        }

        pub fn with<T>(&self, f: impl FnOnce(&mut CfState) -> T) -> Option<T> {
            self.state.lock().ok().map(|mut s| f(&mut s))
        }

        /// Calls to Cloudflare as `METHOD /path`.
        pub fn cf_calls(&self) -> Vec<String> {
            self.with(|s| {
                s.calls
                    .iter()
                    .filter(|c| c.authority == CLOUDFLARE_HOST)
                    .map(|c| format!("{} {}", c.method, c.path))
                    .collect()
            })
            .unwrap_or_default()
        }

        /// Add an existing tunnel with an ingress configuration.
        pub fn add_tunnel(&self, id: &str, name: &str, ingress: Value) {
            self.with(|s| {
                s.tunnels
                    .insert(id.into(), (name.into(), json!({"ingress": ingress}), true));
            });
        }
    }

    fn ok(result: Value) -> Result<HttpResponse, FetchError> {
        Ok(HttpResponse {
            status: 200,
            body: json!({"success": true, "errors": [], "messages": [], "result": result})
                .to_string()
                .into_bytes(),
            retry_after: None,
            deprecation: None,
        })
    }

    fn fail(status: u16, code: u32, message: &str) -> Result<HttpResponse, FetchError> {
        Ok(HttpResponse {
            status,
            body: json!({"success": false, "errors": [{"code": code, "message": message}], "messages": [], "result": null})
                .to_string()
                .into_bytes(),
            retry_after: None,
            deprecation: None,
        })
    }

    fn plain(status: u16, body: Value) -> Result<HttpResponse, FetchError> {
        Ok(HttpResponse {
            status,
            body: body.to_string().into_bytes(),
            retry_after: None,
            deprecation: None,
        })
    }

    impl Fetcher for FakeCloudflare {
        fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
            let mut guard = self
                .state
                .lock()
                .map_err(|_| FetchError::Network("fake poisoned".into()))?;
            let s = &mut *guard;
            let method = match req.method {
                HttpMethod::Get => "GET",
                HttpMethod::Post => "POST",
                HttpMethod::Put => "PUT",
                HttpMethod::Delete => "DELETE",
            };
            let body: Option<Value> = req
                .body
                .as_ref()
                .and_then(|b| serde_json::from_slice(b).ok());
            let (authority, path) = {
                let rest = req
                    .url
                    .strip_prefix("https://")
                    .or_else(|| req.url.strip_prefix("http://"))
                    .unwrap_or(&req.url);
                let (a, p) = rest.split_once('/').unwrap_or((rest, ""));
                (a.to_owned(), format!("/{p}"))
            };
            if authority != req.authority {
                return Err(FetchError::Refused("authority mismatch".into()));
            }
            let path = path
                .strip_prefix("/client/v4")
                .map(str::to_owned)
                .unwrap_or(path);
            s.calls.push(Call {
                method,
                path: path.clone(),
                authority: authority.clone(),
                body: body.clone(),
                headers: req.headers.clone(),
            });
            let header = |name: &str| {
                req.headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
            };
            if authority != CLOUDFLARE_HOST {
                return capture_server(s, method, &authority, &path, &header, body.as_ref());
            }
            if s.cloudflare_down {
                return Err(FetchError::Network("connection timed out".into()));
            }
            if header("Authorization").as_deref() != Some(&format!("Bearer {}", s.token)) {
                return fail(401, 10000, "Authentication error");
            }
            let bare = path.split('?').next().unwrap_or("").to_owned();
            let query = path
                .split_once('?')
                .map(|(_, q)| q.to_owned())
                .unwrap_or_default();
            let segs: Vec<&str> = bare.split('/').filter(|x| !x.is_empty()).collect();
            let need = |p: &'static str, s: &CfState| s.granted.contains(p);
            s.next += 1;
            let new_id = format!("id{:04}", s.next);
            let body = body.unwrap_or(Value::Null);
            match (method, segs.as_slice()) {
                ("GET", ["user", "tokens", "verify"]) => ok(json!({"id": "tok", "status": "active"})),
                ("GET", ["accounts"]) if s.hide_accounts => ok(json!([])),
                ("GET", ["accounts"]) => ok(Value::Array(
                    s.accounts.iter().map(|(i, n)| json!({"id": i, "name": n})).collect(),
                )),
                ("GET", ["zones"]) => {
                    if !need(PERM_ZONE, s) {
                        return ok(json!([]));
                    }
                    ok(Value::Array(
                        s.zones
                            .iter()
                            .map(|(i, n, a)| {
                                let an = s.accounts.iter().find(|x| x.0 == *a).map_or("", |x| x.1.as_str());
                                json!({"id": i, "name": n, "account": {"id": a, "name": an}})
                            })
                            .collect(),
                    ))
                }
                (_, ["accounts", _, "cfd_tunnel", ..]) if !need(PERM_TUNNEL, s) => fail(403, 10000, "Authentication error"),
                ("GET", ["accounts", _, "cfd_tunnel"]) => ok(Value::Array(
                    s.tunnels
                        .iter()
                        .map(|(i, (n, _, r))| json!({"id": i, "name": n, "status": "healthy", "remote_config": r}))
                        .collect(),
                )),
                ("POST", ["accounts", _, "cfd_tunnel"]) => {
                    let name = str_of(&body, "name").to_owned();
                    let id = format!("tun-{new_id}");
                    s.tunnels.insert(id.clone(), (name.clone(), json!({}), true));
                    ok(json!({"id": id, "name": name, "token": format!("TUNNEL-TOKEN-{id}"), "status": "inactive"}))
                }
                ("GET", ["accounts", _, "cfd_tunnel", id]) => match s.tunnels.get(*id) {
                    Some((n, _, r)) => ok(json!({"id": id, "name": n, "remote_config": r, "deleted_at": null})),
                    None => fail(404, 1003, "Tunnel not found"),
                },
                ("DELETE", ["accounts", _, "cfd_tunnel", id]) => match s.tunnels.remove(*id) {
                    Some(_) => ok(json!({"id": id})),
                    None => fail(404, 1003, "Tunnel not found"),
                },
                ("GET", ["accounts", _, "cfd_tunnel", id, "token"]) => ok(json!(format!("TUNNEL-TOKEN-{id}"))),
                ("GET", ["accounts", _, "cfd_tunnel", id, "configurations"]) => match s.tunnels.get(*id) {
                    Some((_, c, _)) => ok(json!({"tunnel_id": id, "config": c})),
                    None => fail(404, 1003, "Tunnel not found"),
                },
                ("PUT", ["accounts", _, "cfd_tunnel", id, "configurations"]) => match s.tunnels.get_mut(*id) {
                    // Like Cloudflare (mobile-capture.md revision 16 report): a
                    // configuration needs ingress rules ending with a catch-all.
                    Some(_) if !valid_ingress(body.get("config").unwrap_or(&Value::Null)) => fail(
                        400,
                        1056,
                        "Bad Configuration: The tunnel configuration doesn't contain any ingress rules",
                    ),
                    Some(t) => {
                        t.1 = body.get("config").cloned().unwrap_or(Value::Null);
                        ok(json!({"tunnel_id": id, "config": t.1}))
                    }
                    None => fail(404, 1003, "Tunnel not found"),
                },
                (_, ["zones", _, "dns_records", ..]) if !need(PERM_DNS, s) => fail(403, 10000, "Authentication error"),
                ("GET", ["zones", _, "dns_records"]) => {
                    let name = query
                        .split('&')
                        .find_map(|p| p.strip_prefix("name="))
                        .map(str::to_owned);
                    ok(Value::Array(
                        s.dns
                            .values()
                            .filter(|r| name.as_deref().is_none_or(|n| str_of(r, "name") == n))
                            .cloned()
                            .collect(),
                    ))
                }
                ("POST", ["zones", _, "dns_records"]) => {
                    let mut r = body.clone();
                    if let Some(o) = r.as_object_mut() {
                        o.insert("id".into(), json!(format!("dns-{new_id}")));
                    }
                    s.dns.insert(format!("dns-{new_id}"), r.clone());
                    ok(r)
                }
                ("DELETE", ["zones", _, "dns_records", id]) => match s.dns.remove(*id) {
                    Some(_) => ok(json!({"id": id})),
                    None => fail(404, 81044, "Record does not exist."),
                },
                (_, ["accounts", _, "access", "service_tokens", ..]) if !need(PERM_SERVICE_TOKENS, s) => {
                    fail(403, 10000, "Authentication error")
                }
                ("GET", ["accounts", _, "access", "service_tokens"]) => ok(Value::Array(s.service_tokens.values().cloned().collect())),
                ("POST", ["accounts", _, "access", "service_tokens"]) => {
                    let id = format!("st-{new_id}");
                    let t = json!({"id": id, "name": str_of(&body, "name"), "client_id": format!("{id}.access"),
                                   "client_secret": format!("SERVICE-SECRET-{id}")});
                    s.service_tokens.insert(id, t.clone());
                    ok(t)
                }
                ("DELETE", ["accounts", _, "access", "service_tokens", id]) => match s.service_tokens.remove(*id) {
                    Some(_) => ok(json!({"id": id})),
                    None => fail(404, 12130, "not found"),
                },
                (_, ["accounts", _, "access", "organizations"]) if !need(PERM_ORGANIZATIONS, s) => fail(403, 10000, "Authentication error"),
                ("GET", ["accounts", _, "access", "organizations"]) => ok(json!({"auth_domain": s.team_domain, "name": "owner"})),
                (_, ["accounts", _, "access", "apps" | "policies", ..]) if !need(PERM_ACCESS_APPS, s) => {
                    fail(403, 10000, "Authentication error")
                }
                ("GET", ["accounts", _, "access", "apps"]) => ok(Value::Array(s.apps.values().cloned().collect())),
                ("GET", ["accounts", _, "access", "apps", id]) => match s.apps.get(*id) {
                    Some(a) => ok(a.clone()),
                    None => fail(404, 12130, "not found"),
                },
                ("POST", ["accounts", _, "access", "apps"]) => {
                    let id = format!("app-{new_id}");
                    let mut a = body.clone();
                    if let Some(o) = a.as_object_mut() {
                        o.insert("id".into(), json!(id));
                        o.insert("aud".into(), json!(format!("aud-{id}")));
                    }
                    s.apps.insert(id, a.clone());
                    ok(a)
                }
                ("PUT", ["accounts", _, "access", "apps", id]) => match s.apps.get_mut(*id) {
                    Some(a) => {
                        let mut b = body.clone();
                        if let Some(o) = b.as_object_mut() {
                            o.insert("id".into(), json!(id));
                            o.insert("aud".into(), a.get("aud").cloned().unwrap_or(Value::Null));
                        }
                        *a = b.clone();
                        ok(b)
                    }
                    None => fail(404, 12130, "not found"),
                },
                ("DELETE", ["accounts", _, "access", "apps", id]) => match s.apps.remove(*id) {
                    Some(_) => ok(json!({"id": id})),
                    None => fail(404, 12130, "not found"),
                },
                ("POST", ["accounts", _, "access", "policies"]) => {
                    let id = format!("pol-{new_id}");
                    let mut p = body.clone();
                    if let Some(o) = p.as_object_mut() {
                        o.insert("id".into(), json!(id));
                    }
                    s.policies.insert(id, p.clone());
                    ok(p)
                }
                ("PUT", ["accounts", _, "access", "policies", id]) => {
                    let mut p = body.clone();
                    if let Some(o) = p.as_object_mut() {
                        o.insert("id".into(), json!(id));
                    }
                    s.policies.insert((*id).to_owned(), p.clone());
                    ok(p)
                }
                ("DELETE", ["accounts", _, "access", "policies", id]) => {
                    let used = s.apps.values().any(|a| {
                        list_of(a.get("policies").unwrap_or(&Value::Null))
                            .iter()
                            .any(|p| str_of(p, "id") == *id)
                    });
                    if used {
                        return fail(400, 12130, "policy is used by an application");
                    }
                    match s.policies.remove(*id) {
                        Some(_) => ok(json!({"id": id})),
                        None => fail(404, 12130, "not found"),
                    }
                }
                _ => fail(404, 7000, "No route for that URI"),
            }
        }
    }

    /// Ingress rules that end with a rule without hostname and path.
    fn valid_ingress(config: &Value) -> bool {
        list_of(config.get("ingress").unwrap_or(&Value::Null))
            .last()
            .is_some_and(|r| r.get("hostname").is_none() && r.get("path").is_none())
    }

    /// The capture server's health, status and pairing answers.
    fn capture_server(
        s: &mut CfState,
        method: &str,
        authority: &str,
        path: &str,
        header: &dyn Fn(&str) -> Option<String>,
        body: Option<&Value>,
    ) -> Result<HttpResponse, FetchError> {
        let local = authority.starts_with("127.0.0.1") || authority.starts_with("192.168.");
        if s.loopback_down && authority.starts_with("127.0.0.1") {
            return Err(FetchError::Network("connection refused".into()));
        }
        if path.starts_with("/api/v1/desktop/") {
            // Revision 16c: Cloudflare Access in front of the public address lets
            // only the service token through; others are sent to its sign-in page
            // (a redirect the fetcher refuses, as the real one does).
            let access_ok = s.service_tokens.values().any(|t| {
                header("CF-Access-Client-Id").as_deref() == Some(str_of(t, "client_id"))
                    && header("CF-Access-Client-Secret").as_deref()
                        == Some(str_of(t, "client_secret"))
            });
            if !local && !access_ok {
                return Err(FetchError::Refused(format!(
                    "a request to {} was refused: only {authority} is allowed",
                    s.team_domain
                )));
            }
            if s.refuse_desktop_token {
                return plain(
                    401,
                    json!({"error": {"code": "unauthorized", "message": "The desktop token is not valid."}}),
                );
            }
        }
        let mut health = json!({"ok": true, "version": s.server_version,
                                "api_level": s.server_api_level});
        if let (Some(cv), Some(o)) = (&s.server_capture_version, health.as_object_mut()) {
            o.insert("capture_version".into(), json!(cv));
        }
        if let (Some(e), Some(o)) = (&s.wifi_error, health.as_object_mut()) {
            o.insert("wifi_error".into(), json!(e));
            o.insert("wifi_listening".into(), json!(false));
        }
        match (method, path) {
            ("GET", api::HEALTH_PATH) if local => {
                if s.local_health {
                    plain(200, health)
                } else {
                    Err(FetchError::Network("connection refused".into()))
                }
            }
            ("GET", api::HEALTH_PATH) => {
                let token_ok = s.service_tokens.values().any(|t| {
                    header("CF-Access-Client-Id").as_deref() == Some(str_of(t, "client_id"))
                        && header("CF-Access-Client-Secret").as_deref()
                            == Some(str_of(t, "client_secret"))
                });
                if s.public_health && token_ok {
                    plain(200, health)
                } else if !token_ok {
                    plain(403, json!({}))
                } else {
                    plain(502, json!({}))
                }
            }
            ("GET", api::DESKTOP_STATUS_PATH) if s.local_health => plain(
                200,
                json!({"waiting": s.waiting + s.inbox.len(), "last_activity": 1_790_000_000, "version": s.server_version,
                       "access_problem": s.access_problem, "wifi": s.wifi}),
            ),
            ("PUT", "/api/v1/desktop/pin") if s.local_health => {
                let pin = body
                    .as_ref()
                    .map(|b| str_of(b, "pin").to_owned())
                    .unwrap_or_default();
                match api::validate_pin(&pin) {
                    Ok(p) => {
                        s.pins.push(p);
                        plain(200, json!({"pin_set": true}))
                    }
                    Err(m) => plain(422, json!({"error": {"code": "invalid", "message": m}})),
                }
            }
            ("POST", "/api/v1/desktop/pairing-codes") if s.local_health => plain(
                201,
                json!({"code": s.pairing_code, "expires_at": 0, "expires_in": 600, "client_url": null}),
            ),
            ("GET", "/api/v1/desktop/devices") if s.local_health => {
                plain(200, json!({"devices": s.devices}))
            }
            ("DELETE", p) if s.local_health && p.starts_with("/api/v1/desktop/devices/") => {
                let id = p.trim_start_matches("/api/v1/desktop/devices/").to_owned();
                let before = s.devices.len();
                s.devices.retain(|d| str_of(d, "id") != id);
                if s.devices.len() < before {
                    plain(200, json!({"revoked": true}))
                } else {
                    plain(
                        404,
                        json!({"error": {"code": "not_found", "message": "No such phone."}}),
                    )
                }
            }
            ("GET", "/api/v1/desktop/inbox") if s.local_health => {
                plain(200, json!({"items": s.inbox}))
            }
            ("POST", p) if s.local_health && p.starts_with("/api/v1/desktop/inbox/") => {
                let id = p
                    .trim_start_matches("/api/v1/desktop/inbox/")
                    .trim_end_matches("/ack")
                    .to_owned();
                s.inbox.retain(|i| str_of(i, "id") != id);
                plain(200, json!({"acknowledged": true}))
            }
            ("PUT", "/api/v1/desktop/lists") if s.local_health => plain(200, json!({})),
            _ if !s.local_health => Err(FetchError::Network("connection refused".into())),
            _ => plain(
                404,
                json!({"error": {"code": "not_found", "message": "No such address."}}),
            ),
        }
    }

    /// What the fake computer was asked to do.
    #[derive(Debug, Default)]
    pub struct HostState {
        pub report: HostReport,
        pub installs: Vec<InstallRequest>,
        pub uninstalls: Vec<UninstallRequest>,
        pub starts: usize,
        pub fail_install: Option<String>,
        pub updates: Vec<UpdateRequest>,
        pub fail_update: Option<String>,
        /// The `server.toml` text the fake "wrote" (what a real install writes).
        pub config_text: String,
        /// `set_access_aud` calls that changed `server.toml` (revision 8).
        pub aud_restarts: usize,
        /// Wi-Fi capture (revision 10): the addresses listed, the requests seen.
        pub addresses: Vec<LocalAddress>,
        pub wifi_installs: Vec<WifiRequest>,
        pub wifi_removes: Vec<WifiRemove>,
        /// What the fake made: the firewall rules present, by name; certificates.
        pub firewall_rules: Vec<FirewallRule>,
        pub wifi_certs: bool,
        pub fail_wifi: Option<String>,
        /// The fake resolver (revision 12): local names and the addresses they
        /// lead to; any other name does not resolve.
        pub names: Vec<(String, Vec<String>)>,
        /// The names looked up.
        pub lookups: Vec<String>,
        /// Revision 17: Start over's computer part, as asked; the permission prompt
        /// declined; the capture server's data folder is there.
        pub start_overs: Vec<StartOverRequest>,
        pub decline_start_over: bool,
        pub data_folder: bool,
        /// Revision 18: the firewall as the fake reports it — MoneyKeep's rules are
        /// turned off; Windows' own rules for the server (Block and Allow); the
        /// networks Windows treats as Public; the fixes asked; a declined prompt.
        pub rules_disabled: bool,
        pub windows_rules: Vec<FoundRule>,
        pub public_networks: Vec<String>,
        pub firewall_fixes: Vec<FirewallFix>,
        pub decline_firewall_fix: bool,
        /// security-review-1 A6: Install the server again, as asked; it fails
        /// with this; the prompt declined.
        pub reinstalls: Vec<ReinstallRequest>,
        pub fail_reinstall: Option<String>,
        pub decline_reinstall: bool,
        /// macos-release.md C5: macOS's application firewall is on.
        pub app_firewall: Option<bool>,
    }

    /// A computer with services, for tests.
    #[derive(Debug, Clone, Default)]
    pub struct FakeHost {
        pub state: Arc<Mutex<HostState>>,
    }

    pub const DESKTOP_TOKEN: &str = "ZGVza3RvcC10b2tlbi1mb3ItdGVzdHMtMDEyMzQ1Njc";

    impl FakeHost {
        /// A Windows computer with the server program and nothing installed.
        pub fn windows() -> Self {
            let h = Self::default();
            h.with(|s| {
                s.report = HostReport {
                    os: "windows".into(),
                    server_program: true,
                    program_has_app: true,
                    program_version: api::BUILD_VERSION.into(),
                    program_capture_version: api::CAPTURE_VERSION.into(),
                    can_download_cloudflared: true,
                    lan_addresses: vec!["192.168.1.20".into()],
                    ..HostReport::default()
                };
            });
            h
        }

        /// A Mac (macos-release.md Part C) with the server program inside the app
        /// bundle and nothing installed; `cloudflared` is never downloaded there.
        pub fn macos() -> Self {
            let h = Self::windows();
            h.with(|s| {
                s.report.os = "macos".into();
                s.report.can_download_cloudflared = false;
            });
            h
        }

        pub fn with<T>(&self, f: impl FnOnce(&mut HostState) -> T) -> Option<T> {
            self.state.lock().ok().map(|mut s| f(&mut s))
        }
    }

    impl CaptureHost for FakeHost {
        fn inspect(&self) -> HostReport {
            self.with(|s| s.report.clone()).unwrap_or_default()
        }
        fn install(&self, req: &InstallRequest) -> Result<InstallOutcome, String> {
            self.with(|s| {
                if let Some(e) = &s.fail_install {
                    return Err(e.clone());
                }
                // Like the real helper: a service for another tunnel is replaced only
                // when it runs the tunnel named for replacing (revision 16).
                let mut replaced = false;
                if req.tunnel_token.is_some() && s.report.cloudflared_service.is_some() {
                    let runs = s.report.cloudflared_tunnel.clone();
                    match (&req.replace_cloudflared_tunnel, &runs) {
                        (Some(want), Some(have)) if want == have => replaced = true,
                        _ if runs.is_none() && req.replace_cloudflared_tunnel.is_none() => {}
                        _ => {
                            return Err("A cloudflared service for another tunnel is already installed on this computer; it was not replaced.".to_owned());
                        }
                    }
                }
                s.installs.push(req.clone());
                s.config_text = format!(
                    "listen = \"{}\"\naccess_team_domain = \"{}\"\naccess_aud = \"{}\"\nclient_url = \"{}\"\naccess_service_client_id = \"{}\"\n",
                    req.listen, req.team_domain, req.aud, req.client_url, req.service_client_id
                );
                s.report.server = ServiceState::Running;
                let mut actions = vec!["moneykeep-server service installed".to_owned()];
                if let Some(token) = &req.tunnel_token {
                    s.report.cloudflared_installed = true;
                    s.report.cloudflared_service = Some(ServiceState::Running);
                    s.report.cloudflared_tunnel = token
                        .strip_prefix("TUNNEL-TOKEN-")
                        .map(str::to_owned);
                    s.report.cloudflared_account = None;
                    if replaced {
                        actions.push("cloudflared service of a deleted tunnel removed".into());
                    }
                    actions.push("cloudflared service installed".into());
                }
                let host = req.listen.split(':').next().unwrap_or("127.0.0.1");
                Ok(InstallOutcome {
                    desktop_token: DESKTOP_TOKEN.into(),
                    local_url: format!("http://{host}:{SERVER_PORT}"),
                    actions,
                    cloudflared_service: req.tunnel_token.is_some(),
                    cloudflared_replaced: replaced,
                })
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn uninstall(&self, req: &UninstallRequest) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.uninstalls.push(*req);
                let mut done = Vec::new();
                if req.server {
                    s.report.server = ServiceState::NotInstalled;
                    done.push("moneykeep-server service removed".to_owned());
                }
                if req.cloudflared {
                    s.report.cloudflared_service = None;
                    done.push("cloudflared service removed".to_owned());
                }
                done
            })
            .ok_or_else(|| "fake poisoned".to_owned())
        }
        fn update_server(&self, req: &UpdateRequest) -> Result<Vec<String>, String> {
            self.with(|s| {
                if let Some(e) = &s.fail_update {
                    return Err(e.clone());
                }
                s.updates.push(*req);
                s.report.server = ServiceState::Running;
                Ok(vec!["capture server updated and running".to_owned()])
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn set_access_aud(
            &self,
            aud: &str,
            update: &UpdateRequest,
        ) -> Result<(bool, Vec<String>), String> {
            self.with(|s| {
                let mut changed = false;
                let lines: Vec<String> = s
                    .config_text
                    .lines()
                    .map(|l| {
                        if l.starts_with("access_aud") {
                            let new = format!("access_aud = \"{aud}\"");
                            changed = l != new;
                            new
                        } else {
                            l.to_owned()
                        }
                    })
                    .collect();
                if !changed {
                    return Ok((false, Vec::new()));
                }
                if let Some(e) = &s.fail_update {
                    return Err(e.clone());
                }
                s.config_text = lines.join("\n") + "\n";
                s.aud_restarts += 1;
                let _ = update;
                s.report.server = ServiceState::Running;
                Ok((true, vec!["capture server restarted".to_owned()]))
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn start_server(&self) -> Result<(), String> {
            self.with(|s| {
                s.starts += 1;
                s.report.server = ServiceState::Running;
            })
            .ok_or_else(|| "fake poisoned".to_owned())
        }
        fn wifi_addresses(&self) -> Result<Vec<LocalAddress>, String> {
            Ok(self.with(|s| s.addresses.clone()).unwrap_or_default())
        }
        fn wifi_install(&self, req: &WifiRequest) -> Result<WifiOutcome, String> {
            self.with(|s| {
                if let Some(e) = &s.fail_wifi {
                    return Err(e.clone());
                }
                let moved = s
                    .wifi_installs
                    .last()
                    .is_some_and(|l| l.address != req.address || l.name != req.name);
                let fresh = s.wifi_installs.is_empty() || moved;
                s.wifi_installs.push(req.clone());
                s.firewall_rules.retain(|r| r.name != req.firewall.name);
                let windows = s.report.os == "windows";
                if windows {
                    s.firewall_rules.push(req.firewall.clone());
                }
                s.wifi_certs = true;
                s.report.server = ServiceState::Running;
                let fp = if req.address.ends_with(".20") {
                    "A1B2 C3D4 E5F6 0718 293A 4B5C 6D7E 8F90"
                } else {
                    "0F1E 2D3C 4B5A 6978 8796 A5B4 C3D2 E1F0"
                };
                Ok(WifiOutcome {
                    desktop_token: req.new_desktop_token.then(|| DESKTOP_TOKEN.to_owned()),
                    local_url: format!("http://127.0.0.1:{SERVER_PORT}"),
                    ca_name: "MoneyKeep Capture CA (OFFICE-PC)".into(),
                    fingerprint: fp.into(),
                    ca_created: fresh,
                    ca_not_after: 2_100_000_000,
                    cert_not_before: 1_790_000_000,
                    cert_not_after: 1_861_000_000,
                    actions: vec![
                        "capture server installed with the Wi-Fi listener".into(),
                        format!("certificate for {} made", req.address),
                    ],
                    firewall_added: windows,
                    answered: true,
                    check_error: None,
                })
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn wifi_remove(&self, req: &WifiRemove) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.wifi_removes.push(req.clone());
                s.firewall_rules.retain(|r| r.name != req.firewall_rule);
                s.wifi_certs = false;
                vec![
                    format!("firewall rule \"{}\" removed", req.firewall_rule),
                    "Wi-Fi certificates removed".to_owned(),
                ]
            })
            .ok_or_else(|| "fake poisoned".to_owned())
        }
        fn start_over(&self, req: &StartOverRequest) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.start_overs.push(req.clone());
                if s.decline_start_over {
                    return Err(PERMISSION_DECLINED.to_owned());
                }
                let mut done = Vec::new();
                if req.cloudflared && s.report.cloudflared_service.is_some() {
                    let runs = s.report.cloudflared_tunnel.clone();
                    if req.cloudflared_tunnel.is_none() || req.cloudflared_tunnel == runs {
                        s.report.cloudflared_service = None;
                        s.report.cloudflared_tunnel = None;
                        s.report.cloudflared_account = None;
                        done.push("cloudflared service removed".to_owned());
                    }
                }
                if let Some(name) = &req.firewall_rule {
                    s.firewall_rules.retain(|r| r.name != *name);
                    done.push(format!("firewall rule \"{name}\" removed"));
                }
                if req.server {
                    s.report.server = ServiceState::NotInstalled;
                    done.push("capture server service removed".to_owned());
                }
                if req.data_folder {
                    s.data_folder = false;
                    s.wifi_certs = false;
                    done.push("capture server data folder removed".to_owned());
                }
                Ok(done)
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn firewall_check(&self, rule_name: &str) -> Result<FirewallState, String> {
            self.with(|s| {
                let managed = s.report.os == "windows";
                let ours = s.firewall_rules.iter().any(|r| r.name == rule_name);
                FirewallState {
                    managed,
                    rule: if !managed || (ours && !s.rules_disabled) {
                        "ok"
                    } else if ours {
                        "disabled"
                    } else {
                        "missing"
                    }
                    .into(),
                    block_rules: s
                        .windows_rules
                        .iter()
                        .filter(|r| r.action == "Block" && r.direction == "Inbound")
                        .cloned()
                        .collect(),
                    public_networks: s.public_networks.clone(),
                    error: None,
                    server: Some(s.report.server),
                    app_firewall: s.app_firewall,
                }
            })
            .ok_or_else(|| "fake poisoned".to_owned())
        }
        fn firewall_fix(&self, req: &FirewallFix) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.firewall_fixes.push(req.clone());
                if s.decline_firewall_fix {
                    return Err(PERMISSION_DECLINED.to_owned());
                }
                let mut done = Vec::new();
                if let Some(rule) = &req.add_rule {
                    s.firewall_rules.retain(|r| r.name != rule.name);
                    s.firewall_rules.push(rule.clone());
                    s.rules_disabled = false;
                    done.push(format!("firewall rule \"{}\" put back", rule.name));
                }
                for id in &req.remove_block_rules {
                    // Like the real helper: only Block rules are removed.
                    let before = s.windows_rules.len();
                    s.windows_rules
                        .retain(|r| !(r.id == *id && r.action == "Block"));
                    if s.windows_rules.len() < before {
                        done.push(format!("Windows' Block rule {id} removed"));
                    }
                }
                Ok(done)
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn reinstall_server(&self, req: &ReinstallRequest) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.reinstalls.push(req.clone());
                if s.decline_reinstall {
                    return Err(PERMISSION_DECLINED.to_owned());
                }
                if let Some(e) = &s.fail_reinstall {
                    return Err(e.clone());
                }
                s.report.server = ServiceState::Running;
                s.report.data_found = false;
                let mut done = vec!["capture server installed again and running".to_owned()];
                if let Some(rule) = &req.firewall_rule
                    && s.report.os == "windows"
                {
                    s.firewall_rules.retain(|r| r.name != rule.name);
                    s.firewall_rules.push(rule.clone());
                    done.push(format!("firewall rule \"{}\" added", rule.name));
                }
                Ok(done)
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn remove_old_copy(&self) -> Result<Vec<String>, String> {
            self.with(|s| match s.report.old_copy.take() {
                Some(c) => Ok(vec![format!("old copy {} removed", c.path)]),
                None => Err("There is no old copy to remove.".to_owned()),
            })
            .unwrap_or_else(|| Err("fake poisoned".into()))
        }
        fn resolve_name(&self, name: &str) -> Result<Vec<String>, String> {
            self.with(|s| {
                s.lookups.push(name.to_owned());
                s.names
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, a)| a.clone())
                    .unwrap_or_default()
            })
            .ok_or_else(|| "fake poisoned".to_owned())
        }
    }
}
