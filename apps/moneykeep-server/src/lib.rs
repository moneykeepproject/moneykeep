//! MoneyKeep capture server (mobile-capture.md).
//!
//! A small HTTP/1.1 server on the loopback address that `cloudflared` publishes
//! through a Cloudflare Tunnel. It keeps an encrypted **inbox** of what phones send
//! (expense fields and receipt pictures) and the **lists** document the desktop
//! publishes; it never opens a book. Every request must pass the Cloudflare Access
//! token check and carry a bearer token (RFC 6750): a phone's device token or the
//! desktop token. Logs record method, path template, status and timing only.
//!
//! With Wi-Fi capture on (mobile-capture.md revision 10) it also listens with HTTPS
//! on the computer's address on the home or office network, plus a plain-HTTP
//! helper for the certificate page ([`wifi`]).

pub mod config;
pub mod jwt;
pub mod limits;
pub mod phone_app;
pub mod pin;
pub mod precompress;
pub mod secrets;
pub mod services;
pub mod setup;
pub mod store;
pub mod testing;
pub mod upgrade;
pub mod wifi;

mod api;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub use config::Config;
pub use jwt::{CertsSource, HttpCerts, Identity, Verifier};
pub use phone_app::PhoneApp;

/// This program's version with its build's commit (reported by `/api/v1/health`,
/// mobile-capture.md revision 4).
pub const VERSION: &str = mk_capture_api::BUILD_VERSION;
/// The sources this program and its phone app were built from (release-packaging.md,
/// reported by `/api/v1/health` and `services status`).
pub const CAPTURE_VERSION: &str = mk_capture_api::CAPTURE_VERSION;

/// `moneykeep-server version`: the build version, the capture version and whether
/// the phone app is built in.
pub fn version_json() -> serde_json::Value {
    serde_json::json!({
        "version": VERSION,
        "capture_version": CAPTURE_VERSION,
        "has_app": PhoneApp::embedded().is_some(),
    })
}
pub use store::{KeyHome, Store};

/// Where log lines go (standard error by default).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// Log lines to standard error.
pub fn stderr_log() -> Log {
    Arc::new(|line: &str| eprintln!("{line}"))
}

/// `server.log` is rotated when it reaches this size (security-review-1 L6).
pub const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// Old logs kept: `server.log.1` (newest) to `server.log.3`.
pub const LOG_KEEP: usize = 3;

/// Move `path` to `path.1`, `path.1` to `path.2` and so on, dropping the oldest.
pub fn rotate_log(path: &Path) {
    let name = |n: usize| {
        let mut p = path.as_os_str().to_owned();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    };
    let _ = std::fs::remove_file(name(LOG_KEEP));
    for n in (1..LOG_KEEP).rev() {
        let _ = std::fs::rename(name(n), name(n + 1));
    }
    let _ = std::fs::rename(path, name(1));
}

/// Log lines appended to a file (the Windows service has no console), rotated at
/// [`LOG_ROTATE_BYTES`] keeping [`LOG_KEEP`] old files. Lines hold no tokens,
/// bodies, amounts or merchants.
pub fn file_log(path: &Path) -> Log {
    file_log_sized(path, LOG_ROTATE_BYTES)
}

/// [`file_log`] rotating at `limit` bytes (tests).
pub fn file_log_sized(path: &Path, limit: u64) -> Log {
    let path = path.to_path_buf();
    let lock = Arc::new(Mutex::new(()));
    Arc::new(move |line: &str| {
        use std::io::Write;
        let _guard = lock.lock();
        let secs = Clock::default().now();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() >= limit) {
            rotate_log(&path);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{secs} {line}");
        }
    })
}

/// A request method as logged (L6): at most 16 printable ASCII characters.
pub fn log_method(method: &str) -> String {
    method
        .chars()
        .filter(|c| c.is_ascii_graphic())
        .take(16)
        .collect()
}

/// The server's clock (Unix seconds), adjustable in tests.
#[derive(Debug, Default)]
pub struct Clock {
    offset: AtomicI64,
}

impl Clock {
    pub fn now(&self) -> i64 {
        let real = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        real.saturating_add(self.offset.load(Ordering::SeqCst))
    }

    /// Move the clock forward (tests).
    pub fn advance(&self, secs: i64) {
        self.offset.fetch_add(secs, Ordering::SeqCst);
    }
}

/// How to run a server.
pub struct Options {
    pub data_dir: PathBuf,
    pub config: Config,
    /// `--allow-lan`: permit a non-loopback listen address.
    pub allow_lan: bool,
    pub key_home: KeyHome,
    /// Where Access certificates come from; `None` uses the team domain over HTTPS.
    pub certs: Option<Arc<dyn CertsSource>>,
    pub log: Log,
    pub clock: Arc<Clock>,
    /// Worker threads.
    pub threads: usize,
    /// The phone app served at `/`; `None` serves the "not built in" page.
    pub phone_app: Option<PhoneApp>,
    /// Where Wi-Fi capture's private keys are kept instead of the operating
    /// system's credential store (tests: a fake one).
    pub credentials: Option<Arc<dyn secrets::CredentialStore>>,
    /// The Argon2id cost of new Capture PINs.
    pub pin_cost: pin::Cost,
    /// upgrade-resilience-1 U1.5: how long the loopback listener keeps trying to
    /// bind its port (another program, or the previous server, may still hold it).
    pub bind_wait: Duration,
    /// L8: whether this computer has an address now (tests: a fake);
    /// `None` asks the system ([`wifi::address_present`]).
    pub address_present: Option<AddressSource>,
    /// L8: how often the Wi-Fi address is checked (gone: the listener stops; back:
    /// it listens again).
    pub wifi_recheck: Duration,
}

/// Whether this computer has an IPv4 address now.
pub type AddressSource = Arc<dyn Fn(std::net::Ipv4Addr) -> bool + Send + Sync>;

/// U1.5: the loopback listener's bind is retried for this long.
pub const BIND_WAIT: Duration = Duration::from_secs(60);
/// L8: the Wi-Fi address is checked this often.
pub const WIFI_RECHECK: Duration = Duration::from_secs(5);

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("data_dir", &self.data_dir)
            .field("config", &self.config)
            .field("allow_lan", &self.allow_lan)
            .finish_non_exhaustive()
    }
}

impl Options {
    /// Options for `dir` with its `server.toml`.
    pub fn for_dir(dir: &Path, allow_lan: bool) -> Result<Self, String> {
        let text = std::fs::read_to_string(dir.join(config::CONFIG_FILE)).map_err(|e| {
            format!(
                "cannot read {}: {e} (run `moneykeep-server init --data DIR` first)",
                dir.join(config::CONFIG_FILE).display()
            )
        })?;
        Ok(Self {
            data_dir: dir.to_path_buf(),
            config: Config::parse(&text)?,
            allow_lan,
            key_home: KeyHome::Auto,
            certs: None,
            log: stderr_log(),
            clock: Arc::new(Clock::default()),
            threads: 4,
            phone_app: PhoneApp::embedded(),
            credentials: None,
            pin_cost: pin::COST,
            bind_wait: BIND_WAIT,
            address_present: None,
            wifi_recheck: WIFI_RECHECK,
        })
    }
}

/// upgrade-resilience-1 U1.4: the refusal a failed start of the server in `dir`
/// stands for (its exit code), if it is one.
pub fn refusal_for(dir: &Path, error: &str) -> Option<upgrade::Refusal> {
    match upgrade::check_dir(dir) {
        Err((r, _)) => Some(r),
        Ok(()) => upgrade::Refusal::of_error(error),
    }
}

pub(crate) struct State {
    pub store: Mutex<Store>,
    pub limits: Mutex<limits::Limits>,
    pub verifier: Option<Verifier>,
    pub config: Config,
    pub log: Log,
    pub clock: Arc<Clock>,
    pub last_purge: AtomicI64,
    pub phone_app: Option<PhoneApp>,
    /// The latest Access refusal, until a token passes again (revision 8).
    pub access_problem: Mutex<Option<mk_capture_api::AccessProblem>>,
    /// Wi-Fi capture, when on (revision 10).
    pub wifi: Option<WifiRuntime>,
    /// U1.2: Wi-Fi capture is set up but couldn't start (health's `wifi_error`).
    pub wifi_error: Option<String>,
    /// L8: whether this computer has an address now.
    pub address_present: AddressSource,
    /// Phones' addresses behind the Wi-Fi relay, by its loopback port.
    pub peers: wifi::Peers,
    /// Unauthenticated Wi-Fi requests handled now (security-review-1 A4).
    pub gate: Arc<wifi::Gate>,
    pub pin_cost: pin::Cost,
}

/// Wi-Fi capture while the server runs.
pub(crate) struct WifiRuntime {
    pub listen: config::WifiListen,
    pub info: std::sync::RwLock<wifi::Info>,
    pub ca_der: std::sync::RwLock<Vec<u8>>,
    pub listening: AtomicBool,
    pub resolver: Arc<wifi::Resolver>,
    pub secrets: secrets::Secrets,
    pub dir: PathBuf,
}

/// A running server; stops when dropped.
pub struct Running {
    addr: SocketAddr,
    server: Arc<tiny_http::Server>,
    /// The Wi-Fi listener's request handler (on a loopback port) and the helper.
    extra: Arc<Mutex<Vec<Arc<tiny_http::Server>>>>,
    stopping: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    state: Arc<State>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running").field("addr", &self.addr).finish()
    }
}

impl Running {
    /// The address actually listened on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Wait until the server stops (the `run` command).
    pub fn wait(mut self) {
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
    }

    pub fn stop(self) {
        drop(self);
    }

    /// The Wi-Fi HTTPS address, when listening.
    pub fn wifi_addr(&self) -> Option<SocketAddr> {
        let w = self.state.wifi.as_ref()?;
        w.listening
            .load(Ordering::SeqCst)
            .then(|| SocketAddr::from((w.listen.target.ip, w.listen.https_port)))
    }

    /// The Wi-Fi helper's address, when listening.
    pub fn helper_addr(&self) -> Option<SocketAddr> {
        let w = self.state.wifi.as_ref()?;
        w.listening
            .load(Ordering::SeqCst)
            .then(|| SocketAddr::from((w.listen.target.ip, w.listen.http_port)))
    }

    /// Renew the server certificate now if it is due (the server also checks every
    /// minute); true when a new one was made.
    pub fn renew_if_due(&self) -> Result<bool, String> {
        renew(&self.state)
    }

    /// The Wi-Fi certificate facts in use.
    pub fn wifi_info(&self) -> Option<wifi::Info> {
        self.state
            .wifi
            .as_ref()
            .and_then(|w| w.info.read().ok().map(|i| i.clone()))
    }
}

/// Renew the Wi-Fi server certificate when due; the TLS listener uses the new one
/// for new connections at once.
fn renew(state: &State) -> Result<bool, String> {
    let Some(w) = &state.wifi else {
        return Ok(false);
    };
    let now = state.clock.now();
    let due = w
        .info
        .read()
        .map(|i| now >= i.cert_not_after - wifi::RENEW_DAYS * 86_400)
        .unwrap_or(false);
    if !due {
        return Ok(false);
    }
    // U1.2: only the server certificate is renewed; the authority stays.
    let m = wifi::ensure_existing(&w.dir, &w.secrets, &w.listen.target, now)?;
    w.resolver.set(&m)?;
    if let Ok(mut i) = w.info.write() {
        *i = m.info.clone();
    }
    if let Ok(mut d) = w.ca_der.write() {
        d.clone_from(&m.ca_der);
    }
    (state.log)(&format!(
        "wifi: new server certificate, valid until {}",
        m.info.cert_not_after
    ));
    Ok(m.renewed)
}

/// Hand a Wi-Fi request to its lane (security-review-1 A4).
fn dispatch_wifi(
    state: &Arc<State>,
    req: tiny_http::Request,
    public: &std::sync::mpsc::SyncSender<tiny_http::Request>,
) {
    let lane = api::wifi_lane(state, &req);
    if lane != api::WifiLane::Unauthenticated
        && let Some(port) = req.remote_addr().map(|a| a.port())
    {
        // The connection carries a request now: no deadline (the idle timeout
        // still applies).
        wifi::set_deadline(&state.peers, port, None);
    }
    match lane {
        api::WifiLane::Paired => api::handle(state, req, api::Via::Wifi),
        api::WifiLane::Public => {
            if let Err(std::sync::mpsc::TrySendError::Full(req)) = public.try_send(req) {
                // Too many at once: close it rather than wait.
                if let Some(port) = req.remote_addr().map(|a| a.port()) {
                    wifi::close_peer(&state.peers, port);
                }
            }
        }
        api::WifiLane::Unauthenticated => {
            let port = req.remote_addr().map(|a| a.port());
            let ip = port.and_then(|p| wifi::peer_ip(&state.peers, p));
            let ticket = ip.and_then(|ip| state.gate.admit(ip));
            let (Some(port), Some(ticket)) = (port, ticket) else {
                (state.log)("wifi: too many unauthenticated requests; connection closed");
                if let Some(port) = port {
                    wifi::close_peer(&state.peers, port);
                }
                return;
            };
            wifi::set_deadline(
                &state.peers,
                port,
                Some(std::time::Instant::now() + wifi::UNAUTH_BODY_TIMEOUT),
            );
            let state2 = state.clone();
            let spawned = std::thread::Builder::new()
                .name("capture-wifi-unauth".into())
                .spawn(move || {
                    api::handle(&state2, req, api::Via::Wifi);
                    wifi::set_deadline(&state2.peers, port, None);
                    drop(ticket);
                });
            if spawned.is_err() {
                wifi::close_peer(&state.peers, port);
            }
        }
    }
}

/// The Wi-Fi listeners while bound: their threads, and the flag that stops them when
/// the address goes away (L8).
struct WifiBound {
    threads: Vec<JoinHandle<()>>,
    gone: Arc<AtomicBool>,
}

/// Bind the Wi-Fi listeners and start their threads; `None` when the address is not
/// on this computer or a port is taken (tried again while the server runs). Errors
/// are logged when `log` is set (the first try after a change).
fn listen_wifi(
    state: &Arc<State>,
    backend: SocketAddr,
    stopping: &Arc<AtomicBool>,
    log: bool,
) -> Option<WifiBound> {
    let w = state.wifi.as_ref()?;
    let ip = w.listen.target.ip;
    let say = |port: u16, e: &dyn std::fmt::Display| {
        if log {
            (state.log)(&format!("wifi: cannot listen on {ip}:{port}: {e}"));
        }
    };
    let tls = match std::net::TcpListener::bind((ip, w.listen.https_port)) {
        Ok(l) => l,
        Err(e) => {
            say(w.listen.https_port, &e);
            return None;
        }
    };
    let helper = match tiny_http::Server::http((ip, w.listen.http_port)) {
        Ok(h) => Arc::new(h),
        Err(e) => {
            say(w.listen.http_port, &e);
            return None;
        }
    };
    let config = wifi::server_config(w.resolver.clone()).ok()?;
    let gone = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    {
        let (peers, stopping, gone) = (state.peers.clone(), stopping.clone(), gone.clone());
        threads.push(
            std::thread::Builder::new()
                .name("capture-wifi-accept".into())
                .spawn(move || wifi::accept_loop(tls, config, backend, peers, stopping, gone))
                .ok()?,
        );
    }
    {
        let (state2, stopping, gone) = (state.clone(), stopping.clone(), gone.clone());
        threads.push(
            std::thread::Builder::new()
                .name("capture-wifi-helper".into())
                .spawn(move || {
                    while !stopping.load(Ordering::SeqCst) && !gone.load(Ordering::SeqCst) {
                        match helper.recv_timeout(Duration::from_millis(250)) {
                            Ok(Some(req)) => api::helper(&state2, req),
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                })
                .ok()?,
        );
    }
    w.listening.store(true, Ordering::SeqCst);
    (state.log)(&format!(
        "wifi: listening on https://{}:{} (certificate page http://{}:{}/)",
        w.listen.target.host(),
        w.listen.https_port,
        w.listen.target.host(),
        w.listen.http_port
    ));
    Some(WifiBound { threads, gone })
}

/// L8: stop the Wi-Fi listeners (the address went away), freeing their ports.
fn unlisten_wifi(state: &State, bound: WifiBound) {
    bound.gone.store(true, Ordering::SeqCst);
    for t in bound.threads {
        let _ = t.join();
    }
    if let Some(w) = &state.wifi {
        w.listening.store(false, Ordering::SeqCst);
    }
}

/// U1.5: bind the loopback listener, trying again for up to `wait` (the port may
/// still be held by the server that just stopped, or by another program for a
/// moment).
fn bind_loopback(addr: SocketAddr, wait: Duration, log: &Log) -> Result<tiny_http::Server, String> {
    let started = std::time::Instant::now();
    let mut said = false;
    loop {
        match tiny_http::Server::http(addr) {
            Ok(s) => return Ok(s),
            Err(e) if started.elapsed() >= wait => {
                return Err(format!("cannot listen on {addr}: {e}"));
            }
            Err(e) => {
                if !said {
                    log(&format!("cannot listen on {addr} yet ({e}); trying again"));
                    said = true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        for _ in 0..self.threads.len() {
            self.server.unblock();
            if let Ok(x) = self.extra.lock() {
                for s in x.iter() {
                    s.unblock();
                }
            }
        }
        for t in std::mem::take(&mut self.threads) {
            let _ = t.join();
        }
    }
}

/// Check the settings, open the inbox and start listening.
pub fn start(opts: Options) -> Result<Running, String> {
    let addr = opts.config.check(opts.allow_lan)?;
    if !addr.ip().is_loopback() {
        (opts.log)(&format!(
            "warning: listening on {addr}, which other computers on the network can reach; \
             Access tokens and bearer tokens are still required"
        ));
    }
    if !opts.config.access_required {
        if opts.config.wifi_address.trim().is_empty() {
            (opts.log)(
                "warning: access_required = false - Cloudflare Access tokens are not checked (local testing only)",
            );
        } else {
            (opts.log)(
                "Cloudflare is not set up: the loopback listener serves this computer only; phones use Wi-Fi capture",
            );
        }
    }
    // upgrade-resilience-1 U1.4: a newer folder or inbox, or settings this version
    // can't read, are refused before anything is opened.
    upgrade::check_dir(&opts.data_dir).map_err(|(_, e)| e)?;
    let store = Store::open(&opts.data_dir, opts.key_home)?;
    // U1.2: the certificate authority is never replaced here. Damaged certificate
    // files leave the server running without the Wi-Fi listener, saying why.
    let mut wifi_error = None;
    let wifi_rt = match opts.config.wifi()? {
        Some(listen) => {
            let made =
                secrets::Secrets::open(&opts.data_dir, opts.key_home, opts.credentials.clone())
                    .and_then(|secrets| {
                        let m = wifi::ensure_existing(
                            &opts.data_dir,
                            &secrets,
                            &listen.target,
                            opts.clock.now(),
                        )?;
                        Ok(WifiRuntime {
                            resolver: Arc::new(wifi::Resolver::new(&m)?),
                            info: std::sync::RwLock::new(m.info.clone()),
                            ca_der: std::sync::RwLock::new(m.ca_der.clone()),
                            listening: AtomicBool::new(false),
                            listen,
                            secrets,
                            dir: opts.data_dir.clone(),
                        })
                    });
            match made {
                Ok(rt) => Some(rt),
                Err(e) => {
                    (opts.log)(&format!("wifi: not started: {e}"));
                    wifi_error = Some(if e.starts_with(wifi::DAMAGED) {
                        wifi::DAMAGED.to_owned()
                    } else {
                        format!("{}: {e}", wifi::DAMAGED)
                    });
                    None
                }
            }
        }
        None => None,
    };
    let verifier = if opts.config.access_required {
        let team = opts.config.team_domain();
        let source: Arc<dyn CertsSource> = match opts.certs {
            Some(s) => s,
            None => Arc::new(HttpCerts::for_team(&team)),
        };
        Some(Verifier::new(&team, &opts.config.access_aud, source))
    } else {
        None
    };
    let server = Arc::new(bind_loopback(addr, opts.bind_wait, &opts.log)?);
    let actual = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| "the server has no IP address".to_owned())?;
    let state = Arc::new(State {
        store: Mutex::new(store),
        limits: Mutex::new(limits::Limits::default()),
        verifier,
        config: opts.config,
        log: opts.log,
        clock: opts.clock,
        last_purge: AtomicI64::new(0),
        phone_app: opts.phone_app,
        access_problem: Mutex::new(None),
        wifi: wifi_rt,
        wifi_error,
        address_present: opts
            .address_present
            .unwrap_or_else(|| Arc::new(wifi::address_present)),
        peers: Arc::new(Mutex::new(std::collections::HashMap::new())),
        gate: wifi::Gate::new(),
        pin_cost: opts.pin_cost,
    });
    let stopping = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    let extra: Arc<Mutex<Vec<Arc<tiny_http::Server>>>> = Arc::new(Mutex::new(Vec::new()));
    if state.wifi.is_some() {
        // The Wi-Fi listener's requests are handled like the loopback ones, on a
        // private loopback port that only the TLS relay connects to.
        let inner = Arc::new(
            tiny_http::Server::http("127.0.0.1:0")
                .map_err(|e| format!("cannot start the Wi-Fi request handler: {e}"))?,
        );
        let backend = inner
            .server_addr()
            .to_ip()
            .ok_or_else(|| "the Wi-Fi request handler has no address".to_owned())?;
        if let Ok(mut x) = extra.lock() {
            x.push(inner.clone());
        }
        // security-review-1 A4: the Wi-Fi workers serve paired phones; the phone
        // app's files go to their own workers, and unauthenticated requests
        // (pairing) each get a worker of their own within the gate's limits, so
        // neither can keep a paired phone waiting.
        let (public_tx, public_rx) = std::sync::mpsc::sync_channel::<tiny_http::Request>(64);
        let public_rx = Arc::new(Mutex::new(public_rx));
        for i in 0..2 {
            let (rx, state, stopping) = (public_rx.clone(), state.clone(), stopping.clone());
            threads.push(
                std::thread::Builder::new()
                    .name(format!("capture-wifi-public-{i}"))
                    .spawn(move || {
                        while !stopping.load(Ordering::SeqCst) {
                            let next = match rx.lock() {
                                Ok(r) => r.recv_timeout(Duration::from_millis(500)),
                                Err(_) => break,
                            };
                            match next {
                                Ok(req) => api::handle(&state, req, api::Via::Wifi),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(_) => break,
                            }
                        }
                    })
                    .map_err(|e| format!("cannot start a worker thread: {e}"))?,
            );
        }
        for i in 0..2 {
            let (inner, state, stopping) = (inner.clone(), state.clone(), stopping.clone());
            let public_tx = public_tx.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("capture-wifi-{i}"))
                    .spawn(move || {
                        while !stopping.load(Ordering::SeqCst) {
                            match inner.recv_timeout(Duration::from_millis(500)) {
                                Ok(Some(req)) => dispatch_wifi(&state, req, &public_tx),
                                Ok(None) => {}
                                Err(_) => break,
                            }
                        }
                    })
                    .map_err(|e| format!("cannot start a worker thread: {e}"))?,
            );
        }
        let first = listen_wifi(&state, backend, &stopping, true);
        // Every few seconds (L8): the address gone (sleep, Wi-Fi dropped) stops the
        // listener; back, it listens again. Every minute: renew the certificate
        // when due.
        let (state2, stopping2) = (state.clone(), stopping.clone());
        let recheck = opts.wifi_recheck.max(Duration::from_millis(50));
        threads.push(
            std::thread::Builder::new()
                .name("capture-wifi-watch".into())
                .spawn(move || {
                    let mut bound = first;
                    let mut log_next = bound.is_some();
                    let tick = Duration::from_millis(50);
                    let (mut since_check, mut since_renew, mut since_sweep) =
                        (Duration::ZERO, Duration::ZERO, Duration::ZERO);
                    let ip = state2.wifi.as_ref().map(|w| w.listen.target.ip);
                    while !stopping2.load(Ordering::SeqCst) {
                        std::thread::sleep(tick);
                        since_check += tick;
                        since_renew += tick;
                        since_sweep += tick;
                        // A4: unauthenticated requests whose body is late.
                        if since_sweep >= Duration::from_millis(250) {
                            since_sweep = Duration::ZERO;
                            if wifi::sweep(&state2.peers) > 0 {
                                (state2.log)("wifi: closed a connection whose request body did not arrive in time");
                            }
                        }
                        if since_renew >= Duration::from_secs(60) {
                            since_renew = Duration::ZERO;
                            if let Err(e) = renew(&state2) {
                                (state2.log)(&format!("wifi: renewing the certificate failed: {e}"));
                            }
                        }
                        if since_check < recheck {
                            continue;
                        }
                        since_check = Duration::ZERO;
                        let Some(ip) = ip else { break };
                        let present = (state2.address_present)(ip);
                        match bound.take() {
                            Some(b) if !present => {
                                unlisten_wifi(&state2, b);
                                (state2.log)(&format!(
                                    "wifi: {ip} is gone from this computer; listening again when it is back"
                                ));
                                log_next = true;
                            }
                            Some(b) => bound = Some(b),
                            None if present => {
                                bound = listen_wifi(&state2, backend, &stopping2, log_next);
                                log_next = bound.is_some();
                            }
                            None => {}
                        }
                    }
                    if let Some(b) = bound {
                        unlisten_wifi(&state2, b);
                    }
                })
                .map_err(|e| format!("cannot start a worker thread: {e}"))?,
        );
    }
    for i in 0..opts.threads.max(1) {
        let server = server.clone();
        let state = state.clone();
        let stopping = stopping.clone();
        let t = std::thread::Builder::new()
            .name(format!("capture-{i}"))
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match server.recv_timeout(Duration::from_millis(500)) {
                        Ok(Some(req)) => api::handle(&state, req, api::Via::Loopback),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("cannot start a worker thread: {e}"))?;
        threads.push(t);
    }
    (state.log)(&format!("listening on http://{actual}"));
    Ok(Running {
        addr: actual,
        server,
        extra,
        stopping,
        threads,
        state,
    })
}

/// `init`: create the data folder, `server.toml`, the inbox and a desktop token.
/// Returns the desktop token (shown once).
pub fn init(dir: &Path, key_home: KeyHome, now: i64) -> Result<zeroize::Zeroizing<String>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let cfg = dir.join(config::CONFIG_FILE);
    if !cfg.exists() {
        store::write_private(&cfg, config::template().as_bytes())?;
    }
    upgrade::write_format(dir)?;
    let s = Store::open(dir, key_home)?;
    if s.has_desktop_token()? {
        return Err(
            "this data folder already has a desktop token; use `rotate-desktop-token` for a new one"
                .into(),
        );
    }
    let token = store::new_token()?;
    s.set_desktop_token(&store::token_hash(&token), now)?;
    Ok(token)
}

/// `rotate-desktop-token`: a new desktop token (the old one stops working).
pub fn rotate_desktop_token(
    dir: &Path,
    key_home: KeyHome,
    now: i64,
) -> Result<zeroize::Zeroizing<String>, String> {
    let s = Store::open(dir, key_home)?;
    let token = store::new_token()?;
    s.set_desktop_token(&store::token_hash(&token), now)?;
    Ok(token)
}

/// `status`: a summary in words.
pub fn status(dir: &Path, key_home: KeyHome) -> Result<String, String> {
    let opts = Options::for_dir(dir, true)?;
    let s = Store::open(dir, key_home)?;
    let (phones, waiting, bytes) = s.counts()?;
    Ok(format!(
        "data folder: {}\nlisten: {}\naccess checks: {}\npaired phones: {phones}\n\
         waiting submissions: {waiting}\ninbox: {} of {} MB\ninbox key: {}",
        dir.display(),
        opts.config.listen,
        if opts.config.access_required {
            "on"
        } else {
            "off (local testing)"
        },
        bytes / (1024 * 1024),
        opts.config.inbox_limit_mb,
        store::key_location(dir),
    ))
}
