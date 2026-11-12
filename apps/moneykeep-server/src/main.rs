//! `moneykeep-server`: the phone capture server's command line
//! (mobile-capture.md "Commands" and revision 2 section B5).

#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use moneykeep_server::{Clock, Config, KeyHome, Options, services};

#[derive(Debug, Parser)]
#[command(
    name = "moneykeep-server",
    version,
    about = "MoneyKeep phone capture server (an inbox for your phone's expenses)"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Create the data folder, server.toml, the inbox and the desktop token.
    Init {
        #[arg(long)]
        data: PathBuf,
        /// Keep the inbox key in inbox.key in the data folder (services).
        #[arg(long)]
        key_file: bool,
        /// Print the desktop token as JSON (for MoneyKeep's setup).
        #[arg(long)]
        json: bool,
    },
    /// Write server.toml's Access and address settings (MoneyKeep's setup).
    Configure {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        team_domain: String,
        #[arg(long)]
        aud: String,
        #[arg(long, default_value = mk_capture_api::DEFAULT_LISTEN)]
        listen: String,
        #[arg(long)]
        client_url: Option<String>,
        #[arg(long, default_value = "")]
        service_client_id: String,
    },
    /// Set server.toml's access_aud (MoneyKeep's Check, mobile-capture.md revision
    /// 8); prints {"changed": bool}. The server reads it when it next starts.
    SetAud {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        aud: String,
    },
    /// Run the server in the foreground (for systemd, launchd or by hand).
    Run {
        #[arg(long)]
        data: PathBuf,
        /// Allow listening on a non-loopback address.
        #[arg(long)]
        allow_lan: bool,
        /// Override `listen` from server.toml.
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        key_file: bool,
    },
    /// Run as a Windows service (started by the Service Control Manager).
    ServiceRun {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        allow_lan: bool,
        #[arg(long)]
        key_file: bool,
    },
    /// Replace the desktop token (the old one stops working).
    RotateDesktopToken {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        key_file: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show the data folder's state.
    Status {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        key_file: bool,
    },
    /// The background services (MoneyKeep's guided setup).
    Services {
        #[command(subcommand)]
        action: ServicesCmd,
    },
    /// This program's versions as JSON: {"version", "capture_version", "has_app"}
    /// (release-packaging.md: `cargo xtask release` checks them).
    Version,
    /// Wi-Fi capture (MoneyKeep's Wi-Fi setup, mobile-capture.md revision 10).
    Wifi {
        #[command(subcommand)]
        action: WifiCmd,
    },
}

#[derive(Debug, Subcommand)]
enum WifiCmd {
    /// Print this computer's network addresses (adapter, MAC address, network
    /// profile) as JSON.
    Addresses,
    /// Turn Wi-Fi capture on at this address: server.toml, the certificate
    /// authority and the server certificate (keys in the credential store, or
    /// sealed files with --key-file). Prints what was made as JSON. The server
    /// reads it when it next starts.
    Configure {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        address: String,
        /// A local name for the address, e.g. moneykeep.home.arpa.
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value_t = mk_capture_api::DEFAULT_WIFI_HTTPS_PORT)]
        https_port: u16,
        #[arg(long, default_value_t = mk_capture_api::DEFAULT_WIFI_HTTP_PORT)]
        http_port: u16,
        #[arg(long)]
        key_file: bool,
        /// The computer's name in the certificate authority's name.
        #[arg(long)]
        computer: Option<String>,
    },
    /// Turn Wi-Fi capture off: server.toml, the certificates and their keys.
    Remove {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        key_file: bool,
    },
    /// Check that the Wi-Fi address answers over HTTPS with this server's
    /// certificate; prints {"ok": bool, "error": …}.
    Check {
        #[arg(long)]
        data: PathBuf,
        /// Keep trying for this many seconds (the service may be starting).
        #[arg(long, default_value_t = 0)]
        wait: u64,
    },
}

#[derive(Debug, Subcommand)]
enum ServicesCmd {
    /// Print the services' state as JSON.
    Status,
    /// Carry out a plan file written by MoneyKeep (deleted once read), writing a
    /// report file. Needs administrator rights on Windows.
    Apply {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        report: PathBuf,
        /// The plan's SHA-256 (64 hexadecimal digits), from the desktop that wrote
        /// and approved it; a plan that differs is refused.
        #[arg(long)]
        plan_sha256: String,
    },
    /// Move the capture server's data into its protected folder when its service
    /// still uses the folder in a user profile (security-review-1 A1; the installer
    /// runs this as administrator). Prints the report as JSON.
    MoveData,
    /// Print the firewall check as JSON (mobile-capture.md revision 18): Wi-Fi
    /// capture's rule (ok, missing or disabled), Windows' own Block rules for this
    /// program, and the networks Windows treats as Public. Reads only.
    FirewallCheck {
        /// The rule's name.
        #[arg(long, default_value = mk_capture_api::FIREWALL_RULE)]
        rule: String,
    },
    /// Put Wi-Fi capture's firewall rule back when Wi-Fi capture is on in this data
    /// folder's server.toml and the rule is missing or disabled (the installer runs
    /// this as administrator). Prints what was done as JSON.
    RepairFirewall {
        /// The data folder (default: the service's, security-review-1 A1).
        #[arg(long)]
        data: Option<PathBuf>,
    },
}

fn key_home(key_file: bool) -> KeyHome {
    if key_file {
        KeyHome::File
    } else {
        KeyHome::Auto
    }
}

fn print_token(token: &str, json: bool, heading: &str) {
    if json {
        println!("{}", serde_json::json!({ "desktop_token": token }));
    } else {
        println!("{heading}");
        println!("{token}");
    }
}

fn configure(
    data: &Path,
    team_domain: String,
    aud: String,
    listen: String,
    client_url: Option<String>,
    service_client_id: String,
) -> Result<(), String> {
    moneykeep_server::setup::configure(
        data,
        &moneykeep_server::setup::ConfigureStep {
            team_domain,
            aud,
            listen,
            client_url,
            service_client_id,
        },
    )
}

fn set_aud(data: &Path, aud: &str) -> Result<(), String> {
    let changed = moneykeep_server::setup::set_aud(data, aud)?;
    println!("{}", serde_json::json!({ "changed": changed }));
    Ok(())
}

fn apply_plan(plan_file: &Path, report_file: &Path, sha256: &str) -> Result<(), String> {
    // security-review-1 A3: the plan is read once, with no write sharing, and must
    // be the one the desktop approved.
    let bytes = services::read_plan(plan_file, sha256);
    let _ = std::fs::remove_file(plan_file);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sm = services::system();
    let old_sm = services::legacy_system();
    let old_install = services::legacy_install_dir();
    let sig = services::signature_checker();
    let install = services::install_dir();
    let data = services::data_dir();
    let sources = services::source_dirs();
    let machine = services::machine_dir();
    let tmp = match &machine {
        Some(m) => m.join("tmp"),
        None => std::env::temp_dir(),
    };
    let sealer = moneykeep_server::store::platform_sealer(&data);
    let report = match bytes {
        Err(e) => services::Report {
            error: Some(e),
            ..services::Report::default()
        },
        Ok(bytes) => services::apply_bytes(
            &bytes,
            &exe,
            &services::Env {
                sm: sm.as_ref(),
                sig: sig.as_ref(),
                dl: &services::HttpsDownloader,
                probe: &services::HttpProbe,
                can_download: cfg!(windows),
                tmp: &tmp,
                install_dir: &install,
                data_dir: &data,
                machine_dir: machine.as_deref(),
                check_cloudflared: cfg!(windows) || cfg!(target_os = "macos"),
                legacy: Some(services::Legacy {
                    sm: old_sm.as_ref(),
                    install_dir: &old_install,
                }),
                source_dirs: &sources,
                sealer: sealer.as_deref(),
            },
        ),
    };
    let json = serde_json::to_vec(&report).map_err(|e| e.to_string())?;
    services::write_report(report_file, &json)?;
    match report.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn wifi_configure(
    data: &Path,
    address: &str,
    name: &str,
    ports: (u16, u16),
    key_file: bool,
    computer: Option<String>,
) -> Result<(), String> {
    let v = moneykeep_server::setup::wifi_configure(
        data,
        &moneykeep_server::setup::WifiStep {
            address: address.to_owned(),
            name: name.to_owned(),
            https_port: ports.0,
            http_port: ports.1,
            computer,
        },
        key_home(key_file),
    )?;
    println!("{v}");
    Ok(())
}

fn wifi_remove(data: &Path, key_file: bool) -> Result<(), String> {
    let actions = moneykeep_server::setup::wifi_remove(data, key_home(key_file))?;
    println!("{}", serde_json::json!({ "actions": actions }));
    Ok(())
}

fn wifi_check(data: &Path, wait: u64) -> Result<(), String> {
    println!("{}", moneykeep_server::setup::wifi_check(data, wait)?);
    Ok(())
}

fn run(
    data: PathBuf,
    allow_lan: bool,
    listen: Option<String>,
    key_file: bool,
) -> Result<(), String> {
    Options::for_dir(&data, allow_lan).and_then(|mut o| {
        if let Some(l) = listen {
            o.config.listen = l;
        }
        o.key_home = key_home(key_file);
        moneykeep_server::start(o).map(moneykeep_server::Running::wait)
    })
}

/// upgrade-resilience-1 U1.4: a refused start exits with the refusal's own code
/// (systemd keeps it as the unit's exit status; `services status` reads it).
fn run_exit(data: PathBuf, allow_lan: bool, listen: Option<String>, key_file: bool) -> ExitCode {
    match run(data.clone(), allow_lan, listen, key_file) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("moneykeep-server: {e}");
            match moneykeep_server::refusal_for(&data, &e) {
                Some(r) => ExitCode::from(r.code()),
                None => ExitCode::FAILURE,
            }
        }
    }
}

/// `service-run` outside Windows (the macOS launchd agent): the service's start
/// (log in `server.log`, inbox key in its file), then the refusal's exit code
/// when it refuses (U1.4: `launchctl print` shows it as the last exit code).
fn agent_exit(data: PathBuf, allow_lan: bool) -> ExitCode {
    match services::windows::start_for_service(&data, allow_lan) {
        Ok(running) => {
            running.wait();
            ExitCode::SUCCESS
        }
        Err(e) => {
            moneykeep_server::file_log(&data.join("server.log"))(&format!("could not start: {e}"));
            eprintln!("moneykeep-server: {e}");
            match moneykeep_server::refusal_for(&data, &e) {
                Some(r) => ExitCode::from(r.code()),
                None => ExitCode::FAILURE,
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let now = Clock::default().now();
    let result = match cli.command {
        Cmd::Init {
            data,
            key_file,
            json,
        } => moneykeep_server::init(&data, key_home(key_file), now).map(|token| {
            if !json {
                println!("Created {}.", data.display());
                println!(
                    "Edit {} (access_team_domain, access_aud), then run: moneykeep-server run --data {}",
                    data.join("server.toml").display(),
                    data.display()
                );
                println!();
            }
            print_token(
                token.as_str(),
                json,
                "Desktop token (shown once; enter it in MoneyKeep > Settings > Phone capture):",
            );
        }),
        Cmd::Configure {
            data,
            team_domain,
            aud,
            listen,
            client_url,
            service_client_id,
        } => configure(&data, team_domain, aud, listen, client_url, service_client_id),
        Cmd::SetAud { data, aud } => set_aud(&data, &aud),
        Cmd::Run {
            data,
            allow_lan,
            listen,
            key_file,
        } => return run_exit(data, allow_lan, listen, key_file),
        Cmd::ServiceRun {
            data,
            allow_lan,
            key_file,
        } => {
            // Both branches compile on every system; the Windows service always
            // keeps its inbox key in its data folder (and reports a refusal through
            // its service-specific exit code). Elsewhere (the macOS launchd agent,
            // macos-release.md C1) it logs to `server.log` in the data folder as on
            // Windows, and a refusal is its exit code.
            if cfg!(windows) {
                services::windows::run_service(data, allow_lan)
            } else {
                let _ = key_file;
                return agent_exit(data, allow_lan);
            }
        }
        Cmd::RotateDesktopToken {
            data,
            key_file,
            json,
        } => moneykeep_server::rotate_desktop_token(&data, key_home(key_file), now).map(|token| {
            print_token(
                token.as_str(),
                json,
                "New desktop token (shown once; the old one no longer works):",
            );
        }),
        Cmd::Status { data, key_file } => {
            moneykeep_server::status(&data, key_home(key_file)).map(|s| println!("{s}"))
        }
        Cmd::Services { action } => match action {
            ServicesCmd::Status => {
                let sm = services::system();
                let old = services::legacy_system();
                serde_json::to_string(&services::status(sm.as_ref(), Some(old.as_ref())))
                    .map(|s| println!("{s}"))
                    .map_err(|e| e.to_string())
            }
            ServicesCmd::Apply {
                plan,
                report,
                plan_sha256,
            } => apply_plan(&plan, &report, &plan_sha256),
            ServicesCmd::MoveData => {
                let sm = services::system();
                let sig = services::signature_checker();
                let install = services::install_dir();
                let data = services::data_dir();
                let machine = services::machine_dir();
                let tmp = std::env::temp_dir();
                let sealer = moneykeep_server::store::platform_sealer(&data);
                let exe = std::env::current_exe().map_err(|e| e.to_string());
                let exe = match exe {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("moneykeep-server: {e}");
                        return ExitCode::FAILURE;
                    }
                };
                let r = services::move_data_only(&exe, &services::Env {
                    sm: sm.as_ref(),
                    sig: sig.as_ref(),
                    dl: &services::HttpsDownloader,
                    probe: &services::HttpProbe,
                    can_download: false,
                    tmp: &tmp,
                    install_dir: &install,
                    data_dir: &data,
                    machine_dir: machine.as_deref(),
                    check_cloudflared: cfg!(windows) || cfg!(target_os = "macos"),
                    legacy: None,
                    source_dirs: &[],
                    sealer: sealer.as_deref(),
                });
                // U1.1 (L3): Setup reads this report; a failure also exits non-zero.
                let failed = r.error.clone();
                serde_json::to_string(&r)
                    .map(|s| println!("{s}"))
                    .map_err(|e| e.to_string())
                    .and_then(|()| failed.map_or(Ok(()), Err))
            }
            ServicesCmd::FirewallCheck { rule } => {
                let sm = services::system();
                serde_json::to_string(&services::firewall_check(sm.as_ref(), &rule))
                    .map(|s| println!("{s}"))
                    .map_err(|e| e.to_string())
            }
            ServicesCmd::RepairFirewall { data } => {
                let data = data
                    .or_else(|| services::service_data_dir(services::system().as_ref()))
                    .unwrap_or_else(services::data_dir);
                let path = data.join(moneykeep_server::config::CONFIG_FILE);
                // U1.1 (L3): Setup reads {"ok", "actions"} or {"ok": false, "error"}.
                let r = match std::fs::read_to_string(&path) {
                    // No server here: nothing to repair.
                    Err(_) => Ok(vec!["no capture server here".to_owned()]),
                    Ok(text) => moneykeep_server::upgrade::check_format(&data)
                        .and_then(|()| Config::parse(&text))
                        .and_then(|c| {
                            let sm = services::system();
                            services::repair_firewall(sm.as_ref(), &c)
                        }),
                };
                match r {
                    Ok(a) => {
                        println!("{}", serde_json::json!({"ok": true, "actions": a}));
                        Ok(())
                    }
                    Err(e) => {
                        println!("{}", serde_json::json!({"ok": false, "error": e}));
                        Err(e)
                    }
                }
            }
        },
        Cmd::Version => {
            println!("{}", moneykeep_server::version_json());
            Ok(())
        }
        Cmd::Wifi { action } => match action {
            WifiCmd::Addresses => serde_json::to_string(&services::local_addresses())
                .map(|s| println!("{s}"))
                .map_err(|e| e.to_string()),
            WifiCmd::Configure {
                data,
                address,
                name,
                https_port,
                http_port,
                key_file,
                computer,
            } => wifi_configure(
                &data,
                &address,
                &name,
                (https_port, http_port),
                key_file,
                computer,
            ),
            WifiCmd::Remove { data, key_file } => wifi_remove(&data, key_file),
            WifiCmd::Check { data, wait } => wifi_check(&data, wait),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("moneykeep-server: {e}");
            ExitCode::FAILURE
        }
    }
}
