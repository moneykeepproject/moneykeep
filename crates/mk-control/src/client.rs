//! The `keep-mcp` side of the control channel: read the discovery file, say hello,
//! then `list` and `call` one at a time. Events (`tools_changed`, `closed`) arrive on
//! a reader thread, which calls the `on_event` callback so the MCP server can tell
//! its client about tool-set changes without waiting for the next request.

use std::io::{BufReader, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::server::{CallReply, Line, read_line};
use crate::{APPROVAL_TIMEOUT, AiAccess, Discovery, KIND_KEEP_MCP, PROTOCOL, access_from};

/// Called on every event from the app (from the reader thread).
pub type OnEvent = Box<dyn Fn() + Send + 'static>;

/// What the client says in its hello.
#[derive(Debug, Clone)]
pub struct Hello {
    /// The MCP client's name (actor `mcp:<name>`).
    pub client: String,
    /// The canonical path of the requested book; `None` for `keep-mcp --app`.
    pub book_path: Option<String>,
}

/// The app accepted the hello.
#[derive(Debug, Clone)]
pub struct HelloOk {
    pub book_name: String,
    pub book_path: String,
    pub access: AiAccess,
}

/// Why connecting failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The app refused the hello (`bad_token`, `wrong_book`, `access_off`, `denied`,
    /// `approval_timeout`, `busy`).
    Refused { code: String, message: String },
    /// The connection failed or closed without a reply.
    Io(String),
}

/// A connection to the app.
pub struct ControlClient {
    writer: TcpStream,
    replies: Receiver<Value>,
    next_id: u64,
    closed: Arc<AtomicBool>,
    reason: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl ControlClient {
    /// Connect to the app advertised by `d` and say hello. Waits for the owner's
    /// answer (up to the approval timeout, plus a margin).
    pub fn connect(
        d: &Discovery,
        hello: &Hello,
        on_event: OnEvent,
    ) -> Result<(Self, HelloOk), ConnectError> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], d.port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|e| ConnectError::Io(e.to_string()))?;
        let mut writer = stream
            .try_clone()
            .map_err(|e| ConnectError::Io(e.to_string()))?;
        let msg = json!({
            "hello": PROTOCOL,
            "token": d.token,
            "client": {"name": hello.client, "kind": KIND_KEEP_MCP},
            "book_path": hello.book_path,
        });
        writeln!(writer, "{msg}").map_err(|e| ConnectError::Io(e.to_string()))?;
        let _ = stream.set_read_timeout(Some(
            APPROVAL_TIMEOUT.saturating_add(Duration::from_secs(10)),
        ));
        let mut reader = BufReader::new(stream);
        let reply = match read_line(&mut reader) {
            Line::Text(t) => {
                serde_json::from_str::<Value>(&t).map_err(|e| ConnectError::Io(e.to_string()))?
            }
            Line::Eof | Line::TooLong => {
                return Err(ConnectError::Io("the app closed the connection".into()));
            }
        };
        if let Some(e) = reply.get("error") {
            return Err(ConnectError::Refused {
                code: e
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
                    .to_owned(),
                message: e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        if reply.get("ok") != Some(&Value::Bool(true)) {
            return Err(ConnectError::Io("unexpected reply from the app".into()));
        }
        let ok = HelloOk {
            book_name: reply
                .pointer("/book/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            book_path: reply
                .pointer("/book/path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            access: access_from(reply.get("level"), reply.get("groups")),
        };
        let _ = reader.get_ref().set_read_timeout(None);
        let (tx, rx) = channel();
        let closed = Arc::new(AtomicBool::new(false));
        let reason = Arc::new(Mutex::new(None));
        let (c, r) = (Arc::clone(&closed), Arc::clone(&reason));
        std::thread::Builder::new()
            .name("keep-mcp-control".into())
            .spawn(move || {
                while let Line::Text(t) = read_line(&mut reader) {
                    let Ok(v) = serde_json::from_str::<Value>(&t) else {
                        continue;
                    };
                    match v.get("event").and_then(Value::as_str) {
                        Some("closed") => {
                            if let Ok(mut g) = r.lock() {
                                *g = v.get("reason").and_then(Value::as_str).map(str::to_owned);
                            }
                            c.store(true, Ordering::SeqCst);
                            on_event();
                        }
                        Some(_) => on_event(),
                        None => {
                            if tx.send(v).is_err() {
                                break;
                            }
                        }
                    }
                }
                if !c.swap(true, Ordering::SeqCst) {
                    on_event();
                }
            })
            .map_err(|e| ConnectError::Io(e.to_string()))?;
        Ok((
            Self {
                writer,
                replies: rx,
                next_id: 0,
                closed,
                reason,
            },
            ok,
        ))
    }

    /// Whether the app closed the connection.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The reason the app gave when it closed the connection.
    pub fn close_reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|g| g.clone())
    }

    fn request(&mut self, mut msg: Value) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the app closed the connection".into());
        }
        self.next_id += 1;
        let id = self.next_id;
        if let Some(o) = msg.as_object_mut() {
            o.insert("id".into(), json!(id));
        }
        writeln!(self.writer, "{msg}").map_err(|e| e.to_string())?;
        loop {
            let v = self
                .replies
                .recv()
                .map_err(|_| "the app closed the connection".to_owned())?;
            if v.get("id") == Some(&json!(id)) {
                return Ok(v);
            }
        }
    }

    /// The exposed set (`CommandSpec` JSON) and the access it was computed for.
    pub fn list(&mut self) -> Result<(Vec<Value>, AiAccess), String> {
        let v = self.request(json!({"op": "list"}))?;
        if let Some(e) = v.get("error") {
            return Err(e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned());
        }
        let commands = v
            .get("commands")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok((commands, access_from(v.get("level"), v.get("groups"))))
    }

    /// Run a command in the app.
    pub fn call(&mut self, command: &str, args: Value) -> Result<CallReply, String> {
        let v = self.request(json!({"op": "call", "command": command, "args": args}))?;
        if let Some(e) = v.get("error") {
            let code = e.get("code").and_then(Value::as_str).unwrap_or("error");
            let message = e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            return Ok(if code == "not_available" {
                CallReply::NotAvailable(message)
            } else {
                CallReply::Failed {
                    code: code.to_owned(),
                    message,
                }
            });
        }
        Ok(CallReply::Ok {
            value: v.get("result").cloned().unwrap_or(Value::Null),
            changed: false,
        })
    }
}

impl Drop for ControlClient {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}
