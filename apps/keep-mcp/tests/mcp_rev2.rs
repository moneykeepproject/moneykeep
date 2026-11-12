//! mcp-server.md revision 2 acceptance tests AT-1..AT-15, run against keep-mcp's
//! direct route (revision 3) on a plain test book with access on, read and write,
//! all groups and `app_only = false` (AT5's read-only case uses
//! `ai_access.level = "read_only"`). Driven through `Server::handle_line` and
//! `Server::serve`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;

use keep_mcp::{Router, Target};
use mk_commands::ai_access::{AiAccess, Group, Level};
use mk_commands::{Command, CommandResult, CommandSpec, Ctx, Registry};
use mk_core::CommodityCode;
use mk_mcp::Server;
use mk_store::Store;
use serde_json::{Value, json};

const MODERN: &str = "2026-07-28";

// ----- test-only module command (never in the shipped registry) ---------------

fn estimate_args() -> Value {
    json!({"type": "object", "additionalProperties": false})
}

static ESTIMATE_PING: CommandSpec = CommandSpec {
    name: "estimate.ping",
    summary: "Test-only command in the estimates module",
    module: "estimates",
    mutates: false,
    args: estimate_args,
};

struct EstimatePing;
impl Command for EstimatePing {
    fn spec(&self) -> &CommandSpec {
        &ESTIMATE_PING
    }
    fn run(&self, _: &Ctx<'_>, _: Value) -> CommandResult<Value> {
        Ok(json!({"pong": true}))
    }
}

/// A plain test book in a temporary folder (kept alive by the server's backend).
struct TestBook {
    _dir: tempfile::TempDir,
    path: PathBuf,
    control: PathBuf,
}

fn full_access() -> AiAccess {
    AiAccess {
        enabled: true,
        level: Level::ReadWrite,
        groups: Group::ALL.into_iter().collect(),
        app_only: false,
    }
}

fn read_only_access() -> AiAccess {
    AiAccess {
        level: Level::ReadOnly,
        ..full_access()
    }
}

fn book_with(access: &AiAccess, setup: impl FnOnce(&mut Store)) -> TestBook {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.moneykeep");
    let control = dir.path().join("config");
    let mut store = Store::create(&path, "T", &CommodityCode::new("USD").unwrap()).unwrap();
    mk_commands::ai_access::set(&mut store, "test", access).unwrap();
    setup(&mut store);
    TestBook {
        _dir: dir,
        path,
        control,
    }
}

/// keep-mcp over the book, direct route (no app is advertised in `control`).
fn serve_book(b: TestBook, make: Box<dyn Fn() -> Registry>) -> Server {
    let router = Router::new(Target::Book(b.path.clone()), Some(b.control.clone()), make);
    BOOKS.with(|v| v.borrow_mut().push(b));
    Server::new(Box::new(router))
}

thread_local! {
    /// Test books stay on disk for the test's lifetime.
    static BOOKS: std::cell::RefCell<Vec<TestBook>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn server(read_only: bool) -> Server {
    let access = if read_only {
        read_only_access()
    } else {
        full_access()
    };
    serve_book(
        book_with(&access, |_| {}),
        Box::new(Registry::with_builtins),
    )
}

fn with_estimates() -> Registry {
    let mut r = Registry::with_builtins();
    r.register(Box::new(EstimatePing));
    r
}

/// A server whose registry also has a command in the `estimates` module.
fn server_with_module_command() -> Server {
    serve_book(book_with(&full_access(), |_| {}), Box::new(with_estimates))
}

/// A server over a book prepared with the shipped registry.
fn server_after(read_only: bool, setup: impl FnOnce(&mut Store)) -> Server {
    let access = if read_only {
        read_only_access()
    } else {
        full_access()
    };
    serve_book(book_with(&access, setup), Box::new(Registry::with_builtins))
}

// ----- message helpers ---------------------------------------------------------

fn meta(client: Option<&str>) -> Value {
    let mut m = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    if let Some(name) = client {
        m["io.modelcontextprotocol/clientInfo"] = json!({"name": name, "version": "1"});
    }
    m
}

fn send(s: &mut Server, msg: Value) -> Vec<Value> {
    s.handle_line(&msg.to_string())
}

/// Modern request from client `agent`.
fn modern(s: &mut Server, id: i64, method: &str, mut params: Value) -> Vec<Value> {
    params["_meta"] = meta(Some("agent"));
    send(
        s,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
}

fn modern_call(s: &mut Server, id: i64, tool: &str, args: Value) -> Vec<Value> {
    modern(
        s,
        id,
        "tools/call",
        json!({"name": tool, "arguments": args}),
    )
}

fn legacy(s: &mut Server, id: i64, method: &str, params: Value) -> Vec<Value> {
    send(
        s,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
}

fn legacy_init(s: &mut Server, version: &str, client: &str) -> Value {
    let out = legacy(
        s,
        1,
        "initialize",
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": client, "version": "1"},
        }),
    );
    assert_eq!(out.len(), 1);
    assert!(
        send(
            s,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_empty()
    );
    out[0]["result"].clone()
}

fn error_code(msg: &Value) -> i64 {
    msg["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("not an error: {msg}"))
}

fn tool_names(result: &Value) -> Vec<String> {
    result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn audit(s: &mut Server) -> Value {
    let out = modern_call(s, 900, "history_list", json!({"limit": 1}));
    out[0]["result"]["structuredContent"]["result"].clone()
}

// ----- acceptance tests --------------------------------------------------------

#[test]
fn at1_modern_discover() {
    let mut s = server(false);
    let out = modern(&mut s, 1, "server/discover", json!({}));
    assert_eq!(out.len(), 1);
    let r = &out[0]["result"];
    let versions: Vec<&str> = r["supportedVersions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(versions[0], MODERN);
    assert!(versions.contains(&"2025-11-25"));
    assert_eq!(r["capabilities"]["tools"]["listChanged"], true);
    assert_eq!(r["resultType"], "complete");
    assert_eq!(
        r["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "moneykeep"
    );
    assert!(r["_meta"]["io.modelcontextprotocol/serverInfo"]["version"].is_string());
    assert_eq!(r["ttlMs"], 3_600_000);
    assert_eq!(r["cacheScope"], "private");
    let instructions = r["instructions"].as_str().unwrap();
    assert!(instructions.contains("decimal strings") && instructions.contains("undone"));
}

#[test]
fn at2_modern_without_handshake() {
    let mut s = server(false);
    let out = modern(&mut s, 1, "tools/list", json!({}));
    let r = &out[0]["result"];
    let names = tool_names(r);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(names.contains(&"txn_create".to_owned()));
    assert!(r.get("nextCursor").is_none());
    assert_eq!(r["resultType"], "complete");
    assert_eq!(r["ttlMs"], 60_000);
    assert_eq!(r["cacheScope"], "private");
}

#[test]
fn at3_missing_metadata() {
    let mut s = server(false);
    let out = send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
        "_meta": {"io.modelcontextprotocol/protocolVersion": MODERN}}}),
    );
    assert_eq!(error_code(&out[0]), -32602);
    let out = send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    assert_eq!(error_code(&out[0]), -32602);
    let message = out[0]["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(MODERN) && message.contains("initialize"),
        "{message}"
    );
}

#[test]
fn at4_unsupported_version() {
    let mut s = server(false);
    let out = send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "1900-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}}}}),
    );
    let e = &out[0]["error"];
    assert_eq!(e["code"], -32022);
    assert_eq!(e["message"], "Unsupported protocol version");
    assert_eq!(e["data"]["supported"][0], MODERN);
    assert!(
        e["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!("2025-11-25"))
    );
    assert_eq!(e["data"]["requested"], "1900-01-01");
}

#[test]
fn at5_unknown_tool_is_protocol_error_in_both_eras() {
    // Modern.
    let mut s = server(false);
    let out = modern_call(&mut s, 1, "no_such_tool", json!({}));
    assert_eq!(error_code(&out[0]), -32602);
    assert!(
        out[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no_such_tool")
    );
    let mut ro = server(true);
    let out = modern_call(&mut ro, 1, "txn_create", json!({}));
    assert_eq!(error_code(&out[0]), -32602);
    assert!(
        out[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("txn_create")
    );
    let out = modern(&mut ro, 2, "tools/list", json!({}));
    let names = tool_names(&out[0]["result"]);
    assert!(
        !names
            .iter()
            .any(|n| n == "txn_create" || n == "module_enable")
    );
    assert!(names.contains(&"account_list".to_owned()));
    // A tool hidden by a disabled module.
    let mut m = server_with_module_command();
    assert_eq!(
        error_code(&modern_call(&mut m, 1, "estimate_ping", json!({}))[0]),
        -32602
    );
    // Legacy.
    let mut s = server(false);
    legacy_init(&mut s, "2025-06-18", "old");
    assert_eq!(
        error_code(&legacy(&mut s, 2, "tools/call", json!({"name": "no_such_tool"}))[0]),
        -32602
    );
    let mut ro = server(true);
    legacy_init(&mut ro, "2025-06-18", "old");
    assert_eq!(
        error_code(
            &legacy(
                &mut ro,
                2,
                "tools/call",
                json!({"name": "txn_create", "arguments": {}})
            )[0]
        ),
        -32602
    );
}

#[test]
fn at6_tool_execution_error() {
    let mut s = server(false);
    modern_call(
        &mut s,
        1,
        "account_create",
        json!({"name": "Checking", "type": "asset"}),
    );
    let before = audit(&mut s);
    let out = modern_call(
        &mut s,
        2,
        "txn_create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Checking", "value": "100.00"},
        {"account": "Opening Balances", "value": "-99.99"}]}),
    );
    assert_eq!(out.len(), 1);
    let r = &out[0]["result"];
    assert_eq!(r["isError"], true);
    assert_eq!(r["structuredContent"]["error"]["code"], "invalid_args");
    let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, r["structuredContent"]);
    let list = modern_call(&mut s, 3, "txn_list", json!({}));
    assert_eq!(list[0]["result"]["structuredContent"]["result"], json!([]));
    assert_eq!(audit(&mut s), before);
}

#[test]
fn at7_subscriptions() {
    let mut s = server_with_module_command();
    let out = modern(
        &mut s,
        7,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true, "resourcesListChanged": true}}),
    );
    assert_eq!(
        out.len(),
        1,
        "no immediate response, only the acknowledgment"
    );
    assert_eq!(out[0]["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(
        out[0]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    assert_eq!(
        out[0]["params"]["notifications"],
        json!({"toolsListChanged": true})
    );
    assert!(out[0].get("id").is_none());

    // Enabling estimates adds estimate_ping: tagged list_changed after the result.
    let out = modern_call(&mut s, 8, "module_enable", json!({"module": "estimates"}));
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["id"], 8);
    assert_eq!(out[1]["method"], "notifications/tools/list_changed");
    assert_eq!(
        out[1]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    let names = tool_names(&modern(&mut s, 9, "tools/list", json!({}))[0]["result"]);
    assert!(names.contains(&"estimate_ping".to_owned()));

    // A toggle that does not change the tool set sends nothing.
    let out = modern_call(&mut s, 10, "module_enable", json!({"module": "time"}));
    assert_eq!(out.len(), 1);

    // Undo of the time toggle changes nothing either; undo of estimates does.
    assert_eq!(modern_call(&mut s, 11, "history_undo", json!({})).len(), 1);
    let out = modern_call(&mut s, 12, "history_undo", json!({}));
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[1]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );

    // After cancellation, nothing more for id 7.
    assert!(send(&mut s, json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}})).is_empty());
    assert_eq!(
        modern_call(&mut s, 13, "module_enable", json!({"module": "estimates"})).len(),
        1
    );
    assert!(
        s.shutdown().is_empty(),
        "a cancelled subscription gets no closure result"
    );

    // Graceful closure on EOF, through serve().
    let mut s = server_with_module_command();
    let listen = json!({"jsonrpc": "2.0", "id": 7, "method": "subscriptions/listen",
        "params": {"notifications": {"toolsListChanged": true}, "_meta": meta(Some("agent"))}});
    let enable = json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call",
        "params": {"name": "module_enable", "arguments": {"module": "estimates"}, "_meta": meta(Some("agent"))}});
    let input = format!("{listen}\n{enable}\n");
    let mut output = Vec::new();
    s.serve(std::io::Cursor::new(input.into_bytes()), &mut output)
        .unwrap();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines[0]["method"],
        "notifications/subscriptions/acknowledged"
    );
    assert_eq!(lines[1]["id"], 8);
    assert_eq!(lines[2]["method"], "notifications/tools/list_changed");
    assert_eq!(lines[3]["id"], 7);
    assert_eq!(
        lines[3]["result"],
        json!({"resultType": "complete",
        "_meta": {"io.modelcontextprotocol/subscriptionId": 7}})
    );
}

#[test]
fn at7_concurrent_subscriptions_each_get_a_tagged_copy() {
    let mut s = server_with_module_command();
    modern(
        &mut s,
        7,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true}}),
    );
    modern(
        &mut s,
        21,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true}}),
    );
    let out = modern_call(&mut s, 30, "module_enable", json!({"module": "estimates"}));
    let tags: Vec<&Value> = out[1..]
        .iter()
        .map(|n| &n["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"])
        .collect();
    assert_eq!(tags, [&json!(7), &json!(21)]);
    assert_eq!(s.shutdown().len(), 2);
}

#[test]
fn at8_no_unsolicited_modern_notifications() {
    let mut s = server_with_module_command();
    let out = modern_call(&mut s, 1, "module_enable", json!({"module": "estimates"}));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["result"]["isError"], false);
}

#[test]
fn at9_legacy_client() {
    let mut s = server_with_module_command();
    let init = legacy_init(&mut s, "2025-06-18", "old-client");
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["capabilities"]["tools"]["listChanged"], true);
    assert_eq!(init["serverInfo"]["name"], "moneykeep");
    assert_eq!(legacy(&mut s, 2, "ping", json!({}))[0]["result"], json!({}));
    let tools = legacy(&mut s, 3, "tools/list", json!({}));
    assert!(tool_names(&tools[0]["result"]).contains(&"txn_create".to_owned()));
    let out = legacy(
        &mut s,
        4,
        "tools/call",
        json!({"name": "module_enable", "arguments": {"module": "estimates"}}),
    );
    assert_eq!(out[0]["result"]["isError"], false);
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[1],
        json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
    );
    let out = legacy(
        &mut s,
        5,
        "tools/call",
        json!({"name": "history_list", "arguments": {"limit": 1}}),
    );
    assert_eq!(
        out[0]["result"]["structuredContent"]["result"][0]["actor"],
        "mcp:old-client"
    );
    // An unsupported legacy version is answered with the latest legacy version.
    let mut t = server(false);
    assert_eq!(
        legacy_init(&mut t, "2023-01-01", "x")["protocolVersion"],
        "2025-11-25"
    );
}

#[test]
fn at10_modern_ping_is_method_not_found() {
    let mut s = server(false);
    assert_eq!(error_code(&modern(&mut s, 1, "ping", json!({}))[0]), -32601);
    assert_eq!(
        error_code(&modern(&mut s, 2, "logging/setLevel", json!({"level": "info"}))[0]),
        -32601
    );
    assert_eq!(
        error_code(&modern(&mut s, 3, "initialize", json!({}))[0]),
        -32601
    );
}

#[test]
fn at11_dual_era_in_one_process() {
    let mut s = server(false);
    legacy_init(&mut s, "2025-06-18", "legacy-client");
    let mut params =
        json!({"name": "account_create", "arguments": {"name": "Cash", "type": "asset"}});
    params["_meta"] = meta(Some("modern-client"));
    let out = send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params}),
    );
    assert_eq!(out[0]["result"]["isError"], false);
    assert_eq!(out[0]["result"]["resultType"], "complete");
    let log = legacy(
        &mut s,
        3,
        "tools/call",
        json!({"name": "history_list", "arguments": {"limit": 1}}),
    );
    assert_eq!(
        log[0]["result"]["structuredContent"]["result"][0]["actor"],
        "mcp:modern-client"
    );
    // Legacy calls in the same process keep the legacy actor.
    legacy(
        &mut s,
        4,
        "tools/call",
        json!({"name": "account_create", "arguments": {"name": "Bank", "type": "asset"}}),
    );
    let log = legacy(
        &mut s,
        5,
        "tools/call",
        json!({"name": "history_list", "arguments": {"limit": 1}}),
    );
    assert_eq!(
        log[0]["result"]["structuredContent"]["result"][0]["actor"],
        "mcp:legacy-client"
    );
}

#[test]
fn at12_audit_actor() {
    let mut s = server(false);
    modern_call(
        &mut s,
        1,
        "account_create",
        json!({"name": "Cash", "type": "asset"}),
    );
    assert_eq!(audit(&mut s)[0]["actor"], "mcp:agent");
    let mut params =
        json!({"name": "account_create", "arguments": {"name": "Bank", "type": "asset"}});
    params["_meta"] = meta(None);
    let out = send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params}),
    );
    assert_eq!(out[0]["result"]["isError"], false);
    assert_eq!(audit(&mut s)[0]["actor"], "mcp:unknown");
}

fn has_network_ref(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, v)| {
            (k == "$ref" && v.as_str().is_some_and(|r| !r.starts_with('#'))) || has_network_ref(v)
        }),
        Value::Array(a) => a.iter().any(has_network_ref),
        _ => false,
    }
}

#[test]
fn at13_schemas() {
    let mut s = server(false);
    let tools = modern(&mut s, 1, "tools/list", json!({}))[0]["result"]["tools"].clone();
    let meta_validator = jsonschema::draft202012::meta::validator();
    let tools = tools.as_array().unwrap();
    assert!(tools.len() >= 30);
    for t in tools {
        let schema = &t["inputSchema"];
        assert_eq!(schema["type"], "object", "{}", t["name"]);
        assert!(meta_validator.is_valid(schema), "{}: {schema}", t["name"]);
        assert!(!has_network_ref(schema), "{}", t["name"]);
    }
    for no_args in ["book_info", "module_list", "history_undo", "history_redo"] {
        let t = tools.iter().find(|t| t["name"] == no_args).unwrap();
        assert_eq!(
            t["inputSchema"],
            json!({"type": "object", "additionalProperties": false}),
            "{no_args}"
        );
    }
}

#[test]
fn at14_success_shape() {
    let mut s = server(false);
    let out = modern_call(&mut s, 1, "book_info", json!({}));
    let r = &out[0]["result"];
    assert_eq!(r["isError"], false);
    let content = r["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, r["structuredContent"]);
    assert_eq!(text["name"], "T");
    // Non-object results are wrapped in structuredContent, and the text is the raw JSON.
    let out = modern_call(&mut s, 2, "account_list", json!({}));
    let r = &out[0]["result"];
    let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(r["structuredContent"], json!({"result": text}));
}

#[test]
fn at15_error_code_hygiene() {
    let mut codes: Vec<i64> = Vec::new();
    let mut collect = |msgs: Vec<Value>| {
        for m in msgs {
            if let Some(c) = m["error"]["code"].as_i64() {
                codes.push(c);
            }
        }
    };
    let mut s = server(false);
    collect(s.handle_line("{not json"));
    collect(s.handle_line("[1, 2]"));
    collect(send(&mut s, json!({"jsonrpc": "2.0", "id": 1})));
    collect(send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
    ));
    collect(send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": [1]}),
    ));
    collect(send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
        "_meta": {"io.modelcontextprotocol/protocolVersion": MODERN}}}),
    ));
    collect(send(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
        "_meta": {"io.modelcontextprotocol/protocolVersion": "1900-01-01",
                  "io.modelcontextprotocol/clientCapabilities": {}}}}),
    ));
    collect(modern(&mut s, 1, "ping", json!({})));
    collect(modern(&mut s, 1, "no/such", json!({})));
    collect(modern(&mut s, 1, "tools/call", json!({})));
    collect(modern(
        &mut s,
        1,
        "tools/call",
        json!({"name": "book_info", "arguments": [1]}),
    ));
    collect(modern_call(&mut s, 1, "no_such_tool", json!({})));
    collect(modern(&mut s, 1, "subscriptions/listen", json!({})));
    collect(modern_call(
        &mut s,
        1,
        "txn_create",
        json!({"date": "bad", "splits": []}),
    ));
    let mut ro = server(true);
    collect(modern_call(&mut ro, 1, "txn_create", json!({})));
    legacy_init(&mut ro, "2025-06-18", "x");
    collect(legacy(&mut ro, 2, "no/such", json!({})));
    collect(legacy(
        &mut ro,
        3,
        "tools/call",
        json!({"name": "txn_create"}),
    ));

    assert!(codes.len() >= 14, "{codes:?}");
    for c in &codes {
        assert!(
            !(-32019..=-32000).contains(c),
            "reserved implementation code {c}"
        );
        assert_ne!(*c, -32002);
        if (-32099..=-32020).contains(c) {
            assert_eq!(*c, -32022, "only -32022 from the MCP range");
        }
        assert!(
            [-32700, -32600, -32601, -32602, -32603, -32022].contains(c),
            "unexpected code {c}"
        );
    }
    assert!(codes.contains(&-32022));
}

/// connectivity.md AT11: feeds are tools, credential handling is not.
#[test]
fn connect_tools_without_credentials() {
    use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
    let fake = FakeSimplefin::new(
        r#"{"accounts": [{"id": "a1", "name": "Everyday", "conn_id": "c1",
        "currency": "USD", "balance": "1.00", "balance-date": 1790812800,
        "transactions": [{"id": "t1", "posted": 1790553600, "amount": "-4.00", "description": "Shop"}]}],
        "connections": [{"conn_id": "c1", "name": "Bank"}]}"#,
    );
    let secrets = MemorySecrets::new(true);
    let make = move || {
        let mut r = Registry::with_builtins();
        r.set_connector(Connector {
            fetcher: std::sync::Arc::new(fake.clone()),
            secrets: Box::new(secrets.clone()),
            utc_offset_secs: 0,
            now: Some(1_790_812_800),
            host: None,
        });
        r
    };
    let r = make();
    let b = book_with(&full_access(), |_| {});
    let mut store = Store::open(&b.path).unwrap();
    let actor = mk_commands::Actor("app".into());
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        (
            "account.create",
            json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
        ),
    ] {
        r.invoke(&mut store, &actor, c, a).unwrap();
    }
    let conn = r
        .invoke(
            &mut store,
            &actor,
            "connect.add",
            json!({"setup_input": FakeSimplefin::token("m"), "confirm": true}),
        )
        .unwrap();
    r.invoke(&mut store, &actor, "connect.link_account",
        json!({"connection": conn["id"], "remote_account": "a1", "account": "Checking", "sync_from": "2026-09-01"}))
        .unwrap();
    drop(store);
    let mut s = serve_book(b, Box::new(make));
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    assert!(names.contains(&"connect_sync".to_owned()), "{names:?}");
    assert!(names.contains(&"connect_list".to_owned()));
    assert!(!names.contains(&"connect_add".to_owned()));
    assert!(!names.contains(&"connect_disconnect".to_owned()));
    let out = modern_call(&mut s, 2, "connect_add", json!({"setup_input": "x"}));
    assert!(out[0].get("error").is_some(), "{}", out[0]);
    let out = modern_call(&mut s, 3, "connect_sync", json!({"now": true}));
    let text = out[0].to_string();
    assert!(text.contains("\"new\":1"), "{text}");
    assert!(
        !text.contains("s3cret") && !text.contains("demo:"),
        "{text}"
    );
    let listed = modern_call(&mut s, 4, "connect_list", json!({}))[0].to_string();
    assert!(!listed.contains("s3cret"));
}

/// connectivity-processors.md AT12.
#[test]
fn processor_tools_listed_without_connect_add() {
    let mut s = server_after(false, |store| {
        let r = Registry::with_builtins();
        let actor = mk_commands::Actor("app".into());
        r.invoke(
            store,
            &actor,
            "module.enable",
            json!({"module": "payment-processing"}),
        )
        .unwrap();
    });
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    for t in ["processor_settings", "processor_set", "connect_sync"] {
        assert!(names.contains(&t.to_owned()), "{t}: {names:?}");
    }
    assert!(!names.contains(&"connect_add".to_owned()));
}

/// connectivity-brokerage.md AT17: the brokerage tools are listed, credential tools are
/// not, and moving an OFX connection to a new host is refused over MCP.
#[test]
fn brokerage_tools_and_new_host_refused() {
    let fake = mk_commands::ofx_conn::FakeOfx::standard();
    let secrets = mk_commands::connect::MemorySecrets::new(true);
    let make = move || {
        let mut r = Registry::with_builtins();
        r.set_connector(mk_commands::connect::Connector {
            fetcher: std::sync::Arc::new(fake.clone()),
            secrets: Box::new(secrets.clone()),
            utc_offset_secs: 0,
            now: Some(1_791_072_000),
            host: None,
        });
        r
    };
    let r = make();
    let b = book_with(&full_access(), |_| {});
    let mut store = Store::open(&b.path).unwrap();
    let actor = mk_commands::Actor("app".into());
    for m in ["bank-feeds", "investments"] {
        r.invoke(&mut store, &actor, "module.enable", json!({"module": m}))
            .unwrap();
    }
    let c = r
        .invoke(
            &mut store,
            &actor,
            "connect.add",
            json!({"provider": "ofx", "user_id": "user1", "password": "pass1", "host_acknowledged": true,
                "institution": {"name": "Test Bank", "url": "https://ofx.testbank.example/ofx",
                    "org": "TESTBANK", "fid": "1234", "ofx_version": "103"}}),
        )
        .unwrap();
    drop(store);
    let mut s = serve_book(b, Box::new(make));
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    for t in [
        "connect_ofx_profile",
        "connect_set_ofx_profile",
        "connect_client_uid",
        "connect_add_manual_account",
        "connect_holdings_check",
        "connect_sync",
    ] {
        assert!(names.contains(&t.to_owned()), "{t}: {names:?}");
    }
    assert!(!names.contains(&"connect_add".to_owned()));
    assert!(!names.contains(&"connect_answer_challenge".to_owned()));
    let out = modern_call(
        &mut s,
        2,
        "connect_set_ofx_profile",
        json!({"connection": c["id"], "accept_new_host": true}),
    );
    let r = &out[0]["result"];
    assert_eq!(r["isError"], true, "{r}");
    assert!(r.to_string().contains("only be changed in the app"), "{r}");
    let out = modern_call(
        &mut s,
        3,
        "connect_ofx_profile",
        json!({"connection": c["id"]}),
    );
    let text = out[0]["result"].to_string();
    assert!(
        text.contains("ofx.testbank.example") && !text.contains("pass1"),
        "{text}"
    );
}

/// expenses.md AT10: the expense and receipt tools are listed, except the binary
/// upload and download of receipt files.
#[test]
fn expenses_at10_receipt_files_not_over_mcp() {
    let mut s = server_after(false, |store| {
        let r = Registry::with_builtins();
        let actor = mk_commands::Actor("app".into());
        r.invoke(
            store,
            &actor,
            "module.enable",
            json!({"module": "expenses"}),
        )
        .unwrap();
    });
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    for t in [
        "expense_create",
        "expense_update",
        "expense_delete",
        "expense_get",
        "expense_list",
        "expense_post",
        "expense_match_candidates",
        "expense_record_reimbursement",
        "expense_report",
        "expense_mark_reported",
        "receipt_link",
        "receipt_unlink",
        "receipt_inbox",
        "receipt_thumbnail",
    ] {
        assert!(names.contains(&t.to_owned()), "{t}: {names:?}");
    }
    assert!(!names.contains(&"receipt_add".to_owned()));
    assert!(!names.contains(&"receipt_get".to_owned()));
    let out = modern_call(
        &mut s,
        2,
        "receipt_add",
        json!({"content_base64": "AAAA", "file_name": "a.jpg"}),
    );
    assert!(out[0].get("error").is_some(), "{}", out[0]);
}

/// expenses.md revision 2: the report is read-only, so a read-only server still
/// offers it; marking expenses reported is a writing tool and is hidden there.
#[test]
fn expenses_report_is_read_only_and_marking_is_not() {
    let mut s = server_after(true, |store| {
        let r = Registry::with_builtins();
        let actor = mk_commands::Actor("app".into());
        r.invoke(
            store,
            &actor,
            "module.enable",
            json!({"module": "expenses"}),
        )
        .unwrap();
    });
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    assert!(names.contains(&"expense_report".to_owned()), "{names:?}");
    assert!(!names.contains(&"expense_mark_reported".to_owned()));
}

/// mobile-capture.md AT14: the phone capture commands that handle the desktop token or
/// a pairing code are not offered over MCP; the others are.
#[test]
fn capture_secret_commands_not_over_mcp() {
    let mut s = server_after(false, |store| {
        let r = Registry::with_builtins();
        let actor = mk_commands::Actor("app".into());
        r.invoke(
            store,
            &actor,
            "module.enable",
            json!({"module": "expenses"}),
        )
        .unwrap();
    });
    let names = tool_names(&modern(&mut s, 1, "tools/list", json!({}))[0]["result"]);
    for t in [
        "capture_settings",
        "capture_test",
        "capture_collect",
        "capture_devices",
        "capture_revoke_device",
        "capture_publish_lists",
        // mobile-capture.md revision 2: the dry run and the status light.
        "capture_setup_plan",
        "capture_status",
    ] {
        assert!(names.contains(&t.to_owned()), "{t}: {names:?}");
    }
    for t in [
        "capture_set_server",
        "capture_pair_code",
        "capture_setup_check_token",
        "capture_setup_apply",
        "capture_setup_remove",
        "capture_forget_cloudflare_token",
        "capture_start_server",
        "capture_setup_repair",
        "capture_update_server",
    ] {
        assert!(!names.contains(&t.to_owned()), "{t} is offered over MCP");
    }
    let out = modern_call(
        &mut s,
        2,
        "capture_set_server",
        json!({"url": "https://capture.example.com", "token": "x"}),
    );
    assert!(out[0].get("error").is_some(), "{}", out[0]);
}
