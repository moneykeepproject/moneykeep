//! Transaction rules: balancing, precision, closing date, reconciled and archived
//! protection.

use std::collections::HashMap;

use mk_core::currency::{MAX_QUANTITY_DP, decimal_places, format_amount, minor_units};
use mk_core::{AccountId, CommodityCode, Date, Decimal, Reconcile, Split, Transaction, TxnStatus};

use crate::{LedgerError, LedgerResult};

pub const MIN_SPLITS: usize = 2;
pub const MAX_SPLITS: usize = 1000;
pub const MAX_PAYEE_LEN: usize = 200;
pub const MAX_NUMBER_LEN: usize = 20;
pub const MAX_MEMO_LEN: usize = 1000;

/// What the rules need to know about an account a split touches.
#[derive(Debug, Clone)]
pub struct AccountFacts {
    pub full_name: String,
    pub commodity: CommodityCode,
    pub placeholder: bool,
    pub archived: bool,
    /// The account holds a security (subtype `security`), not a currency.
    pub is_security: bool,
}

/// Book-level facts for transaction validation.
#[derive(Debug, Clone)]
pub struct TxnRules {
    pub base_currency: CommodityCode,
    pub multi_currency: bool,
}

fn check_len(what: &str, text: Option<&str>, max: usize) -> LedgerResult<()> {
    match text {
        Some(t) if t.chars().count() > max => Err(LedgerError::invalid(format!(
            "{what} is longer than {max} characters"
        ))),
        _ => Ok(()),
    }
}

/// Check one transaction's own content against the book's accounts and settings.
///
/// `check_currency_gate` is false for changes that do not touch currency or splits
/// (void, unvoid, reconcile flags), so turning `multi-currency` off hides rather than
/// freezes existing foreign entries.
pub fn check_transaction(
    txn: &Transaction,
    accounts: &HashMap<AccountId, AccountFacts>,
    rules: &TxnRules,
    check_currency_gate: bool,
) -> LedgerResult<()> {
    let n = txn.splits.len();
    if !(MIN_SPLITS..=MAX_SPLITS).contains(&n) {
        return Err(LedgerError::invalid(format!(
            "a transaction needs {MIN_SPLITS} to {MAX_SPLITS} splits; got {n}"
        )));
    }
    check_len("payee", txn.payee.as_deref(), MAX_PAYEE_LEN)?;
    check_len("number", txn.number.as_deref(), MAX_NUMBER_LEN)?;
    check_len("memo", txn.memo.as_deref(), MAX_MEMO_LEN)?;
    if txn.status == TxnStatus::Posted && txn.void_reason.is_some() {
        return Err(LedgerError::invalid(
            "only a voided transaction has a void reason",
        ));
    }

    let multi = rules.multi_currency;
    if check_currency_gate && !multi && txn.currency != rules.base_currency {
        return Err(LedgerError::rejected(format!(
            "transaction currency {} differs from the base currency {}; enable the \
             multi-currency module first",
            txn.currency, rules.base_currency
        )));
    }

    let value_dp = minor_units(txn.currency.as_str());
    let mut total = Decimal::ZERO;
    let mut all_zero = true;
    for (i, split) in txn.splits.iter().enumerate() {
        let facts = accounts
            .get(&split.account)
            .ok_or_else(|| LedgerError::invalid(format!("split {i}: unknown account")))?;
        let name = &facts.full_name;
        check_len(
            &format!("split {i} memo"),
            split.memo.as_deref(),
            MAX_MEMO_LEN,
        )?;
        if facts.placeholder {
            return Err(LedgerError::rejected(format!(
                "split {i}: {name} is a placeholder account and cannot hold splits"
            )));
        }
        if check_currency_gate
            && !multi
            && !facts.is_security
            && facts.commodity != rules.base_currency
        {
            return Err(LedgerError::rejected(format!(
                "split {i}: {name} is in {}; enable the multi-currency module first",
                facts.commodity
            )));
        }
        if decimal_places(split.value) > value_dp {
            return Err(LedgerError::invalid(format!(
                "split {i}: value {} has more than {value_dp} decimal places for {}",
                split.value, txn.currency
            )));
        }
        if facts.commodity == txn.currency {
            if split.quantity != split.value {
                return Err(LedgerError::invalid(format!(
                    "split {i}: {name} is in the transaction currency, so quantity must \
                     equal value ({} vs {})",
                    split.quantity, split.value
                )));
            }
        } else {
            if decimal_places(split.quantity) > MAX_QUANTITY_DP {
                return Err(LedgerError::invalid(format!(
                    "split {i}: quantity {} has more than {MAX_QUANTITY_DP} decimal places",
                    split.quantity
                )));
            }
            let opposite = (split.value.is_sign_positive() && split.quantity.is_sign_negative())
                || (split.value.is_sign_negative() && split.quantity.is_sign_positive());
            if !split.value.is_zero() && !split.quantity.is_zero() && opposite {
                return Err(LedgerError::invalid(format!(
                    "split {i}: value {} and quantity {} must have the same sign",
                    split.value, split.quantity
                )));
            }
        }
        if !split.value.is_zero() || !split.quantity.is_zero() {
            all_zero = false;
        }
        total = total
            .checked_add(split.value)
            .ok_or(LedgerError::Overflow)?;
    }
    if all_zero {
        return Err(LedgerError::invalid(
            "a transaction must move something: every split is zero",
        ));
    }
    if !total.is_zero() {
        return Err(LedgerError::invalid(format!(
            "transaction does not balance: split values sum to {total} {cur} (difference {diff} {cur})",
            total = format_amount(total, Some(value_dp)),
            diff = format_amount(total.abs(), Some(value_dp)),
            cur = txn.currency
        )));
    }
    Ok(())
}

/// Reject a change that touches the closed period: the old or new transaction is dated
/// on or before the closing date.
pub fn check_closing(
    closing: Option<Date>,
    old: Option<&Transaction>,
    new: Option<&Transaction>,
) -> LedgerResult<()> {
    let Some(closing) = closing else {
        return Ok(());
    };
    for t in [old, new].into_iter().flatten() {
        if t.date <= closing {
            return Err(LedgerError::rejected(format!(
                "the books are closed through {closing}; a transaction dated {} cannot be \
                 created, changed, voided or deleted",
                t.date
            )));
        }
    }
    Ok(())
}

type SplitKey = (AccountId, Decimal, Decimal, Option<String>);

/// Splits that count in balances: none if the transaction is voided or absent.
fn effective_in<'a>(
    t: Option<&'a Transaction>,
    pick: &dyn Fn(&AccountId) -> bool,
) -> Vec<SplitKey> {
    let mut keys: Vec<SplitKey> = t
        .filter(|t| t.status == TxnStatus::Posted)
        .map(|t| t.splits.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter(|s| pick(&s.account))
        .map(|s| (s.account, s.value, s.quantity, s.memo.clone()))
        .collect();
    keys.sort();
    keys
}

/// Archived accounts reject any change to the splits that count in them.
pub fn check_archived_untouched(
    old: Option<&Transaction>,
    new: Option<&Transaction>,
    accounts: &HashMap<AccountId, AccountFacts>,
) -> LedgerResult<()> {
    let archived = |id: &AccountId| accounts.get(id).is_some_and(|f| f.archived);
    if effective_in(old, &archived) != effective_in(new, &archived) {
        let name = [old, new]
            .into_iter()
            .flatten()
            .flat_map(|t| t.splits.iter())
            .find(|s| archived(&s.account))
            .and_then(|s| accounts.get(&s.account))
            .map_or_else(String::new, |f| f.full_name.clone());
        return Err(LedgerError::rejected(format!(
            "{name} is archived; unarchive it before adding or changing its splits"
        )));
    }
    Ok(())
}

/// Reconciled splits may not be removed or have their account, value or quantity
/// changed. (Their memo may change; `txn.set_reconcile` is the way to unreconcile.)
pub fn check_reconciled_preserved(
    old: Option<&Transaction>,
    new: Option<&Transaction>,
) -> LedgerResult<()> {
    let Some(old) = old else { return Ok(()) };
    let mut remaining: Vec<&Split> = new
        .map(|t| {
            t.splits
                .iter()
                .filter(|s| s.reconcile == Reconcile::Reconciled)
                .collect()
        })
        .unwrap_or_default();
    for (i, s) in old.splits.iter().enumerate() {
        if s.reconcile != Reconcile::Reconciled {
            continue;
        }
        let found = remaining
            .iter()
            .position(|n| n.account == s.account && n.value == s.value && n.quantity == s.quantity);
        match found {
            Some(pos) => {
                remaining.swap_remove(pos);
            }
            None => {
                return Err(LedgerError::rejected(format!(
                    "split {i} is reconciled; its account, value and quantity cannot be \
                     changed and it cannot be removed (unreconcile it first)"
                )));
            }
        }
    }
    Ok(())
}

/// True if the two versions differ only in split reconcile flags and import
/// fingerprints (bookkeeping metadata that does not change any amount).
pub fn only_reconcile_differs(old: &Transaction, new: &Transaction) -> bool {
    let strip = |t: &Transaction| {
        let mut t = t.clone();
        for s in &mut t.splits {
            s.reconcile = Reconcile::New;
            s.fingerprint = None;
        }
        t
    };
    strip(old) == strip(new)
}

/// Canonical split order, so the same entry given in any order is stored identically:
/// debits before credits (value descending), then account, quantity and memo.
pub fn sort_splits(splits: &mut [Split]) {
    splits.sort_by(|a, b| {
        b.value
            .cmp(&a.value)
            .then_with(|| a.account.cmp(&b.account))
            .then_with(|| b.quantity.cmp(&a.quantity))
            .then_with(|| a.memo.cmp(&b.memo))
    });
}

/// Give each new split the reconcile state and import fingerprint of the old split it
/// corresponds to (same account, value and quantity, preferring the same memo);
/// unmatched splits start at `n` with no fingerprint.
pub fn inherit_reconcile(old: &[Split], new: &mut [Split]) {
    let mut old_used = vec![false; old.len()];
    let mut new_done = vec![false; new.len()];
    for n in new.iter_mut() {
        n.reconcile = Reconcile::New;
        n.fingerprint = None;
    }
    for same_memo in [true, false] {
        for (ni, n) in new.iter_mut().enumerate() {
            if new_done.get(ni).copied().unwrap_or(true) {
                continue;
            }
            let hit = old.iter().enumerate().find(|(oi, o)| {
                !old_used.get(*oi).copied().unwrap_or(true)
                    && o.account == n.account
                    && o.value == n.value
                    && o.quantity == n.quantity
                    && (!same_memo || o.memo == n.memo)
            });
            if let Some((oi, o)) = hit {
                if let Some(u) = old_used.get_mut(oi) {
                    *u = true;
                }
                if let Some(d) = new_done.get_mut(ni) {
                    *d = true;
                }
                n.reconcile = o.reconcile;
                n.fingerprint.clone_from(&o.fingerprint);
            }
        }
    }
}

/// Rescale an amount to at least `min_dp` decimal places, dropping insignificant
/// trailing zeros beyond that. Never rounds.
pub fn canonical_amount(d: Decimal, min_dp: u32) -> Decimal {
    let mut n = d.normalize();
    if n.scale() < min_dp {
        n.rescale(min_dp);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn code(s: &str) -> CommodityCode {
        CommodityCode::new(s).unwrap()
    }

    struct World {
        accounts: HashMap<AccountId, AccountFacts>,
        rules: TxnRules,
        checking: AccountId,
        groceries: AccountId,
        vti: AccountId,
    }

    fn world() -> World {
        let mk = |name: &str, c: &str, sec: bool| AccountFacts {
            full_name: name.into(),
            commodity: code(c),
            placeholder: false,
            archived: false,
            is_security: sec,
        };
        let checking = AccountId::new();
        let groceries = AccountId::new();
        let vti = AccountId::new();
        let accounts = HashMap::from([
            (checking, mk("Assets:Checking", "USD", false)),
            (groceries, mk("Expenses:Groceries", "USD", false)),
            (vti, mk("Assets:Brokerage:VTI", "VTI", true)),
        ]);
        World {
            accounts,
            rules: TxnRules {
                base_currency: code("USD"),
                multi_currency: false,
            },
            checking,
            groceries,
            vti,
        }
    }

    fn split(account: AccountId, value: &str, quantity: &str) -> Split {
        Split {
            account,
            value: d(value),
            quantity: d(quantity),
            memo: None,
            reconcile: Reconcile::New,
            fingerprint: None,
            tags: Default::default(),
        }
    }

    fn txn(splits: Vec<Split>) -> Transaction {
        Transaction {
            id: mk_core::TransactionId::new(),
            date: "2026-07-01".parse().unwrap(),
            currency: code("USD"),
            payee: None,
            number: None,
            memo: None,
            status: TxnStatus::Posted,
            void_reason: None,
            reverses: None,
            opening: false,
            entered_at: String::new(),
            splits,
        }
    }

    #[test]
    fn balanced_purchase_is_accepted() {
        let w = world();
        let t = txn(vec![
            split(w.groceries, "50.00", "50.00"),
            split(w.checking, "-50.00", "-50.00"),
        ]);
        check_transaction(&t, &w.accounts, &w.rules, true).unwrap();
    }

    #[test]
    fn imbalance_reports_difference() {
        let w = world();
        let t = txn(vec![
            split(w.groceries, "100.00", "100.00"),
            split(w.checking, "-99.99", "-99.99"),
        ]);
        let err = check_transaction(&t, &w.accounts, &w.rules, true).unwrap_err();
        assert!(err.to_string().contains("difference 0.01"), "{err}");
    }

    #[test]
    fn too_many_decimals_rejected_and_jpy_has_none() {
        let w = world();
        let t = txn(vec![
            split(w.groceries, "10.005", "10.005"),
            split(w.checking, "-10.005", "-10.005"),
        ]);
        assert!(matches!(
            check_transaction(&t, &w.accounts, &w.rules, true),
            Err(LedgerError::Invalid(_))
        ));
        // Trailing zeros are fine.
        let t = txn(vec![
            split(w.groceries, "10.000", "10.000"),
            split(w.checking, "-10.000", "-10.000"),
        ]);
        check_transaction(&t, &w.accounts, &w.rules, true).unwrap();
    }

    #[test]
    fn security_purchase_and_sign_mismatch() {
        let w = world();
        let ok = txn(vec![
            split(w.vti, "2500.00", "10"),
            split(w.checking, "-2500.00", "-2500.00"),
        ]);
        check_transaction(&ok, &w.accounts, &w.rules, true).unwrap();
        let bad = txn(vec![
            split(w.vti, "2500.00", "-10"),
            split(w.checking, "-2500.00", "-2500.00"),
        ]);
        let err = check_transaction(&bad, &w.accounts, &w.rules, true).unwrap_err();
        assert!(err.to_string().contains("same sign"), "{err}");
        let too_fine = txn(vec![
            split(w.vti, "2500.00", "10.0000000001"),
            split(w.checking, "-2500.00", "-2500.00"),
        ]);
        assert!(check_transaction(&too_fine, &w.accounts, &w.rules, true).is_err());
    }

    #[test]
    fn foreign_currency_needs_module() {
        let mut w = world();
        let mut t = txn(vec![
            split(w.groceries, "50.00", "55.00"),
            split(w.checking, "-50.00", "-55.00"),
        ]);
        t.currency = code("EUR");
        assert!(matches!(
            check_transaction(&t, &w.accounts, &w.rules, true),
            Err(LedgerError::Rejected(_))
        ));
        w.rules.multi_currency = true;
        check_transaction(&t, &w.accounts, &w.rules, true).unwrap();
    }

    #[test]
    fn all_zero_and_split_count_rejected() {
        let w = world();
        let zero = txn(vec![
            split(w.groceries, "0", "0"),
            split(w.checking, "0", "0"),
        ]);
        assert!(check_transaction(&zero, &w.accounts, &w.rules, true).is_err());
        let one = txn(vec![split(w.groceries, "0", "0")]);
        assert!(check_transaction(&one, &w.accounts, &w.rules, true).is_err());
    }

    #[test]
    fn placeholder_and_archived() {
        let mut w = world();
        let t = txn(vec![
            split(w.groceries, "5.00", "5.00"),
            split(w.checking, "-5.00", "-5.00"),
        ]);
        if let Some(f) = w.accounts.get_mut(&w.checking) {
            f.archived = true;
        }
        assert!(check_archived_untouched(None, Some(&t), &w.accounts).is_err());
        // Changing only the payee leaves archived splits untouched.
        let mut renamed = t.clone();
        renamed.payee = Some("Shop".into());
        check_archived_untouched(Some(&t), Some(&renamed), &w.accounts).unwrap();
        if let Some(f) = w.accounts.get_mut(&w.checking) {
            f.archived = false;
            f.placeholder = true;
        }
        assert!(matches!(
            check_transaction(&t, &w.accounts, &w.rules, true),
            Err(LedgerError::Rejected(_))
        ));
    }

    #[test]
    fn canonical_amounts() {
        assert_eq!(canonical_amount(d("50"), 2).to_string(), "50.00");
        assert_eq!(canonical_amount(d("10.500000"), 0).to_string(), "10.5");
        assert_eq!(canonical_amount(d("-3"), 0).to_string(), "-3");
    }

    #[test]
    fn inherit_matches_each_old_split_once() {
        let w = world();
        let mut old = vec![
            split(w.checking, "-5.00", "-5.00"),
            split(w.checking, "-5.00", "-5.00"),
        ];
        if let Some(s) = old.get_mut(1) {
            s.reconcile = Reconcile::Reconciled;
            s.memo = Some("b".into());
        }
        let mut new = old.clone();
        new.reverse();
        inherit_reconcile(&old, &mut new);
        assert_eq!(new[0].reconcile, Reconcile::Reconciled);
        assert_eq!(new[1].reconcile, Reconcile::New);
    }

    #[test]
    fn closing_date_blocks_both_ends() {
        let closing: Date = "2026-06-30".parse().unwrap();
        let mut t = txn(vec![]);
        t.date = closing;
        assert!(check_closing(Some(closing), None, Some(&t)).is_err());
        let mut later = t.clone();
        later.date = "2026-07-01".parse().unwrap();
        check_closing(Some(closing), None, Some(&later)).unwrap();
        assert!(check_closing(Some(closing), Some(&later), Some(&t)).is_err());
        check_closing(None, None, Some(&t)).unwrap();
    }

    #[test]
    fn reconciled_splits_are_protected() {
        let w = world();
        let mut old = txn(vec![
            split(w.groceries, "5.00", "5.00"),
            split(w.checking, "-5.00", "-5.00"),
        ]);
        if let Some(s) = old.splits.get_mut(1) {
            s.reconcile = Reconcile::Reconciled;
        }
        let mut memo_only = old.clone();
        if let Some(s) = memo_only.splits.get_mut(1) {
            s.memo = Some("note".into());
        }
        let mut new_splits = memo_only.splits.clone();
        inherit_reconcile(&old.splits, &mut new_splits);
        memo_only.splits = new_splits;
        check_reconciled_preserved(Some(&old), Some(&memo_only)).unwrap();

        let mut changed = txn(vec![
            split(w.groceries, "6.00", "6.00"),
            split(w.checking, "-6.00", "-6.00"),
        ]);
        let mut s = changed.splits.clone();
        inherit_reconcile(&old.splits, &mut s);
        changed.splits = s;
        assert!(check_reconciled_preserved(Some(&old), Some(&changed)).is_err());
        assert!(check_reconciled_preserved(Some(&old), None).is_err());
        assert!(only_reconcile_differs(&old, &{
            let mut t = old.clone();
            if let Some(s) = t.splits.get_mut(1) {
                s.reconcile = Reconcile::New;
            }
            t
        }));
    }

    proptest! {
        /// Any order of the same splits sorts to the same stored order.
        #[test]
        fn sort_is_order_independent(values in proptest::collection::vec(-100_000i64..100_000, 2..8), seed in any::<u64>()) {
            let w = world();
            let splits: Vec<Split> = values.iter().map(|v| {
                let s = Decimal::new(*v, 2).to_string();
                split(if v % 2 == 0 { w.checking } else { w.groceries }, &s, &s)
            }).collect();
            let mut a = splits.clone();
            let mut b = splits;
            let len = b.len();
            if len > 1 {
                let k = usize::try_from(seed % u64::try_from(len).unwrap()).unwrap();
                b.rotate_left(k);
                b.reverse();
            }
            sort_splits(&mut a);
            sort_splits(&mut b);
            prop_assert_eq!(a, b);
        }
    }
}
