//! Options, bonds and crypto (investments-options-bonds-crypto.md): the brokerage
//! view's Option…, Bond… and Crypto… forms, which ask in plain words what happened
//! and preview the transaction and any gain (`inv.preview`) before saving; the
//! dialog of an expired option (Home → Attention); and the redemption of a matured
//! bond. Every figure comes from a command.

use serde_json::{Value, json};

use super::form::set;
use super::ui::{Align, Column, Tone, Ui, col, col_num};
use super::util::Json;
use super::{App, MessageAction};
use crate::fmt;

const NOTE: &str = "MoneyKeep keeps records; it does not give tax advice.";

/// The groups of the brokerage view's action menu.
pub(crate) const GROUPS: [(&str, &str); 3] = [
    ("option", "Option…"),
    ("bond", "Bond…"),
    ("crypto", "Crypto…"),
];

/// What happened, per group: (key, plain words).
const OPTION_WHAT: [(&str, &str); 5] = [
    ("buy", "I bought contracts"),
    ("sell", "I sold contracts"),
    ("expire", "They expired"),
    ("exercise", "I exercised them"),
    ("assigned", "They were assigned to me"),
];
const BOND_WHAT: [(&str, &str); 4] = [
    ("buy", "I bought bonds"),
    ("sell", "I sold bonds"),
    ("interest", "I received interest"),
    ("redeem", "They matured or were called"),
];
const CRYPTO_WHAT: [(&str, &str); 6] = [
    ("buy", "I bought coins"),
    ("sell", "I sold coins"),
    ("swap", "I swapped one coin for another"),
    ("fee", "I paid a fee in coin"),
    ("reward", "I received a reward"),
    ("transfer", "I moved coins to another account"),
];
const TAXES: [(&str, &str); 3] = [
    ("taxable", "Taxable"),
    ("municipal", "Municipal (tax-exempt)"),
    ("treasury", "US Treasury"),
];

/// An open Option…, Bond… or Crypto… form.
#[derive(Debug, Clone, Default)]
pub(crate) struct ObcForm {
    /// `option`, `bond` or `crypto`.
    pub group: &'static str,
    /// The investment transaction being edited (revision 2), saved with `inv.update`.
    pub editing: Option<String>,
    pub confirm_delete: bool,
    pub what: String,
    /// Options bought or sold: `open` or `close`.
    pub open: String,
    pub account: String,
    pub date: String,
    pub security: String,
    pub quantity: String,
    pub price: String,
    pub commission: String,
    pub amount: String,
    pub accrued: String,
    pub to: String,
    pub to_quantity: String,
    pub value: String,
    pub fee: String,
    pub fee_quantity: String,
    pub to_account: String,
    /// Bond bought: add coupon reminders after saving.
    pub reminders: bool,
    /// A new security typed in the form.
    pub new_open: bool,
    pub n_symbol: String,
    pub n_name: String,
    pub n_underlying: String,
    pub n_right: String,
    pub n_strike: String,
    pub n_expiration: String,
    pub n_coupon: String,
    pub n_maturity: String,
    pub n_tax: usize,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub preview_error: Option<String>,
    pub error: Option<String>,
}

/// The dialog of an option still open after its expiration.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExpiryForm {
    pub account: String,
    pub security: String,
    pub symbol: String,
    pub name: String,
    pub expiration: String,
    /// Written (short) contracts.
    pub short: bool,
    /// `expire`, `exercise`, `assigned` or `closed`.
    pub choice: String,
    pub date: String,
    pub price: String,
    pub commission: String,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub error: Option<String>,
}

fn whats(group: &str) -> &'static [(&'static str, &'static str)] {
    match group {
        "option" => &OPTION_WHAT,
        "bond" => &BOND_WHAT,
        _ => &CRYPTO_WHAT,
    }
}

fn title(group: &str) -> &'static str {
    match group {
        "option" => "Option",
        "bond" => "Bond",
        _ => "Crypto",
    }
}

/// Open a group's form for a brokerage account.
pub(crate) fn open(app: &mut App, group: &str, account: &str) {
    let group = GROUPS
        .iter()
        .find(|(k, _)| *k == group)
        .map_or("option", |(k, _)| *k);
    app.inv.obc = Some(ObcForm {
        group,
        what: "buy".into(),
        open: "open".into(),
        account: account.to_owned(),
        date: fmt::date(&app.today()),
        n_right: "call".into(),
        reminders: true,
        ..ObcForm::default()
    });
}

/// The command (without `inv.`) the form's answers lead to.
fn command(f: &ObcForm) -> &'static str {
    match (f.group, f.what.as_str(), f.open.as_str()) {
        ("option", "buy", "close") => "option_cover",
        ("option", "buy", _) => "option_buy",
        ("option", "sell", "open") => "option_write",
        ("option", "sell", _) => "option_sell",
        ("option", "expire", _) => "option_expire",
        ("option", "exercise", _) => "option_exercise",
        ("option", "assigned", _) => "option_assigned",
        ("bond", "sell", _) => "bond_sell",
        ("bond", "interest", _) => "bond_interest",
        ("bond", "redeem", _) => "bond_redeem",
        ("bond", _, _) => "bond_buy",
        ("crypto", "sell", _) => "sell",
        ("crypto", "swap", _) => "crypto_swap",
        ("crypto", "fee", _) => "crypto_fee",
        ("crypto", "reward", _) => "crypto_reward",
        ("crypto", "transfer", _) => "transfer_lots",
        _ => "buy",
    }
}

fn put(a: &mut Value, key: &str, text: &str) {
    let t = text.trim().replace(',', "");
    if !t.is_empty() {
        set(a, key, json!(t));
    }
}

/// The command arguments from the form's text.
fn args(f: &ObcForm) -> Value {
    let mut a = json!({"date": fmt::date_arg_lenient(&f.date), "account": f.account});
    let cmd = command(f);
    if cmd == "crypto_swap" {
        put(&mut a, "from", &f.security);
        put(&mut a, "from_quantity", &f.quantity);
        put(&mut a, "to", &f.to);
        put(&mut a, "to_quantity", &f.to_quantity);
        put(&mut a, "value", &f.value);
        put(&mut a, "fee", &f.fee);
        return a;
    }
    put(&mut a, "security", &f.security);
    match cmd {
        "bond_buy" | "bond_sell" | "bond_redeem" => {
            put(&mut a, "units", &f.quantity);
            put(&mut a, "price", &f.price);
            put(&mut a, "commission", &f.commission);
            if cmd != "bond_redeem" {
                put(&mut a, "accrued_interest", &f.accrued);
            }
        }
        "bond_interest" => put(&mut a, "amount", &f.amount),
        "buy" | "sell" => {
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "amount", &f.amount);
            put(&mut a, "commission", &f.commission);
        }
        "crypto_fee" | "crypto_reward" => {
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "value", &f.value);
        }
        "transfer_lots" => {
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "to_account", &f.to_account);
            put(&mut a, "fee_quantity", &f.fee_quantity);
            put(&mut a, "fee_value", &f.value);
        }
        "option_expire" => put(&mut a, "quantity", &f.quantity),
        "option_exercise" | "option_assigned" => {
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "commission", &f.commission);
        }
        _ => {
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "price", &f.price);
            put(&mut a, "commission", &f.commission);
        }
    }
    a
}

/// The first answer still missing, in plain words.
fn missing(f: &ObcForm) -> Option<&'static str> {
    let empty = |s: &str| s.trim().is_empty();
    let cmd = command(f);
    if empty(&f.security) {
        return Some(match f.group {
            "option" => "Choose the option",
            "bond" => "Choose the bond",
            _ => "Choose the coin",
        });
    }
    let needs_quantity = !matches!(
        cmd,
        "option_expire" | "option_exercise" | "option_assigned" | "bond_interest" | "bond_redeem"
    );
    if needs_quantity && empty(&f.quantity) {
        return Some(match f.group {
            "option" => "Enter the number of contracts",
            "bond" => "Enter the number of bonds",
            _ => "Enter the quantity",
        });
    }
    match cmd {
        "option_buy" | "option_sell" | "option_write" | "option_cover" | "bond_buy"
        | "bond_sell"
            if empty(&f.price) =>
        {
            Some("Enter the price")
        }
        "bond_interest" if empty(&f.amount) => Some("Enter the amount received"),
        "buy" | "sell" if empty(&f.amount) => Some("Enter the total"),
        "crypto_swap" if empty(&f.to) || empty(&f.to_quantity) => {
            Some("Enter the coin and quantity received")
        }
        "crypto_swap" if empty(&f.value) => Some("Enter the value of what you received"),
        "transfer_lots" if empty(&f.to_account) => Some("Choose the account receiving them"),
        _ => None,
    }
}

fn refresh(app: &mut App, f: &mut ObcForm) {
    if let Some(m) = missing(f) {
        f.preview = None;
        f.preview_key.clear();
        f.preview_error = Some(m.to_owned());
        return;
    }
    let mut a = args(f);
    set(&mut a, "action", json!(command(f)));
    if let Some(t) = &f.editing {
        set(&mut a, "txn", json!(t));
    }
    let key = format!("{}|{a}", app.generation);
    if key == f.preview_key {
        return;
    }
    f.preview_key = key;
    match app.backend.invoke("inv.preview", a) {
        Ok(v) => {
            f.preview = Some(v);
            f.preview_error = None;
        }
        Err(e) => {
            f.preview = None;
            f.preview_error = Some(plain(&e.display()));
        }
    }
}

/// A command message with its first letter capitalised and the prefix dropped.
fn plain(msg: &str) -> String {
    let m = msg.strip_prefix("invalid arguments: ").unwrap_or(msg);
    let mut c = m.chars();
    c.next().map_or(String::new(), |x| {
        x.to_uppercase().collect::<String>() + c.as_str()
    })
}

fn field(ui: &mut Ui, label: &str, text: &mut String, hint: &str) {
    ui.form(label, |ui| {
        ui.width(240).mono().align(Align::Right);
        if !hint.is_empty() {
            ui.hint(hint);
        }
        ui.field(&format!("{label} field"), text);
    });
}

/// Securities of the form's group (symbol, label).
fn choices(app: &mut App, group: &str) -> Vec<(String, String)> {
    let kind = match group {
        "option" => "option",
        "bond" => "bond",
        _ => "crypto",
    };
    super::util::array(app.read("security.list", json!({})))
        .iter()
        .filter(|s| s.str("kind") == kind)
        .map(|s| {
            let label = if kind == "crypto" {
                format!("{} — {}", s.str("symbol"), s.str("name"))
            } else {
                s.str("name").to_owned()
            };
            (s.str("symbol").to_owned(), label)
        })
        .collect()
}

fn security_combo(ui: &mut Ui, label: &str, items: &[(String, String)], value: &mut String) {
    let opts: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
    let mut idx = items
        .iter()
        .position(|(s, _)| s.eq_ignore_ascii_case(value.trim()))
        .unwrap_or(usize::MAX);
    ui.width(300);
    if ui.combo(label, &mut idx, &opts).changed()
        && let Some((s, _)) = items.get(idx)
    {
        *value = s.clone();
    }
    if idx == usize::MAX
        && let Some(it) = ui.last()
    {
        it.value = "Choose…".into();
    }
}

/// The form's new-security fields; returns whether Create was pressed.
fn new_security(ui: &mut Ui, f: &mut ObcForm) -> bool {
    let mut create = false;
    ui.card(
        match f.group {
            "option" => "New option",
            "bond" => "New bond",
            _ => "New coin",
        },
        |ui| {
            match f.group {
                "option" => {
                    field(ui, "Underlying stock or ETF", &mut f.n_underlying, "AAPL");
                    ui.form("Call or put", |ui| {
                        ui.radio(&mut f.n_right, "call".to_owned(), "Call");
                        ui.radio(&mut f.n_right, "put".to_owned(), "Put");
                    });
                    field(ui, "Strike", &mut f.n_strike, "200");
                    ui.form("Expiration", |ui| {
                        super::register::date_field(ui, "Expiration field", &mut f.n_expiration)
                    });
                }
                "bond" => {
                    field(ui, "Symbol or CUSIP", &mut f.n_symbol, "");
                    ui.form("Name", |ui| {
                        ui.width(300).hint("Treasury 4.25% 2027");
                        ui.field("Bond name", &mut f.n_name);
                    });
                    field(ui, "Coupon rate %", &mut f.n_coupon, "4.25");
                    ui.form("Maturity", |ui| {
                        super::register::date_field(ui, "Maturity field", &mut f.n_maturity)
                    });
                    ui.form("Interest is", |ui| {
                        super::form::choice(ui, "Interest is", &mut f.n_tax, &TAXES.map(|t| t.1));
                    });
                    ui.secondary("Each bond is 1,000 of face value; coupons twice a year.");
                }
                _ => {
                    field(ui, "Symbol", &mut f.n_symbol, "BTC");
                    ui.form("Name", |ui| {
                        ui.width(300).hint("Bitcoin");
                        ui.field("Coin name", &mut f.n_name);
                    });
                }
            }
            ui.row(|ui| {
                if ui.button("Create").clicked() {
                    create = true;
                }
                if ui.button("Never mind").clicked() {
                    f.new_open = false;
                }
            });
        },
    );
    create
}

fn create_security(app: &mut App, f: &mut ObcForm) {
    let args = match f.group {
        "option" => json!({"kind": "option", "underlying": f.n_underlying.trim(),
            "right": f.n_right, "strike": f.n_strike.trim().replace(',', ""),
            "expiration": fmt::date_arg_lenient(&f.n_expiration)}),
        "bond" => json!({"kind": "bond", "symbol": f.n_symbol.trim(), "name": f.n_name.trim(),
            "coupon_rate": f.n_coupon.trim(), "maturity": fmt::date_arg_lenient(&f.n_maturity),
            "tax": TAXES.get(f.n_tax).map_or("taxable", |t| t.0)}),
        _ => json!({"kind": "crypto", "symbol": f.n_symbol.trim(), "name": f.n_name.trim()}),
    };
    match app.run("security.create", args) {
        Ok(s) => {
            f.security = s.str("symbol").to_owned();
            f.new_open = false;
            f.error = None;
        }
        Err(e) => f.error = Some(plain(&e.display())),
    }
}

/// The preview: what will be recorded and any gain.
pub(crate) fn preview_card(ui: &mut Ui, p: &Value) {
    ui.heading("This will record");
    let cols = vec![
        Column {
            weight: 3,
            ..col("Account")
        },
        Column {
            width: 130,
            ..col_num("Quantity")
        },
        Column {
            width: 130,
            ..col_num("Amount")
        },
    ];
    let t = p.at("transaction");
    let mut sentence = Vec::new();
    ui.table("", cols, |ui| {
        for (i, s) in t.list("splits").iter().enumerate() {
            let name = s.str("account_name").to_owned();
            let value = s.str("value").to_owned();
            let q = if s.str("quantity") != value {
                s.str("quantity").to_owned()
            } else {
                String::new()
            };
            sentence.push(format!("{name} {}", fmt::group(&value)));
            ui.table_row(&format!("split{i}"), false, false, |ui| {
                ui.tip(name.clone());
                ui.label(name);
                ui.mono();
                ui.amount(q);
                ui.mono();
                ui.tone(if fmt::is_negative(&value) {
                    Tone::Decrease
                } else {
                    Tone::Normal
                });
                ui.amount(fmt::group(&value));
            });
        }
    });
    // The preview in one sentence for screen readers.
    ui.named(format!("Preview: {}", sentence.join("; ")));
    ui.caption("");
    let d = p.at("disposal");
    if d.is_object() {
        let short = d.str("short_term_gain");
        let long = d.str("long_term_gain");
        let mut parts = Vec::new();
        if !fmt::is_zero(short) {
            parts.push(format!("short-term {}", fmt::group(short)));
        }
        if !fmt::is_zero(long) {
            parts.push(format!("long-term {}", fmt::group(long)));
        }
        let terms = if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        };
        ui.bold();
        ui.label(format!(
            "Proceeds {} − cost {} = {} {}{terms}",
            fmt::group(d.str("proceeds")),
            fmt::group(d.str("basis")),
            if fmt::is_negative(d.str("gain")) {
                "loss"
            } else {
                "gain"
            },
            fmt::group(d.str("gain").trim_start_matches('-')),
        ));
        ui.secondary(NOTE);
    }
}

/// Every Option…, Bond… and Crypto… form and the expiry dialog (drawn over any view).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    form_dialog(app, ui);
    expiry_dialog(app, ui);
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.inv.obc.take() else {
        return;
    };
    refresh(app, &mut f);
    let items = choices(app, f.group);
    let others: Vec<(String, String)> = if f.group == "crypto" && f.what == "transfer" {
        super::util::array(app.read("account.list", json!({})))
            .iter()
            .filter(|a| a.str("subtype") == "brokerage" && a.str("id") != f.account)
            .map(|a| (a.str("id").to_owned(), a.str("full_name").to_owned()))
            .collect()
    } else {
        Vec::new()
    };
    let (mut save, mut create) = (false, false);
    let mut cancel = ui.dialog_closed("obc-form") || ui.key("escape");
    let cmd = command(&f);
    let heading = if f.editing.is_some() {
        format!("Edit — {}", title(f.group))
    } else {
        title(f.group).to_owned()
    };
    let (mut delete, mut void) = (false, false);
    ui.dialog("obc-form", &heading, 640, |ui| {
        ui.group(|ui| {
            ui.form("What happened?", |ui| {
                let mut what = f.what.clone();
                ui.width(300);
                let labels: Vec<String> = whats(f.group).iter().map(|w| w.1.to_owned()).collect();
                let mut idx = whats(f.group).iter().position(|w| w.0 == what).unwrap_or(0);
                if ui.combo("What happened", &mut idx, &labels).changed()
                    && let Some(w) = whats(f.group).get(idx)
                {
                    what = w.0.to_owned();
                }
                f.what = what;
            });
            if f.group == "option" && matches!(f.what.as_str(), "buy" | "sell") {
                ui.form("Opening a new position or closing one?", |ui| {
                    let (o, c) = if f.what == "buy" {
                        (
                            "Opening (buy to open)",
                            "Closing contracts I wrote (buy to close)",
                        )
                    } else {
                        (
                            "Opening: I wrote them (sell to open)",
                            "Closing contracts I held (sell to close)",
                        )
                    };
                    ui.radio(&mut f.open, "open".to_owned(), o);
                    ui.radio(&mut f.open, "close".to_owned(), c);
                });
            }
            ui.form("Date", |ui| {
                super::register::date_field(ui, "Date field", &mut f.date)
            });
            let sec_label = match (f.group, cmd) {
                ("option", _) => "Option",
                ("bond", _) => "Bond",
                (_, "crypto_swap") => "Coin given",
                _ => "Coin",
            };
            ui.form(sec_label, |ui| {
                security_combo(ui, &format!("{sec_label} field"), &items, &mut f.security);
                if ui.button("New…").clicked() {
                    f.new_open = true;
                }
            });
            if f.new_open {
                create = new_security(ui, &mut f);
            }
            match cmd {
                "option_buy" | "option_sell" | "option_write" | "option_cover" => {
                    field(ui, "Contracts", &mut f.quantity, "1");
                    field(ui, "Price per share", &mut f.price, "0.00");
                    field(ui, "Commission", &mut f.commission, "0.00");
                }
                "option_expire" => {
                    field(ui, "Contracts", &mut f.quantity, "all");
                }
                "option_exercise" | "option_assigned" => {
                    field(ui, "Contracts", &mut f.quantity, "all");
                    field(ui, "Commission", &mut f.commission, "0.00");
                }
                "bond_buy" | "bond_sell" => {
                    field(ui, "Bonds", &mut f.quantity, "number of bonds");
                    field(ui, "Price (% of face)", &mut f.price, "98.50");
                    field(
                        ui,
                        if cmd == "bond_buy" {
                            "Accrued interest paid"
                        } else {
                            "Accrued interest received"
                        },
                        &mut f.accrued,
                        "0.00",
                    );
                    field(ui, "Commission", &mut f.commission, "0.00");
                    if cmd == "bond_buy" && f.editing.is_none() {
                        ui.form("", |ui| ui.switch(&mut f.reminders, "Add coupon reminders"));
                    }
                }
                "bond_interest" => field(ui, "Amount received", &mut f.amount, "0.00"),
                "bond_redeem" => {
                    field(ui, "Bonds", &mut f.quantity, "all");
                    field(ui, "Price (% of face)", &mut f.price, "100");
                }
                "buy" | "sell" => {
                    field(ui, "Quantity", &mut f.quantity, "0.00000000");
                    field(
                        ui,
                        if cmd == "buy" {
                            "Total paid"
                        } else {
                            "Total received"
                        },
                        &mut f.amount,
                        "0.00",
                    );
                    field(ui, "Fee included", &mut f.commission, "0.00");
                }
                "crypto_swap" => {
                    field(ui, "Quantity given", &mut f.quantity, "0.00000000");
                    ui.form("Coin received", |ui| {
                        security_combo(ui, "Coin received field", &items, &mut f.to)
                    });
                    field(ui, "Quantity received", &mut f.to_quantity, "0.00000000");
                    field(ui, "Value of what you received", &mut f.value, "0.00");
                    field(ui, "Fee paid in cash", &mut f.fee, "0.00");
                }
                "crypto_fee" | "crypto_reward" => {
                    field(ui, "Quantity", &mut f.quantity, "0.00000000");
                    field(ui, "Value", &mut f.value, "latest price");
                }
                _ => {
                    field(ui, "Quantity", &mut f.quantity, "0.00000000");
                    ui.form("To account", |ui| {
                        security_combo(ui, "To account field", &others, &mut f.to_account)
                    });
                    field(ui, "Network fee in coin", &mut f.fee_quantity, "optional");
                    field(ui, "Value of the fee", &mut f.value, "latest price");
                }
            }
        });
        match (&f.preview, &f.preview_error) {
            (Some(p), _) => preview_card(ui, p),
            (None, Some(e)) => ui.secondary(e.clone()),
            (None, None) => {}
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.confirm_delete {
            ui.label("Delete this transaction? You can undo it afterwards.");
            ui.row(|ui| {
                if ui.button("Delete transaction").clicked() {
                    delete = true;
                }
                if ui.button("Keep it").clicked() {
                    f.confirm_delete = false;
                }
            });
        }
        ui.actions(|ui| {
            if f.editing.is_some() {
                if ui.button("Delete…").clicked() {
                    f.confirm_delete = true;
                }
                if ui.button("Void").clicked() {
                    void = true;
                }
                ui.spacer();
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.enabled(f.preview.is_some());
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if create {
        create_security(app, &mut f);
    }
    if cancel {
        return;
    }
    if let Some(t) = f.editing.clone()
        && (delete || void)
    {
        let (c, done) = if delete {
            ("txn.delete", "deleted")
        } else {
            ("txn.void", "voided")
        };
        match app.run(c, json!({"id": t})) {
            Ok(_) => {
                app.notify_with(format!("{} {done}.", title(f.group)), MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(plain(&e.display())),
        }
    }
    if save && f.preview.is_some() {
        let result = match &f.editing {
            Some(t) => {
                let mut a = args(&f);
                set(&mut a, "txn", json!(t));
                set(&mut a, "action", json!(cmd));
                app.run("inv.update", a)
            }
            None => app.run(&format!("inv.{cmd}"), args(&f)),
        };
        match result {
            Ok(_) if f.editing.is_some() => {
                app.notify_with(format!("{} saved.", title(f.group)), MessageAction::Undo);
                return;
            }
            Ok(_) => {
                let mut text = format!("{} recorded.", title(f.group));
                if cmd == "bond_buy" && f.reminders {
                    match app.run(
                        "inv.bond_reminders",
                        json!({"account": f.account, "security": f.security.trim(),
                               "today": app.today()}),
                    ) {
                        Ok(r) => {
                            text =
                                format!("Bond recorded, with {} coupon reminders.", r.at("count"));
                        }
                        Err(e) => text = format!("Bond recorded. {}", plain(&e.display())),
                    }
                }
                app.notify_with(text, MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(plain(&e.display())),
        }
    }
    app.inv.obc = Some(f);
}

// ----- editing from the Activity tab (revision 2) ---------------------------------------

/// The form (group, what happened, opening or closing) that records `action`; crypto
/// buys and sells are the core actions on a coin.
fn form_of(action: &str, crypto: bool) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match action {
        "option_buy" => ("option", "buy", "open"),
        "option_cover" => ("option", "buy", "close"),
        "option_write" => ("option", "sell", "open"),
        "option_sell" => ("option", "sell", "close"),
        "option_expire" => ("option", "expire", "open"),
        "option_exercise" => ("option", "exercise", "open"),
        "option_assigned" => ("option", "assigned", "open"),
        "bond_buy" => ("bond", "buy", "open"),
        "bond_sell" => ("bond", "sell", "open"),
        "bond_interest" => ("bond", "interest", "open"),
        "bond_redeem" => ("bond", "redeem", "open"),
        "crypto_swap" => ("crypto", "swap", "open"),
        "crypto_fee" => ("crypto", "fee", "open"),
        "crypto_reward" => ("crypto", "reward", "open"),
        "transfer_lots" => ("crypto", "transfer", "open"),
        "buy" if crypto => ("crypto", "buy", "open"),
        "sell" if crypto => ("crypto", "sell", "open"),
        _ => return None,
    })
}

const HAND_EDITED: &str = "This transaction was changed by hand, so it opens as splits.";

/// The (account, value, quantity) of each split, sorted, for comparing transactions.
fn split_key(t: &Value) -> Vec<(String, String, String)> {
    let norm = |s: &str| {
        s.parse::<mk_core::Decimal>()
            .map(|d| d.normalize().to_string())
            .unwrap_or_else(|_| s.to_owned())
    };
    let mut v: Vec<(String, String, String)> = t
        .list("splits")
        .iter()
        .map(|s| {
            (
                s.str("account").to_owned(),
                norm(s.str("value")),
                norm(s.str("quantity")),
            )
        })
        .collect();
    v.sort();
    v
}

/// Activity row click (or Enter) on a transaction recorded by an option, bond or
/// crypto action: open its form pre-filled, titled "Edit — …". Returns false when
/// the row belongs to the core action forms (a stock's buy, or a coin sold from
/// chosen lots, which only the Sell form can show).
pub(crate) fn open_edit(app: &mut App, txn: &str, account: &str) -> bool {
    let Some(m) = app.read("inv.get", json!({"txn": txn})) else {
        return false;
    };
    let a = m.at("args").clone();
    let action = m.str("action").to_owned();
    let symbol_of = |app: &mut App, r: &str| -> (String, String) {
        app.backend
            .invoke("security.get", json!({"security": r}))
            .map(|s| (s.str("symbol").to_owned(), s.str("kind").to_owned()))
            .unwrap_or_else(|_| (r.to_owned(), String::new()))
    };
    let sec_ref = if a.str("from").is_empty() {
        a.str("security").to_owned()
    } else {
        a.str("from").to_owned()
    };
    let (security, kind) = symbol_of(app, &sec_ref);
    let Some((group, what, open)) = form_of(&action, kind == "crypto") else {
        return false;
    };
    if matches!(action.as_str(), "buy" | "sell") && !a.list("lots").is_empty() {
        return false;
    }
    let either = |k1: &str, k2: &str| -> String {
        let v = a.str(k1);
        if v.is_empty() {
            m.str(k2).to_owned()
        } else {
            v.to_owned()
        }
    };
    let to = if a.str("to").is_empty() {
        String::new()
    } else {
        symbol_of(app, a.str("to")).0
    };
    let to_account = if a.str("to_account").is_empty() {
        String::new()
    } else {
        app.backend
            .invoke("account.get", json!({"account": a.str("to_account")}))
            .map(|x| x.str("id").to_owned())
            .unwrap_or_else(|_| a.str("to_account").to_owned())
    };
    let quantity = [a.str("quantity"), a.str("units"), a.str("from_quantity")]
        .into_iter()
        .find(|q| !q.is_empty())
        .unwrap_or_default()
        .to_owned();
    let date = if a.str("date").is_empty() {
        m.str("date")
    } else {
        a.str("date")
    };
    let mut f = ObcForm {
        group,
        editing: Some(txn.to_owned()),
        what: what.to_owned(),
        open: open.to_owned(),
        account: account.to_owned(),
        date: fmt::date(date),
        security,
        quantity,
        price: a.str("price").to_owned(),
        commission: a.str("commission").to_owned(),
        accrued: a.str("accrued_interest").to_owned(),
        to,
        to_quantity: a.str("to_quantity").to_owned(),
        fee: a.str("fee").to_owned(),
        fee_quantity: a.str("fee_quantity").to_owned(),
        to_account,
        n_right: "call".into(),
        ..ObcForm::default()
    };
    match action.as_str() {
        // The figures worked out when recorded, so the form shows what was posted.
        "buy" | "sell" | "bond_interest" => f.amount = either("amount", "amount"),
        "crypto_fee" | "crypto_reward" => f.value = either("value", "amount"),
        "transfer_lots" => f.value = a.str("fee_value").to_owned(),
        _ => f.value = a.str("value").to_owned(),
    }
    // A transaction the form cannot show as it is (changed by hand, or from choices
    // the form does not offer) opens as splits instead.
    let mut p = args(&f);
    set(&mut p, "action", json!(command(&f)));
    set(&mut p, "txn", json!(txn));
    let now = app.read("txn.get", json!({"id": txn}));
    let same = missing(&f).is_none()
        && app
            .backend
            .invoke("inv.preview", p)
            .ok()
            .zip(now)
            .is_some_and(|(p, t)| split_key(p.at("transaction")) == split_key(&t));
    if same {
        app.inv.obc = Some(f);
    } else {
        open_as_splits(app, txn, &m);
    }
    true
}

/// The register's split view of an investment transaction, with the note.
fn open_as_splits(app: &mut App, txn: &str, m: &Value) {
    let register = [m.str("sec_account"), m.str("other_account")]
        .into_iter()
        .find(|a| !a.is_empty())
        .map(str::to_owned);
    app.notify(HAND_EDITED);
    if let Some(account) = register {
        app.register.pending_select = Some(txn.to_owned());
        app.register.pending_splits = true;
        app.navigate(super::View::Register { account });
    }
}

// ----- expired options and matured bonds ------------------------------------------------

/// Open the dialog of an option open after its expiration (Home → Attention).
pub(crate) fn open_expiry(app: &mut App, account: &str, security: &str) {
    let Some(s) = app.read("security.get", json!({"security": security})) else {
        return;
    };
    let exp = s.at("option").str("expiration").to_owned();
    let held = app
        .read(
            "inv.holdings",
            json!({"accounts": [account], "as_of": app.today()}),
        )
        .and_then(|h| h.at("accounts").as_array().and_then(|a| a.first().cloned()))
        .map(|a| {
            a.list("holdings")
                .iter()
                .find(|x| x.str("security") == s.str("id"))
                .map(|x| x.str("quantity").to_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    app.inv.expiry = Some(ExpiryForm {
        account: account.to_owned(),
        security: s.str("id").to_owned(),
        symbol: s.str("symbol").to_owned(),
        name: s.str("name").to_owned(),
        date: fmt::date(&exp),
        expiration: exp,
        short: fmt::is_negative(&held),
        choice: "expire".into(),
        ..ExpiryForm::default()
    });
}

/// Open the Bond… form on the redemption of a matured bond.
pub(crate) fn open_redeem(app: &mut App, account: &str, security: &str) {
    let s = app.read("security.get", json!({"security": security}));
    open(app, "bond", account);
    if let (Some(f), Some(s)) = (app.inv.obc.as_mut(), s) {
        f.what = "redeem".into();
        f.security = s.str("symbol").to_owned();
        f.date = fmt::date(s.at("bond").str("maturity"));
        f.price = "100".into();
    }
}

fn expiry_command(e: &ExpiryForm) -> &'static str {
    match (e.choice.as_str(), e.short) {
        ("exercise", _) => "option_exercise",
        ("assigned", _) => "option_assigned",
        ("closed", true) => "option_cover",
        ("closed", false) => "option_sell",
        _ => "option_expire",
    }
}

fn expiry_args(e: &ExpiryForm) -> Value {
    let mut a = json!({"account": e.account, "security": e.security,
                       "date": fmt::date_arg_lenient(&e.date)});
    match expiry_command(e) {
        "option_sell" | "option_cover" => {
            // Every contract still open, at the price typed.
            put(&mut a, "price", &e.price);
            put(&mut a, "commission", &e.commission);
        }
        "option_exercise" | "option_assigned" => put(&mut a, "commission", &e.commission),
        _ => {}
    }
    a
}

fn expiry_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut e) = app.inv.expiry.take() else {
        return;
    };
    let cmd = expiry_command(&e);
    // The contracts still open (all of them) for selling or buying back.
    let mut a = expiry_args(&e);
    if matches!(cmd, "option_sell" | "option_cover") {
        let q = app
            .backend
            .invoke(
                "inv.holdings",
                json!({"accounts": [e.account], "as_of": fmt::date_arg_lenient(&e.date)}),
            )
            .ok()
            .and_then(|h| h.at("accounts").as_array().and_then(|x| x.first().cloned()))
            .and_then(|acc| {
                acc.list("holdings")
                    .iter()
                    .find(|x| x.str("security") == e.security)
                    .map(|x| x.str("quantity").trim_start_matches('-').to_owned())
            })
            .unwrap_or_default();
        put(&mut a, "quantity", &q);
    }
    let ready = !matches!(cmd, "option_sell" | "option_cover") || !e.price.trim().is_empty();
    let mut pa = a.clone();
    set(&mut pa, "action", json!(cmd));
    let key = format!("{}|{pa}", app.generation);
    if ready && key != e.preview_key {
        e.preview_key = key;
        match app.backend.invoke("inv.preview", pa) {
            Ok(v) => {
                e.preview = Some(v);
                e.error = None;
            }
            Err(err) => {
                e.preview = None;
                e.error = Some(plain(&err.display()));
            }
        }
    } else if !ready {
        e.preview = None;
        e.preview_key.clear();
    }
    let mut save = false;
    let mut cancel = ui.dialog_closed("option-expiry") || ui.key("escape");
    ui.dialog("option-expiry", "An option has expired", 660, |ui| {
        ui.bold();
        ui.label(e.name.clone());
        ui.secondary(format!(
            "It expired on {} and is still open in this account. What happened?",
            fmt::date(&e.expiration)
        ));
        let short = e.short;
        ui.radio(&mut e.choice, "expire".to_owned(), "Expired worthless");
        ui.enabled(!short);
        ui.radio(&mut e.choice, "exercise".to_owned(), "Exercised");
        ui.enabled(short);
        ui.radio(&mut e.choice, "assigned".to_owned(), "Assigned");
        ui.radio(
            &mut e.choice,
            "closed".to_owned(),
            "Closed earlier (sold or bought back)",
        );
        ui.group(|ui| {
            ui.form("Date", |ui| {
                super::register::date_field(ui, "Date field", &mut e.date)
            });
            if e.choice == "closed" {
                field(ui, "Price per share", &mut e.price, "0.00");
            }
            if e.choice != "expire" {
                field(ui, "Commission", &mut e.commission, "0.00");
            }
        });
        if let Some(p) = &e.preview {
            preview_card(ui, p);
        }
        if let Some(m) = &e.error {
            ui.error_text(m.clone());
        }
        ui.actions(|ui| {
            if ui.button("Not now").clicked() {
                cancel = true;
            }
            ui.enabled(e.preview.is_some());
            if ui.primary("Record").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save && e.preview.is_some() {
        match app.run(&format!("inv.{cmd}"), a) {
            Ok(_) => {
                app.notify_with(format!("{} recorded.", e.symbol), MessageAction::Undo);
                return;
            }
            Err(err) => e.error = Some(plain(&err.display())),
        }
    }
    app.inv.expiry = Some(e);
}

/// The Details column of the holdings table.
pub(crate) fn holding_details(row: &Value) -> String {
    match row.str("kind") {
        "option" => {
            // The name already says the underlying, expiration, strike and right.
            let mut t = match row.at("days_left").as_i64() {
                Some(0) => "Expired".to_owned(),
                Some(1) => "1 day left".to_owned(),
                Some(n) => format!("{n} days left"),
                None => String::new(),
            };
            if row.at("short").as_bool() == Some(true) {
                t.push_str(" · written");
            }
            t
        }
        "bond" if row.at("maturity").is_string() => format!(
            "{}% · face {} · {}",
            row.str("coupon_rate"),
            fmt::group(row.str("face")),
            fmt::date(row.str("maturity"))
        ),
        _ => String::new(),
    }
}

/// Open a review state for screenshots: `form|<group>|<what>|<open>|<security>|…`,
/// `expiry|<account>|<security>`, `edit|<txn>|<account>` or
/// `report|<name>|<from>|<to>`.
impl App {
    pub fn open_obc_review(&mut self, spec: &str) {
        let parts: Vec<&str> = spec.split('|').collect();
        let get = |i: usize| parts.get(i).copied().unwrap_or("").to_owned();
        match parts.first().copied() {
            Some("expiry") => open_expiry(self, &get(1), &get(2)),
            // `edit|<txn>|<account>`: the transaction's form, from the Activity tab.
            Some("edit") => {
                self.inv.tab = 1;
                open_edit(self, &get(1), &get(2));
            }
            // `report|<name>|<from>|<to>`.
            Some("report") => super::reports::open(self, &get(1), "custom", &get(2), &get(3)),
            Some("form") => {
                open(self, &get(1), &get(2));
                if let Some(f) = self.inv.obc.as_mut() {
                    f.what = get(3);
                    if !get(4).is_empty() {
                        f.open = get(4);
                    }
                    f.security = get(5);
                    f.quantity = get(6);
                    f.price = get(7);
                    f.commission = get(8);
                    f.accrued = get(9);
                    f.to = get(10);
                    f.to_quantity = get(11);
                    f.value = get(12);
                    if !get(13).is_empty() {
                        f.date = get(13);
                    }
                }
            }
            _ => {}
        }
    }
}
