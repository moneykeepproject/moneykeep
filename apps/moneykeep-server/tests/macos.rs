//! macos-release.md Part C at the server, on any system: the launchd agent's
//! property list (MAC-6), the inbox key sealed with the login keychain through a
//! fake key store (MAC-7), and which `cloudflared` may be run (MAC-8). Nothing here
//! calls `launchctl`, `codesign` or the real keychain.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moneykeep_server::KeyHome;
use moneykeep_server::secrets::{KEYCHAIN_SERVICE, KeychainSealer, MemoryCredentials};
use moneykeep_server::services::{
    self, Downloader, Env, NOT_CLOUDFLARE_SIGNED, Probe, SignatureChecker, macos,
};
use moneykeep_server::store::{self, KEY_UNOPENABLE, KeySealer};
use moneykeep_server::upgrade::Refusal;

const NOW: i64 = 1_800_000_000;

/// MAC-6: the server's agent, golden: label, the program inside the app bundle,
/// the data folder argument, `RunAtLoad`, `KeepAlive`, `ThrottleInterval` 10, the
/// log paths; paths with spaces and `&` are XML-escaped and read back unchanged.
#[test]
fn mac6_server_agent_plist() {
    let exe = "/Applications/MoneyKeep.app/Contents/MacOS/moneykeep-server";
    let data = "/Users/Pat & Sam/Library/Application Support/MoneyKeep/capture-server";
    let args: Vec<String> = [exe, "service-run", "--data", data, "--key-file"]
        .map(str::to_owned)
        .to_vec();
    // A Mac path on every system (`Path::join` would add `\` on Windows).
    let log = PathBuf::from(format!("{data}/{}", macos::SERVER_LOG));
    let text = macos::agent_plist(services::SERVER_LABEL, &args, &log);
    let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key>
  <string>com.moneykeep.capture-server</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Applications/MoneyKeep.app/Contents/MacOS/moneykeep-server</string>
    <string>service-run</string>
    <string>--data</string>
    <string>/Users/Pat &amp; Sam/Library/Application Support/MoneyKeep/capture-server</string>
    <string>--key-file</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/Users/Pat &amp; Sam/Library/Application Support/MoneyKeep/capture-server/server.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/Pat &amp; Sam/Library/Application Support/MoneyKeep/capture-server/server.log</string>
</dict>
</plist>
";
    assert_eq!(text, want);
    assert_eq!(macos::plist_arguments(&text).unwrap(), args);
    assert_eq!(
        services::data_of(&macos::plist_arguments(&text).unwrap()).unwrap(),
        PathBuf::from(data)
    );
    // Markup in a path cannot end the string early.
    let odd = vec!["/tmp/a<b>\"c'd/moneykeep-server".to_owned()];
    let t = macos::agent_plist("x", &odd, Path::new("/tmp/l"));
    assert!(t.contains("a&lt;b&gt;&quot;c&apos;d"));
    assert_eq!(macos::plist_arguments(&t).unwrap(), odd);
}

/// C3, U1.4: `launchctl print` read for the state and the last exit code (a
/// refusal's code), and `socketfilterfw --getglobalstate` for C5.
#[test]
fn mac_launchctl_and_firewall_output() {
    let running = "gui/501/com.moneykeep.capture-server = {\n\tactive count = 1\n\tstate = running\n\tpid = 812\n\tlast exit code = (never exited)\n}\n";
    assert!(macos::print_running(running));
    assert_eq!(macos::print_last_exit(running), None);
    let code = Refusal::Key.code();
    let refused = format!(
        "gui/501/com.moneykeep.capture-server = {{\n\tstate = waiting\n\tlast exit code = {code}: EX_SOMETHING\n}}\n"
    );
    assert!(!macos::print_running(&refused));
    assert_eq!(macos::print_last_exit(&refused), Some(u32::from(code)));
    assert_eq!(
        macos::print_last_exit(&refused).and_then(Refusal::from_code),
        Some(Refusal::Key)
    );
    assert_eq!(
        macos::parse_firewall_state("Firewall is enabled. (State = 1)\n"),
        Some(true)
    );
    assert_eq!(
        macos::parse_firewall_state(
            "Firewall is blocking all non-essential incoming connections. (State = 2)"
        ),
        Some(true)
    );
    assert_eq!(
        macos::parse_firewall_state("Firewall is disabled. (State = 0)\n"),
        Some(false)
    );
    assert_eq!(macos::parse_firewall_state("nonsense"), None);
}

/// C6: the `cloudflared` agent's command line keeps the token file's path whole
/// (the data folder's path has a space), so the tunnel it runs is still known.
#[test]
fn mac_cloudflared_agent_command_line() {
    let tunnel = "0f5a8b7c-1234-4abc-9def-0123456789ab";
    let file = format!(
        "/Users/pat/Library/Application Support/MoneyKeep/capture-server/{}",
        services::token_file_name(Some(tunnel), Some("acct01"))
    );
    let args: Vec<String> = [
        "/opt/homebrew/bin/cloudflared",
        "tunnel",
        "run",
        "--token-file",
        &file,
    ]
    .map(str::to_owned)
    .to_vec();
    let line = macos::command_line(&args);
    assert_eq!(services::split_command_line(&line), args);
    assert_eq!(
        services::tunnel_of(&line),
        (Some(tunnel.to_owned()), Some("acct01".to_owned()))
    );
    assert!(!line.contains("--token "), "the token is never on the line");
}

fn sealer(dir: &Path, creds: &Arc<MemoryCredentials>) -> KeychainSealer {
    KeychainSealer::new(dir, creds.clone())
}

/// MAC-7 (C4) with a fake key store: sealing, opening, the damaged-seal fallback
/// to the plain key (sealed again) and the refusal for an inbox with rows and no
/// key, which never makes a new one.
#[test]
fn mac7_keychain_sealer() {
    // The sealers here are fakes; `init` below must not seal with the real login keychain.
    store::without_platform_sealer();
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let creds = MemoryCredentials::new();
    let s = sealer(dir, &creds);
    // The keychain account is the data folder's path.
    assert_eq!(
        Path::new(s.account()),
        dir.canonicalize().unwrap().as_path()
    );
    assert_eq!(KEYCHAIN_SERVICE, "MoneyKeep capture server");
    // Opening never makes a keychain key.
    assert!(s.unseal(b"MKK1").is_err());
    assert!(creds.names().is_empty());

    // Seal and open: a new folder's key is sealed, never written plain.
    let key = store::load_key_with(dir, KeyHome::File, Some(&s)).unwrap();
    assert!(!dir.join(store::KEY_FILE).exists());
    let blob = std::fs::read(dir.join(store::SEALED_KEY_FILE)).unwrap();
    assert!(blob.starts_with(b"MKK1"));
    let text = s.unseal(&blob).unwrap();
    assert_eq!(
        base64_decode(std::str::from_utf8(&text).unwrap()),
        key.to_vec()
    );
    assert_eq!(creds.names(), vec![s.account().to_owned()]);
    // The keychain holds the sealing key, not the inbox key.
    assert!(!creds.values()[0].contains(std::str::from_utf8(&text).unwrap()));
    assert_eq!(
        *store::load_key_with(dir, KeyHome::File, Some(&sealer(dir, &creds))).unwrap(),
        *key
    );
    // Another data folder's (or user's) keychain key does not open it.
    assert!(
        sealer(dir, &MemoryCredentials::new())
            .unseal(&blob)
            .is_err()
    );

    // U1.2: a damaged seal with the plain key present: the plain key is used and
    // sealed again.
    let d2 = tempfile::tempdir().unwrap();
    let dir2 = d2.path();
    moneykeep_server::init(dir2, KeyHome::File, NOW).unwrap();
    let plain_key = store::load_key_with(dir2, KeyHome::File, None).unwrap();
    std::fs::write(dir2.join(store::SEALED_KEY_FILE), b"MKK1 cut sh").unwrap();
    let creds2 = MemoryCredentials::new();
    let opened = store::load_key_with(dir2, KeyHome::File, Some(&sealer(dir2, &creds2))).unwrap();
    assert_eq!(*opened, *plain_key);
    assert!(!dir2.join(store::KEY_FILE).exists(), "sealed again");
    assert_eq!(
        *store::load_key_with(dir2, KeyHome::File, Some(&sealer(dir2, &creds2))).unwrap(),
        *plain_key
    );

    // The keychain key lost (another Mac, the keychain reset) and no plain key:
    // the inbox has rows, so it is refused and no new key is made.
    assert!(store::inbox_has_rows(dir2));
    let before = std::fs::read(dir2.join(store::SEALED_KEY_FILE)).unwrap();
    let empty = MemoryCredentials::new();
    assert_eq!(
        store::load_key_with(dir2, KeyHome::File, Some(&sealer(dir2, &empty))).unwrap_err(),
        KEY_UNOPENABLE
    );
    assert!(empty.names().is_empty(), "no keychain key made");
    assert_eq!(
        std::fs::read(dir2.join(store::SEALED_KEY_FILE)).unwrap(),
        before
    );
    assert!(!dir2.join(store::KEY_FILE).exists());
    // No key file at all while the inbox has rows: refused too.
    std::fs::remove_file(dir2.join(store::SEALED_KEY_FILE)).unwrap();
    assert_eq!(
        store::load_key_with(dir2, KeyHome::File, Some(&sealer(dir2, &creds2))).unwrap_err(),
        KEY_UNOPENABLE
    );
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .unwrap()
}

struct Signed(&'static str);

impl SignatureChecker for Signed {
    fn signer(&self, _: &Path) -> Result<String, String> {
        Ok(self.0.to_owned())
    }
}

struct NoDownloads;

impl Downloader for NoDownloads {
    fn download(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
        Err("no downloads".into())
    }
}

struct Healthy;

impl Probe for Healthy {
    fn healthy(&self, _: &Path) -> bool {
        true
    }
}

fn check(sig: &dyn SignatureChecker, cf: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir();
    let env = Env {
        sm: &services::Unsupported,
        sig,
        dl: &NoDownloads,
        probe: &Healthy,
        can_download: false,
        tmp: &tmp,
        install_dir: &tmp,
        data_dir: &tmp,
        machine_dir: None,
        check_cloudflared: true,
        legacy: None,
        source_dirs: &[],
        sealer: None,
    };
    services::verify_cloudflared(&env, Path::new(cf))
}

/// MAC-8 (C6): `cloudflared` outside the two folders, or signed by another team,
/// is refused with the A2-style message; Cloudflare's Team ID in one of the two
/// folders is run. No `osascript` remains.
#[test]
fn mac8_cloudflared_rules() {
    let theirs = macos::CLOUDFLARE_TEAM_ID.map(|t| format!("TeamIdentifier={t}"));
    let theirs: &'static str = Box::leak(
        theirs
            .unwrap_or_else(|| "TeamIdentifier=ABCDE12345".to_owned())
            .into_boxed_str(),
    );
    for dir in macos::CLOUDFLARED_DIRS {
        let cf = format!("{dir}/cloudflared");
        let r = check(&Signed(theirs), &cf);
        match macos::CLOUDFLARE_TEAM_ID {
            Some(_) => assert_eq!(r, Ok(())),
            None => assert_eq!(r.unwrap_err(), macos::TEAM_UNKNOWN),
        }
    }
    for elsewhere in [
        "/Users/pat/bin/cloudflared",
        "/usr/local/bin/sub/cloudflared",
        "/tmp/cloudflared",
        "/usr/local/bin/cloudflared-copy",
    ] {
        assert_eq!(
            check(&Signed(theirs), elsewhere).unwrap_err(),
            macos::NOT_CLOUDFLARED_PLACE,
            "{elsewhere}"
        );
    }
    if macos::CLOUDFLARE_TEAM_ID.is_some() {
        assert_eq!(
            check(
                &Signed("TeamIdentifier=ZZZZZ99999"),
                "/opt/homebrew/bin/cloudflared"
            )
            .unwrap_err(),
            NOT_CLOUDFLARE_SIGNED
        );
    }
    // Unsigned, or a signer that is neither form: refused.
    assert_eq!(
        check(&Signed("TeamIdentifier="), "/opt/homebrew/bin/cloudflared").unwrap_err(),
        NOT_CLOUDFLARE_SIGNED
    );
    assert_eq!(
        check(&Signed("Someone Else"), "/opt/homebrew/bin/cloudflared").unwrap_err(),
        NOT_CLOUDFLARE_SIGNED
    );
    // codesign's display of a signature.
    assert_eq!(
        macos::parse_team_id(
            "Executable=/opt/homebrew/bin/cloudflared\nAuthority=Developer ID Application: X (ABCDE12345)\nTeamIdentifier=ABCDE12345\n"
        ),
        Some("ABCDE12345".to_owned())
    );
    assert_eq!(macos::parse_team_id("TeamIdentifier=not set"), None);
    // The administrator path through osascript is gone (C6).
    let src = include_str!("../src/services.rs");
    assert!(!src.contains("osascript"));
    assert!(!src.contains("with administrator privileges"));
}

/// C1: the server program inside the app bundle is the one the agent runs.
#[test]
fn mac_bundle_program_dir() {
    assert!(services::bundle_program_dir(Path::new(
        "/Applications/MoneyKeep.app/Contents/MacOS"
    )));
    assert!(services::bundle_program_dir(Path::new(
        "/Users/pat/Apps/MoneyKeep.app/Contents/MacOS"
    )));
    assert!(!services::bundle_program_dir(Path::new(
        "/Users/pat/moneykeep/target/release"
    )));
    assert!(!services::bundle_program_dir(Path::new(
        "/Applications/MoneyKeep/Contents/MacOS"
    )));
}
