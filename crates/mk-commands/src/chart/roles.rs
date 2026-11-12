//! Account roles (chart-presets.md "Roles"): the fixed jobs an account does for the
//! program, so a renamed or moved account keeps working.

use mk_core::{Account, AccountId};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx};

pub(crate) const OPENING_BALANCES: &str = "opening_balances";
pub(crate) const UNCATEGORIZED_INCOME: &str = "uncategorized_income";
pub(crate) const UNCATEGORIZED_EXPENSE: &str = "uncategorized_expense";
pub(crate) const RECEIVABLE: &str = "receivable";
pub(crate) const DEPOSIT_CLEARING: &str = "deposit_clearing";
pub(crate) const SALES_TAX: &str = "sales_tax";
pub(crate) const PAYABLE: &str = "payable";
pub(crate) const REIMBURSEMENTS: &str = "reimbursements";
pub(crate) const RETAINED_EARNINGS: &str = "retained_earnings";
pub(crate) const INV_DIVIDENDS: &str = "inv_dividends";
pub(crate) const INV_INTEREST: &str = "inv_interest";
pub(crate) const INV_REALIZED: &str = "inv_realized";
pub(crate) const INV_FEES: &str = "inv_fees";

/// Every role with the purpose shown as "Used by MoneyKeep for <purpose>".
pub(crate) const ALL: &[(&str, &str)] = &[
    (OPENING_BALANCES, "opening balances"),
    (UNCATEGORIZED_INCOME, "imported money that has no category"),
    (UNCATEGORIZED_EXPENSE, "imported money that has no category"),
    (RECEIVABLE, "money customers owe you"),
    (
        DEPOSIT_CLEARING,
        "customer payments waiting to be deposited",
    ),
    (SALES_TAX, "sales tax you collect"),
    (PAYABLE, "bills you owe"),
    (REIMBURSEMENTS, "expenses waiting to be paid back"),
    (RETAINED_EARNINGS, "retained earnings on the balance sheet"),
    (INV_DIVIDENDS, "investment dividends"),
    (INV_INTEREST, "investment interest"),
    (INV_REALIZED, "realized investment gains"),
    (INV_FEES, "investment fees"),
];

pub(crate) fn purpose(role: &str) -> Option<&'static str> {
    ALL.iter().find(|(r, _)| *r == role).map(|(_, p)| *p)
}

/// The account holding `role`, if any.
pub(crate) fn holder<'c>(chart: &'c Chart, role: &str) -> Option<&'c Account> {
    chart
        .accounts
        .iter()
        .find(|a| a.role.as_deref() == Some(role))
}

/// Give `role` to the account `id` (no-op when it already has it or when another
/// account holds it).
pub(crate) fn give(ctx: &Ctx<'_>, id: AccountId, role: &str) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    if holder(&chart, role).is_some() {
        return Ok(());
    }
    let old = chart
        .get(id)
        .ok_or_else(|| CommandError::Integrity("role for a missing account".into()))?
        .clone();
    if old.role.is_some() {
        return Ok(());
    }
    let mut new = old.clone();
    new.role = Some(role.to_owned());
    ctx.save_account(Some(&old), &new)
}

/// The lookup rule: the account holding `role`; else the first account `find`
/// accepts; else one made by `create`. A found or created account gets the role.
pub(crate) fn find_or_create(
    ctx: &Ctx<'_>,
    role: &str,
    find: &dyn Fn(&Account) -> bool,
    create: &dyn Fn(&Ctx<'_>) -> CommandResult<AccountId>,
) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    if let Some(a) = holder(&chart, role) {
        return Ok(a.id);
    }
    let id = match chart.accounts.iter().find(|a| find(a)) {
        Some(a) => a.id,
        None => create(ctx)?,
    };
    give(ctx, id, role)?;
    Ok(id)
}
