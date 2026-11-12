//! mobile-capture.md revision 2 acceptance tests 23 and 24 at the service layer, and
//! the installation plan: a fake service manager, signature checker and downloader
//! stand in for the operating system (nothing is installed or downloaded).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use moneykeep_server::services::{
    self, CLOUDFLARED_MSI_URL, Downloader, Env, Plan, Probe, ServiceManager, SignatureChecker,
    State,
};

#[derive(Default)]
struct FakeSm {
    server: RefCell<State>,
    /// The program the service is registered with.
    path: RefCell<Option<PathBuf>>,
    /// Starting fails (a broken new program).
    fail_start: RefCell<bool>,
    cloudflared: RefCell<Option<PathBuf>>,
    service: RefCell<Option<(State, String)>>,
    log: RefCell<Vec<String>>,
    /// Firewall rules present (Windows-like), by name.
    rules: RefCell<Vec<services::FirewallRule>>,
    /// Revision 16b: cloudflared's event-log source key is present; the install
    /// fails with this error (after making the service, like cloudflared does).
    event_log: RefCell<bool>,
    install_error: RefCell<Option<String>>,
    /// The key survives removing the service (as after `sc.exe delete`).
    keep_event_log: RefCell<bool>,
    /// Revision 18: a Windows-like firewall (the scan answers); MoneyKeep's rules
    /// are turned off; the other rules Windows has (its own, by id); the networks.
    firewall: RefCell<bool>,
    rules_disabled: RefCell<bool>,
    other_rules: RefCell<Vec<services::FoundRule>>,
    networks: RefCell<Vec<services::NetProfile>>,
    /// The service's command line (program first).
    command: RefCell<Option<Vec<String>>>,
    /// security-review-1 A1: folder owners (security identifiers) by path; folders
    /// protected, with their use.
    owners: RefCell<Vec<(PathBuf, String)>>,
    protected: RefCell<Vec<(PathBuf, services::FolderUse)>>,
    /// L1: `cloudflared --version`, and the command lines set for its service.
    cf_version: RefCell<Option<String>>,
    cf_commands: RefCell<Vec<Vec<String>>>,
    /// A2: holding the installer, its signature check, msiexec, the release.
    events: std::rc::Rc<RefCell<Vec<String>>>,
}

/// Logs its release (A2: the installer is held until msiexec has finished).
struct Held {
    _file: std::fs::File,
    events: std::rc::Rc<RefCell<Vec<String>>>,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.events.borrow_mut().push("release".into());
    }
}

impl ServiceManager for FakeSm {
    fn folder_owner(&self, dir: &Path) -> Result<Option<String>, String> {
        Ok(self
            .owners
            .borrow()
            .iter()
            .find(|(p, _)| p == dir)
            .map(|(_, o)| o.clone())
            .or_else(|| Some("S-1-5-32-544".into())))
    }
    fn protect_folder(&self, dir: &Path, use_: services::FolderUse) -> Result<(), String> {
        self.protected.borrow_mut().push((dir.to_path_buf(), use_));
        Ok(())
    }
    fn hold_file(&self, file: &Path) -> Result<Box<dyn std::any::Any>, String> {
        // The real hold: read only, others may only read (FILE_SHARE_READ).
        let f = services::hold_read_only(file)?;
        self.events.borrow_mut().push("hold".into());
        Ok(Box::new(Held {
            _file: f,
            events: self.events.clone(),
        }))
    }
    fn cloudflared_version(&self, _: &Path) -> Option<String> {
        self.cf_version.borrow().clone()
    }
    fn set_cloudflared_command(&self, exe: &Path, args: &[String]) -> Result<(), String> {
        let line: Vec<String> = std::iter::once(exe.display().to_string())
            .chain(args.iter().cloned())
            .collect();
        *self.service.borrow_mut() = Some((State::Running, line.join(" ")));
        self.cf_commands.borrow_mut().push(args.to_vec());
        Ok(())
    }
    fn add_firewall_rule(&self, rule: &services::FirewallRule) -> Result<bool, String> {
        services::firewall_script(rule)?;
        self.rules.borrow_mut().retain(|r| r.name != rule.name);
        self.rules.borrow_mut().push(rule.clone());
        Ok(true)
    }
    fn remove_firewall_rule(&self, name: &str) -> Result<bool, String> {
        self.rules.borrow_mut().retain(|r| r.name != name);
        Ok(true)
    }
    fn firewall_scan(
        &self,
        rule_name: &str,
        program: &str,
    ) -> Result<Option<services::FirewallScan>, String> {
        if !*self.firewall.borrow() {
            return Ok(None);
        }
        let named = self
            .rules
            .borrow()
            .iter()
            .filter(|r| r.name == rule_name)
            .map(|r| services::FoundRule {
                id: format!("{{{}}}", r.name.len()),
                display_name: r.name.clone(),
                enabled: !*self.rules_disabled.borrow(),
                action: "Allow".into(),
                direction: "Inbound".into(),
                program: r
                    .program
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            })
            .collect();
        let program_rules = self
            .other_rules
            .borrow()
            .iter()
            .filter(|r| {
                r.program
                    .to_ascii_lowercase()
                    .ends_with(&program.to_ascii_lowercase())
            })
            .cloned()
            .collect();
        Ok(Some(services::FirewallScan {
            named,
            program_rules,
        }))
    }
    fn network_profiles(&self) -> Result<Vec<services::NetProfile>, String> {
        Ok(self.networks.borrow().clone())
    }
    fn remove_firewall_rule_id(&self, id: &str) -> Result<(), String> {
        self.log.borrow_mut().push(format!("remove rule id {id}"));
        self.other_rules.borrow_mut().retain(|r| r.id != id);
        Ok(())
    }
    fn server_state(&self) -> State {
        *self.server.borrow()
    }
    fn install_server(&self, exe: &Path, args: &[String]) -> Result<(), String> {
        self.log.borrow_mut().push(format!(
            "install_server {} {}",
            exe.display(),
            args.join(" ")
        ));
        *self.path.borrow_mut() = Some(exe.to_path_buf());
        *self.command.borrow_mut() = Some(
            std::iter::once(exe.display().to_string())
                .chain(args.iter().cloned())
                .collect(),
        );
        // The program the service now runs: a broken one does not start.
        let broken = std::fs::read(exe).is_ok_and(|b| b == b"BROKEN");
        if broken && *self.fail_start.borrow() {
            *self.server.borrow_mut() = State::Stopped;
            return Ok(());
        }
        *self.server.borrow_mut() = State::Running;
        Ok(())
    }
    fn stop_server(&self, _: std::time::Duration) -> Result<(), String> {
        self.log.borrow_mut().push("stop_server".into());
        *self.server.borrow_mut() = State::Stopped;
        Ok(())
    }
    fn server_path(&self) -> Option<PathBuf> {
        self.path.borrow().clone()
    }
    fn server_command(&self) -> Option<Vec<String>> {
        self.command.borrow().clone()
    }
    fn uninstall_server(&self) -> Result<(), String> {
        self.log.borrow_mut().push("uninstall_server".into());
        *self.server.borrow_mut() = State::NotInstalled;
        Ok(())
    }
    fn start_server(&self) -> Result<(), String> {
        self.log.borrow_mut().push("start_server".into());
        Ok(())
    }
    fn cloudflared_path(&self) -> Option<PathBuf> {
        self.cloudflared.borrow().clone()
    }
    fn cloudflared_service(&self) -> Option<(State, String)> {
        self.service.borrow().clone()
    }
    fn install_cloudflared_package(&self, installer: &Path) -> Result<(), String> {
        assert!(installer.is_file(), "the installer exists while it is run");
        self.events.borrow_mut().push("msiexec".into());
        self.log.borrow_mut().push("install_package".into());
        *self.cloudflared.borrow_mut() = Some(PathBuf::from(
            "C:/Program Files/cloudflared/cloudflared.exe",
        ));
        Ok(())
    }
    fn install_cloudflared_service(&self, _: &Path, token: &str) -> Result<(), String> {
        self.log
            .borrow_mut()
            .push("install_cloudflared_service".into());
        if *self.event_log.borrow() {
            // The real cloudflared refuses before making the service.
            return Err("cloudflared.exe failed: Cannot install event logger: SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\Cloudflared registry key already exists".into());
        }
        *self.service.borrow_mut() = Some((
            State::Running,
            format!("cloudflared.exe tunnel run --token {token}"),
        ));
        *self.event_log.borrow_mut() = true;
        match self.install_error.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    fn delete_cloudflared_event_log(&self) -> Result<bool, String> {
        let had = *self.event_log.borrow();
        if had {
            self.log.borrow_mut().push("delete_event_log".into());
        }
        *self.event_log.borrow_mut() = false;
        Ok(had)
    }
    fn uninstall_cloudflared_service(&self, _: &Path) -> Result<(), String> {
        self.log
            .borrow_mut()
            .push("uninstall_cloudflared_service".into());
        *self.service.borrow_mut() = None;
        // `cloudflared service uninstall` removes its event-log source too.
        if !*self.keep_event_log.borrow() {
            *self.event_log.borrow_mut() = false;
        }
        Ok(())
    }
}

struct FakeSig(Result<String, String>);

impl SignatureChecker for FakeSig {
    fn signer(&self, file: &Path) -> Result<String, String> {
        // The downloaded installer exists while it is checked (the fake's
        // cloudflared.exe is only a name).
        assert!(file.is_file() || !file.to_string_lossy().ends_with(".msi"));
        self.0.clone()
    }
}

#[derive(Default)]
struct FakeDl {
    urls: RefCell<Vec<String>>,
}

impl Downloader for FakeDl {
    fn download(&self, url: &str, _: usize) -> Result<Vec<u8>, String> {
        self.urls.borrow_mut().push(url.to_owned());
        Ok(b"MSI installer bytes".to_vec())
    }
}

const SIGNED: &str =
    "CN=\"Cloudflare, Inc.\", O=\"Cloudflare, Inc.\", L=San Francisco, S=California, C=US";

fn plan(tunnel: Option<&str>) -> Plan {
    Plan {
        install_server: true,
        tunnel_token: tunnel.map(str::to_owned),
        ..Plan::default()
    }
}

/// Wi-Fi capture on in the test data folder's server.toml (default ports).
fn wifi_on(tmp: &Path) {
    let dir = tmp.join("capture-server");
    std::fs::create_dir_all(&dir).unwrap();
    let c = moneykeep_server::Config {
        wifi_address: "192.168.1.20".into(),
        ..moneykeep_server::Config::default()
    };
    std::fs::write(
        dir.join("server.toml"),
        moneykeep_server::config::render(&c),
    )
    .unwrap();
}

/// Healthy when the service runs.
struct FakeProbe<'a>(&'a FakeSm);

impl Probe for FakeProbe<'_> {
    fn healthy(&self, _: &Path) -> bool {
        *self.0.server.borrow() == State::Running
    }
}

#[allow(clippy::too_many_arguments)]
fn run_apply(
    p: &Plan,
    exe: &Path,
    sm: &FakeSm,
    sig: &FakeSig,
    dl: &FakeDl,
    can_download: bool,
    tmp: &Path,
    install: &Path,
) -> services::Report {
    services::apply(
        p,
        exe,
        &Env {
            sm,
            sig,
            dl,
            probe: &FakeProbe(sm),
            can_download,
            tmp,
            install_dir: install,
            data_dir: &tmp.join("capture-server"),
            machine_dir: None,
            check_cloudflared: false,
            legacy: None,
            source_dirs: &[],
            sealer: None,
        },
    )
}

/// A stand-in "built" server program.
fn built(dir: &Path, content: &[u8]) -> PathBuf {
    let p = dir.join("build").join(services::program_name());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    p
}

fn apply(p: &Plan, sm: &FakeSm, sig: &FakeSig, dl: &FakeDl, tmp: &Path) -> services::Report {
    let work = tempfile::tempdir().unwrap();
    let exe = built(work.path(), b"NEW");
    run_apply(
        p,
        &exe,
        sm,
        sig,
        dl,
        true,
        tmp,
        &work.path().join("install"),
    )
}

/// AT24: a downloaded installer with an invalid signature, or signed by someone
/// else, is refused and deleted; a valid Cloudflare signature is installed.
#[test]
fn at24_installer_signature_checked() {
    let tmp = tempfile::tempdir().unwrap();
    for (sig, why) in [
        (Err("NotSigned".to_owned()), "its signature is not valid"),
        (Err("HashMismatch".to_owned()), "its signature is not valid"),
        (
            Ok("CN=Someone Else, O=Someone Else".to_owned()),
            "not Cloudflare, Inc.",
        ),
    ] {
        let sm = FakeSm::default();
        let dl = FakeDl::default();
        let r = apply(&plan(Some("TOKEN-A")), &sm, &FakeSig(sig), &dl, tmp.path());
        assert!(!r.ok);
        let e = r.error.unwrap();
        assert!(
            e.contains("installer was refused") && e.contains(why),
            "{e}"
        );
        assert_eq!(dl.urls.borrow().as_slice(), [CLOUDFLARED_MSI_URL]);
        let log = sm.log.borrow();
        assert!(!log.iter().any(|l| l == "install_package"), "{log:?}");
        assert!(!log.iter().any(|l| l == "install_cloudflared_service"));
        assert_eq!(
            std::fs::read_dir(tmp.path())
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name() != "capture-server")
                .count(),
            0,
            "installer deleted"
        );
    }
    let sm = FakeSm::default();
    let dl = FakeDl::default();
    let r = apply(
        &plan(Some("TOKEN-A")),
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &dl,
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(r.cloudflared_service);
    assert_eq!(
        sm.log.borrow()[1..],
        [
            "install_package".to_owned(),
            "install_cloudflared_service".to_owned()
        ]
    );
    assert_eq!(
        std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != "capture-server")
            .count(),
        0
    );
    // No download where it is not allowed (Linux and macOS): a clear message.
    let sm = FakeSm::default();
    let work = tempfile::tempdir().unwrap();
    let r = run_apply(
        &plan(Some("TOKEN-A")),
        &built(work.path(), b"NEW"),
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        false,
        tmp.path(),
        &work.path().join("install"),
    );
    assert!(r.error.unwrap().contains("cloudflared is not installed"));
}

/// AT23: an existing cloudflared service with a different tunnel is not replaced;
/// the same tunnel is left as it is.
#[test]
fn at23_other_tunnel_service_not_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("/usr/bin/cloudflared"));
    *sm.service.borrow_mut() = Some((
        State::Running,
        "\"C:\\cloudflared.exe\" tunnel run --token OTHER-TUNNEL-TOKEN".into(),
    ));
    let dl = FakeDl::default();
    let r = apply(
        &plan(Some("NEW-TOKEN")),
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &dl,
        tmp.path(),
    );
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("it was not replaced"));
    assert!(!sm.log.borrow().iter().any(|l| l.contains("cloudflared")));
    assert!(
        sm.service
            .borrow()
            .as_ref()
            .unwrap()
            .1
            .contains("OTHER-TUNNEL-TOKEN")
    );
    // The same tunnel (setup resumed): nothing to do.
    let r = apply(
        &plan(Some("OTHER-TUNNEL-TOKEN")),
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &dl,
        tmp.path(),
    );
    assert!(r.ok);
    assert!(!r.cloudflared_service);
    assert!(dl.urls.borrow().is_empty());
}

/// The plan installs the server with the inbox key in its data folder, on the LAN
/// address only when asked; removal and start; the report never holds the token.
#[test]
fn plan_install_remove_start() {
    let tmp = tempfile::tempdir().unwrap();
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("/usr/bin/cloudflared"));
    let mut p = plan(Some("SECRET-TUNNEL-TOKEN"));
    p.allow_lan = Some(true);
    let r = apply(
        &p,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    let first = sm.log.borrow()[0].clone();
    assert!(
        first.contains(&format!(
            "--data {} --key-file --allow-lan",
            tmp.path().join("capture-server").display()
        )),
        "{first}"
    );
    let report_json = serde_json::to_string(&r).unwrap();
    assert!(!report_json.contains("SECRET-TUNNEL-TOKEN"));
    assert!(!format!("{p:?}").contains("SECRET-TUNNEL-TOKEN"));
    // Remove both.
    let r = apply(
        &Plan {
            remove_server: true,
            remove_cloudflared: true,
            ..Plan::default()
        },
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok);
    assert_eq!(sm.server_state(), State::NotInstalled);
    assert!(sm.cloudflared_service().is_none());
    let r = apply(
        &Plan {
            start_server: true,
            ..Plan::default()
        },
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok);
    assert_eq!(sm.log.borrow().last().unwrap(), "start_server");
    // A plan file round-trips.
    let back: Plan = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back.tunnel_token.as_deref(), Some("SECRET-TUNNEL-TOKEN"));
}

/// The real downloader refuses hosts other than GitHub's.
#[test]
fn downloader_refuses_other_hosts() {
    let e = services::HttpsDownloader
        .download("https://example.com/cloudflared.msi", 10)
        .unwrap_err();
    assert!(e.contains("not allowed"), "{e}");
    let e = services::HttpsDownloader
        .download("http://github.com/x", 10)
        .unwrap_err();
    assert!(e.contains("not allowed"), "{e}");
}

/// mobile-capture.md AT37: setup registers the service with the install-folder copy,
/// never the build folder; Update server replaces a build-folder registration.
#[test]
fn at37_service_runs_the_install_folder_copy() {
    let work = tempfile::tempdir().unwrap();
    let install = work.path().join("install");
    let exe = built(work.path(), b"VERSION-1");
    let sm = FakeSm::default();
    let r = run_apply(
        &plan(None),
        &exe,
        &sm,
        &FakeSig(Err("x".into())),
        &FakeDl::default(),
        true,
        work.path(),
        &install,
    );
    assert!(r.ok, "{r:?}");
    let registered = sm.server_path().unwrap();
    assert_eq!(registered, install.join(services::program_name()));
    assert_eq!(std::fs::read(&registered).unwrap(), b"VERSION-1");
    // An older install registered with the build folder ...
    *sm.path.borrow_mut() = Some(exe.clone());
    std::fs::write(&exe, b"VERSION-2").unwrap();
    let update = Plan {
        update_server: true,
        ..Plan::default()
    };
    let r = run_apply(
        &update,
        &exe,
        &sm,
        &FakeSig(Err("x".into())),
        &FakeDl::default(),
        true,
        work.path(),
        &install,
    );
    assert!(r.ok, "{r:?}");
    // ... now runs the install-folder copy.
    assert_eq!(
        sm.server_path().unwrap(),
        install.join(services::program_name())
    );
    assert!(
        r.actions.iter().any(|a| a.contains("the service now runs")),
        "{:?}",
        r.actions
    );
}

/// mobile-capture.md AT38 (service layer): Update server stops, copies, restarts and
/// checks health; a new program that does not start is replaced by the old one.
#[test]
fn at38_update_and_rollback() {
    let work = tempfile::tempdir().unwrap();
    let install = work.path().join("install");
    let target = install.join(services::program_name());
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(&target, b"OLD").unwrap();
    let sm = FakeSm::default();
    *sm.server.borrow_mut() = State::Running;
    *sm.path.borrow_mut() = Some(target.clone());
    let update = Plan {
        update_server: true,
        ..Plan::default()
    };
    let exe = built(work.path(), b"NEW");
    let r = run_apply(
        &update,
        &exe,
        &sm,
        &FakeSig(Err("x".into())),
        &FakeDl::default(),
        true,
        work.path(),
        &install,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert!(
        !install
            .join(format!("{}.old", services::program_name()))
            .exists(),
        ".old removed once healthy"
    );
    let log = sm.log.borrow().clone();
    assert_eq!(log[0], "stop_server", "{log:?}");
    assert!(log[1].starts_with("install_server"));
    assert_eq!(sm.server_state(), State::Running);
    // A broken new program: the old one is restored and started.
    *sm.fail_start.borrow_mut() = true;
    let broken = built(work.path(), b"BROKEN");
    let r = run_apply(
        &update,
        &broken,
        &sm,
        &FakeSig(Err("x".into())),
        &FakeDl::default(),
        true,
        work.path(),
        &install,
    );
    assert!(!r.ok);
    assert!(
        r.error
            .unwrap()
            .contains("previous server program was restored")
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(sm.server_state(), State::Running);
}

#[test]
fn service_command_lines() {
    assert_eq!(
        services::program_of(
            "\"C:\\Program Files\\MoneyKeep\\capture\\moneykeep-server.exe\" service-run --data x"
        ),
        Some(PathBuf::from(
            "C:\\Program Files\\MoneyKeep\\capture\\moneykeep-server.exe"
        ))
    );
    assert_eq!(
        services::program_of("/usr/bin/x run"),
        Some(PathBuf::from("/usr/bin/x"))
    );
    assert!(services::install_dir().ends_with("capture"));
}

/// Healthy when the new service runs and its data folder holds the server.
struct DataProbe<'a>(&'a FakeSm);

impl Probe for DataProbe<'_> {
    fn healthy(&self, data_dir: &Path) -> bool {
        *self.0.server.borrow() == State::Running && data_dir.join("server.toml").is_file()
    }
}

/// An installation from before the rename: the old-name service running the old
/// install folder's program with the old data folder.
struct OldInstall {
    work: tempfile::TempDir,
    old_sm: FakeSm,
    old_install: PathBuf,
    old_data: PathBuf,
    new_install: PathBuf,
    new_data: PathBuf,
}

fn old_install() -> OldInstall {
    let work = tempfile::tempdir().unwrap();
    let old_install = work.path().join("old-name/capture");
    let old_data = work.path().join("old-name-data/capture-server");
    std::fs::create_dir_all(&old_install).unwrap();
    std::fs::write(old_install.join(services::program_name()), b"OLD").unwrap();
    std::fs::create_dir_all(old_data.join("inbox")).unwrap();
    std::fs::write(
        old_data.join("server.toml"),
        b"listen = \"127.0.0.1:8787\"\n",
    )
    .unwrap();
    std::fs::write(old_data.join("inbox.key"), b"KEY").unwrap();
    std::fs::write(old_data.join("inbox/receipt-1.jpg"), vec![7u8; 4096]).unwrap();
    std::fs::write(old_data.join("setup-plan.json"), b"{}").unwrap();
    let old_sm = FakeSm::default();
    *old_sm.server.borrow_mut() = State::Running;
    *old_sm.path.borrow_mut() = Some(old_install.join(services::program_name()));
    // A3: the old folder is the one the old service's command line names.
    *old_sm.command.borrow_mut() = Some(vec![
        old_install
            .join(services::program_name())
            .display()
            .to_string(),
        "service-run".into(),
        "--data".into(),
        old_data.display().to_string(),
        "--key-file".into(),
    ]);
    OldInstall {
        new_install: work.path().join("new-name/capture"),
        new_data: work.path().join("new-name-data/capture-server"),
        work,
        old_sm,
        old_install,
        old_data,
    }
}

fn migrate(o: &OldInstall, sm: &FakeSm, exe: &Path) -> services::Report {
    let plan = Plan {
        update_server: true,
        allow_lan: Some(true),
        ..Plan::default()
    };
    services::apply(
        &plan,
        exe,
        &Env {
            sm,
            sig: &FakeSig(Err("x".into())),
            dl: &FakeDl::default(),
            probe: &DataProbe(sm),
            can_download: false,
            tmp: o.work.path(),
            install_dir: &o.new_install,
            data_dir: &o.new_data,
            machine_dir: None,
            check_cloudflared: false,
            legacy: Some(services::Legacy {
                sm: &o.old_sm,
                install_dir: &o.old_install,
            }),
            source_dirs: &[],
            sealer: None,
        },
    )
}

/// branding.md AT8: Update server over an old-name installation: the old service is
/// stopped, the data folder copied and checked, the new service installed from the
/// new install folder with the new data folder and the same arguments, healthy; then
/// the old service, data folder and install folder are removed.
#[test]
fn branding_at8_update_migrates_the_old_name_installation() {
    let o = old_install();
    let exe = built(o.work.path(), b"NEW");
    let sm = FakeSm::default();
    let r = migrate(&o, &sm, &exe);
    assert!(r.ok, "{r:?}");
    // New service, new folders.
    let target = o.new_install.join(services::program_name());
    assert_eq!(sm.server_path().unwrap(), target);
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(sm.server_state(), State::Running);
    let installed = sm.log.borrow()[0].clone();
    assert!(
        installed.contains(&format!("--data {}", o.new_data.display())),
        "{installed}"
    );
    assert!(installed.contains("--key-file --allow-lan"), "{installed}");
    assert_eq!(std::fs::read(o.new_data.join("inbox.key")).unwrap(), b"KEY");
    // security-review-1 A1: only the known files move.
    assert!(!o.new_data.join("inbox/receipt-1.jpg").exists());
    assert!(
        !o.new_data.join("setup-plan.json").exists(),
        "hand-over files stay behind"
    );
    // Old removed.
    assert_eq!(
        *o.old_sm.log.borrow(),
        ["stop_server".to_owned(), "uninstall_server".to_owned()]
    );
    assert_eq!(o.old_sm.server_state(), State::NotInstalled);
    // A1: the old data folder is left in place, marked as moved.
    assert!(o.old_data.join("inbox.key").is_file());
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(o.old_data.join(services::MOVED_MARKER)).unwrap())
            .unwrap();
    assert_eq!(marker["to"], o.new_data.display().to_string());
    assert!(!o.old_install.exists());
    assert!(
        !o.work.path().join("old-name").exists(),
        "empty parent removed"
    );
    assert!(
        r.actions.iter().any(|a| a.contains("MoneyKeepCapture")),
        "{:?}",
        r.actions
    );
    // A later Update server is an ordinary update.
    let again = migrate(&o, &sm, &exe);
    assert!(again.ok, "{again:?}");
    assert!(
        again
            .actions
            .iter()
            .any(|a| a == "capture server updated and running")
    );
}

/// branding.md AT8: when the new server does not answer, the new service and the
/// copies are removed and the old service is started again; the old installation is
/// untouched.
#[test]
fn branding_at8_failed_migration_restores_the_old_service() {
    let o = old_install();
    let broken = built(o.work.path(), b"BROKEN");
    let sm = FakeSm::default();
    *sm.fail_start.borrow_mut() = true;
    let r = migrate(&o, &sm, &broken);
    assert!(!r.ok);
    let e = r.error.unwrap();
    assert!(e.contains("did not answer its health check"), "{e}");
    assert!(e.contains("started again"), "{e}");
    assert_eq!(
        sm.server_state(),
        State::NotInstalled,
        "new service removed"
    );
    assert_eq!(
        *o.old_sm.log.borrow(),
        ["stop_server".to_owned(), "start_server".to_owned()]
    );
    assert!(!o.new_data.exists(), "copies removed");
    assert_eq!(std::fs::read(o.old_data.join("inbox.key")).unwrap(), b"KEY");
    assert!(o.old_data.join("inbox/receipt-1.jpg").is_file());
    assert_eq!(
        std::fs::read(o.old_install.join(services::program_name())).unwrap(),
        b"OLD"
    );
}

/// The status reports an old-name service, so the desktop's light can say so.
#[test]
fn status_reports_the_old_name() {
    let sm = FakeSm::default();
    let old = FakeSm::default();
    assert!(!services::status(&sm, Some(&old)).old_name);
    *old.server.borrow_mut() = State::Stopped;
    assert!(services::status(&sm, Some(&old)).old_name);
    assert!(!services::status(&sm, None).old_name);
}

/// mobile-capture.md revision 10, test 71 (service layer): the Wi-Fi firewall rule
/// is for the two ports and the Private and Domain profiles only (never Public),
/// limited to the installed server; Remove Wi-Fi capture takes it away.
#[test]
fn at71_firewall_rule_private_and_domain_only() {
    let rule = services::FirewallRule {
        name: "MoneyKeep capture (Wi-Fi)".into(),
        ports: vec![14443, 14080],
        profiles: vec!["Private".into(), "Domain".into(), "Public".into()],
        program: None,
    };
    assert_eq!(rule.safe_profiles(), vec!["Private", "Domain"]);
    let script = services::firewall_script(&rule).unwrap();
    assert!(script.contains("New-NetFirewallRule -DisplayName 'MoneyKeep capture (Wi-Fi)'"));
    assert!(script.contains("-Direction Inbound -Action Allow"));
    assert!(script.contains("-Protocol TCP -LocalPort 14443,14080 -Profile Private,Domain"));
    assert!(!script.contains("Public"));
    let only_public = services::FirewallRule {
        profiles: vec!["Public".into()],
        ..rule.clone()
    };
    assert!(services::firewall_script(&only_public).is_err());
    assert!(services::firewall_remove_script("O'Neil").contains("'O''Neil'"));
    // Through a plan: added after the service, with its program.
    let sm = FakeSm::default();
    let tmp = tempfile::tempdir().unwrap();
    wifi_on(tmp.path());
    let p = Plan {
        install_server: true,
        firewall_rule: Some(services::FirewallRule {
            profiles: vec!["Private".into(), "Domain".into()],
            ..rule.clone()
        }),
        ..Plan::default()
    };
    let r = apply(
        &p,
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(
        r.actions
            .iter()
            .any(|a| a.contains("Private and Domain networks only")),
        "{r:?}"
    );
    let added = sm.rules.borrow()[0].clone();
    assert!(added.program.is_some(), "limited to the installed server");
    let p = Plan {
        remove_firewall_rule: Some("MoneyKeep capture (Wi-Fi)".into()),
        ..Plan::default()
    };
    let r = apply(
        &p,
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok);
    assert!(sm.rules.borrow().is_empty());
}

/// The address lists of each system are read with their adapters.
#[test]
fn local_address_lists_are_parsed() {
    let w = services::parse_windows_addresses(
        r#"[{"address":"192.168.1.20","adapter":"Wi-Fi","mac":"A4-B1-C2-D3-E4-F5","profile":"Private","dhcp":true},
            {"address":"127.0.0.1","adapter":"Loopback","mac":"","profile":"","dhcp":false}]"#,
    );
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].mac, "a4:b1:c2:d3:e4:f5");
    let l = services::parse_ip_addr(
        "1: lo    inet 127.0.0.1/8 scope host lo\n2: wlan0    inet 192.168.1.20/24 brd 192.168.1.255 scope global dynamic wlan0\n",
        |a| format!("mac-of-{a}"),
    );
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].adapter.as_str(), l[0].dhcp), ("wlan0", true));
    assert_eq!(l[0].mac, "mac-of-wlan0");
    let m = services::parse_ifconfig(
        "lo0: flags=8049<UP>\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags=8863<UP>\n\tether aa:bb:cc:dd:ee:ff\n\tinet 10.0.0.7 netmask 0xffffff00\n",
    );
    assert_eq!(m.len(), 1);
    assert_eq!(
        (m[0].adapter.as_str(), m[0].mac.as_str()),
        ("en0", "aa:bb:cc:dd:ee:ff")
    );
}

// ----- mobile-capture.md revision 16 ----------------------------------------------------------

const OLD_TUNNEL: &str = "0d5c0a2e-6f1b-4c3d-9e8f-112233445566";

/// A tunnel token as Cloudflare hands it out: base64 of `{"a", "t", "s"}`.
fn token_for(tunnel: &str) -> String {
    use base64::Engine as _;
    let json = format!(r#"{{"a":"acc1","t":"{tunnel}","s":"c2VjcmV0LXNlY3JldA=="}}"#);
    base64::engine::general_purpose::STANDARD.encode(json)
}

/// Test 88 (this computer): the service's tunnel and account come from its
/// command line (Windows service, systemd unit, launchd plist), never its secret.
#[test]
fn r16_service_tunnel_from_its_command_line() {
    let token = token_for(OLD_TUNNEL);
    for line in [
        format!(
            "\"C:\\Program Files (x86)\\cloudflared\\cloudflared.exe\" tunnel run --token {token}"
        ),
        format!("ExecStart=/usr/bin/cloudflared --no-autoupdate tunnel run --token={token}\n"),
        format!(
            "<array>\n<string>/opt/homebrew/bin/cloudflared</string>\n<string>tunnel</string>\n<string>run</string>\n<string>--token</string>\n<string>{token}</string>\n</array>"
        ),
    ] {
        let (t, a) = services::tunnel_of(&line);
        assert_eq!(t.as_deref(), Some(OLD_TUNNEL), "{line}");
        assert_eq!(a.as_deref(), Some("acc1"));
    }
    // A tunnel id written out (a config-file tunnel).
    let (t, a) = services::tunnel_of(
        "cloudflared.exe --config C:\\cf\\config.yml tunnel run 0D5C0A2E-6F1B-4C3D-9E8F-112233445566",
    );
    assert_eq!(t.as_deref(), Some(OLD_TUNNEL));
    assert_eq!(a, None);
    // Unknown.
    assert_eq!(
        services::tunnel_of("cloudflared.exe tunnel run --token NOT-A-TOKEN"),
        (None, None)
    );
    // `services status` reports it.
    let sm = FakeSm::default();
    *sm.service.borrow_mut() = Some((
        State::Running,
        format!("cloudflared.exe tunnel run --token {token}"),
    ));
    let st = services::status(&sm, None);
    assert_eq!(st.cloudflared_tunnel.as_deref(), Some(OLD_TUNNEL));
    assert_eq!(st.cloudflared_account.as_deref(), Some("acc1"));
    assert!(!serde_json::to_string(&st).unwrap().contains(&token));
}

/// Test 88 (this computer): Replace removes the service of the deleted tunnel and
/// installs cloudflared for the new one in the same run; a service running any
/// other tunnel is still never replaced.
#[test]
fn r16_replace_the_service_of_a_deleted_tunnel() {
    let tmp = tempfile::tempdir().unwrap();
    let old = token_for(OLD_TUNNEL);
    let fresh = |sm: &FakeSm| {
        *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cloudflared.exe"));
        *sm.service.borrow_mut() = Some((
            State::Running,
            format!("\"C:\\cloudflared.exe\" tunnel run --token {old}"),
        ));
    };
    // Named for replacing: replaced.
    let sm = FakeSm::default();
    fresh(&sm);
    let mut p = plan(Some("NEW-TOKEN"));
    p.replace_cloudflared_tunnel = Some(OLD_TUNNEL.into());
    let r = apply(
        &p,
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(r.cloudflared_replaced && r.cloudflared_service);
    let log = sm.log.borrow().clone();
    let un = log
        .iter()
        .position(|l| l == "uninstall_cloudflared_service");
    let ins = log.iter().position(|l| l == "install_cloudflared_service");
    assert!(un.unwrap() < ins.unwrap(), "{log:?}");
    assert!(
        sm.service
            .borrow()
            .as_ref()
            .unwrap()
            .1
            .contains("NEW-TOKEN")
    );
    assert!(
        r.actions
            .iter()
            .any(|a| a == "cloudflared service of a deleted tunnel removed")
    );
    // Another tunnel than the one named: not replaced.
    let sm = FakeSm::default();
    fresh(&sm);
    let mut p = plan(Some("NEW-TOKEN"));
    p.replace_cloudflared_tunnel = Some("11111111-2222-3333-4444-555555555555".into());
    let r = apply(
        &p,
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("it was not replaced"));
    assert!(!sm.log.borrow().iter().any(|l| l.contains("cloudflared")));
}

/// mobile-capture.md revision 17 (Start over): one plan removes the services, the
/// firewall rule and empties the data folder; a `cloudflared` service is removed
/// only while it runs the tunnel the desktop named.
#[test]
fn r17_start_over_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("capture-server");
    std::fs::create_dir_all(data.join("certs")).unwrap();
    std::fs::write(data.join("server.toml"), b"listen = \"127.0.0.1:8787\"\n").unwrap();
    std::fs::write(data.join("certs").join("ca.pem"), b"CA").unwrap();
    std::fs::write(data.join("setup-report.json"), b"{}").unwrap();
    let token = |t: &str| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .encode(format!("{{\"a\":\"acc1\",\"t\":\"{t}\",\"s\":\"x\"}}"))
    };
    let other = "11111111-2222-3333-4444-555555555555";
    let mine = "0d5c0a2e-6f1b-4c3d-9e8f-112233445566";
    let sm = FakeSm::default();
    *sm.server.borrow_mut() = State::Running;
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cf/cloudflared.exe"));
    *sm.service.borrow_mut() = Some((
        State::Running,
        format!("cloudflared.exe tunnel run --token {}", token(other)),
    ));
    let plan = Plan {
        remove_server: true,
        remove_cloudflared: true,
        remove_cloudflared_tunnel: Some(mine.into()),
        remove_firewall_rule: Some(mk_capture_api::FIREWALL_RULE.into()),
        clear_data_dir: true,
        ..Plan::default()
    };
    let r = apply(
        &plan,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(
        sm.cloudflared_service().is_some(),
        "another tunnel's service is kept"
    );
    assert!(r.actions.iter().any(|a| a.contains("kept")), "{r:?}");
    assert_eq!(sm.server_state(), State::NotInstalled);
    assert!(!data.join("server.toml").exists() && !data.join("certs").exists());
    assert!(
        data.join("setup-report.json").exists(),
        "the helper's own report stays"
    );
    // The service of the named tunnel goes.
    *sm.service.borrow_mut() = Some((
        State::Running,
        format!("cloudflared.exe tunnel run --token {}", token(mine)),
    ));
    let r = apply(
        &plan,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(sm.cloudflared_service().is_none());
}

/// Test 98 (this computer): emptying the data folder touches nothing beside it —
/// books and backups next to it are byte-for-byte unchanged — and a folder that
/// is not the capture server's is refused.
#[test]
fn r17_t98_clearing_leaves_books_and_backups() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("capture-server");
    std::fs::create_dir_all(data.join("inbox")).unwrap();
    std::fs::write(data.join("inbox").join("a.json"), b"{}").unwrap();
    let book = root.path().join("Household.moneykeep");
    let backups = root.path().join("MoneyKeep Backups");
    std::fs::create_dir_all(&backups).unwrap();
    let backup = backups.join("Household 2026-11-09.moneykeep");
    std::fs::write(&book, b"BOOK BYTES \x00\x01").unwrap();
    std::fs::write(&backup, b"BACKUP BYTES \x02").unwrap();
    let n = services::clear_data_dir(&data).unwrap();
    assert_eq!(n, 1);
    assert_eq!(std::fs::read(&book).unwrap(), b"BOOK BYTES \x00\x01");
    assert_eq!(std::fs::read(&backup).unwrap(), b"BACKUP BYTES \x02");
    let e = services::clear_data_dir(&backups).unwrap_err();
    assert!(e.contains("does not look like"), "{e}");
    assert!(backup.exists());
    assert_eq!(
        services::clear_data_dir(&root.path().join("missing")).unwrap(),
        0
    );
}

/// Test 89b (revision 16b): with cloudflared's event-log key present and no service,
/// the step deletes it before installing; an install that reports only the
/// event-logger error while the service now exists counts as installed; another
/// error still fails; removing the service removes the key too.
#[test]
fn r16b_t89b_stale_event_log_key() {
    let tmp = tempfile::tempdir().unwrap();
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cf/cloudflared.exe"));
    *sm.event_log.borrow_mut() = true;
    let plan = Plan {
        install_server: true,
        tunnel_token: Some("TOKEN".into()),
        ..Plan::default()
    };
    let r = apply(
        &plan,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    let log = sm.log.borrow().clone();
    let del = log.iter().position(|l| l == "delete_event_log").unwrap();
    let inst = log
        .iter()
        .position(|l| l == "install_cloudflared_service")
        .unwrap();
    assert!(del < inst, "{log:?}");
    assert!(
        r.actions
            .iter()
            .any(|a| a.contains("stale cloudflared event-log entry removed"))
    );
    // An event-logger-only error with the service present: installed, with a note.
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cf/cloudflared.exe"));
    *sm.install_error.borrow_mut() = Some(
        "cloudflared.exe failed: Cannot install event logger: SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\Cloudflared registry key already exists".into(),
    );
    let r = apply(
        &plan,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(r.cloudflared_service);
    assert!(r.actions.iter().any(|a| a.starts_with("note:")), "{r:?}");
    // Any other error fails.
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cf/cloudflared.exe"));
    *sm.install_error.borrow_mut() = Some("cloudflared.exe failed: access denied".into());
    let r = apply(
        &plan,
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(!r.ok);
    // Removing the service (Remove, Start over) removes the key with it.
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from("C:/cf/cloudflared.exe"));
    *sm.service.borrow_mut() = Some((State::Running, "cloudflared.exe tunnel run".into()));
    *sm.event_log.borrow_mut() = true;
    *sm.keep_event_log.borrow_mut() = true;
    let r = apply(
        &Plan {
            remove_cloudflared: true,
            ..Plan::default()
        },
        &sm,
        &FakeSig(Err("unused".into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    assert!(!*sm.event_log.borrow());
    assert!(
        r.actions
            .iter()
            .any(|a| a == "cloudflared event-log entry removed")
    );
    assert!(services::event_logger_only(
        "Cannot install event logger: SYSTEM\\...\\Cloudflared registry key already exists"
    ));
    assert_eq!(
        services::CLOUDFLARED_EVENT_LOG_KEY,
        r"HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared"
    );
}

fn windows_rule(id: &str, action: &str, program: &str) -> services::FoundRule {
    services::FoundRule {
        id: id.into(),
        display_name: "moneykeep-server.exe".into(),
        enabled: true,
        action: action.into(),
        direction: "Inbound".into(),
        program: program.into(),
    }
}

/// mobile-capture.md test 106 (service layer, a fake for the firewall calls): a
/// missing or disabled rule is reported and put back with the right ports and
/// profiles; Windows' Block rules for moneykeep-server.exe are reported and removed,
/// its Allow rules and other programs' rules left alone; a Public network is
/// reported.
#[test]
fn t106_firewall_check_and_repair() {
    let sm = FakeSm::default();
    // No firewall MoneyKeep manages: nothing to report.
    let c = services::firewall_check(&sm, "MoneyKeep capture (Wi-Fi)");
    assert!(!c.managed);
    *sm.firewall.borrow_mut() = true;
    let exe = r"C:\Program Files\MoneyKeep\capture\moneykeep-server.exe";
    *sm.other_rules.borrow_mut() = vec![
        windows_rule("{block-tcp}", "Block", exe),
        windows_rule("{allow-udp}", "Allow", exe),
        windows_rule("{other}", "Block", r"C:\Tools\other.exe"),
        services::FoundRule {
            direction: "Outbound".into(),
            ..windows_rule("{out}", "Block", exe)
        },
    ];
    *sm.networks.borrow_mut() = vec![
        services::NetProfile {
            name: "Cafe Wi-Fi".into(),
            interface: "Wi-Fi".into(),
            category: "Public".into(),
        },
        services::NetProfile {
            name: "Home".into(),
            interface: "Ethernet".into(),
            category: "Private".into(),
        },
    ];
    let c = services::firewall_check(&sm, "MoneyKeep capture (Wi-Fi)");
    assert!(c.managed);
    assert_eq!(c.rule, services::RuleState::Missing);
    let blocks: Vec<&str> = c.block_rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        blocks,
        vec!["{block-tcp}"],
        "inbound Block for the server only"
    );
    assert_eq!(c.public_networks, vec!["Cafe Wi-Fi".to_owned()]);
    // The JSON MoneyKeep reads.
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["rule"], "missing");
    assert_eq!(v["block_rules"][0]["action"], "Block");
    // The installer's repair: Wi-Fi capture on, the rule put back.
    let mut config = moneykeep_server::Config::default();
    let none = services::repair_firewall(&sm, &config).unwrap();
    assert!(none[0].contains("Wi-Fi capture is off"), "{none:?}");
    assert!(sm.rules.borrow().is_empty());
    config.wifi_address = "192.168.1.20".into();
    let done = services::repair_firewall(&sm, &config).unwrap();
    assert!(done[0].contains("put back"), "{done:?}");
    let rule = sm.rules.borrow()[0].clone();
    assert_eq!(rule.name, "MoneyKeep capture (Wi-Fi)");
    assert_eq!(rule.ports, vec![14443, 14080]);
    assert_eq!(
        rule.profiles,
        vec!["Private".to_owned(), "Domain".to_owned()]
    );
    assert_eq!(
        services::firewall_check(&sm, "MoneyKeep capture (Wi-Fi)").rule,
        services::RuleState::Ok
    );
    // Disabled counts too.
    *sm.rules_disabled.borrow_mut() = true;
    assert_eq!(
        services::firewall_check(&sm, "MoneyKeep capture (Wi-Fi)").rule,
        services::RuleState::Disabled
    );
    *sm.rules_disabled.borrow_mut() = false;
    // Fix (a plan): the Block rule goes; an Allow rule named in the plan is left.
    let tmp = tempfile::tempdir().unwrap();
    let p = Plan {
        remove_block_rules: vec!["{block-tcp}".into(), "{allow-udp}".into()],
        ..Plan::default()
    };
    let r = apply(
        &p,
        &sm,
        &FakeSig(Ok(SIGNED.into())),
        &FakeDl::default(),
        tmp.path(),
    );
    assert!(r.ok, "{r:?}");
    let left: Vec<String> = sm
        .other_rules
        .borrow()
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(left, vec!["{allow-udp}", "{other}", "{out}"]);
    assert!(r.actions[0].contains("Block rule"), "{r:?}");
    assert!(r.actions[1].contains("left alone"), "{r:?}");
    assert!(
        services::firewall_check(&sm, "MoneyKeep capture (Wi-Fi)")
            .block_rules
            .is_empty()
    );
}

/// Revision 18: PowerShell's JSON for the firewall and the networks (one element
/// comes without its list).
#[test]
fn t106_windows_firewall_json_is_read() {
    let scan = services::parse_firewall_scan(
        r#"{"named":{"id":"{a}","display_name":"MoneyKeep capture (Wi-Fi)","enabled":false,"action":"Allow","direction":"Inbound","program":""},"program_rules":[{"id":"TCP Query User{x}","display_name":"moneykeep-server.exe","enabled":true,"action":"Block","direction":"Inbound","program":"C:\\Program Files\\MoneyKeep\\moneykeep-server.exe"},{"id":"UDP{y}","display_name":"moneykeep-server.exe","enabled":true,"action":"Allow","direction":"Inbound","program":"C:\\Program Files\\MoneyKeep\\MONEYKEEP-SERVER.EXE"}]}"#,
    )
    .unwrap();
    assert_eq!(scan.named.len(), 1);
    assert!(!scan.named[0].enabled);
    assert_eq!(scan.program_rules.len(), 2);
    assert_eq!(
        services::parse_firewall_scan("").unwrap(),
        services::FirewallScan::default()
    );
    assert!(services::parse_firewall_scan("not json").is_err());
    let n = services::parse_network_profiles(
        r#"{"name":"Home","interface":"Wi-Fi","category":"Private"}"#,
    )
    .unwrap();
    assert_eq!(n[0].category, "Private");
    let n = services::parse_network_profiles(
        r#"[{"name":"A","interface":"Wi-Fi","category":"Public"},{"name":"B","interface":"Ethernet","category":"DomainAuthenticated"}]"#,
    )
    .unwrap();
    assert_eq!(n.len(), 2);
    let script = services::firewall_scan_script("O'Neil", "moneykeep-server.exe");
    assert!(script.contains("-DisplayName 'O''Neil'"), "{script}");
    assert!(script.contains("-ieq 'moneykeep-server.exe'"), "{script}");
    assert!(script.contains("ConvertTo-Json"), "{script}");
    assert!(!script.contains("Remove-"), "the check only reads");
    assert!(services::NETWORK_PROFILES_SCRIPT.contains("Get-NetConnectionProfile"));
    assert_eq!(
        services::firewall_remove_id_script("{a'b}"),
        "Remove-NetFirewallRule -Name '{a''b}' -ErrorAction Stop"
    );
}

// ----- security-review-1 Batch A ------------------------------------------------------

/// A signature check that notes when it ran (with the fake's events).
struct EventSig {
    events: std::rc::Rc<RefCell<Vec<String>>>,
    result: Result<String, String>,
}

impl SignatureChecker for EventSig {
    fn signer(&self, file: &Path) -> Result<String, String> {
        assert!(file.is_file() || !file.to_string_lossy().ends_with(".msi"));
        self.events.borrow_mut().push(format!(
            "signature {}",
            file.file_name().unwrap().to_string_lossy()
        ));
        self.result.clone()
    }
}

/// An environment as on Windows: MoneyKeep's machine folder (a stand-in for
/// `%ProgramData%\MoneyKeep`) and the signature check of `cloudflared`.
fn windows_env<'a>(
    sm: &'a FakeSm,
    sig: &'a dyn SignatureChecker,
    dl: &'a FakeDl,
    probe: &'a dyn Probe,
    root: &'a Path,
    machine: &'a Path,
    data: &'a Path,
) -> Env<'a> {
    Env {
        sm,
        sig,
        dl,
        probe,
        can_download: true,
        tmp: root,
        install_dir: root,
        data_dir: data,
        machine_dir: Some(machine),
        check_cloudflared: true,
        legacy: None,
        source_dirs: &[],
        sealer: None,
    }
}

/// SR-A1: the service runs as its own virtual account with its data under
/// ProgramData; the folders are protected (inheritance off: SYSTEM, Administrators,
/// and the service's account for its data); a junction or a folder owned by Users
/// is refused; the move copies the known files, skips a link and leaves the old
/// folder in place.
#[test]
fn sr_a1_service_account_data_folder_and_move() {
    use services::windows;
    // The service's configuration.
    let cfg = windows::install_config(Path::new("C:/x/moneykeep-server.exe"), &["a".into()]);
    assert_eq!(cfg.account, "NT SERVICE\\MoneyKeepCapture");
    assert!(cfg.sid_unrestricted);
    // Service SIDs as Windows makes them (TrustedInstaller's is well known).
    assert_eq!(
        windows::service_sid("TrustedInstaller"),
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
    );
    let svc = windows::service_sid("MoneyKeepCapture");
    let data_acl = windows::folder_acl_script(
        Path::new("C:/ProgramData/MoneyKeep/capture-server"),
        services::FolderUse::Data,
    );
    assert!(
        data_acl.contains("SetAccessRuleProtection($true, $false)"),
        "{data_acl}"
    );
    assert!(data_acl.contains(
        "SetOwner((New-Object System.Security.Principal.SecurityIdentifier 'S-1-5-32-544'))"
    ));
    assert!(
        data_acl
            .contains("'S-1-5-18'), [System.Security.AccessControl.FileSystemRights]'FullControl'")
    );
    assert!(data_acl.contains(
        "'S-1-5-32-544'), [System.Security.AccessControl.FileSystemRights]'FullControl'"
    ));
    assert!(data_acl.contains(&format!(
        "'{svc}'), [System.Security.AccessControl.FileSystemRights]'Modify'"
    )));
    assert!(!data_acl.contains("S-1-5-32-545"), "nothing for Users");
    let machine_acl = windows::folder_acl_script(
        Path::new("C:/ProgramData/MoneyKeep"),
        services::FolderUse::Machine,
    );
    assert!(machine_acl.contains("'S-1-5-32-545'), [System.Security.AccessControl.FileSystemRights]'ReadAndExecute', [System.Security.AccessControl.InheritanceFlags]::None"));
    assert!(!machine_acl.contains(&svc));
    let admin_acl = windows::folder_acl_script(
        Path::new("C:/ProgramData/MoneyKeep/tmp"),
        services::FolderUse::AdminOnly,
    );
    assert!(!admin_acl.contains("S-1-5-32-545") && !admin_acl.contains(&svc));
    assert!(windows::owner_script(Path::new("C:/a'b")).contains("-LiteralPath 'C:/a''b'"));

    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("ProgramData/MoneyKeep");
    std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
    let data = machine.join("capture-server");
    let work = tempfile::tempdir().unwrap();
    let exe = built(work.path(), b"NEW");
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    // A new installation: the folders are made and protected, the service gets
    // the data folder under ProgramData.
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    let env = windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data);
    let r = services::apply(
        &Plan {
            install_server: true,
            ..Plan::default()
        },
        &exe,
        &env,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(
        *sm.protected.borrow(),
        [
            (machine.clone(), services::FolderUse::Machine),
            (data.clone(), services::FolderUse::Data)
        ]
    );
    let args = sm.server_command().unwrap();
    assert_eq!(services::data_of(&args).unwrap(), data);
    assert_eq!(r.data_dir.as_deref(), Some(data.as_path()));
    // A folder owned by Users is refused.
    sm.owners
        .borrow_mut()
        .push((data.clone(), "S-1-5-32-545".into()));
    let r = services::apply(&Plan::default(), &exe, &env);
    assert_eq!(
        r.error.unwrap(),
        format!(
            "The capture server's folder isn't safe to use: {}",
            data.display()
        )
    );
    sm.owners.borrow_mut().clear();
    // A junction (a link here) is refused, whoever owns it.
    #[cfg(unix)]
    {
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::remove_dir_all(&data).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &data).unwrap();
        let r = services::apply(&Plan::default(), &exe, &env);
        assert!(r.error.unwrap().contains("isn't safe to use"), "link");
        std::fs::remove_file(&data).unwrap();
    }

    // The move: the service still uses the folder in the user's profile.
    let old = root
        .path()
        .join("Users/pat/AppData/Local/MoneyKeep/data/capture-server");
    std::fs::create_dir_all(&old).unwrap();
    for (f, b) in [
        ("server.toml", &b"listen = \"127.0.0.1:8787\"\n"[..]),
        ("inbox.sqlite", b"STORE"),
        ("wifi.json", b"{}"),
        ("wifi-ca.crt", b"CA"),
        ("wifi-ca-key.sealed", b"SEALED"),
        ("notes.txt", b"not ours"),
    ] {
        std::fs::write(old.join(f), b).unwrap();
    }
    let secret = root.path().join("secret.txt");
    std::fs::write(&secret, b"SOMEONE ELSE'S").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, old.join("inbox.key")).unwrap();
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    sm.install_server(
        Path::new("C:/Program Files/MoneyKeep/capture/moneykeep-server.exe"),
        &services::server_args(&old, true),
    )
    .unwrap();
    sm.log.borrow_mut().clear();
    let env = windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data);
    let r = services::apply(
        &Plan {
            start_server: true,
            ..Plan::default()
        },
        &exe,
        &env,
    );
    assert!(r.ok, "{r:?}");
    for f in [
        "server.toml",
        "inbox.sqlite",
        "wifi.json",
        "wifi-ca.crt",
        "wifi-ca-key.sealed",
    ] {
        assert_eq!(
            std::fs::read(data.join(f)).unwrap(),
            // upgrade-resilience-1 U1.3: the old server.toml is renamed.
            std::fs::read(old.join(if f == "server.toml" {
                services::RETIRED_CONFIG
            } else {
                f
            }))
            .unwrap(),
            "{f}"
        );
    }
    assert!(!data.join("notes.txt").exists(), "only the known files");
    #[cfg(unix)]
    {
        assert!(!data.join("inbox.key").exists(), "a link is not followed");
        assert!(
            r.actions
                .iter()
                .any(|a| a.contains("inbox.key") && a.contains("skipped")),
            "{r:?}"
        );
    }
    // The service now uses the protected folder, with its other arguments.
    assert_eq!(sm.log.borrow()[0], "stop_server");
    let args = sm.server_command().unwrap();
    assert_eq!(services::data_of(&args).unwrap(), data);
    assert!(args.iter().any(|a| a == "--allow-lan"));
    // The old folder is left, marked.
    assert!(old.join("inbox.sqlite").is_file());
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(old.join(services::MOVED_MARKER)).unwrap()).unwrap();
    assert_eq!(marker["to"], data.display().to_string());
    assert!(r.actions.iter().any(|a| a.contains("moved to")), "{r:?}");
    // A second run does not move again.
    let r = services::apply(&Plan::default(), &exe, &env);
    assert!(
        r.ok && !r.actions.iter().any(|a| a.contains("moved to")),
        "{r:?}"
    );
}

/// SR-A1: Windows service command lines are read back word by word.
#[test]
fn sr_a1_command_lines_are_split() {
    let line = r#""C:\Program Files\MoneyKeep\capture\moneykeep-server.exe" service-run --data "C:\Users\Pat Smith\AppData\Local\MoneyKeep\data\capture-server" --key-file"#;
    let w = services::split_command_line(line);
    assert_eq!(w.len(), 5, "{w:?}");
    assert_eq!(
        services::data_of(&w).unwrap(),
        PathBuf::from(r"C:\Users\Pat Smith\AppData\Local\MoneyKeep\data\capture-server")
    );
    assert_eq!(
        services::split_command_line(r#"a "b\"c" d\\e"#),
        ["a", "b\"c", "d\\\\e"]
    );
    let q = services::windows::quote_arg(r"C:\Program Files\x\");
    assert_eq!(services::split_command_line(&q), [r"C:\Program Files\x\"]);
}

/// SR-A2: only Cloudflare's own cloudflared is run: the signer must be exactly
/// Cloudflare's organisation; a cloudflared that is not is refused before it runs;
/// the downloaded installer is held (others may only read it) from its signature
/// check until msiexec has finished, in a folder MoneyKeep protects; cloudflared is
/// looked for only in the given folders, never PATH.
#[test]
fn sr_a2_only_cloudflares_cloudflared() {
    assert!(services::is_cloudflare_signer(SIGNED));
    assert!(services::is_cloudflare_signer(
        "O=\"Cloudflare, Inc.\", C=US"
    ));
    for bad in [
        "CN=\"Cloudflare, Inc.\", O=\"Cloudflare, Inc. Evil\", C=US",
        "O=\"Cloudflare, Inc.\", C=GB",
        "CN=\"O=Cloudflare, Inc.\", O=Someone, C=US",
        "O=Cloudflare, C=US",
        "",
    ] {
        assert!(!services::is_cloudflare_signer(bad), "{bad}");
    }
    assert_eq!(
        services::HOLD_SHARE_MODE,
        0x1,
        "FILE_SHARE_READ only: no write or delete sharing"
    );
    // Never PATH: only the folders given.
    let dir = tempfile::tempdir().unwrap();
    let name = if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    std::fs::write(dir.path().join(name), b"x").unwrap();
    assert!(services::find_in("cloudflared", &[]).is_none());
    assert_eq!(
        services::find_in("cloudflared", &[dir.path().to_path_buf()]).unwrap(),
        dir.path().join(name)
    );

    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("MoneyKeep");
    let data = machine.join("capture-server");
    let work = tempfile::tempdir().unwrap();
    let exe = built(work.path(), b"NEW");
    let dl = FakeDl::default();
    // An installed cloudflared not signed by Cloudflare is not run.
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from(
        "C:/Program Files/cloudflared/cloudflared.exe",
    ));
    let probe = FakeProbe(&sm);
    let sig = FakeSig(Ok("CN=Someone, O=Someone, C=US".into()));
    let env = windows_env(&sm, &sig, &dl, &probe, &machine, &machine, &data);
    let r = services::apply(&plan(Some("TOKEN-A")), &exe, &env);
    assert_eq!(r.error.unwrap(), services::NOT_CLOUDFLARE_SIGNED);
    assert!(
        !sm.log
            .borrow()
            .iter()
            .any(|l| l.contains("cloudflared_service"))
    );
    // Removing it does not run it either: the service is deleted directly.
    *sm.service.borrow_mut() = Some((
        State::Running,
        "cloudflared.exe tunnel run --token X".into(),
    ));
    let r = services::apply(
        &Plan {
            remove_cloudflared: true,
            ..Plan::default()
        },
        &exe,
        &env,
    );
    assert!(
        !sm.log
            .borrow()
            .iter()
            .any(|l| l == "uninstall_cloudflared_service")
    );
    assert!(
        r.error.unwrap().contains("could not be removed"),
        "the fake has no sc.exe"
    );
    // The download: held from the check until msiexec has finished.
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    let sig = EventSig {
        events: sm.events.clone(),
        result: Ok(SIGNED.into()),
    };
    let tmp = machine.join("tmp");
    let env = Env {
        tmp: &tmp,
        ..windows_env(&sm, &sig, &dl, &probe, &machine, &machine, &data)
    };
    let r = services::apply(&plan(Some("TOKEN-A")), &exe, &env);
    assert!(r.ok, "{r:?}");
    assert_eq!(
        *sm.events.borrow(),
        [
            "hold",
            "signature cloudflared-windows-amd64.msi",
            "msiexec",
            "release",
            "signature cloudflared.exe"
        ]
    );
    // In MoneyKeep's protected tmp folder, in a folder of its own, removed after.
    let protected: Vec<_> = sm
        .protected
        .borrow()
        .iter()
        .map(|(p, u)| (p.clone(), *u))
        .collect();
    assert!(
        protected.contains(&(tmp.clone(), services::FolderUse::AdminOnly)),
        "{protected:?}"
    );
    assert!(protected.iter().any(|(p, u)| p.parent() == Some(tmp.as_path()) && *u == services::FolderUse::AdminOnly));
    assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
}

/// SR-A3: a plan whose hash does not match is refused with nothing changed; a plan
/// naming another firewall rule, another port, another program or another folder is
/// refused whole, as is a plan asking for something the helper does not do.
#[test]
fn sr_a3_plan_hash_and_what_it_may_ask() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("setup-plan.json");
    let bytes = br#"{"install_server": true}"#;
    std::fs::write(&file, bytes).unwrap();
    let good = services::sha256_hex(bytes);
    assert_eq!(services::read_plan(&file, &good).unwrap(), bytes);
    assert_eq!(
        services::read_plan(&file, &services::sha256_hex(b"other")).unwrap_err(),
        services::PLAN_CHANGED
    );
    assert_eq!(
        services::PLAN_CHANGED,
        "The setup plan changed after you approved it. Nothing was changed."
    );
    // The report is a new file: a link left at its name is replaced, not written
    // through.
    #[cfg(unix)]
    {
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"KEEP").unwrap();
        let report = dir.path().join("setup-report.json");
        std::os::unix::fs::symlink(&target, &report).unwrap();
        services::write_report(&report, b"{}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"KEEP");
        assert_eq!(std::fs::read(&report).unwrap(), b"{}");
    }

    let tmp = tempfile::tempdir().unwrap();
    wifi_on(tmp.path());
    let rule = |name: &str, ports: Vec<u16>| services::FirewallRule {
        name: name.into(),
        ports,
        profiles: vec!["Private".into(), "Domain".into()],
        program: None,
    };
    let refused = |p: &Plan| {
        let sm = FakeSm::default();
        let r = apply(
            p,
            &sm,
            &FakeSig(Ok(SIGNED.into())),
            &FakeDl::default(),
            tmp.path(),
        );
        assert!(!r.ok);
        assert!(
            sm.log.borrow().is_empty(),
            "nothing changed: {:?}",
            sm.log.borrow()
        );
        assert!(sm.rules.borrow().is_empty());
        r.error.unwrap()
    };
    let e = refused(&Plan {
        install_server: true,
        firewall_rule: Some(rule("Open everything", vec![14443, 14080])),
        ..Plan::default()
    });
    assert!(
        e.contains("another firewall rule") && e.contains("Nothing was changed"),
        "{e}"
    );
    let e = refused(&Plan {
        install_server: true,
        firewall_rule: Some(rule(mk_capture_api::FIREWALL_RULE, vec![14443, 3389])),
        ..Plan::default()
    });
    assert!(e.contains("other ports"), "{e}");
    let e = refused(&Plan {
        install_server: true,
        firewall_rule: Some(services::FirewallRule {
            program: Some(PathBuf::from("C:/Windows/System32/cmd.exe")),
            ..rule(mk_capture_api::FIREWALL_RULE, vec![14443, 14080])
        }),
        ..Plan::default()
    });
    assert!(e.contains("another program"), "{e}");
    let e = refused(&Plan {
        firewall_rule: Some(services::FirewallRule {
            profiles: vec!["Private".into(), "Public".into()],
            ..rule(mk_capture_api::FIREWALL_RULE, vec![14443, 14080])
        }),
        ..Plan::default()
    });
    assert!(e.contains("other networks"), "{e}");
    let e = refused(&Plan {
        install_server: true,
        data_dir: Some(PathBuf::from("C:/Users/Public")),
        ..Plan::default()
    });
    assert!(e.contains("another data folder"), "{e}");
    let e = refused(&Plan {
        remove_firewall_rule: Some("Remote Desktop".into()),
        ..Plan::default()
    });
    assert!(e.contains("removing another firewall rule"), "{e}");
    // Anything else (an unknown request) is refused whole.
    let sm = FakeSm::default();
    let work = tempfile::tempdir().unwrap();
    let exe = built(work.path(), b"NEW");
    let probe = FakeProbe(&sm);
    let sig = FakeSig(Ok(SIGNED.into()));
    let dl = FakeDl::default();
    let data = tmp.path().join("capture-server");
    let env = Env {
        sm: &sm,
        sig: &sig,
        dl: &dl,
        probe: &probe,
        can_download: false,
        tmp: tmp.path(),
        install_dir: work.path(),
        data_dir: &data,
        machine_dir: None,
        check_cloudflared: false,
        legacy: None,
        source_dirs: &[],
        sealer: None,
    };
    let r = services::apply_bytes(
        br#"{"install_server": true, "legacy_data_dir": "C:/Windows"}"#,
        &exe,
        &env,
    );
    assert!(r.error.unwrap().contains("unknown field"));
    assert!(sm.log.borrow().is_empty());
    // The right rule passes, for the installed server's program.
    let r = services::apply_bytes(
        &serde_json::to_vec(&serde_json::json!({
            "install_server": true,
            "firewall_rule": {"name": mk_capture_api::FIREWALL_RULE, "ports": [14080, 14443], "profiles": ["Private", "Domain"]},
        }))
        .unwrap(),
        &exe,
        &env,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(sm.rules.borrow()[0].program, sm.server_path());
}

/// L1: with a cloudflared that reads its token from a file (2025.4.0 or later), the
/// token goes in a file only administrators can open and the service's command line
/// names the file (and so still shows the tunnel); an older one keeps the token on
/// its command line and says so.
#[test]
fn l1_tunnel_token_off_the_command_line() {
    assert!(services::supports_token_file(
        "cloudflared version 2025.4.0 (built 2025-04-09-1200 UTC)"
    ));
    assert!(services::supports_token_file(
        "cloudflared version 2025.11.1 (built x)"
    ));
    assert!(!services::supports_token_file(
        "cloudflared version 2025.2.1 (built x)"
    ));
    assert!(!services::supports_token_file("garbage"));
    let tunnel = "0d5c0a2e-6f1b-4c3d-9e8f-112233445566";
    let token = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(format!(
            "{{\"a\":\"acc1\",\"t\":\"{tunnel}\",\"s\":\"secret\"}}"
        ))
    };
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("MoneyKeep");
    let data = machine.join("capture-server");
    let work = tempfile::tempdir().unwrap();
    let exe = built(work.path(), b"NEW");
    let dl = FakeDl::default();
    let sig = FakeSig(Ok(SIGNED.into()));
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from(
        "C:/Program Files/cloudflared/cloudflared.exe",
    ));
    *sm.cf_version.borrow_mut() = Some("cloudflared version 2025.4.0 (built x)".into());
    let probe = FakeProbe(&sm);
    let env = windows_env(&sm, &sig, &dl, &probe, &machine, &machine, &data);
    let r = services::apply(&plan(Some(&token)), &exe, &env);
    assert!(r.ok, "{r:?}");
    let file = machine
        .join("cloudflared")
        .join(format!("{tunnel}.acc1.token"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), token);
    assert_eq!(
        *sm.cf_commands.borrow(),
        [vec![
            "tunnel".to_owned(),
            "run".to_owned(),
            "--token-file".to_owned(),
            file.display().to_string()
        ]]
    );
    assert!(
        sm.protected
            .borrow()
            .contains(&(machine.join("cloudflared"), services::FolderUse::AdminOnly))
    );
    let (_, line) = sm.cloudflared_service().unwrap();
    assert!(!line.contains(&token));
    assert_eq!(
        services::tunnel_of(&line),
        (Some(tunnel.to_owned()), Some("acc1".to_owned()))
    );
    // The same tunnel again: already running it.
    let r = services::apply(&plan(Some(&token)), &exe, &env);
    assert!(
        r.actions
            .iter()
            .any(|a| a == "cloudflared already runs this tunnel"),
        "{r:?}"
    );
    // Removing the service removes the token file.
    let r = services::apply(
        &Plan {
            remove_cloudflared: true,
            ..Plan::default()
        },
        &exe,
        &env,
    );
    assert!(r.ok, "{r:?}");
    assert!(!file.exists());
    // An older cloudflared: the token stays on its command line, and the report says
    // administrators can see it.
    let sm = FakeSm::default();
    *sm.cloudflared.borrow_mut() = Some(PathBuf::from(
        "C:/Program Files/cloudflared/cloudflared.exe",
    ));
    *sm.cf_version.borrow_mut() = Some("cloudflared version 2024.12.2 (built x)".into());
    let probe = FakeProbe(&sm);
    let env = windows_env(&sm, &sig, &dl, &probe, &machine, &machine, &data);
    let r = services::apply(&plan(Some(&token)), &exe, &env);
    assert!(r.ok, "{r:?}");
    assert!(sm.cf_commands.borrow().is_empty());
    assert!(
        r.actions
            .iter()
            .any(|a| a.contains("administrators of this computer can see it"))
    );
}

/// A sealer for the test (Windows uses DPAPI).
struct XorSealer;

impl moneykeep_server::store::KeySealer for XorSealer {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok([
            b"SEALED:".as_slice(),
            &plain.iter().map(|b| b ^ 0x5a).collect::<Vec<_>>(),
        ]
        .concat())
    }
    fn unseal(&self, sealed: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
        let body = sealed.strip_prefix(b"SEALED:").ok_or("not sealed")?;
        Ok(zeroize::Zeroizing::new(
            body.iter().map(|b| b ^ 0x5a).collect(),
        ))
    }
}

/// L5: with a sealer, the inbox key file is never plain text: a new key is written
/// sealed, and a plain `inbox.key` from before is sealed and removed; the same key
/// comes back.
#[test]
fn l5_inbox_key_is_sealed() {
    use moneykeep_server::store::{self, KEY_FILE, SEALED_KEY_FILE};
    let dir = tempfile::tempdir().unwrap();
    let k1 = store::load_key_with(
        dir.path(),
        moneykeep_server::KeyHome::File,
        Some(&XorSealer),
    )
    .unwrap();
    assert!(!dir.path().join(KEY_FILE).exists());
    let sealed = std::fs::read(dir.path().join(SEALED_KEY_FILE)).unwrap();
    assert!(sealed.starts_with(b"SEALED:"));
    let k2 = store::load_key_with(
        dir.path(),
        moneykeep_server::KeyHome::File,
        Some(&XorSealer),
    )
    .unwrap();
    assert_eq!(*k1, *k2);
    // A plain key from before.
    let old = tempfile::tempdir().unwrap();
    let plain = store::load_key_with(old.path(), moneykeep_server::KeyHome::File, None).unwrap();
    let text = std::fs::read_to_string(old.path().join(KEY_FILE)).unwrap();
    let again = store::load_key_with(
        old.path(),
        moneykeep_server::KeyHome::File,
        Some(&XorSealer),
    )
    .unwrap();
    assert_eq!(*plain, *again);
    assert!(
        !old.path().join(KEY_FILE).exists(),
        "the plain file is gone"
    );
    let sealed = std::fs::read(old.path().join(SEALED_KEY_FILE)).unwrap();
    assert!(!sealed.windows(text.len()).any(|w| w == text.as_bytes()));
    assert_eq!(
        *store::load_key_with(
            old.path(),
            moneykeep_server::KeyHome::File,
            Some(&XorSealer)
        )
        .unwrap(),
        *plain
    );
    // upgrade-resilience-1 U1.6: DPAPI goes through the Windows API, with the
    // entropy the PowerShell sealing used (keys sealed then still open).
    assert_eq!(
        services::windows::DPAPI_ENTROPY,
        b"MoneyKeep capture inbox key"
    );
}

/// A stand-in for DPAPI (security-review-1 L5): "sealed" bytes are marked, so a
/// test can tell a sealed key from a plain one and open it again.
struct FakeSealer;

impl moneykeep_server::store::KeySealer for FakeSealer {
    fn seal(&self, plain: &[u8]) -> moneykeep_server::store::StoreResult<Vec<u8>> {
        Ok([b"SEALED:".as_slice(), plain].concat())
    }
    fn unseal(
        &self,
        sealed: &[u8],
    ) -> moneykeep_server::store::StoreResult<zeroize::Zeroizing<Vec<u8>>> {
        sealed
            .strip_prefix(b"SEALED:".as_slice())
            .map(|p| zeroize::Zeroizing::new(p.to_vec()))
            .ok_or_else(|| "not sealed".to_owned())
    }
}

/// Every file of a folder and its bytes, sorted (to see that nothing changed).
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

/// The user's old data folder from before A1, as on the owner's computer: Wi-Fi
/// capture on ports 15443 and 15080, a plain (pre-L5) inbox key, one phone paired
/// over Wi-Fi and one expense waiting in the inbox, and the Wi-Fi certificate.
fn old_user_folder(old: &Path) {
    std::fs::create_dir_all(old).unwrap();
    let c = moneykeep_server::Config {
        wifi_address: "192.168.1.20".into(),
        wifi_https_port: 15443,
        wifi_http_port: 15080,
        ..moneykeep_server::Config::default()
    };
    std::fs::write(
        old.join("server.toml"),
        moneykeep_server::config::render(&c),
    )
    .unwrap();
    let s = moneykeep_server::Store::open(old, moneykeep_server::KeyHome::File).unwrap();
    let phone = s
        .add_device("Pat's phone", b"phone-token-hash", 1_790_000_000, "wifi")
        .unwrap();
    s.add_submission(
        &phone,
        "client-1",
        &mk_capture_api::ExpenseFields::default(),
        &[],
        1_790_000_100,
    )
    .unwrap();
    drop(s);
    for (f, b) in [
        ("wifi.json", &b"{\"address\":\"192.168.1.20\"}"[..]),
        ("wifi-ca.crt", b"CA CERT"),
        ("wifi-server.crt", b"SERVER CERT"),
        ("wifi-ca-key.sealed", b"SEALED CA KEY"),
        ("wifi-tls-key.sealed", b"SEALED TLS KEY"),
        ("server.log", b"log lines"),
    ] {
        std::fs::write(old.join(f), b).unwrap();
    }
    assert!(old.join("inbox.key").is_file(), "a plain key, as before L5");
}

/// SR-A6: the capture service is missing and the data is still in the user's old
/// folder. The firewall check says so (the desktop then shows no firewall banner);
/// a firewall Fix plan is accepted with the old folder's ports; a plan naming any
/// other source folder is refused whole; a failed install leaves the old folder
/// untouched and no service behind; Install the server again copies the known
/// files, seals the inbox key, marks the old folder moved, installs the service
/// with the data folder, recreates the firewall rule with the old ports, and the
/// pairing and the inbox item are still there.
#[test]
fn sr_a6_install_the_server_again() {
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("ProgramData/MoneyKeep");
    std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
    let data = machine.join("capture-server");
    let old = root
        .path()
        .join("Users/pat/AppData/Local/MoneyKeep/data/capture-server");
    old_user_folder(&old);
    let sources = vec![old.clone()];
    let work = tempfile::tempdir().unwrap();
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    let rule = |ports: Vec<u16>| services::FirewallRule {
        name: mk_capture_api::FIREWALL_RULE.into(),
        ports,
        profiles: vec!["Private".into(), "Domain".into()],
        program: None,
    };
    let sm = FakeSm::default();
    *sm.firewall.borrow_mut() = true;
    let probe = FakeProbe(&sm);
    let env = Env {
        source_dirs: &sources,
        sealer: Some(&FakeSealer),
        ..windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data)
    };

    // Detecting it: no service, data only in the old folder.
    assert!(services::data_found(&data, &sources));
    assert!(!services::data_found(&data, &[]));
    let check = services::firewall_check(&sm, mk_capture_api::FIREWALL_RULE);
    assert_eq!(check.server, Some(State::NotInstalled));
    assert_eq!(check.rule, services::RuleState::Missing);

    // A plan naming any other source folder is refused whole.
    let elsewhere = root
        .path()
        .join("Users/eve/AppData/Local/MoneyKeep/data/capture-server");
    old_user_folder(&elsewhere);
    let before = snapshot(&old);
    for (p, why) in [
        (
            Plan {
                reinstall_server: true,
                source_data_dir: Some(elsewhere.clone()),
                ..Plan::default()
            },
            "copying from another folder",
        ),
        (
            Plan {
                reinstall_server: true,
                source_data_dir: Some(PathBuf::from("C:/Windows/System32/config")),
                ..Plan::default()
            },
            "copying from another folder",
        ),
        (
            Plan {
                source_data_dir: Some(old.clone()),
                ..Plan::default()
            },
            "without installing the server again",
        ),
    ] {
        let r = services::apply(&p, &built(work.path(), b"NEW"), &env);
        let e = r.error.unwrap();
        assert!(e.contains(why) && e.contains("Nothing was changed"), "{e}");
        assert!(sm.log.borrow().is_empty(), "{:?}", sm.log.borrow());
        assert!(!data.exists(), "not even the data folder was made");
    }
    assert_eq!(snapshot(&old), before);

    // A firewall Fix plan with server.toml only in the old folder: accepted with
    // its ports, for the program where the service is installed; other ports are
    // still refused.
    let r = services::apply(
        &Plan {
            firewall_rule: Some(rule(vec![14443, 14080])),
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.error.unwrap().contains("other ports"));
    let r = services::apply(
        &Plan {
            firewall_rule: Some(rule(vec![15080, 15443])),
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(sm.rules.borrow()[0].ports, [15080, 15443]);
    assert_eq!(
        sm.rules.borrow()[0].program.as_deref(),
        Some(work.path().join(services::program_name()).as_path())
    );
    sm.rules.borrow_mut().clear();

    let again = Plan {
        reinstall_server: true,
        source_data_dir: Some(old.clone()),
        firewall_rule: Some(rule(vec![15443, 15080])),
        ..Plan::default()
    };
    // A failed install (the server does not answer): the service is removed again,
    // the copies go, and the old folder is left exactly as it was.
    *sm.fail_start.borrow_mut() = true;
    let r = services::apply(&again, &built(work.path(), b"BROKEN"), &env);
    let e = r.error.unwrap();
    assert!(e.contains("did not answer its health check"), "{e}");
    assert!(e.contains("was not changed"), "{e}");
    assert_eq!(sm.server_state(), State::NotInstalled);
    assert!(sm.log.borrow().iter().any(|l| l == "uninstall_server"));
    assert_eq!(snapshot(&old), before, "the old folder is untouched");
    assert!(!old.join(services::MOVED_MARKER).exists());
    assert!(!data.join("server.toml").exists() && !data.join("inbox.sqlite").exists());
    assert!(!data.join("inbox.key.sealed").exists());
    assert!(
        sm.rules.borrow().is_empty(),
        "no rule for a server that isn't there"
    );
    *sm.fail_start.borrow_mut() = false;
    sm.log.borrow_mut().clear();

    // Install the server again.
    let r = services::apply(&again, &built(work.path(), b"NEW"), &env);
    assert!(r.ok, "{r:?}");
    for f in [
        "server.toml",
        "inbox.sqlite",
        "wifi.json",
        "wifi-ca.crt",
        "wifi-server.crt",
        "wifi-ca-key.sealed",
        "wifi-tls-key.sealed",
    ] {
        assert_eq!(
            std::fs::read(data.join(f)).unwrap(),
            // upgrade-resilience-1 U1.3: the old server.toml is renamed.
            std::fs::read(old.join(if f == "server.toml" {
                services::RETIRED_CONFIG
            } else {
                f
            }))
            .unwrap(),
            "{f}"
        );
    }
    // L5: the inbox key is sealed there, never plain.
    assert!(!data.join("inbox.key").exists());
    let sealed = std::fs::read(data.join("inbox.key.sealed")).unwrap();
    let old_key = std::fs::read_to_string(old.join("inbox.key")).unwrap();
    assert_eq!(
        sealed,
        [b"SEALED:".as_slice(), old_key.trim().as_bytes()].concat()
    );
    // The old folder is marked moved (Remove the old copy is offered), its
    // server.toml renamed (upgrade-resilience-1 U1.3), otherwise untouched.
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(old.join(services::MOVED_MARKER)).unwrap()).unwrap();
    assert_eq!(marker["to"], data.display().to_string());
    let mut now = snapshot(&old);
    now.retain(|(n, _)| n != services::MOVED_MARKER);
    for (n, _) in &mut now {
        if n == services::RETIRED_CONFIG {
            *n = "server.toml".into();
        }
    }
    now.sort();
    assert_eq!(now, before);
    // The service: installed from the install folder with the data folder, running.
    assert_eq!(sm.server_state(), State::Running);
    let args = sm.server_command().unwrap();
    assert_eq!(services::data_of(&args).unwrap(), data);
    assert_eq!(
        args[0],
        work.path()
            .join(services::program_name())
            .display()
            .to_string()
    );
    assert!(!args.iter().any(|a| a == "--allow-lan"));
    let cfg = services::windows::install_config(Path::new(&args[0]), &args[1..]);
    assert_eq!(cfg.account, "NT SERVICE\\MoneyKeepCapture");
    // The firewall rule with the old folder's ports, for the installed program.
    assert_eq!(sm.rules.borrow().len(), 1);
    assert_eq!(sm.rules.borrow()[0].ports, [15443, 15080]);
    assert_eq!(sm.rules.borrow()[0].program, sm.server_path());
    assert!(
        r.actions.iter().any(|a| a.contains("installed again")),
        "{r:?}"
    );
    // The Wi-Fi pairing and the inbox item are still there: open a copy of the
    // data folder with the unsealed key.
    let check = root.path().join("check");
    std::fs::create_dir_all(&check).unwrap();
    std::fs::copy(data.join("inbox.sqlite"), check.join("inbox.sqlite")).unwrap();
    let key = moneykeep_server::store::load_key_with(
        &data,
        moneykeep_server::KeyHome::File,
        Some(&FakeSealer),
    )
    .unwrap();
    use base64::Engine as _;
    std::fs::write(
        check.join("inbox.key"),
        base64::engine::general_purpose::STANDARD.encode(*key),
    )
    .unwrap();
    let s = moneykeep_server::Store::open(&check, moneykeep_server::KeyHome::File).unwrap();
    let phones = s.devices().unwrap();
    assert_eq!(phones.len(), 1);
    assert_eq!(phones[0].name, "Pat's phone");
    assert_eq!(s.inbox().unwrap().len(), 1);
    // Detected as installed now; a second Install the server again changes nothing.
    assert!(!services::status(&sm, None).data_found);
    let r = services::apply(&again, &built(work.path(), b"NEW"), &env);
    // upgrade-resilience-1 UR-3: the folder moved out of is no source any more.
    assert!(
        r.error.as_deref().unwrap().contains("moved out of before"),
        "{r:?}"
    );
    let r = services::apply(
        &Plan {
            source_data_dir: None,
            ..again.clone()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.error.unwrap().contains("already installed"));
    assert_eq!(sm.server_state(), State::Running);
}

/// SR-A6: with the data already in the protected folder (the service was removed,
/// the data kept), Install the server again uses it and copies nothing.
#[test]
fn sr_a6_install_again_with_data_in_place() {
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("ProgramData/MoneyKeep");
    let data = machine.join("capture-server");
    old_user_folder(&data);
    let old = root
        .path()
        .join("Users/pat/AppData/Local/MoneyKeep/data/capture-server");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("server.toml"), b"OLDER").unwrap();
    let sources = vec![old.clone()];
    let work = tempfile::tempdir().unwrap();
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    let env = Env {
        source_dirs: &sources,
        sealer: Some(&FakeSealer),
        ..windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data)
    };
    assert!(services::data_found(&data, &[]));
    let r = services::apply(
        &Plan {
            reinstall_server: true,
            source_data_dir: Some(old.clone()),
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.ok, "{r:?}");
    assert_ne!(std::fs::read(data.join("server.toml")).unwrap(), b"OLDER");
    assert!(!old.join(services::MOVED_MARKER).exists());
    assert_eq!(sm.server_state(), State::Running);
    // Nothing found anywhere: not installed again.
    let empty = tempfile::tempdir().unwrap();
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    let nothing = empty.path().join("capture-server");
    let env = Env {
        sealer: Some(&FakeSealer),
        ..windows_env(&sm, &sig, &dl, &probe, work.path(), empty.path(), &nothing)
    };
    let r = services::apply(
        &Plan {
            reinstall_server: true,
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.error.unwrap().contains("data was not found"));
    assert_eq!(sm.server_state(), State::NotInstalled);
}

// ----- upgrade-resilience-1 (Batch U1) ---------------------------------------------

/// Never healthy (a server that doesn't answer after the move).
struct DeadProbe;

impl Probe for DeadProbe {
    fn healthy(&self, _: &Path) -> bool {
        false
    }
}

/// The owner's computer before the move: the service runs an older program from
/// outside the install folder (Setup's `/D=C:\MK`) with the data in the user's
/// folder (a plain `inbox.key`, as before L5), with `--allow-lan`.
struct PreMove {
    root: tempfile::TempDir,
    machine: PathBuf,
    data: PathBuf,
    old: PathBuf,
    old_program: PathBuf,
    install: PathBuf,
    exe: PathBuf,
}

fn pre_move(sm: &FakeSm) -> PreMove {
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("ProgramData/MoneyKeep");
    std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
    let data = machine.join("capture-server");
    let old = root
        .path()
        .join("Users/pat/AppData/Local/MoneyKeep/data/capture-server");
    old_user_folder(&old);
    let old_program = root.path().join("MK").join(services::program_name());
    std::fs::create_dir_all(old_program.parent().unwrap()).unwrap();
    std::fs::write(&old_program, b"OLD PROGRAM").unwrap();
    let install = root.path().join("Program Files/MoneyKeep/capture");
    let exe = built(root.path(), b"THIS BUILD");
    sm.install_server(&old_program, &services::server_args(&old, true))
        .unwrap();
    sm.log.borrow_mut().clear();
    PreMove {
        root,
        machine,
        data,
        old,
        old_program,
        install,
        exe,
    }
}

fn move_env<'a>(
    sm: &'a FakeSm,
    probe: &'a dyn Probe,
    p: &'a PreMove,
    sig: &'a FakeSig,
    dl: &'a FakeDl,
) -> Env<'a> {
    Env {
        sm,
        sig,
        dl,
        probe,
        can_download: false,
        tmp: p.root.path(),
        install_dir: &p.install,
        data_dir: &p.data,
        machine_dir: Some(&p.machine),
        check_cloudflared: false,
        legacy: None,
        source_dirs: &[],
        sealer: Some(&FakeSealer),
    }
}

/// UR-1 (H1, M1, M3): the move first refreshes the service's program (outside the
/// install folder before) with this build's, and the service runs it on the new
/// folder; a server that fails its health check after the move is pointed back with
/// its old program and arguments, the copies are removed and no `moved.json` is
/// left; a destination with an old sealed key and a source with a plain key ends with
/// the source's key; a program that can't be refreshed moves nothing.
#[test]
fn ur1_safe_data_move() {
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    // The health check fails: everything goes back.
    let sm = FakeSm::default();
    let p = pre_move(&sm);
    let before = snapshot(&p.old);
    let r = services::move_data_only(&p.exe, &move_env(&sm, &DeadProbe, &p, &sig, &dl));
    let e = r.error.unwrap();
    assert!(e.contains("did not answer its health check"), "{e}");
    assert!(e.contains("keeps using"), "{e}");
    let args = sm.server_command().unwrap();
    assert_eq!(PathBuf::from(&args[0]), p.old_program);
    assert_eq!(services::data_of(&args).unwrap(), p.old);
    assert!(args.iter().any(|a| a == "--allow-lan"));
    assert_eq!(snapshot(&p.old), before, "the old folder is untouched");
    assert!(!p.old.join(services::MOVED_MARKER).exists());
    assert!(
        std::fs::read_dir(&p.data)
            .ok()
            .is_none_or(|mut d| d.next().is_none()),
        "the copies are removed"
    );
    assert_eq!(sm.log.borrow().last().unwrap(), "start_server");

    // M3: an old sealed key in the destination (an earlier, unfinished move) and a
    // plain key in the source: the source's key wins.
    std::fs::create_dir_all(&p.data).unwrap();
    std::fs::write(
        p.data.join("inbox.key.sealed"),
        b"SEALED:STALEKEYFROMBEFORE",
    )
    .unwrap();
    let probe = FakeProbe(&sm);
    let r = services::move_data_only(&p.exe, &move_env(&sm, &probe, &p, &sig, &dl));
    assert!(r.ok, "{r:?}");
    let source_key = std::fs::read_to_string(p.old.join("inbox.key")).unwrap();
    assert_eq!(
        std::fs::read(p.data.join("inbox.key.sealed")).unwrap(),
        [b"SEALED:".as_slice(), source_key.trim().as_bytes()].concat()
    );
    // H1: the service runs this build's program from the install folder.
    let target = p.install.join(services::program_name());
    assert_eq!(std::fs::read(&target).unwrap(), b"THIS BUILD");
    let args = sm.server_command().unwrap();
    assert_eq!(PathBuf::from(&args[0]), target);
    assert_eq!(services::data_of(&args).unwrap(), p.data);
    assert!(args.iter().any(|a| a == "--allow-lan"));
    // The format number, the marker and (U1.3) the retired settings.
    assert_eq!(
        moneykeep_server::upgrade::read_format(&p.data),
        Some(moneykeep_server::upgrade::DATA_FORMAT)
    );
    assert!(p.old.join(services::MOVED_MARKER).is_file());
    assert!(!p.old.join("server.toml").exists());
    assert!(p.old.join(services::RETIRED_CONFIG).is_file());

    // A program that can't be refreshed: nothing is moved, the old server starts.
    let sm = FakeSm::default();
    let p = pre_move(&sm);
    std::fs::create_dir_all(p.install.parent().unwrap()).unwrap();
    std::fs::write(&p.install, b"a file where the folder should be").unwrap();
    let probe = FakeProbe(&sm);
    let r = services::move_data_only(&p.exe, &move_env(&sm, &probe, &p, &sig, &dl));
    let e = r.error.unwrap();
    assert!(e.contains("program could not be updated"), "{e}");
    assert!(!p.data.join("server.toml").exists());
    assert_eq!(*sm.log.borrow(), ["stop_server", "start_server"]);
    assert_eq!(
        services::data_of(&sm.server_command().unwrap()).unwrap(),
        p.old
    );
}

/// UR-3 (M2): a folder holding `moved.json` is never offered or accepted as a
/// source: not counted as data found, refused in a plan, never moved from.
#[test]
fn ur3_moved_folder_is_never_a_source() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("ProgramData/MoneyKeep/capture-server");
    let old = root
        .path()
        .join("Users/pat/AppData/Local/MoneyKeep/data/capture-server");
    old_user_folder(&old);
    assert!(services::data_found(&data, std::slice::from_ref(&old)));
    std::fs::write(old.join(services::MOVED_MARKER), b"{\"to\":\"x\",\"at\":1}").unwrap();
    assert!(!services::data_found(&data, std::slice::from_ref(&old)));
    let machine = data.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
    let work = tempfile::tempdir().unwrap();
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    let sm = FakeSm::default();
    let probe = FakeProbe(&sm);
    let sources = vec![old.clone()];
    let env = Env {
        source_dirs: &sources,
        ..windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data)
    };
    let r = services::apply(
        &Plan {
            reinstall_server: true,
            source_data_dir: Some(old.clone()),
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &env,
    );
    assert!(r.error.unwrap().contains("moved out of before"));
    assert_eq!(sm.server_state(), State::NotInstalled);
    // A service still naming it: not moved again.
    sm.install_server(
        Path::new("C:/MoneyKeep/moneykeep-server.exe"),
        &services::server_args(&old, false),
    )
    .unwrap();
    let r = services::move_data_only(&built(work.path(), b"NEW"), &env);
    assert!(
        r.error.as_deref().unwrap_or("").contains("not moved again"),
        "{r:?}"
    );
    assert!(!data.join("server.toml").exists());
}

/// UR-4 (helper part): the helper never touches a data folder of a newer format.
#[test]
fn ur4_helper_refuses_a_newer_folder() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("capture-server");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("format.json"), b"{\"format\": 99}").unwrap();
    std::fs::write(data.join("server.toml"), b"listen = \"127.0.0.1:8787\"\n").unwrap();
    let work = tempfile::tempdir().unwrap();
    let sm = FakeSm::default();
    let r = run_apply(
        &Plan {
            desktop_token_sha256: Some("ab".repeat(32)),
            install_server: true,
            ..Plan::default()
        },
        &built(work.path(), b"NEW"),
        &sm,
        &FakeSig(Err("x".into())),
        &FakeDl::default(),
        false,
        root.path(),
        &work.path().join("install"),
    );
    assert_eq!(r.error.unwrap(), moneykeep_server::store::NEWER_DATA);
    assert_eq!(sm.server_state(), State::NotInstalled);
    assert!(!data.join("inbox.sqlite").exists());
}

/// UR-5 (M5, L1, L2): the service is registered with its failure actions; Update
/// keeps `--allow-lan` when the plan doesn't say; installing over a program keeps
/// it as `.prev` and puts it back when the new one fails its health check.
#[test]
fn ur5_service_looks_after_itself() {
    let cfg = services::windows::install_config(Path::new("C:/x.exe"), &[]);
    assert_eq!(cfg.restart_after_secs, [10, 30, 60]);
    assert_eq!(cfg.failure_reset_secs, 86_400);
    assert!(cfg.failure_actions_on_exit_code);
    // L1.
    let work = tempfile::tempdir().unwrap();
    let install = work.path().join("install");
    let target = install.join(services::program_name());
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(&target, b"OLD").unwrap();
    let sm = FakeSm::default();
    let data = work.path().join("capture-server");
    sm.install_server(&target, &services::server_args(&data, true))
        .unwrap();
    let run = |p: &Plan, exe: &Path| {
        run_apply(
            p,
            exe,
            &sm,
            &FakeSig(Err("x".into())),
            &FakeDl::default(),
            false,
            work.path(),
            &install,
        )
    };
    let update = Plan {
        update_server: true,
        ..Plan::default()
    };
    let r = run(&update, &built(work.path(), b"NEW"));
    assert!(r.ok, "{r:?}");
    assert!(
        sm.server_command()
            .unwrap()
            .iter()
            .any(|a| a == "--allow-lan")
    );
    let r = run(
        &Plan {
            allow_lan: Some(false),
            ..update.clone()
        },
        &built(work.path(), b"NEW"),
    );
    assert!(r.ok, "{r:?}");
    assert!(
        !sm.server_command()
            .unwrap()
            .iter()
            .any(|a| a == "--allow-lan")
    );
    // L2 on install: a broken program over a working one.
    *sm.fail_start.borrow_mut() = true;
    let r = run(&plan(None), &built(work.path(), b"BROKEN"));
    assert!(
        r.error
            .unwrap()
            .contains("previous server program was restored"),
        "{:?}",
        sm.log.borrow()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert!(!services::prev_program(&install).exists());
    assert_eq!(sm.server_state(), State::Running);
    // A working one: the previous program goes once it passed.
    *sm.fail_start.borrow_mut() = false;
    let r = run(&plan(None), &built(work.path(), b"NEWER"));
    assert!(r.ok, "{r:?}");
    assert_eq!(std::fs::read(&target).unwrap(), b"NEWER");
    assert!(!services::prev_program(&install).exists());
}

thread_local! {
    static PROCESSES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A Windows stand-in that records every process it would start.
struct RecordingApi;

impl services::windows::WindowsApi for RecordingApi {
    fn query(_: &str) -> Option<services::windows::Queried> {
        None
    }
    fn install(_: &services::windows::InstallConfig) -> Result<(), String> {
        Ok(())
    }
    fn stop(_: &str) -> Result<(), String> {
        Ok(())
    }
    fn start(_: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(_: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_hidden(program: &Path, _: &[&str]) -> Result<std::process::Output, String> {
        PROCESSES.with(|p| p.borrow_mut().push(program.display().to_string()));
        Err("no processes here".into())
    }
    fn run_hidden_input(
        program: &Path,
        _: &[&str],
        _: &[u8],
    ) -> Result<std::process::Output, String> {
        PROCESSES.with(|p| p.borrow_mut().push(program.display().to_string()));
        Err("no processes here".into())
    }
    fn run_service(_: PathBuf, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn dpapi(protect: bool, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        assert_eq!(entropy, services::windows::DPAPI_ENTROPY);
        Ok(if protect {
            [b"DPAPI:".as_slice(), data].concat()
        } else {
            data.strip_prefix(b"DPAPI:".as_slice())
                .ok_or("not sealed")?
                .to_vec()
        })
    }
}

/// UR-6 (M6): sealing and opening the inbox key start no process (no PowerShell);
/// a blocked PowerShell step says so.
#[test]
fn ur6_key_sealing_starts_no_process() {
    use services::windows::{POWERSHELL_BLOCKED, dpapi_with, powershell_blocked};
    let sealed = dpapi_with::<RecordingApi>(true, b"key text").unwrap();
    assert_eq!(
        dpapi_with::<RecordingApi>(false, &sealed).unwrap(),
        b"key text"
    );
    assert!(PROCESSES.with(|p| p.borrow().is_empty()));
    assert!(powershell_blocked(
        "Cannot invoke method. Method invocation is supported only on core types in this language mode."
    ));
    assert!(powershell_blocked(
        "File x.ps1 cannot be loaded because running scripts is disabled on this system."
    ));
    assert!(!powershell_blocked("Access to the path is denied"));
    assert!(POWERSHELL_BLOCKED.starts_with("Windows blocked a PowerShell step"));
}

/// UR-9 (L6): MoneyKeep's machine folder made by someone else: the refusal says to
/// remove it.
#[test]
fn ur9_l6_machine_folder_not_ours() {
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("ProgramData/MoneyKeep");
    std::fs::create_dir_all(&machine).unwrap();
    let data = machine.join("capture-server");
    let work = tempfile::tempdir().unwrap();
    let (sig, dl) = (FakeSig(Err("unused".into())), FakeDl::default());
    let sm = FakeSm::default();
    sm.owners
        .borrow_mut()
        .push((machine.clone(), "S-1-5-21-1-2-3-1001".into()));
    let probe = FakeProbe(&sm);
    let env = windows_env(&sm, &sig, &dl, &probe, work.path(), &machine, &data);
    let r = services::move_data_only(&built(work.path(), b"NEW"), &env);
    assert_eq!(
        r.error.unwrap(),
        format!(
            "The capture server's folder isn't safe to use: {m} Remove the folder {m} (it was not made by MoneyKeep's setup) and try again.",
            m = machine.display()
        )
    );
}
