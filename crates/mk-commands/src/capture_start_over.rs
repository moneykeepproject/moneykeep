//! Start over (mobile-capture.md revision 17): remove everything phone capture set
//! up — in Cloudflare, on this computer and in MoneyKeep — so someone stuck halfway,
//! or who deleted pieces by hand, can begin again from nothing. Books and backups
//! are never touched.
//!
//! `capture.start_over_preview` changes nothing: it inspects this computer, asks
//! Cloudflare (with the saved token, if any) and the capture server, and lists what
//! would be removed in three groups and what is kept. `capture.start_over` (typed
//! `START OVER`) then removes:
//!
//! 1. **In Cloudflare**, only what MoneyKeep recorded as created by its setup, by id,
//!    with revision 16's removal steps (a setup-created tunnel is deleted, otherwise
//!    only MoneyKeep's ingress rule is removed and the catch-all kept). A step that
//!    fails, or every step when Cloudflare can't be reached or the token no longer
//!    works, is listed as left with its dashboard place, and the rest goes on.
//! 2. **On this computer**, in one elevated step: the capture server's service, the
//!    `cloudflared` service when MoneyKeep installed it or its tunnel is gone
//!    (revision 16; one in another account, revision 16a, or of a live tunnel is
//!    kept), the Wi-Fi firewall rule and the capture server's data folder.
//! 3. **In MoneyKeep**: the saved Cloudflare token, the setup state, the server
//!    address and desktop token, the Capture PIN record — one audited write.
//!
//! Sources: Cloudflare API reference — "List Cloudflare Tunnels"
//! (`GET /accounts/{account_id}/cfd_tunnel`, `is_deleted=false` lists the tunnels
//! not deleted,
//! <https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/subresources/cloudflared/methods/list/>)
//! and "Verify Token" (`GET /user/tokens/verify`,
//! <https://developers.cloudflare.com/api/resources/user/subresources/tokens/methods/verify/>).

use mk_capture_api as api;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::capture::MODULE;
use crate::capture_setup::{
    self as cs, HostOp, HostReport, Mem, Op, PERMISSION_DECLINED, RStep, ServiceState,
    ServiceTunnel, SetupState, StartOverRequest, UndoFailure, cf, cf_answer, coded, job,
};
use crate::capture_wifi::WifiRec;
use crate::connect::{Connector, FetchError, HttpMethod, HttpResponse, NetStep};
use crate::{CommandError, CommandResult, Registry};

/// What a Start over that did not finish on this computer left (book setting).
pub(crate) const RECORD_KEY: &str = "capture.start_over";
const MAX_JSON: usize = 256 * 1024;

pub mod text {
    /// What the confirmation field must hold (case-insensitive).
    pub const CONFIRM_WORD: &str = "START OVER";
    pub const CLEARED: &str = "Phone capture is cleared. Set up phone capture to begin again.";
    pub const LEFT: &str = "Phone capture is cleared in MoneyKeep, but some things are left. Each one says where to finish by hand.";
    /// Shown before the permission prompt appears.
    pub const PERMISSION: &str = "Windows will ask for permission once, for the part on this computer (the services, the firewall rule and the capture server's data folder).";
    pub const DECLINED: &str = "Windows permission was not given, so nothing was changed on this computer. Press Try again to remove it.";
    pub const NO_TOKEN: &str = "MoneyKeep has no saved Cloudflare API token, so it can't remove these. Delete them by hand in the Cloudflare dashboard:";
    pub const TOKEN_REFUSED: &str = "Cloudflare no longer accepts the saved API token, so MoneyKeep can't remove these. Delete them by hand in the Cloudflare dashboard:";
    pub const UNREACHABLE: &str = "Cloudflare can't be reached, so MoneyKeep can't remove these now. Delete them by hand in the Cloudflare dashboard:";
    pub const NOTHING_CLOUDFLARE: &str =
        "Nothing in Cloudflare is recorded as made by MoneyKeep's setup.";
    pub const PHONES: &str = "Phones still holding the old pairing show \"This phone is no longer paired\" next time and keep their Outbox; pairing again sends it.";
    pub const NOT_CONFIRMED: &str = "Type START OVER to confirm.";
    /// Where to finish the computer part by hand.
    pub const COMPUTER_WHERE: &str = "This computer: Settings > Phone capture > Start over… > Try again, or see the help: Remove phone capture";
    pub const EARLIER: &str = "Left by an earlier Start over";
    /// "3 expenses from phones not yet collected will be lost — collect them first?"
    pub fn uncollected(n: usize) -> String {
        let s = if n == 1 { "" } else { "s" };
        format!("{n} expense{s} from phones not yet collected will be lost — collect them first?")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartOverArgs {
    /// `START OVER` (any case): the owner confirmed.
    confirm: String,
}

/// Something left, with where to remove it by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Left {
    pub what: String,
    #[serde(rename = "where")]
    pub place: String,
}

/// What an unfinished Start over left (kept so Try again finishes it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Record {
    /// The part on this computer is still to do.
    #[serde(default)]
    pub computer_left: bool,
    /// Its `cloudflared` service is to be removed (MoneyKeep installed it, or its
    /// tunnel was gone), and the tunnel it ran.
    #[serde(default)]
    pub cloudflared: bool,
    #[serde(default)]
    pub cloudflared_tunnel: Option<String>,
    #[serde(default)]
    pub firewall_rule: Option<String>,
    /// Cloudflare items that were left, to delete by hand.
    #[serde(default)]
    pub cloudflare_left: Vec<Left>,
}

fn record(ctx: &crate::Ctx<'_>) -> CommandResult<Record> {
    Ok(ctx
        .tx
        .get_setting(MODULE, RECORD_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// Which answer of the reads is which.
#[derive(Debug, Clone, Default)]
pub(crate) struct Reads {
    host: HostReport,
    verify: Option<usize>,
    tunnels: Option<usize>,
    lookup: Option<usize>,
    status: Option<usize>,
    has_token: bool,
}

/// One Start over, planned from what MoneyKeep recorded and what it found.
#[derive(Debug, Clone, Default)]
pub(crate) struct Run {
    /// Cloudflare items MoneyKeep made: (step, what, where).
    cloudflare: Vec<(RStep, String, String)>,
    /// Why Cloudflare's part can't run (they are then listed to delete by hand).
    cf_problem: Option<String>,
    /// This computer: what will be removed, in words.
    computer: Vec<String>,
    request: StartOverRequest,
    moneykeep: Vec<String>,
    kept: Vec<String>,
    uncollected: Option<usize>,
    /// Cloudflare items an earlier Start over left.
    earlier: Vec<Left>,
    os: String,
}

impl Run {
    fn preview(&self) -> Value {
        let items: Vec<Value> = self
            .cloudflare
            .iter()
            .map(|(_, what, place)| json!({"what": what, "where": place}))
            .chain(
                self.earlier
                    .iter()
                    .map(|l| json!({"what": l.what, "where": l.place, "earlier": true})),
            )
            .collect();
        let by_hand = self.cf_problem.is_some() || !self.earlier.is_empty();
        json!({
            "cloudflare": {
                "items": items,
                "problem": self.cf_problem,
                "by_hand": by_hand,
                "note": if items.is_empty() { Some(text::NOTHING_CLOUDFLARE) } else { None },
            },
            "computer": self.computer,
            "permission": (self.os == "windows" && self.request.needs_permission())
                .then_some(text::PERMISSION),
            "moneykeep": self.moneykeep,
            "kept": self.kept,
            "uncollected": self.uncollected,
            "uncollected_text": self.uncollected.filter(|n| *n > 0).map(text::uncollected),
            "confirm_word": text::CONFIRM_WORD,
        })
    }
}

/// The confirmation typed: `START OVER`, any case, spaces around ignored.
pub fn confirmed(typed: &str) -> bool {
    typed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .eq_ignore_ascii_case(text::CONFIRM_WORD)
}

/// The Cloudflare items MoneyKeep recorded as made by its setup.
fn cloudflare_items(st: &SetupState) -> Vec<(RStep, String, String)> {
    let tunnel_created = st.tunnel.as_ref().is_some_and(|t| t.created);
    let rule = st.tunnel.as_ref().is_some_and(|t| !t.created) && st.ingress;
    [
        (st.app.is_some(), RStep::App),
        (st.allow_policy.is_some(), RStep::AllowPolicy),
        (st.service_policy.is_some(), RStep::ServicePolicy),
        (st.service_token.is_some(), RStep::ServiceToken),
        (st.dns_record.is_some(), RStep::Dns),
        (tunnel_created, RStep::Tunnel),
        (rule, RStep::WriteIngress),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, step)| {
        (
            step,
            cs::undo_what(st, step, None),
            cs::undo_where(step).to_owned(),
        )
    })
    .collect()
}

/// Plan a Start over from the records and the answers.
#[allow(clippy::too_many_arguments)]
fn assess(
    st: &SetupState,
    wifi: &WifiRec,
    rec: &Record,
    conf: bool,
    reads: &Reads,
    responses: &[Result<HttpResponse, FetchError>],
) -> Run {
    let host = &reads.host;
    let answer = |i: Option<usize>| i.and_then(|i| responses.get(i));
    let mut run = Run {
        cloudflare: cloudflare_items(st),
        earlier: rec.cloudflare_left.clone(),
        os: host.os.clone(),
        ..Run::default()
    };
    // Can Cloudflare's part run?
    let verify = cf_answer(answer(reads.verify));
    run.cf_problem = if !reads.has_token {
        Some(text::NO_TOKEN.to_owned())
    } else if verify.ok {
        None
    } else if matches!(verify.status, 400 | 401 | 403) {
        Some(text::TOKEN_REFUSED.to_owned())
    } else {
        Some(text::UNREACHABLE.to_owned())
    };
    if run.cloudflare.is_empty() {
        run.cf_problem = None;
    }
    // This computer.
    if host.server != ServiceState::NotInstalled {
        run.computer
            .push("The capture server service (moneykeep-server)".to_owned());
        run.request.server = true;
    }
    if host.cloudflared_service.is_some() {
        let runs = host.cloudflared_tunnel.clone().filter(|t| !t.is_empty());
        let setup_tunnel = st
            .tunnel
            .as_ref()
            .filter(|t| t.created)
            .map(|t| t.id.clone());
        let installed = st.installed_cloudflared || rec.cloudflared;
        let ours = runs.is_some() && runs == setup_tunnel;
        let state = cs::service_tunnel_state(st.account_id(), host, answer(reads.lookup));
        let (remove, words) = if installed {
            (
                true,
                "The cloudflared service (MoneyKeep installed it)".to_owned(),
            )
        } else if ours {
            (
                true,
                "The cloudflared service (it runs the tunnel MoneyKeep's setup made)".to_owned(),
            )
        } else {
            match state {
                ServiceTunnel::Gone(_) => (
                    true,
                    "The cloudflared service (its tunnel no longer exists in your Cloudflare account)"
                        .to_owned(),
                ),
                ServiceTunnel::OtherAccount(_) => (
                    false,
                    "The cloudflared service for a tunnel in another Cloudflare account".to_owned(),
                ),
                ServiceTunnel::Exists(name) if !name.is_empty() => (
                    false,
                    format!("The cloudflared service for your tunnel \"{name}\""),
                ),
                ServiceTunnel::Exists(_) | ServiceTunnel::Unknown => (
                    false,
                    "The cloudflared service (MoneyKeep did not install it)".to_owned(),
                ),
            }
        };
        if remove {
            run.computer.push(words);
            run.request.cloudflared = true;
            run.request.cloudflared_tunnel = runs;
        } else {
            run.kept.push(words);
        }
    }
    let rule = wifi
        .firewall_rule
        .clone()
        .or_else(|| rec.firewall_rule.clone())
        .or_else(|| (host.os == "windows").then(|| api::FIREWALL_RULE.to_owned()));
    if let Some(name) = rule {
        run.computer
            .push(format!("The Wi-Fi firewall rule \"{name}\""));
        run.request.firewall_rule = Some(name);
    }
    run.computer.push(
        "The capture server's data folder (certificates, paired phones, the inbox)".to_owned(),
    );
    run.request.data_folder = true;
    // In MoneyKeep.
    if wifi.pin_set {
        run.moneykeep.push("The Capture PIN".to_owned());
    }
    if st.token_ref.is_some() {
        run.moneykeep
            .push("The saved Cloudflare API token".to_owned());
    }
    run.moneykeep.push(
        "The setup state (Wi-Fi and Cloudflare), the capture server's address and desktop token"
            .to_owned(),
    );
    run.uncollected = answer(reads.status)
        .and_then(|r| r.as_ref().ok())
        .filter(|r| r.status == 200)
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .and_then(|v| v.get("waiting").and_then(Value::as_u64))
        .and_then(|n| usize::try_from(n).ok());
    if let Some(n) = run.uncollected.filter(|n| *n > 0) {
        let s = if n == 1 { "" } else { "s" };
        run.moneykeep
            .push(format!("{n} expense{s} from phones not yet collected"));
    }
    if conf {
        run.moneykeep.push("The list of paired phones".to_owned());
    }
    // Kept.
    let mut kept = vec!["Your books".to_owned(), "Your backups".to_owned()];
    if let Some(t) = st.tunnel.as_ref().filter(|t| !t.created) {
        kept.push(format!(
            "Your tunnel \"{}\" (only MoneyKeep's rule for {} is removed)",
            t.name, st.hostname
        ));
    }
    let tunnels = cf_answer(answer(reads.tunnels));
    if tunnels.ok {
        for t in cs::list_of(&tunnels.result) {
            let id = cs::str_of(&t, "id");
            let ours = st.tunnel.as_ref().is_some_and(|x| x.id == id);
            let deleted = t.get("deleted_at").is_some_and(|d| !d.is_null());
            if !ours && !deleted && !id.is_empty() {
                kept.push(format!(
                    "Your tunnel \"{}\" in Cloudflare",
                    cs::str_of(&t, "name")
                ));
            }
        }
    }
    kept.append(&mut run.kept);
    kept.push(
        "Cloudflare DNS records, Access applications and other things MoneyKeep didn't make"
            .to_owned(),
    );
    kept.push("The MoneyKeep program".to_owned());
    run.kept = kept;
    run
}

impl Registry {
    /// `capture.start_over_preview` and `capture.start_over`: inspect this computer
    /// first (the tunnel a `cloudflared` service runs decides the reads).
    pub(crate) fn start_over_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        name: &str,
        args: Value,
    ) -> CommandResult<NetStep> {
        let preview = name == "capture.start_over_preview";
        if preview {
            let _: crate::builtin::NoArgs = crate::parse_args(args)?;
        } else {
            let a: StartOverArgs = crate::parse_args(args)?;
            if !confirmed(&a.confirm) {
                return Err(coded("confirm_needed", text::NOT_CONFIRMED));
            }
        }
        let mem = Mem::default();
        if c.host.is_none() {
            // A front end that cannot inspect this computer still clears the rest.
            let host = HostReport {
                notes: vec!["This MoneyKeep cannot inspect this computer.".into()],
                ..HostReport::default()
            };
            return self.start_over_reads(store, actor, c, mem, preview, host);
        }
        job(
            c,
            Op::StartOverHost { mem, preview },
            Vec::new(),
            Some(HostOp::Inspect),
        )
    }

    /// The reads: the token (and the account's tunnels, to name the ones kept),
    /// the tunnel a `cloudflared` service here runs, the capture server's waiting
    /// count.
    pub(crate) fn start_over_reads(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        mut mem: Mem,
        preview: bool,
        host: HostReport,
    ) -> CommandResult<NetStep> {
        let (st, token, ep) = self.setup_read(store, actor, |ctx| {
            let st = cs::load(ctx)?;
            let token = match &st.token_ref {
                Some(r) => cs::get_secret(ctx, r).ok().flatten(),
                None => None,
            };
            let ep = crate::capture::endpoint(ctx, c).ok();
            Ok((st, token, ep))
        })?;
        let mut reqs = Vec::new();
        let mut reads = Reads {
            has_token: token.is_some(),
            ..Reads::default()
        };
        if let Some(t) = &token {
            reads.verify = Some(reqs.len());
            reqs.push(cf(t, HttpMethod::Get, "/user/tokens/verify", &[], "", None).req);
            let acc = st.account_id().to_owned();
            if !acc.is_empty() {
                reads.tunnels = Some(reqs.len());
                reqs.push(
                    cf(
                        t,
                        HttpMethod::Get,
                        "/accounts/{account}/cfd_tunnel",
                        &[("account", acc.as_str())],
                        "?is_deleted=false",
                        None,
                    )
                    .req,
                );
                let other = host
                    .cloudflared_account
                    .as_deref()
                    .is_some_and(|a| !a.is_empty() && a != acc);
                if let Some(id) = host
                    .cloudflared_tunnel
                    .as_deref()
                    .filter(|id| !id.is_empty() && !other && host.cloudflared_service.is_some())
                {
                    reads.lookup = Some(reqs.len());
                    reqs.push(
                        cf(
                            t,
                            HttpMethod::Get,
                            "/accounts/{account}/cfd_tunnel/{tunnel}",
                            &[("account", acc.as_str()), ("tunnel", id)],
                            "",
                            None,
                        )
                        .req,
                    );
                }
            }
            mem.token.clone_from(t);
        }
        if let Some(ep) = ep {
            reads.status = Some(reqs.len());
            reqs.push(ep.req(HttpMethod::Get, "/status", None, MAX_JSON));
        }
        reads.host = host;
        job(
            c,
            Op::StartOverRead {
                mem,
                preview,
                reads,
            },
            reqs,
            None,
        )
    }

    /// The reads answered: the preview, or the run's Cloudflare part (revision
    /// 16's removal steps; skipped and listed when Cloudflare's part can't run).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_over_assessed(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        mut mem: Mem,
        preview: bool,
        reads: Reads,
        responses: &[Result<HttpResponse, FetchError>],
    ) -> CommandResult<NetStep> {
        let (st, wifi, rec, conf) = self.setup_read(store, actor, |ctx| {
            Ok((
                cs::load(ctx)?,
                crate::capture_wifi::load(ctx)?,
                record(ctx)?,
                crate::capture::conf(ctx)?.is_some(),
            ))
        })?;
        let run = assess(&st, &wifi, &rec, conf, &reads, responses);
        if preview {
            return Ok(NetStep::Done(run.preview()));
        }
        // The computer part is one elevated step after Cloudflare's.
        mem.skip.push(RStep::Uninstall);
        if let Some(problem) = &run.cf_problem {
            mem.skip.extend([
                RStep::App,
                RStep::AllowPolicy,
                RStep::ServicePolicy,
                RStep::ServiceToken,
                RStep::Dns,
                RStep::Tunnel,
                RStep::ReadIngress,
                RStep::WriteIngress,
            ]);
            let reason = problem
                .trim_end_matches(':')
                .trim_end_matches(". Delete them by hand in the Cloudflare dashboard")
                .to_owned();
            for (_, what, place) in &run.cloudflare {
                mem.failed.push(UndoFailure {
                    what: what.clone(),
                    reason: reason.clone(),
                    place: place.clone(),
                });
            }
        }
        mem.start_over = Some(Box::new(run));
        self.remove_next(store, actor, c, mem, &st)
    }

    /// After Cloudflare's part: this computer, in one elevated step.
    pub(crate) fn start_over_computer(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        mem: Mem,
    ) -> CommandResult<NetStep> {
        let req = mem
            .start_over
            .as_ref()
            .map(|r| r.request.clone())
            .unwrap_or_default();
        if req.is_empty() {
            return self.start_over_finish(store, actor, mem, Ok(Vec::new()));
        }
        if c.host.is_none() {
            return self.start_over_finish(
                store,
                actor,
                mem,
                Err("This MoneyKeep cannot remove services from this computer; use the desktop app.".into()),
            );
        }
        job(
            c,
            Op::StartOverComputer { mem },
            Vec::new(),
            Some(HostOp::StartOver(req)),
        )
    }

    /// MoneyKeep's part (one audited write), then the result list.
    pub(crate) fn start_over_finish(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        mem: Mem,
        computer: Result<Vec<String>, String>,
    ) -> CommandResult<NetStep> {
        let run = mem.start_over.map(|r| *r).unwrap_or_default();
        let declined = computer
            .as_ref()
            .err()
            .is_some_and(|e| e.contains(PERMISSION_DECLINED));
        let mut steps: Vec<Value> = Vec::new();
        let mut left: Vec<Left> = Vec::new();
        for (_, what, place) in &run.cloudflare {
            match mem.failed.iter().find(|f| f.what == *what) {
                Some(f) => {
                    steps.push(
                        json!({"group": "cloudflare", "what": what, "result": "left",
                                      "reason": f.reason, "where": place}),
                    );
                    left.push(Left {
                        what: what.clone(),
                        place: place.clone(),
                    });
                }
                None => {
                    steps.push(json!({"group": "cloudflare", "what": what, "result": "removed"}))
                }
            }
        }
        for l in &run.earlier {
            steps.push(
                json!({"group": "cloudflare", "what": l.what, "result": "left",
                              "reason": text::EARLIER, "where": l.place}),
            );
            if !left.contains(l) {
                left.push(l.clone());
            }
        }
        for what in &run.computer {
            match &computer {
                Ok(_) => {
                    steps.push(json!({"group": "computer", "what": what, "result": "removed"}))
                }
                Err(e) => {
                    let reason = if declined {
                        text::DECLINED.to_owned()
                    } else {
                        mk_core::text::clean(e, 400).text
                    };
                    steps.push(json!({"group": "computer", "what": what, "result": "left",
                                      "reason": reason, "where": text::COMPUTER_WHERE}));
                }
            }
        }
        for what in &run.moneykeep {
            steps.push(json!({"group": "moneykeep", "what": what, "result": "removed"}));
        }
        let computer_left = computer.is_err();
        let rec = Record {
            computer_left,
            cloudflared: run.request.cloudflared,
            cloudflared_tunnel: run.request.cloudflared_tunnel.clone(),
            firewall_rule: run.request.firewall_rule.clone(),
            cloudflare_left: left.clone(),
        };
        let keep = computer_left;
        let audit = json!({
            "cloudflare_removed": run.cloudflare.len().saturating_sub(
                run.cloudflare.iter().filter(|(_, w, _)| mem.failed.iter().any(|f| f.what == *w)).count()),
            "cloudflare_left": left.len(),
            "computer": if computer_left { "left" } else { "removed" },
            "moneykeep": "cleared",
        });
        let spec = self.setup_spec("capture.start_over")?;
        self.execute(store, actor, spec, audit, None, &|ctx, _| {
            let st = cs::load(ctx)?;
            if let Some(r) = &st.token_ref {
                cs::delete_secret(ctx, r)?;
            }
            if let Some(r) = st
                .service_token
                .as_ref()
                .and_then(|s| s.secret_ref.as_ref())
            {
                cs::delete_secret(ctx, r)?;
            }
            if let Some(conf) = crate::capture::conf(ctx)?
                && let Some(r) = conf.secret_ref
            {
                cs::delete_secret(ctx, &r)?;
            }
            for key in [
                cs::SETUP_KEY,
                crate::capture_wifi::WIFI_KEY,
                crate::capture::SERVER_KEY,
                crate::capture::STATE_KEY,
            ] {
                ctx.save_setting(MODULE, key, None)?;
            }
            if keep {
                let v = serde_json::to_value(&rec)
                    .map_err(|e| CommandError::Integrity(e.to_string()))?;
                ctx.save_setting(MODULE, RECORD_KEY, Some(&v))?;
            } else {
                ctx.save_setting(MODULE, RECORD_KEY, None)?;
            }
            Ok(Value::Null)
        })?;
        let cleared = left.is_empty() && !computer_left;
        Ok(NetStep::Done(json!({
            "cleared": cleared,
            "text": if cleared { text::CLEARED } else { text::LEFT },
            "steps": steps,
            "left": left,
            "computer_left": computer_left,
            "declined": declined,
            "computer_actions": computer.unwrap_or_default(),
            "phones": text::PHONES,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_word() {
        assert!(confirmed("START OVER"));
        assert!(confirmed("start over"));
        assert!(confirmed("  Start   Over "));
        assert!(!confirmed("STARTOVER"));
        assert!(!confirmed("START"));
        assert!(!confirmed(""));
    }
}
