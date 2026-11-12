//! Layering check: which workspace crates may depend on which.
//!
//! Every workspace crate must appear in `RULES`. Adding a crate without a rule fails
//! the check, so the layering is always a deliberate decision.

use std::collections::BTreeMap;
use std::process::Command;

use serde_json::Value;

/// (crate, workspace crates it may depend on)
const RULES: &[(&str, &[&str])] = &[
    ("mk-core", &[]),
    // upgrade-resilience-1 U1.6: the inbox key sealed with Windows DPAPI; the
    // workspace's only unsafe code (two calls into crypt32.dll), nothing else.
    ("mk-dpapi", &[]),
    // Phone capture API types, limits and validation: pure, no IO, builds for wasm32.
    ("mk-capture-api", &[]),
    // Pure ledger rules (validation, balance arithmetic): no storage.
    ("mk-ledger", &["mk-core"]),
    // Pure import logic: parsers take bytes, no file system or storage.
    ("mk-import", &["mk-core"]),
    ("mk-store", &["mk-core"]),
    (
        "mk-commands",
        &[
            "mk-capture-api",
            "mk-core",
            "mk-import",
            "mk-ledger",
            "mk-store",
        ],
    ),
    // Connectivity: the only network-capable crate (connectivity.md); implements the
    // registry's fetcher and credential-store interfaces.
    ("mk-connect", &["mk-commands", "mk-core"]),
    // The MCP server library calls the registry; it opens a store but never runs SQL.
    // Only keep-mcp uses it (mcp-server.md revision 3).
    ("mk-mcp", &["mk-commands", "mk-store"]),
    // The local control channel (mcp-server.md revision 3): loopback TCP with std,
    // the discovery file; shared by the desktop app and keep-mcp. No UI.
    ("mk-control", &["mk-commands", "mk-core"]),
    // Watched-folder loop: file IO plus registry commands; no UI.
    ("mk-watch", &["mk-commands", "mk-store"]),
    // The UI controller (ui-slint.md revision 2): app state and view models over
    // registry commands; no toolkit, no storage, no file IO.
    ("mk-ui", &["mk-capture-api", "mk-core"]),
    // The views (ui-slint.md revision 2): .slint files, icons and the glue that
    // shows mk-ui's view models; no business logic.
    ("mk-views", &["mk-core", "mk-ui"]),
    // Desktop app: window, dialogs, config, worker thread.
    (
        "moneykeep-app",
        &[
            "mk-core",
            "mk-commands",
            "mk-connect",
            "mk-control",
            "mk-ui",
            "mk-views",
            "mk-watch",
        ],
    ),
    (
        "moneykeep-cli",
        &[
            "mk-core",
            "mk-store",
            "mk-commands",
            "mk-connect",
            "mk-watch",
        ],
    ),
    // The optional MCP server program (mcp-server.md revision 3).
    (
        "keep-mcp",
        &[
            "mk-commands",
            "mk-connect",
            "mk-control",
            "mk-mcp",
            "mk-store",
        ],
    ),
    // Phone capture server (mobile-capture.md): network IO allowed; never opens a
    // book (no mk-store, no mk-commands); reuses mk-import's receipt picture code and
    // mk-core's product names (old and new, for the migration in branding.md step 6).
    (
        "moneykeep-server",
        &["mk-capture-api", "mk-core", "mk-dpapi", "mk-import"],
    ),
    // Phone capture web client (wasm32): the shared API crate, mk-ui's page builder
    // and mk-views' phone window (ui-slint.md revision 2).
    (
        "moneykeep-capture",
        &["mk-capture-api", "mk-ui", "mk-views"],
    ),
    ("xtask", &[]),
];

/// Crates whose normal dependencies are limited to this list (pure crates that must
/// build for wasm32 and do no IO).
const PURE: &[(&str, &[&str])] = &[("mk-capture-api", &["serde"])];

/// Build-script dependencies a pure crate may have (they run on the build machine
/// only and are never linked into the crate): mk-capture-api's capture version
/// (release-packaging.md) hashes the sources found through `cargo metadata`.
const PURE_BUILD: &[(&str, &[&str])] = &[("mk-capture-api", &["serde_json", "sha2"])];

/// External crates restricted to specific workspace crates.
const RESTRICTED_EXTERNAL: &[(&str, &[&str])] = &[
    // Only the storage layer talks to SQLite (the capture server's inbox is its own
    // file, never a book).
    ("rusqlite", &["mk-store", "moneykeep-server"]),
    // Network and credential-store crates live in mk-connect and the capture server.
    ("ureq", &["mk-connect", "moneykeep-server"]),
    ("rustls", &["mk-connect", "moneykeep-server"]),
    ("rustls-native-certs", &["mk-connect", "moneykeep-server"]),
    ("keyring", &["mk-connect", "moneykeep-server"]),
    // The HTTP server crate: the capture server only.
    ("tiny_http", &["moneykeep-server"]),
    // Windows service control (mobile-capture.md revision 2): the capture server only.
    ("windows-service", &["moneykeep-server"]),
    // The Windows API itself: only the DPAPI crate (upgrade-resilience-1 U1.6).
    ("windows-sys", &["mk-dpapi"]),
    // Browser bindings: the phone client only.
    ("web-sys", &["moneykeep-capture"]),
    ("wasm-bindgen", &["moneykeep-capture"]),
    // The UI toolkit (Slint, Royalty-free License 2.0): the views in mk-views; the
    // desktop shell reaches the window (winit) and the system clipboard itself.
    ("slint", &["mk-views", "moneykeep-app"]),
    ("slint-build", &["mk-views"]),
    ("i-slint-backend-testing", &["mk-views"]),
    ("arboard", &["moneykeep-app"]),
];

/// Crates the import and definition code may not reach, directly or transitively
/// (input-safety.md R1): scripting and template engines, dynamic loading, process
/// spawning and HTTP clients.
const DENIED: &[&str] = &[
    // Scripting, expression and template engines.
    "rhai",
    "mlua",
    "rlua",
    "hlua",
    "pyo3",
    "rustpython",
    "rustpython-vm",
    "boa_engine",
    "deno_core",
    "v8",
    "rusty_v8",
    "rquickjs",
    "quickjs-rs",
    "js-sandbox",
    "wasmtime",
    "wasmer",
    "wasmi",
    "gluon",
    "dyon",
    "ketos",
    "rune",
    "steel-core",
    "starlark",
    "koto",
    "evalexpr",
    "tera",
    "handlebars",
    "minijinja",
    "liquid",
    // Dynamic library loading.
    "libloading",
    "dlopen",
    "dlopen2",
    "dlib",
    "sharedlib",
    "abi_stable",
    // Process spawning.
    "duct",
    "subprocess",
    "cmd_lib",
    "xshell",
    "async-process",
    "tokio-process",
    "shell-escape",
    "run_script",
    // HTTP and network clients.
    "reqwest",
    "ureq",
    "hyper",
    "curl",
    "isahc",
    "attohttpc",
    "surf",
    "http-client",
    "minreq",
    "ehttp",
    "awc",
    "tungstenite",
    "native-tls",
    "rustls",
    // HTTP servers (mobile-capture.md: only the capture server listens).
    "tiny_http",
];

/// Workspace crates that must stay network-free (input-safety.md R1, connectivity.md):
/// only `mk-connect` and the apps may reach an HTTP client.
const GUARDED: &[&str] = &[
    "mk-capture-api",
    "mk-core",
    "mk-ledger",
    "mk-store",
    "mk-import",
    "mk-commands",
    "mk-ui",
    "mk-mcp",
    "mk-control",
    "mk-watch",
];

/// Programs that must not link the MCP server library, directly or transitively
/// (mcp-server.md revision 3: only keep-mcp does).
const MCP_FREE: &[&str] = &["moneykeep-app", "moneykeep-cli"];

/// The path from `root` to `mk-mcp` through normal dependencies, if there is one.
pub fn mcp_path(graph: &BTreeMap<String, Vec<String>>, root: &str) -> Option<String> {
    let mut seen: Vec<String> = vec![root.to_owned()];
    let mut queue: Vec<Vec<String>> = vec![vec![root.to_owned()]];
    while let Some(path) = queue.pop() {
        let last = path.last()?.clone();
        for dep in graph.get(&last).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.contains(dep) {
                continue;
            }
            seen.push(dep.clone());
            let mut p = path.clone();
            p.push(dep.clone());
            if dep == "mk-mcp" {
                return Some(p.join(" -> "));
            }
            queue.push(p);
        }
    }
    None
}

/// Source files that must not use the standard library's file, process or network
/// APIs (the parsers and the definitions module).
const GUARDED_SOURCES: &[&str] = &[
    "crates/mk-capture-api/src",
    "crates/mk-import/src",
    "crates/mk-commands/src/definitions.rs",
];
const DENIED_APIS: &[&str] = &[
    "std::fs",
    "std::process",
    "std::net",
    "File::open",
    "Command::new",
    "TcpStream",
];

/// Denied crates reachable from `root` through normal dependencies, each with the
/// path that reaches it. `graph` maps a crate to its normal dependencies.
pub fn denied_reachable(graph: &BTreeMap<String, Vec<String>>, root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = vec![root.to_owned()];
    let mut queue: Vec<Vec<String>> = vec![vec![root.to_owned()]];
    while let Some(path) = queue.pop() {
        let Some(last) = path.last() else { continue };
        for dep in graph.get(last).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.contains(dep) {
                continue;
            }
            seen.push(dep.clone());
            let mut p = path.clone();
            p.push(dep.clone());
            if DENIED.contains(&dep.as_str()) {
                out.push(format!(
                    "{root} must not depend on {dep} ({})",
                    p.join(" -> ")
                ));
            }
            queue.push(p);
        }
    }
    out
}

/// Lines of `text` using a denied standard-library API.
pub fn denied_api_uses(file: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .flat_map(|(i, l)| {
            DENIED_APIS
                .iter()
                .filter(move |api| l.contains(**api))
                .map(move |api| format!("{file}:{}: uses {api}", i + 1))
        })
        .collect()
}

fn source_files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
    } else if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            source_files(&e.path(), out);
        }
    }
}

/// The denied-crate and denied-API checks over the real dependency graph.
fn check_denied(root_dir: &std::path::Path) -> Result<Vec<String>, String> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(root_dir)
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let meta: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad cargo metadata: {e}"))?;
    let names: BTreeMap<String, String> = field(&meta, "packages")
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            Some((
                field(p, "id").as_str()?.to_owned(),
                field(p, "name").as_str()?.to_owned(),
            ))
        })
        .collect();
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in field(field(&meta, "resolve"), "nodes")
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(name) = field(node, "id").as_str().and_then(|id| names.get(id)) else {
            continue;
        };
        let deps = field(node, "deps")
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|d| {
                field(d, "dep_kinds")
                    .as_array()
                    .is_some_and(|ks| ks.iter().any(|k| field(k, "kind").is_null()))
            })
            .filter_map(|d| {
                field(d, "pkg")
                    .as_str()
                    .and_then(|id| names.get(id))
                    .cloned()
            })
            .collect();
        graph.insert(name.clone(), deps);
    }
    let mut errors = Vec::new();
    for root in GUARDED {
        errors.extend(denied_reachable(&graph, root));
    }
    for app in MCP_FREE {
        if let Some(p) = mcp_path(&graph, app) {
            errors.push(format!("{app} must not depend on mk-mcp ({p})"));
        }
    }
    for src in GUARDED_SOURCES {
        let mut files = Vec::new();
        source_files(&root_dir.join(src), &mut files);
        for f in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            errors.extend(denied_api_uses(&f.display().to_string(), &text));
        }
    }
    Ok(errors)
}

pub fn check() -> Result<(), String> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let meta: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad cargo metadata: {e}"))?;

    let packages = field(&meta, "packages")
        .as_array()
        .ok_or("cargo metadata has no packages")?;
    let workspace: Vec<&str> = packages
        .iter()
        .filter_map(|p| field(p, "name").as_str())
        .collect();
    let rules: BTreeMap<&str, &[&str]> = RULES.iter().copied().collect();

    let mut errors = Vec::new();
    for pkg in packages {
        let name = field(pkg, "name").as_str().unwrap_or("?");
        let Some(allowed) = rules.get(name) else {
            errors.push(format!(
                "{name}: no layering rule; add it to xtask/src/layering.rs RULES"
            ));
            continue;
        };
        let deps = field(pkg, "dependencies")
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let pure = PURE.iter().find(|(n, _)| *n == name).map(|(_, a)| *a);
        for dep in deps {
            let dep_name = field(dep, "name").as_str().unwrap_or("?");
            let is_dev = field(dep, "kind").as_str() == Some("dev");
            let is_build = field(dep, "kind").as_str() == Some("build");
            let build_ok = PURE_BUILD
                .iter()
                .any(|(n, a)| *n == name && a.contains(&dep_name));
            if is_build && pure.is_some() && !build_ok {
                errors.push(format!(
                    "{name} must stay pure: {dep_name} is not an allowed build dependency"
                ));
            }
            if let Some(allowed) = pure
                && !is_dev
                && !is_build
                && !allowed.contains(&dep_name)
            {
                errors.push(format!(
                    "{name} must stay pure: it may depend only on {} (not {dep_name})",
                    allowed.join(", ")
                ));
            }
            if workspace.contains(&dep_name) && !is_dev && !allowed.contains(&dep_name) {
                errors.push(format!("{name} must not depend on {dep_name}"));
            }
            for (ext, owners) in RESTRICTED_EXTERNAL {
                if dep_name == *ext && !owners.contains(&name) {
                    errors.push(format!(
                        "{name} must not depend on {ext} (allowed only in {})",
                        owners.join(", ")
                    ));
                }
            }
        }
    }

    let root_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."));
    errors.extend(check_denied(root_dir)?);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

static NULL: Value = Value::Null;

/// `v[key]` without indexing (missing keys read as null).
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&NULL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn denied_crate_reached_through_a_dependency_fails() {
        let ok = graph(&[("mk-import", &["regex", "serde"]), ("regex", &["memchr"])]);
        assert!(denied_reachable(&ok, "mk-import").is_empty());
        let bad = graph(&[
            ("mk-import", &["regex", "helper"]),
            ("helper", &["reqwest"]),
            ("reqwest", &["hyper"]),
        ]);
        let errs = denied_reachable(&bad, "mk-import");
        assert!(
            errs.iter()
                .any(|e| e.contains("reqwest") && e.contains("mk-import -> helper -> reqwest")),
            "{errs:?}"
        );
        let direct = graph(&[("mk-import", &["libloading"])]);
        assert_eq!(denied_reachable(&direct, "mk-import").len(), 1);
    }

    /// connectivity.md AT12: an HTTP dependency of mk-commands fails the check.
    #[test]
    fn http_client_in_commands_fails() {
        let bad = graph(&[
            ("mk-commands", &["mk-store", "ureq"]),
            ("ureq", &["rustls"]),
        ]);
        let errs = denied_reachable(&bad, "mk-commands");
        assert!(errs.iter().any(|e| e.contains("ureq")), "{errs:?}");
        let via_connect = graph(&[("mk-commands", &["mk-connect"]), ("mk-connect", &["ureq"])]);
        assert!(!denied_reachable(&via_connect, "mk-commands").is_empty());
        let ok = graph(&[("mk-connect", &["ureq"])]);
        assert!(GUARDED.iter().all(|g| denied_reachable(&ok, g).is_empty()));
        assert!(!GUARDED.contains(&"mk-connect"));
    }

    /// mobile-capture.md: the capture server may listen and fetch; the pure API crate
    /// and the network-free crates may not reach an HTTP server or client.
    #[test]
    fn capture_crates_layering() {
        assert!(GUARDED.contains(&"mk-capture-api"));
        assert!(!GUARDED.contains(&"moneykeep-server"));
        let bad = graph(&[("mk-capture-api", &["tiny_http"])]);
        assert!(!denied_reachable(&bad, "mk-capture-api").is_empty());
        let via = graph(&[("mk-commands", &["helper"]), ("helper", &["tiny_http"])]);
        assert!(!denied_reachable(&via, "mk-commands").is_empty());
        assert!(
            RESTRICTED_EXTERNAL
                .iter()
                .any(|(c, owners)| *c == "tiny_http" && owners == &["moneykeep-server"])
        );
        assert_eq!(PURE, &[("mk-capture-api", &["serde"][..])]);
        assert_eq!(
            PURE_BUILD,
            &[("mk-capture-api", &["serde_json", "sha2"][..])]
        );
        assert!(GUARDED_SOURCES.contains(&"crates/mk-capture-api/src"));
    }

    /// mcp-server.md revision 3 AT29: the desktop app and the CLI do not depend on
    /// mk-mcp, directly or through another crate; keep-mcp does.
    #[test]
    fn at29_app_and_cli_do_not_depend_on_mk_mcp() {
        for app in MCP_FREE {
            let allowed = RULES.iter().find(|(n, _)| n == app).map(|(_, a)| *a);
            assert!(allowed.is_some_and(|a| !a.contains(&"mk-mcp")), "{app}");
        }
        assert!(MCP_FREE.contains(&"moneykeep-app") && MCP_FREE.contains(&"moneykeep-cli"));
        let keep = RULES
            .iter()
            .find(|(n, _)| *n == "keep-mcp")
            .map(|(_, a)| *a);
        assert!(keep.is_some_and(|a| a.contains(&"mk-mcp")));
        // No rule lets a crate the app or CLI may use reach mk-mcp either.
        for (name, deps) in RULES {
            if *name != "keep-mcp" && *name != "mk-mcp" {
                assert!(!deps.contains(&"mk-mcp"), "{name} may depend on mk-mcp");
            }
        }
        let via = graph(&[("moneykeep-app", &["helper"]), ("helper", &["mk-mcp"])]);
        assert_eq!(
            mcp_path(&via, "moneykeep-app").as_deref(),
            Some("moneykeep-app -> helper -> mk-mcp")
        );
        let ok = graph(&[
            ("moneykeep-cli", &["mk-commands"]),
            ("keep-mcp", &["mk-mcp"]),
        ]);
        assert!(mcp_path(&ok, "moneykeep-cli").is_none());
        // The manifests themselves.
        for manifest in [
            include_str!("../../apps/moneykeep-app/Cargo.toml"),
            include_str!("../../apps/moneykeep-cli/Cargo.toml"),
        ] {
            assert!(!manifest.contains("mk-mcp"), "{manifest}");
        }
    }

    #[test]
    fn denied_api_in_source_fails() {
        assert!(denied_api_uses("a.rs", "use std::fs::read;").len() == 1);
        assert!(denied_api_uses("a.rs", "// std::process is not used").is_empty());
        assert!(denied_api_uses("a.rs", "let x = 1;").is_empty());
    }
}
