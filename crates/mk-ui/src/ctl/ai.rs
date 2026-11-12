//! AI assistant access in the app (mcp-server.md revision 3, app-desktop.md
//! revision 33): Settings → Security → AI assistant access, the approval dialog,
//! the top-bar "AI assistant connected" indicator with its pop-up, and History's
//! "Changes by AI assistants" filter.
//!
//! The app shell runs the control channel; it tells the controller about approval
//! requests and connected assistants, and acts on the `Ai*` events it emits.

use serde_json::{Value, json};

use super::App;
use super::ui::Ui;
use crate::{AppEvent, View};

/// The privacy note under the switch (always visible).
pub const NOTE: &str = "An AI assistant on this computer can use this book through the keep-mcp program. What the assistant reads is sent to the AI service you use, under that service's terms. Its changes are recorded in History and can be undone.";
pub const SWITCH: &str = "Allow AI assistants to use this book";
pub const APP_ONLY: &str = "Only through MoneyKeep, and ask me each time";
pub const APP_ONLY_PROTECTED: &str = "Password-protected books are only used through MoneyKeep.";
pub const NOT_INSTALLED: &str = "keep-mcp is not installed. Install it (for example from keep-launcher) to connect an assistant.";
pub const OTHER_WINDOW: &str = "Another MoneyKeep window is serving AI assistants.";
pub const INDICATOR: &str = "AI assistant connected";
pub const HISTORY_FILTER: &str = "Changes by AI assistants";

/// The permission groups: stored name and app label (mcp-server.md revision 3).
pub const GROUPS: [(&str, &str); 4] = [
    ("delete", "Delete and void"),
    ("settings", "Change features and book settings"),
    ("files", "Read files on this computer"),
    ("connections", "Use bank feeds and connections"),
];

/// An assistant connected over the control channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiClient {
    pub name: String,
    /// "Read and write; also: Delete and void".
    pub access: String,
}

/// An approval the owner has not answered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Approval {
    pub id: u64,
    pub client: String,
    pub book: String,
    pub access: String,
}

#[derive(Debug, Default)]
pub(crate) struct AiState {
    pub connected: Vec<AiClient>,
    pub approvals: Vec<Approval>,
    /// `keep-mcp config` snippets for the open book, per client (label, JSON);
    /// `None` when keep-mcp is not installed.
    pub keep_mcp: Option<Vec<(String, String)>>,
    pub other_window: bool,
    /// The "Copy assistant setup" dialog, with the chosen client.
    pub setup: Option<usize>,
    pub popup: bool,
    pub history_only: bool,
}

/// The settings as the controller edits them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    enabled: bool,
    read_write: bool,
    groups: Vec<String>,
    app_only: bool,
}

impl Settings {
    fn from_json(v: &Value) -> Self {
        Self {
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            read_write: v.get("level").and_then(Value::as_str) == Some("read_write"),
            groups: v
                .get("groups")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|g| g.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            app_only: v.get("app_only").and_then(Value::as_bool).unwrap_or(true),
        }
    }

    fn to_json(&self) -> Value {
        let groups: Vec<&str> = GROUPS
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| self.groups.iter().any(|g| g == n))
            .collect();
        json!({
            "enabled": self.enabled,
            "level": if self.read_write { "read_write" } else { "read_only" },
            "groups": groups,
            "app_only": self.app_only,
        })
    }
}

/// "Read only" or "Read and write", with "; also: <labels>".
pub fn access_text(level: &str, groups: &[String]) -> String {
    let mut s = if level == "read_write" {
        "Read and write".to_owned()
    } else {
        "Read only".to_owned()
    };
    let labels: Vec<&str> = GROUPS
        .iter()
        .filter(|(n, _)| groups.iter().any(|g| g == n))
        .map(|(_, l)| *l)
        .collect();
    if !labels.is_empty() {
        s.push_str("; also: ");
        s.push_str(&labels.join(", "));
    }
    s
}

impl App {
    /// The control channel asks the owner to allow `client` (approval `id`).
    /// `access` is the owner-facing access text (see [`access_text`]).
    pub fn ai_approval_requested(&mut self, id: u64, client: &str, book: &str, access: &str) {
        self.ai.approvals.retain(|a| a.id != id);
        self.ai.approvals.push(Approval {
            id,
            client: client.to_owned(),
            book: book.to_owned(),
            access: access.to_owned(),
        });
    }

    /// Approval `id` is no longer waited for (timed out, or the client left).
    pub fn ai_approval_ended(&mut self, id: u64) {
        self.ai.approvals.retain(|a| a.id != id);
    }

    /// The assistants connected now.
    pub fn ai_connections(&mut self, clients: Vec<AiClient>) {
        self.ai.connected = clients;
        if self.ai.connected.is_empty() {
            self.ai.popup = false;
        }
    }

    /// An assistant changed the book: refresh the screens as for any change.
    pub fn ai_book_changed(&mut self) {
        self.generation += 1;
        self.refresh_history_hints();
    }

    /// The `keep-mcp config` snippets for the open book (client label, JSON), or
    /// `None` when keep-mcp is not installed.
    pub fn ai_keep_mcp(&mut self, snippets: Option<Vec<(String, String)>>) {
        self.ai.keep_mcp = snippets;
    }

    /// Whether another app window serves AI assistants (this one does not listen).
    pub fn ai_other_window(&mut self, on: bool) {
        self.ai.other_window = on;
    }

    /// Changes to the book so far (the app shell tells the control channel).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The assistants the indicator lists.
    pub fn ai_connected(&self) -> &[AiClient] {
        &self.ai.connected
    }
}

/// Settings → Security → AI assistant access.
pub(crate) fn settings_card(app: &mut App, ui: &mut Ui) {
    let Some(v) = app.backend.ai_access() else {
        return;
    };
    let (protected, _) = app.backend.protection();
    let mut s = Settings::from_json(&v);
    let before = s.clone();
    let mut setup = false;
    ui.card("AI assistant access", |ui| {
        ui.switch(&mut s.enabled, SWITCH);
        ui.secondary(NOTE);
        if app.ai.other_window {
            ui.label(OTHER_WINDOW);
        }
        if !s.enabled {
            return;
        }
        ui.caption("Access");
        ui.row(|ui| {
            ui.radio(&mut s.read_write, false, "Read only");
            ui.radio(&mut s.read_write, true, "Read and write");
        });
        ui.caption("Also allow");
        for (name, label) in GROUPS {
            let mut on = s.groups.iter().any(|g| g == name);
            // Read files also applies to read-only commands.
            ui.enabled(s.read_write || name == "files");
            if ui.checkbox(&mut on, label).changed() {
                s.groups.retain(|g| g != name);
                if on {
                    s.groups.push(name.to_owned());
                }
            }
        }
        let mut app_only = s.app_only || protected;
        ui.enabled(!protected);
        ui.checkbox(&mut app_only, APP_ONLY);
        if protected {
            ui.secondary(APP_ONLY_PROTECTED);
        } else {
            s.app_only = app_only;
        }
        if app.ai.keep_mcp.is_some() {
            ui.row(|ui| {
                if ui.button("Copy assistant setup…").clicked() {
                    setup = true;
                }
            });
        } else {
            ui.secondary(NOT_INSTALLED);
        }
    });
    if setup {
        app.ai.setup = Some(0);
    }
    if s != before {
        match app.backend.set_ai_access(s.to_json()) {
            Ok(_) => {
                app.generation += 1;
                app.refresh_history_hints();
                app.emit(AppEvent::AiAccessChanged);
            }
            Err(e) => app.notify_error(&e),
        }
    }
}

/// The approval dialog, the indicator's pop-up and the setup dialog.
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    approval_dialog(app, ui);
    popup(app, ui);
    setup_dialog(app, ui);
}

fn approval_dialog(app: &mut App, ui: &mut Ui) {
    let Some(a) = app.ai.approvals.first().cloned() else {
        return;
    };
    let mut answer = if ui.dialog_closed("ai-approval") {
        Some(false)
    } else {
        None
    };
    ui.dialog("ai-approval", "Allow AI assistant?", 460, |ui| {
        ui.group(|ui| {
            ui.label(format!(
                "{} wants to use {} through keep-mcp. Access: {}.",
                a.client, a.book, a.access
            ));
        });
        ui.actions(|ui| {
            if ui.button("Deny").clicked() {
                answer = Some(false);
            }
            if ui.primary("Allow").clicked() {
                answer = Some(true);
            }
        });
    });
    if let Some(allow) = answer {
        app.ai.approvals.retain(|x| x.id != a.id);
        app.emit(AppEvent::AiAnswer { id: a.id, allow });
    }
}

/// The top-bar indicator (only while an assistant is connected).
pub(crate) fn indicator(app: &mut App, ui: &mut Ui) {
    if app.ai.connected.is_empty() {
        return;
    }
    let names: Vec<&str> = app.ai.connected.iter().map(|c| c.name.as_str()).collect();
    ui.tip(format!("Connected: {}", names.join(", ")));
    if ui
        .icon_button("assistant", INDICATOR, app.ai.popup)
        .clicked()
    {
        app.ai.popup = !app.ai.popup;
    }
}

fn popup(app: &mut App, ui: &mut Ui) {
    if !app.ai.popup {
        return;
    }
    let mut close = ui.dialog_closed("ai-assistants");
    let mut disconnect = None;
    let mut history = false;
    let clients = app.ai.connected.clone();
    ui.dialog("ai-assistants", "AI assistants", 460, |ui| {
        ui.group(|ui| {
            for c in &clients {
                ui.row(|ui| {
                    ui.next().grow();
                    ui.label(format!("{} — {}", c.name, c.access));
                    ui.named(format!("Disconnect {}", c.name));
                    if ui.button("Disconnect").clicked() {
                        disconnect = Some(c.name.clone());
                    }
                });
            }
            if ui.link("Open AI history").clicked() {
                history = true;
            }
        });
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if let Some(name) = disconnect {
        app.ai.connected.retain(|c| c.name != name);
        app.emit(AppEvent::AiDisconnect { client: name });
    }
    if history {
        close = true;
        app.ai.history_only = true;
        app.navigate(View::History);
    }
    if close || app.ai.connected.is_empty() {
        app.ai.popup = false;
    }
}

fn setup_dialog(app: &mut App, ui: &mut Ui) {
    let (Some(mut idx), Some(snippets)) = (app.ai.setup, app.ai.keep_mcp.clone()) else {
        app.ai.setup = None;
        return;
    };
    let mut close = ui.dialog_closed("ai-setup");
    ui.dialog("ai-setup", "Assistant setup", 560, |ui| {
        ui.group(|ui| {
            let labels: Vec<String> = snippets.iter().map(|(l, _)| l.clone()).collect();
            ui.form("Assistant", |ui| {
                ui.combo("Assistant", &mut idx, &labels);
            });
            let text = snippets
                .get(idx)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            ui.secondary("Paste this into the assistant's MCP configuration:");
            ui.next().mono();
            ui.label(text.clone());
            ui.row(|ui| {
                if ui.button("Copy").clicked() {
                    app.copy(text);
                }
            });
        });
        ui.actions(|ui| {
            if ui.primary("Done").clicked() {
                close = true;
            }
        });
    });
    app.ai.setup = if close { None } else { Some(idx) };
}

/// History's filter: entries whose actor starts `mcp:`.
pub(crate) fn history_filter(app: &mut App, ui: &mut Ui) {
    if ui.toggle(HISTORY_FILTER, app.ai.history_only).clicked() {
        app.ai.history_only = !app.ai.history_only;
    }
}
