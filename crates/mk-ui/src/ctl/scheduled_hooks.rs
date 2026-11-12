//! Entering and skipping a scheduled occurrence from the register (the
//! Scheduled screen owns the forms).

use serde_json::{Value, json};

use super::util::Json;
use super::{App, MessageAction};
use crate::fmt;

/// Enter an occurrence now (an estimate opens the Scheduled screen's form).
pub(crate) fn enter(app: &mut App, o: &Value) {
    if o.str("amount_kind") == "estimate" {
        super::scheduled::enter_estimate(app, o);
        return;
    }
    match app.run(
        "schedule.enter",
        json!({"schedule": o.str("schedule"), "date": o.str("nominal")}),
    ) {
        Ok(t) => app.notify(format!(
            "Entered {} on {}",
            o.str("name"),
            fmt::date(t.str("date"))
        )),
        Err(e) => app.notify_error(&e),
    }
}

pub(crate) fn skip(app: &mut App, schedule: &str, nominal: &str) {
    match app.run(
        "schedule.skip",
        json!({"schedule": schedule, "date": nominal}),
    ) {
        Ok(_) => app.notify_with("Skipped —", MessageAction::Undo),
        Err(e) => app.notify_error(&e),
    }
}

/// Repeat this…: a new schedule from the transaction (Scheduled screen form).
pub(crate) fn repeat_txn(app: &mut App, txn: &str) {
    super::scheduled::repeat_txn(app, txn);
}
