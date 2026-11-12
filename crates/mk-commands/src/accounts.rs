//! Account commands (spec: ledger-accounts.md).

use std::collections::HashMap;

use mk_core::{Account, AccountId, AccountSubtype, AccountType, CommodityCode, Date};
use mk_ledger::{check_code, check_description, check_subtype, normalise_name, would_create_cycle};
use mk_store::now_rfc3339;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::balances::raw_balances;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const MULTI_CURRENCY: &str = "multi-currency";

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "account.create",
        "Create an account in the chart of accounts",
        true,
        create,
    );
    r.add(
        "account.update",
        "Change an account's name, code, description, tax line, subtype or placeholder flag, \
         or switch a money account between bank, cash, credit card, loan and the like",
        true,
        update,
    );
    r.add(
        "account.move",
        "Move an account under a new parent (same type) or to the top level",
        true,
        move_account,
    );
    r.add(
        "account.archive",
        "Archive an account with a zero balance (hides it; keeps history)",
        true,
        archive,
    );
    r.add("account.unarchive", "Unarchive an account", true, unarchive);
    r.add(
        "account.delete",
        "Delete an account that has never had a split and has no children",
        true,
        delete,
    );
    r.add(
        "account.get",
        "Get one account by id, full name or code",
        false,
        get,
    );
    r.add("account.list", "List accounts in tree order", false, list);
}

/// The chart of accounts, loaded once per command.
pub(crate) struct Chart {
    pub accounts: Vec<Account>,
    by_id: HashMap<AccountId, usize>,
}

impl Chart {
    pub fn load(ctx: &Ctx<'_>) -> CommandResult<Self> {
        let accounts = ctx.tx.accounts()?;
        let by_id = accounts
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();
        Ok(Self { accounts, by_id })
    }

    pub fn get(&self, id: AccountId) -> Option<&Account> {
        self.by_id.get(&id).and_then(|i| self.accounts.get(*i))
    }

    pub fn full_name(&self, id: AccountId) -> String {
        let mut parts = Vec::new();
        let mut cursor = Some(id);
        while let Some(c) = cursor {
            let Some(a) = self.get(c) else { break };
            parts.push(a.name.as_str());
            cursor = a.parent;
            if parts.len() > self.accounts.len() {
                break;
            }
        }
        parts.reverse();
        parts.join(":")
    }

    pub fn children(&self, id: AccountId) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(move |a| a.parent == Some(id))
    }

    /// The account and all its descendants.
    pub fn subtree(&self, id: AccountId) -> Vec<AccountId> {
        let mut out = vec![id];
        let mut i = 0;
        while let Some(cur) = out.get(i).copied() {
            out.extend(self.children(cur).map(|c| c.id));
            i += 1;
            if out.len() > self.accounts.len() {
                break;
            }
        }
        out
    }

    /// Output scale for amounts of `commodity`: the currency's minor units, or `None`
    /// for a security (held by a `security` account), whose quantities are not padded.
    pub fn scale_of(&self, commodity: &CommodityCode) -> Option<u32> {
        let is_security = self
            .accounts
            .iter()
            .any(|a| a.subtype == AccountSubtype::Security && a.commodity == *commodity);
        (!is_security).then(|| mk_core::currency::minor_units(commodity.as_str()))
    }

    pub fn parents(&self) -> HashMap<AccountId, Option<AccountId>> {
        self.accounts.iter().map(|a| (a.id, a.parent)).collect()
    }

    /// Resolve a reference: id, full name (case-insensitive) or code.
    pub fn resolve(&self, reference: &str) -> CommandResult<&Account> {
        let r = reference.trim();
        if let Ok(id) = r.parse::<AccountId>()
            && let Some(a) = self.get(id)
        {
            return Ok(a);
        }
        let lower = r.to_lowercase();
        if let Some(a) = self
            .accounts
            .iter()
            .find(|a| self.full_name(a.id).to_lowercase() == lower)
        {
            return Ok(a);
        }
        if let Some(a) = self.accounts.iter().find(|a| a.code.as_deref() == Some(r)) {
            return Ok(a);
        }
        Err(CommandError::NotFound(format!(
            "no account with id, full name or code {reference:?}"
        )))
    }

    /// Accounts in tree order: each parent followed by its children, siblings sorted
    /// by name (case-insensitive).
    pub fn tree_order(&self) -> Vec<(&Account, usize)> {
        let mut out = Vec::new();
        let roots: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.parent.is_none_or(|p| self.get(p).is_none()))
            .collect();
        let mut stack: Vec<(&Account, usize)> = roots.into_iter().rev().map(|a| (a, 0)).collect();
        while let Some((a, depth)) = stack.pop() {
            out.push((a, depth));
            if out.len() > self.accounts.len() {
                break;
            }
            let kids: Vec<&Account> = self.children(a.id).collect();
            stack.extend(kids.into_iter().rev().map(|k| (k, depth + 1)));
        }
        out
    }

    fn sibling_clash(
        &self,
        parent: Option<AccountId>,
        name: &str,
        except: Option<AccountId>,
    ) -> bool {
        let lower = name.to_lowercase();
        self.accounts
            .iter()
            .any(|a| a.parent == parent && Some(a.id) != except && a.name.to_lowercase() == lower)
    }

    fn code_clash(&self, code: &str, except: Option<AccountId>) -> bool {
        self.accounts
            .iter()
            .any(|a| a.code.as_deref() == Some(code) && Some(a.id) != except)
    }
}

/// Account as returned to clients: stored fields plus full name and usage.
pub(crate) fn account_json(ctx: &Ctx<'_>, chart: &Chart, a: &Account) -> CommandResult<Value> {
    let mut v = serde_json::to_value(a)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("full_name".into(), json!(chart.full_name(a.id)));
        if let Some(p) = a.role.as_deref().and_then(crate::chart::role_purpose) {
            o.insert("role_purpose".into(), json!(p));
        }
        o.insert("has_splits".into(), json!(ctx.tx.account_ever_used(a.id)?));
    }
    Ok(v)
}

fn check_commodity(
    ctx: &Ctx<'_>,
    subtype: AccountSubtype,
    commodity: &CommodityCode,
) -> CommandResult<()> {
    let base: CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    if subtype == AccountSubtype::Security {
        if *commodity == base {
            return Err(CommandError::InvalidArgs(format!(
                "a security account holds a security, so its commodity must differ from the \
                 base currency {base}"
            )));
        }
    } else if *commodity != base
        && !ctx.restoring.get()
        && !is_module_enabled(ctx.tx, MULTI_CURRENCY)?
    {
        return Err(CommandError::Rejected(format!(
            "account commodity {commodity} differs from the base currency {base}; enable the \
             multi-currency module first"
        )));
    }
    Ok(())
}

fn check_parent(
    chart: &Chart,
    parent: Option<AccountId>,
    account_type: AccountType,
) -> CommandResult<()> {
    if let Some(p) = parent {
        let pa = chart
            .get(p)
            .ok_or_else(|| CommandError::NotFound("parent account".into()))?;
        if pa.account_type != account_type {
            return Err(CommandError::InvalidArgs(format!(
                "parent {} is {}, but the account is {account_type}; a parent must have the \
                 same type",
                chart.full_name(p),
                pa.account_type
            )));
        }
    }
    Ok(())
}

fn clean_opt(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// A tax line (chart-presets.md): 1-40 printable characters after trimming, no line
/// breaks. Returns the trimmed text.
pub(crate) fn check_tax_line(s: &str) -> CommandResult<String> {
    let t = s.trim();
    let n = t.chars().count();
    if n == 0 || n > 40 {
        return Err(CommandError::InvalidArgs(format!(
            "a tax line is 1 to 40 characters (this one has {n})"
        )));
    }
    if t.chars().any(char::is_control) {
        return Err(CommandError::InvalidArgs(
            "a tax line is one line of printable text".into(),
        ));
    }
    Ok(t.to_owned())
}

pub(crate) const TAX_LINE_ON_HEADING: &str =
    "Remove the tax line first: totals can't be posted to a heading.";

// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    /// 1-100 characters, no ':'; unique among siblings ignoring case.
    name: String,
    #[serde(rename = "type")]
    account_type: AccountType,
    /// Defaults to the type's general subtype (e.g. other-asset).
    #[serde(default)]
    subtype: Option<AccountSubtype>,
    /// Parent account (id, full name or code); must have the same type.
    #[serde(default)]
    parent: Option<String>,
    /// Defaults to the book's base currency.
    #[serde(default)]
    commodity: Option<CommodityCode>,
    /// Account number, 1-20 characters, unique in the book.
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    placeholder: bool,
    /// Where the account's total goes on a tax return, e.g. "Sch C L18" (1-40
    /// characters; not on a placeholder). Empty means none.
    #[serde(default)]
    tax_line: Option<String>,
}

fn create(ctx: &Ctx<'_>, a: CreateArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let name = normalise_name(&a.name)?;
    let subtype = a
        .subtype
        .unwrap_or_else(|| a.account_type.default_subtype());
    check_subtype(a.account_type, subtype)?;
    let parent = a
        .parent
        .as_deref()
        .map(|p| chart.resolve(p).map(|acc| acc.id))
        .transpose()?;
    check_parent(&chart, parent, a.account_type)?;
    if chart.sibling_clash(parent, &name, None) {
        return Err(CommandError::Rejected(format!(
            "an account named {name:?} already exists there (names are compared ignoring case)"
        )));
    }
    let commodity = match a.commodity {
        Some(c) => c,
        None if subtype == AccountSubtype::Security => {
            return Err(CommandError::InvalidArgs(
                "a security account needs a commodity (the security symbol)".into(),
            ));
        }
        None => ctx.tx.info()?.base_currency.parse()?,
    };
    check_commodity(ctx, subtype, &commodity)?;
    let code = a.code.as_deref().map(check_code).transpose()?;
    if let Some(c) = &code
        && chart.code_clash(c, None)
    {
        return Err(CommandError::Rejected(format!(
            "account code {c:?} is already used"
        )));
    }
    let description = clean_opt(a.description);
    if let Some(d) = &description {
        check_description(d)?;
    }
    let tax_line = clean_opt(a.tax_line)
        .map(|t| check_tax_line(&t))
        .transpose()?;
    if tax_line.is_some() && a.placeholder {
        return Err(CommandError::Rejected(
            "a placeholder (heading) account cannot have a tax line".into(),
        ));
    }
    let account = Account {
        id: AccountId::new(),
        name,
        account_type: a.account_type,
        subtype,
        parent,
        commodity,
        code,
        description,
        placeholder: a.placeholder,
        archived: false,
        created_at: now_rfc3339()?,
        tax_line,
        role: None,
        retirement: false,
        opening_none: false,
    };
    ctx.save_account(None, &account)?;
    let chart = Chart::load(ctx)?;
    account_json(ctx, &chart, &account)
}

// ---------------------------------------------------------------------------

/// A field that may be left alone (absent), cleared (null) or set.
#[derive(Debug, Default, JsonSchema)]
#[schemars(transparent)]
pub(crate) struct Patch<T>(pub Option<Option<T>>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(d).map(|v| Patch(Some(v)))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    /// Account id, full name or code.
    account: String,
    #[serde(default)]
    name: Option<String>,
    /// New code, or null to remove it.
    #[serde(default)]
    code: Patch<String>,
    /// New description, or null to remove it.
    #[serde(default)]
    description: Patch<String>,
    /// New type, given together with `subtype`. Only a money account (bank, cash,
    /// other-asset, credit-card, loan, other-liability) may change type, and only to
    /// another money subtype; this is allowed even when it has splits.
    #[serde(default, rename = "type")]
    account_type: Option<AccountType>,
    /// New subtype. With splits it may change only among the money subtypes.
    #[serde(default)]
    subtype: Option<AccountSubtype>,
    /// New parent (id, full name or code; null for the top level). When the type
    /// changes and this is absent, the account goes under the first group of the new
    /// type that already holds an account of the new subtype, else to the top level.
    #[serde(default)]
    parent: Patch<String>,
    #[serde(default)]
    placeholder: Option<bool>,
    /// Allowed only while the account has never had a split.
    #[serde(default)]
    commodity: Option<CommodityCode>,
    /// New tax line; null or an empty string removes it.
    #[serde(default)]
    tax_line: Patch<String>,
    /// A retirement account (IRA, 401(k), Roth and similar); brokerage accounts only.
    #[serde(default)]
    retirement: Option<bool>,
    /// The account started from zero and needs no opening balance.
    #[serde(default)]
    opening_none: Option<bool>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = chart.resolve(&a.account)?.clone();
    let mut new = old.clone();
    let used = ctx.tx.account_ever_used(old.id)?;
    let live = ctx.tx.account_has_live_splits(old.id)?;
    if let Some(name) = &a.name {
        new.name = normalise_name(name)?;
    }
    if let Some(code) = a.code.0 {
        new.code = code.as_deref().map(check_code).transpose()?;
        if let Some(c) = &new.code
            && chart.code_clash(c, Some(old.id))
        {
            return Err(CommandError::Rejected(format!(
                "account code {c:?} is already used"
            )));
        }
    }
    if let Some(d) = a.description.0 {
        new.description = clean_opt(d);
        if let Some(d) = &new.description {
            check_description(d)?;
        }
    }
    new.account_type = a.account_type.unwrap_or(old.account_type);
    let type_changes = new.account_type != old.account_type;
    if let Some(sub) = a.subtype {
        check_subtype(new.account_type, sub)?;
        new.subtype = sub;
    } else if type_changes {
        return Err(CommandError::InvalidArgs(
            "a new type needs a subtype too (for example liability with credit-card)".into(),
        ));
    }
    let money = old.subtype.is_money() && new.subtype.is_money();
    if type_changes && !money {
        return Err(CommandError::Rejected(format!(
            "{} is {}/{}; only a bank, cash, other asset, credit card, loan or other \
             liability account can change type, and only to another of those",
            chart.full_name(old.id),
            old.account_type,
            old.subtype
        )));
    }
    if new.subtype != old.subtype && used && !money {
        return Err(CommandError::Rejected(
            "the subtype can change only while the account has no splits".into(),
        ));
    }
    if let Some(c) = a.commodity {
        if c != old.commodity && used {
            return Err(CommandError::Rejected(
                "the commodity cannot change after the account has had a split".into(),
            ));
        }
        new.commodity = c;
    }
    if new.subtype != old.subtype || new.commodity != old.commodity {
        check_commodity(ctx, new.subtype, &new.commodity)?;
    }
    if let Some(t) = a.tax_line.0 {
        new.tax_line = clean_opt(t).map(|t| check_tax_line(&t)).transpose()?;
    }
    if let Some(p) = a.placeholder {
        if p && !old.placeholder && live {
            return Err(CommandError::Rejected(
                "an account with splits cannot become a placeholder".into(),
            ));
        }
        new.placeholder = p;
    }
    if new.placeholder && new.tax_line.is_some() {
        return Err(CommandError::Rejected(TAX_LINE_ON_HEADING.into()));
    }
    if let Some(r) = a.retirement {
        if r && new.subtype != AccountSubtype::Brokerage {
            return Err(CommandError::Rejected(
                "only a brokerage account can be a retirement account".into(),
            ));
        }
        new.retirement = r;
    }
    if let Some(z) = a.opening_none {
        new.opening_none = z;
    }
    if type_changes && (new.placeholder || chart.children(old.id).next().is_some()) {
        return Err(CommandError::Rejected(format!(
            "{} is a group (placeholder or with sub-accounts), so its type cannot change: \
             its sub-accounts would have a different type",
            chart.full_name(old.id)
        )));
    }
    new.parent = match a.parent.0 {
        Some(p) => p
            .as_deref()
            .map(|p| chart.resolve(p).map(|acc| acc.id))
            .transpose()?,
        None if type_changes => default_group(&chart, &new),
        None => old.parent,
    };
    check_parent(&chart, new.parent, new.account_type)?;
    if let Some(p) = new.parent
        && new.parent != old.parent
        && would_create_cycle(old.id, p, &chart.parents())
    {
        return Err(CommandError::Rejected(
            "an account cannot be moved under itself or one of its descendants".into(),
        ));
    }
    if (new.name != old.name || new.parent != old.parent)
        && chart.sibling_clash(new.parent, &new.name, Some(old.id))
    {
        let place = new
            .parent
            .map_or_else(|| "the top level".to_owned(), |p| chart.full_name(p));
        return Err(CommandError::Rejected(format!(
            "an account named {:?} already exists under {place} (names are compared ignoring \
             case); change the name of one of them first",
            new.name
        )));
    }
    if new != old {
        ctx.save_account(Some(&old), &new)?;
    }
    let chart = Chart::load(ctx)?;
    account_json(ctx, &chart, &new)
}

/// Where a money account goes when its type changes and no parent is given
/// (ledger-accounts.md revision 3): the first placeholder of the new type, by full
/// name, that already holds an account of the new subtype; else the top level.
pub(crate) fn default_group(chart: &Chart, acc: &Account) -> Option<AccountId> {
    let mut groups: Vec<(String, AccountId)> = chart
        .accounts
        .iter()
        .filter(|g| {
            g.placeholder
                && !g.archived
                && g.id != acc.id
                && g.account_type == acc.account_type
                && chart
                    .children(g.id)
                    .any(|c| c.id != acc.id && c.subtype == acc.subtype)
        })
        .map(|g| (chart.full_name(g.id).to_lowercase(), g.id))
        .collect();
    groups.sort();
    groups.first().map(|(_, id)| *id)
}

// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    /// Account id, full name or code.
    account: String,
    /// New parent (id, full name or code), or null for the top level.
    parent: Option<String>,
}

fn move_account(ctx: &Ctx<'_>, a: MoveArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = chart.resolve(&a.account)?.clone();
    let parent = a
        .parent
        .as_deref()
        .map(|p| chart.resolve(p).map(|acc| acc.id))
        .transpose()?;
    check_parent(&chart, parent, old.account_type)?;
    if let Some(p) = parent
        && would_create_cycle(old.id, p, &chart.parents())
    {
        return Err(CommandError::Rejected(
            "an account cannot be moved under itself or one of its descendants".into(),
        ));
    }
    if chart.sibling_clash(parent, &old.name, Some(old.id)) {
        return Err(CommandError::Rejected(format!(
            "an account named {:?} already exists under the new parent",
            old.name
        )));
    }
    let mut new = old.clone();
    new.parent = parent;
    if new != old {
        ctx.save_account(Some(&old), &new)?;
    }
    let chart = Chart::load(ctx)?;
    account_json(ctx, &chart, &new)
}

// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountRef {
    /// Account id, full name or code.
    account: String,
}

fn archive(ctx: &Ctx<'_>, a: AccountRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = chart.resolve(&a.account)?.clone();
    let balances = raw_balances(ctx, &[old.id], None, mk_core::Basis::All)?;
    let non_zero: Vec<String> = balances
        .iter()
        .filter(|(_, v)| !v.is_zero())
        .map(|(c, v)| format!("{v} {c}"))
        .collect();
    if !non_zero.is_empty() {
        return Err(CommandError::Rejected(format!(
            "{} cannot be archived: its balance is {} (it must be zero)",
            chart.full_name(old.id),
            non_zero.join(", ")
        )));
    }
    if chart.children(old.id).any(|c| !c.archived) {
        return Err(CommandError::Rejected(format!(
            "{} has unarchived sub-accounts; archive them first",
            chart.full_name(old.id)
        )));
    }
    if Some(old.id) == ctx.tx.opening_account_id()? && ctx.tx.account_has_live_splits(old.id)? {
        return Err(CommandError::Rejected(
            "the Opening Balances account cannot be archived while it holds splits".into(),
        ));
    }
    let mut new = old.clone();
    new.archived = true;
    if new != old {
        ctx.save_account(Some(&old), &new)?;
    }
    account_json(ctx, &chart, &new)
}

fn unarchive(ctx: &Ctx<'_>, a: AccountRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = chart.resolve(&a.account)?.clone();
    let mut new = old.clone();
    new.archived = false;
    if new != old {
        ctx.save_account(Some(&old), &new)?;
    }
    account_json(ctx, &chart, &new)
}

fn delete(ctx: &Ctx<'_>, a: AccountRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = chart.resolve(&a.account)?.clone();
    let name = chart.full_name(old.id);
    if Some(old.id) == ctx.tx.opening_account_id()? {
        return Err(CommandError::Rejected(
            "the Opening Balances account cannot be deleted".into(),
        ));
    }
    if chart.children(old.id).next().is_some() {
        return Err(CommandError::Rejected(format!(
            "{name} has sub-accounts; move or delete them first"
        )));
    }
    if ctx.tx.account_ever_used(old.id)? {
        return Err(CommandError::Rejected(format!(
            "{name} has had splits and cannot be deleted; archive it instead"
        )));
    }
    ctx.remove_account(&old)?;
    Ok(json!({ "deleted": old.id, "full_name": name }))
}

fn get(ctx: &Ctx<'_>, a: AccountRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?;
    account_json(ctx, &chart, acc)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default, rename = "type")]
    account_type: Option<AccountType>,
    #[serde(default)]
    include_archived: bool,
    /// The date notes are worked out for (default: today).
    #[serde(default)]
    today: Option<Date>,
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let today = crate::sales_reports::today_or(a.today)?;
    let notes = crate::processors::account_notes(ctx, today)?;
    let mut out = Vec::new();
    for (acc, depth) in chart.tree_order() {
        if a.account_type.is_some_and(|t| t != acc.account_type) {
            continue;
        }
        if acc.archived && !a.include_archived {
            continue;
        }
        let mut v = account_json(ctx, &chart, acc)?;
        if let Some(o) = v.as_object_mut() {
            o.insert("depth".into(), json!(depth));
            if let Some(n) = notes.get(&acc.id) {
                o.insert("note".into(), json!(n));
            }
        }
        out.push(v);
    }
    Ok(Value::Array(out))
}

/// Check that the account may go from `current` to `target` (either may be absent),
/// with the same rules the account commands apply. Used by undo and redo.
pub(crate) fn check_account_transition(
    ctx: &Ctx<'_>,
    current: Option<&Account>,
    target: Option<&Account>,
) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    match (current, target) {
        (None, None) => Ok(()),
        (Some(cur), None) => {
            let name = chart.full_name(cur.id);
            if Some(cur.id) == ctx.tx.opening_account_id()? {
                return Err(CommandError::Rejected(
                    "the Opening Balances account cannot be deleted".into(),
                ));
            }
            if chart.children(cur.id).next().is_some() {
                return Err(CommandError::Rejected(format!("{name} has sub-accounts")));
            }
            if ctx.tx.account_ever_used(cur.id)? {
                return Err(CommandError::Rejected(format!(
                    "{name} has been used by a transaction since, so it cannot be removed"
                )));
            }
            Ok(())
        }
        (cur, Some(t)) => {
            let name = normalise_name(&t.name)?;
            if name != t.name {
                return Err(CommandError::InvalidArgs("invalid account name".into()));
            }
            check_subtype(t.account_type, t.subtype)?;
            check_parent(&chart, t.parent, t.account_type)?;
            if let Some(p) = t.parent
                && cur.is_some()
                && would_create_cycle(t.id, p, &chart.parents())
            {
                return Err(CommandError::Rejected(
                    "an account cannot be under itself or one of its descendants".into(),
                ));
            }
            if chart.sibling_clash(t.parent, &t.name, Some(t.id)) {
                return Err(CommandError::Rejected(format!(
                    "an account named {:?} already exists there",
                    t.name
                )));
            }
            if let Some(c) = &t.code
                && chart.code_clash(c, Some(t.id))
            {
                return Err(CommandError::Rejected(format!(
                    "account code {c:?} is already used"
                )));
            }
            let used = cur.is_some() && ctx.tx.account_ever_used(t.id)?;
            let live = cur.is_some() && ctx.tx.account_has_live_splits(t.id)?;
            let changed = |f: &dyn Fn(&Account) -> bool| cur.is_none_or(f);
            // ledger-accounts.md revision 3: money accounts switch freely among the
            // six money subtypes, splits or not.
            let money = cur.is_some_and(|c| c.subtype.is_money()) && t.subtype.is_money();
            if changed(&|c| c.account_type != t.account_type)
                && cur.is_some()
                && ((used && !money) || chart.children(t.id).next().is_some())
            {
                return Err(CommandError::Rejected(
                    "the account type cannot change once the account has splits or \
                     sub-accounts"
                        .into(),
                ));
            }
            let sub_blocked = changed(&|c| c.subtype != t.subtype) && !money;
            if sub_blocked || changed(&|c| c.commodity != t.commodity) {
                if used {
                    return Err(CommandError::Rejected(
                        "the subtype and commodity cannot change after the account has had a \
                         split"
                            .into(),
                    ));
                }
                check_commodity(ctx, t.subtype, &t.commodity)?;
            }
            if t.placeholder && t.tax_line.is_some() {
                return Err(CommandError::Rejected(TAX_LINE_ON_HEADING.into()));
            }
            if let Some(tl) = &t.tax_line
                && check_tax_line(tl)? != *tl
            {
                return Err(CommandError::InvalidArgs("invalid tax line".into()));
            }
            if let Some(r) = &t.role
                && changed(&|c| c.role.as_ref() != Some(r))
                && chart
                    .accounts
                    .iter()
                    .any(|o| o.id != t.id && o.role.as_ref() == Some(r))
            {
                return Err(CommandError::Rejected(format!(
                    "another account already holds the role {r}"
                )));
            }
            if t.placeholder && live && changed(&|c| !c.placeholder) {
                return Err(CommandError::Rejected(
                    "an account with splits cannot become a placeholder".into(),
                ));
            }
            if t.archived && changed(&|c| !c.archived) {
                let balances = raw_balances(ctx, &[t.id], None, mk_core::Basis::All)?;
                if balances.values().any(|v| !v.is_zero()) {
                    return Err(CommandError::Rejected(format!(
                        "{} cannot be archived: its balance is not zero",
                        chart.full_name(t.id)
                    )));
                }
                if chart.children(t.id).any(|c| !c.archived) {
                    return Err(CommandError::Rejected(format!(
                        "{} has unarchived sub-accounts",
                        chart.full_name(t.id)
                    )));
                }
                if Some(t.id) == ctx.tx.opening_account_id()? && live {
                    return Err(CommandError::Rejected(
                        "the Opening Balances account cannot be archived while it holds splits"
                            .into(),
                    ));
                }
            }
            Ok(())
        }
    }
}

/// `account.create` / `account.update` from JSON arguments (definition installs).
pub(crate) fn create_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    create(ctx, crate::parse_args(args)?)
}

pub(crate) fn update_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    update(ctx, crate::parse_args(args)?)
}
