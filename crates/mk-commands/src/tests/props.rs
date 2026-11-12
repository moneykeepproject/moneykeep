//! Property tests:
//! - ledger-balances.md AT-5: random creates, updates, voids and deletes keep the
//!   trial balance at zero in every currency.
//! - ledger-audit-undo.md AT-7: undoing every command of a random sequence, newest
//!   first, returns the book to its initial state.

use proptest::prelude::*;

use super::*;
use crate::balances::compute_trial;

const ACCOUNTS: &[&str] = &[
    "Assets:Checking",
    "Expenses:Groceries",
    "Liabilities:Visa",
    "Opening Balances",
    "Income",
];

#[derive(Debug, Clone)]
enum Op {
    /// Create a transaction: splits as (account index, cents) plus a balancing split.
    Create {
        day: u8,
        legs: Vec<(usize, i64)>,
        balance_to: usize,
        eur: bool,
    },
    Update {
        pick: usize,
        day: u8,
        legs: Vec<(usize, i64)>,
        balance_to: usize,
    },
    Void {
        pick: usize,
    },
    Unvoid {
        pick: usize,
    },
    Delete {
        pick: usize,
    },
    Reconcile {
        pick: usize,
        split: usize,
        state: u8,
    },
    Reverse {
        pick: usize,
    },
    Close {
        day: Option<u8>,
    },
    Module {
        on: bool,
    },
    AccountCreate {
        n: u8,
    },
    AccountRename {
        pick: usize,
        n: u8,
    },
}

fn legs() -> impl Strategy<Value = Vec<(usize, i64)>> {
    proptest::collection::vec((0..ACCOUNTS.len(), -100_000i64..100_000), 1..4)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (1u8..28, legs(), 0..ACCOUNTS.len(), any::<bool>())
            .prop_map(|(day, legs, balance_to, eur)| Op::Create { day, legs, balance_to, eur }),
        2 => (any::<usize>(), 1u8..28, legs(), 0..ACCOUNTS.len())
            .prop_map(|(pick, day, legs, balance_to)| Op::Update { pick, day, legs, balance_to }),
        1 => any::<usize>().prop_map(|pick| Op::Void { pick }),
        1 => any::<usize>().prop_map(|pick| Op::Unvoid { pick }),
        1 => any::<usize>().prop_map(|pick| Op::Delete { pick }),
        1 => (any::<usize>(), 0usize..4, 0u8..3)
            .prop_map(|(pick, split, state)| Op::Reconcile { pick, split, state }),
        1 => any::<usize>().prop_map(|pick| Op::Reverse { pick }),
        1 => proptest::option::of(1u8..28).prop_map(|day| Op::Close { day }),
        1 => any::<bool>().prop_map(|on| Op::Module { on }),
        1 => (0u8..5).prop_map(|n| Op::AccountCreate { n }),
        1 => (any::<usize>(), 0u8..5).prop_map(|(pick, n)| Op::AccountRename { pick, n }),
    ]
}

fn date(day: u8) -> String {
    format!("2026-07-{day:02}")
}

fn splits_json(legs: &[(usize, i64)], balance_to: usize, eur: bool) -> Value {
    let mut total = 0i64;
    let mut out = Vec::new();
    for (acc, cents) in legs {
        total += cents;
        let v = mk_core::Decimal::new(*cents, 2).to_string();
        let q = if eur {
            Some(mk_core::Decimal::new(cents * 11, 3).round_dp(2).to_string())
        } else {
            None
        };
        out.push(json!({"account": ACCOUNTS[*acc], "value": v, "quantity": q}));
    }
    let v = mk_core::Decimal::new(-total, 2).to_string();
    let q = if eur {
        Some(
            mk_core::Decimal::new(-total * 11, 3)
                .round_dp(2)
                .to_string(),
        )
    } else {
        None
    };
    out.push(json!({"account": ACCOUNTS[balance_to], "value": v, "quantity": q}));
    Value::Array(out)
}

fn txn_ids(b: &mut Book) -> Vec<Value> {
    b.run("txn.list", json!({"include_voided": true}))
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect()
}

fn pick<T: Clone>(items: &[T], i: usize) -> Option<T> {
    if items.is_empty() {
        None
    } else {
        items.get(i % items.len()).cloned()
    }
}

fn new_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Income", "type": "income"}));
    // Ops run as their own actor, so setup is not part of what they undo.
    b.a = Actor("prop".into());
    b
}

/// Apply one op. Rejections are fine (the op was invalid in that state); anything
/// else is a bug. Returns whether the op changed the book.
fn apply(b: &mut Book, op: &Op) -> bool {
    let (cmd, args) = match op {
        Op::Create {
            day,
            legs,
            balance_to,
            eur,
        } => {
            let mut a = json!({"date": date(*day), "splits": splits_json(legs, *balance_to, *eur)});
            if *eur {
                a["currency"] = json!("EUR");
            }
            ("txn.create", a)
        }
        Op::Update {
            pick: p,
            day,
            legs,
            balance_to,
        } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            (
                "txn.update",
                json!({"id": id, "date": date(*day), "splits": splits_json(legs, *balance_to, false)}),
            )
        }
        Op::Void { pick: p } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            ("txn.void", json!({"id": id}))
        }
        Op::Unvoid { pick: p } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            ("txn.unvoid", json!({"id": id}))
        }
        Op::Delete { pick: p } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            ("txn.delete", json!({"id": id}))
        }
        Op::Reconcile {
            pick: p,
            split,
            state,
        } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            let st = ["n", "c", "r"][usize::from(*state)];
            (
                "txn.set_reconcile",
                json!({"splits": [{"id": id, "split": split, "state": st}]}),
            )
        }
        Op::Reverse { pick: p } => {
            let Some(id) = pick(&txn_ids(b), *p) else {
                return false;
            };
            ("txn.reverse", json!({"id": id, "date": "2026-07-28"}))
        }
        Op::Close { day } => ("book.set_closing_date", json!({"date": day.map(date)})),
        Op::Module { on } => (
            if *on {
                "module.enable"
            } else {
                "module.disable"
            },
            json!({"module": "multi-currency"}),
        ),
        Op::AccountCreate { n } => (
            "account.create",
            json!({"name": format!("Extra {n}"), "type": "expense", "parent": "Expenses"}),
        ),
        Op::AccountRename { pick: p, n } => {
            let names = ["Assets:Checking", "Expenses:Groceries", "Liabilities:Visa"];
            let Some(acc) = pick(&names, *p) else {
                return false;
            };
            (
                "account.update",
                json!({"account": acc, "description": format!("note {n}")}),
            )
        }
    };
    match b.try_run(cmd, args.clone()) {
        Ok(_) => true,
        Err(CommandError::Rejected(_) | CommandError::InvalidArgs(_)) => false,
        Err(e) => panic!("{cmd} {args}: unexpected error {e}"),
    }
}

fn assert_trial_zero(b: &mut Book) {
    let actor = b.a.clone();
    let _ = actor;
    let totals =
        b.s.read(|tx| -> CommandResult<_> {
            let a = Actor("t".into());
            let ctx = Ctx::new(tx, &a);
            Ok(compute_trial(&ctx, None)?.value_totals)
        })
        .unwrap();
    for (c, v) in totals {
        assert!(v.is_zero(), "split values in {c} sum to {v}");
    }
    // The command agrees (it reports an integrity error otherwise).
    b.run("report.trial_balance", json!({}));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn balances_at5_trial_balance_always_zero(ops in proptest::collection::vec(op(), 1..25)) {
        let mut b = new_book();
        for op in &ops {
            apply(&mut b, op);
            assert_trial_zero(&mut b);
        }
    }

    #[test]
    fn audit_at7_undo_all_restores_initial_state(ops in proptest::collection::vec(op(), 1..20)) {
        let mut b = new_book();
        let initial = b.state();
        let mut applied = 0usize;
        for op in &ops {
            if apply(&mut b, op) {
                applied += 1;
            }
        }
        for i in 0..applied {
            if let Err(e) = b.try_run("history.undo", json!({})) {
                panic!("undo {i} of {applied} failed: {e}\nops: {ops:?}");
            }
        }
        prop_assert_eq!(b.err("history.undo", json!({})).to_string(), "nothing to undo");
        prop_assert_eq!(b.state(), initial);
        assert_trial_zero(&mut b);
    }
}
