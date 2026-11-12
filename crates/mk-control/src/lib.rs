//! MoneyKeep's local control channel (mcp-server.md revision 3).
//!
//! The running app serves the book it has open to `keep-mcp` over newline-delimited
//! JSON on a loopback TCP socket (`127.0.0.1`, a port the OS chooses), advertised
//! by the discovery file `control.json` in the app's config folder. Both sides live
//! in this crate so they cannot drift apart: [`server`] is the app's handler (driven
//! by the app, or by tests without a window), [`client`] is what `keep-mcp` uses.
//!
//! The channel never carries secrets and has no operation for session operations
//! (encryption, AI access settings, credentials).

pub mod client;
pub mod server;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use mk_commands::ai_access::{AiAccess, Group, Level};

/// The protocol name in the client's hello.
pub const PROTOCOL: &str = "moneykeep-control/1";
/// The discovery file in the app's config folder.
pub const DISCOVERY_FILE: &str = "control.json";
/// Longest accepted line (longer lines close the connection).
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Most connections at once; a further hello gets `busy`.
pub const MAX_CONNECTIONS: usize = 4;
/// How long the owner has to answer the approval dialog.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
/// The client kind of `keep-mcp`.
pub const KIND_KEEP_MCP: &str = "keep-mcp";

/// The contents of `control.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub book_path: String,
}

/// The app's per-user config folder (where `control.json` lives).
pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("org", mk_core::brand::FOLDER, mk_core::brand::FOLDER)
        .map(|d| d.config_dir().to_path_buf())
}

/// The discovery file in `dir`.
pub fn discovery_path(dir: &Path) -> PathBuf {
    dir.join(DISCOVERY_FILE)
}

/// The discovery file in `dir`, if present and of a known version.
pub fn read_discovery(dir: &Path) -> Option<Discovery> {
    let text = std::fs::read_to_string(discovery_path(dir)).ok()?;
    if text.len() > 64 * 1024 {
        return None;
    }
    let d: Discovery = serde_json::from_str(&text).ok()?;
    (d.version == 1).then_some(d)
}

/// Whether process `pid` is running (Linux checks `/proc`; elsewhere this cannot be
/// told without platform calls, and the port check in [`is_live`] decides).
fn pid_running(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        Path::new("/proc").join(pid.to_string()).exists()
    } else {
        true
    }
}

/// Whether a discovery file belongs to a running app: its process is running and
/// something accepts connections on its port.
pub fn is_live(d: &Discovery) -> bool {
    if !pid_running(d.pid) {
        return false;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], d.port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// The live discovery file in `dir`, if any.
pub fn live_discovery(dir: &Path) -> Option<Discovery> {
    read_discovery(dir).filter(is_live)
}

/// 32 random bytes from the OS, as 64 lowercase hex characters (RFC 4648 base16).
pub fn new_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| format!("no random source: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Constant-time comparison (the length is not secret).
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A path as the channel compares it: canonical when the file exists.
pub fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Access settings as sent on the channel (`level`, `groups`).
pub fn access_from(level: Option<&Value>, groups: Option<&Value>) -> AiAccess {
    AiAccess::from_json(&json!({
        "enabled": true,
        "level": level.cloned().unwrap_or(Value::Null),
        "groups": groups.cloned().unwrap_or(Value::Null),
        "app_only": true,
    }))
}

/// The MCP clients `keep-mcp config` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    ClaudeDesktop,
    ClaudeCode,
    Generic,
}

impl ClientKind {
    pub const ALL: [ClientKind; 3] = [
        ClientKind::ClaudeDesktop,
        ClientKind::ClaudeCode,
        ClientKind::Generic,
    ];

    pub fn parse(s: &str) -> Option<ClientKind> {
        match s {
            "claude-desktop" => Some(ClientKind::ClaudeDesktop),
            "claude-code" => Some(ClientKind::ClaudeCode),
            "generic" => Some(ClientKind::Generic),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ClientKind::ClaudeDesktop => "claude-desktop",
            ClientKind::ClaudeCode => "claude-code",
            ClientKind::Generic => "generic",
        }
    }

    /// The name shown in the app.
    pub fn label(self) -> &'static str {
        match self {
            ClientKind::ClaudeDesktop => "Claude Desktop",
            ClientKind::ClaudeCode => "Claude Code",
            ClientKind::Generic => "Other MCP client",
        }
    }
}

/// The JSON to paste into a client's MCP configuration for `keep-mcp` at `exe` with
/// `args` (`[<book path>]` or `["--app"]`).
pub fn config_snippet(client: ClientKind, exe: &str, args: &[String]) -> Value {
    match client {
        ClientKind::ClaudeDesktop => json!({
            "mcpServers": { "moneykeep": { "command": exe, "args": args } }
        }),
        ClientKind::ClaudeCode => json!({
            "mcpServers": { "moneykeep": { "type": "stdio", "command": exe, "args": args } }
        }),
        ClientKind::Generic => json!({ "command": exe, "args": args }),
    }
}

/// The `keep-mcp` executable's file name on this platform.
pub fn keep_mcp_file_name() -> &'static str {
    if cfg!(windows) {
        "keep-mcp.exe"
    } else {
        "keep-mcp"
    }
}

/// `keep-mcp` next to `app_dir` or on `path_var` (the `PATH` value), if installed.
pub fn find_keep_mcp(
    app_dir: Option<&Path>,
    path_var: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let name = keep_mcp_file_name();
    if let Some(d) = app_dir {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::split_paths(path_var?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn tokens_are_64_hex_and_compare_in_constant_time() {
        let a = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_token().unwrap());
        assert!(token_eq(&a, &a.clone()));
        assert!(!token_eq(&a, "x"));
    }

    #[test]
    fn snippets() {
        let s = config_snippet(
            ClientKind::ClaudeDesktop,
            "/opt/keep-mcp",
            &["/b.moneykeep".into()],
        );
        assert_eq!(s["mcpServers"]["moneykeep"]["command"], "/opt/keep-mcp");
        assert_eq!(s["mcpServers"]["moneykeep"]["args"][0], "/b.moneykeep");
        let g = config_snippet(ClientKind::Generic, "k", &["--app".into()]);
        assert_eq!(g, json!({"command": "k", "args": ["--app"]}));
    }
}
