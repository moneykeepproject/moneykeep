//! chart-presets.md seed tests (CP1-CP11, validation rules 1-9, CP22).
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use super::seed::{ChartAccount, Seed, validate};
use super::tax_summary::tag_cmp;
use super::*;

fn seed() -> Seed {
    Seed::parse(seed::SEED_TOML).unwrap()
}

fn resolved(id: &str, modules: &[&str]) -> Vec<ChartAccount> {
    let s = seed();
    let p = s.preset(id).unwrap().clone();
    s.resolve(&p, modules)
}

fn tag(chart: &[ChartAccount], code: &str) -> Option<String> {
    chart
        .iter()
        .find(|a| a.code == code)
        .unwrap_or_else(|| panic!("no account {code}"))
        .tax_line
        .clone()
}

fn has(chart: &[ChartAccount], code: &str) -> bool {
    chart.iter().any(|a| a.code == code)
}

fn refused(mutate: impl FnOnce(&mut Seed), expect: &str) {
    let mut s = seed();
    mutate(&mut s);
    let e = validate(&s).unwrap_err();
    assert!(e.contains(expect), "expected {expect:?} in {e:?}");
}

fn account_mut<'a>(s: &'a mut Seed, code: &str, pack: &str) -> &'a mut seed::SeedAccount {
    s.accounts
        .iter_mut()
        .find(|a| a.code == code && a.pack == pack)
        .unwrap()
}

#[test]
fn cp1_shipped_seed_passes_validation_and_every_preset_resolves() {
    validate(&seed()).unwrap();
    assert!(seed::shipped().is_ok());
    let s = seed();
    let all = s.pack_modules();
    assert_eq!(s.presets.len(), 11);
    for p in &s.presets {
        assert!(!s.resolve(p, &[]).is_empty());
        assert!(s.resolve(p, &all).len() >= s.resolve(p, &[]).len());
    }
    // Seed order of the picker, with the new preset after schedule_c.
    let ids: Vec<&str> = s.presets.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "personal",
            "sole_proprietor",
            "personal_1040",
            "schedule_c",
            "personal_1040_schedule_c",
            "s_corp",
            "partnership",
            "c_corp",
            "small_business",
            "nonprofit",
            "minimal",
        ]
    );
}

#[test]
fn cp2_every_preset_has_the_required_core_accounts() {
    let s = seed();
    for p in &s.presets {
        let c = s.resolve(p, &[]);
        let find = |code: &str| c.iter().find(|a| a.code == code).unwrap();
        let ob = find("3900");
        assert_eq!(ob.name, "Opening Balances");
        assert_eq!(ob.role.as_deref(), Some("opening_balances"));
        assert_eq!(find("4999").name, "Uncategorized Income");
        assert_eq!(find("8999").name, "Uncategorized Expense");
    }
}

#[test]
fn cp3_module_packs_of_small_business() {
    let alone = resolved("small_business", &[]);
    assert!(!has(&alone, "1200"));
    let with = resolved("small_business", &["invoicing", "bills"]);
    let ar = with.iter().find(|a| a.code == "1200").unwrap();
    assert_eq!(ar.name, "Accounts Receivable");
    let ap = with.iter().find(|a| a.code == "2000").unwrap();
    assert_eq!(ap.name, "Accounts Payable");
    assert!(!has(&with, "2250"));
    let tax = resolved("small_business", &["sales-tax"]);
    assert_eq!(
        tax.iter().find(|a| a.code == "2250").unwrap().name,
        "Sales Tax Payable"
    );
    // A module already applied adds nothing more.
    let again = resolved("small_business", &["invoicing", "bills", "invoicing"]);
    assert_eq!(again.len(), with.len());
}

#[test]
fn cp4_subtypes() {
    use mk_core::AccountSubtype as S;
    let sub = |c: &[ChartAccount], code: &str| c.iter().find(|a| a.code == code).unwrap().subtype;
    let b = resolved("small_business", &["invoicing", "sales-tax"]);
    assert_eq!(sub(&b, "1650"), S::FixedAsset);
    assert_eq!(sub(&b, "1250"), S::Clearing);
    assert_eq!(sub(&b, "2250"), S::TaxPayable);
    assert_eq!(sub(&b, "3200"), S::RetainedEarnings);
    let np = resolved("nonprofit", &[]);
    assert_eq!(sub(&np, "3000"), S::NetAssets);
    assert_eq!(sub(&np, "3100"), S::NetAssets);
    let sc = resolved("schedule_c", &[]);
    assert_eq!(sub(&sc, "5010"), S::CostOfGoods);
}

#[test]
fn cp5_personal_1040_tags() {
    let s = seed();
    assert_eq!(s.preset("personal_1040").unwrap().tax_year, Some(2025));
    let c = resolved("personal_1040", &[]);
    assert_eq!(tag(&c, "4010").as_deref(), Some("1040 L1a"));
    assert_eq!(tag(&c, "4700").as_deref(), Some("1040 L2b"));
    assert_eq!(tag(&c, "6320").as_deref(), Some("Sch A L5b"));
    assert_eq!(tag(&c, "6510").as_deref(), Some("Sch A L11"));
}

#[test]
fn cp6_schedule_c_tags_and_combined_preset() {
    let c = resolved("schedule_c", &[]);
    assert_eq!(tag(&c, "4500").as_deref(), Some("Sch C L1"));
    assert_eq!(tag(&c, "7031").as_deref(), Some("Sch C L27b"));
    assert_eq!(tag(&c, "7040").as_deref(), Some("Sch C L30"));
    let s = seed();
    let p = s.preset("personal_1040_schedule_c").unwrap();
    assert_eq!(p.name, "Personal — Form 1040 + Schedule C");
    assert_eq!(p.tax_year, Some(2025));
    assert_eq!(p.modules, ["investments"]);
    let all = s.pack_modules();
    let both = s.resolve(p, &all);
    assert_eq!(
        both.iter().find(|a| a.code == "4010").unwrap().name,
        "W-2 Wages"
    );
    assert_eq!(
        both.iter().find(|a| a.code == "4500").unwrap().name,
        "Gross Receipts"
    );
    // No code, name or role clash (validation rule 6 on this chart).
    validate(&s).unwrap();
}

#[test]
fn cp7_s_corp_tags_and_overrides() {
    let c = resolved("s_corp", &["invoicing"]);
    assert_eq!(tag(&c, "7007").as_deref(), Some("1120-S L7"));
    assert_eq!(tag(&c, "7021").as_deref(), Some("1120-S L20"));
    assert_eq!(tag(&c, "3300").as_deref(), Some("Sch K L16d"));
    assert_eq!(tag(&c, "2650").as_deref(), Some("Sch L L19"));
    assert_eq!(tag(&c, "4700").as_deref(), Some("Sch K L4"));
    assert_eq!(tag(&c, "1200").as_deref(), Some("Sch L L2a"));
    let p = resolved("personal_1040", &[]);
    assert_eq!(tag(&p, "4700").as_deref(), Some("1040 L2b"));
}

#[test]
fn cp8_partnership_tags() {
    let c = resolved("partnership", &["bills"]);
    assert_eq!(tag(&c, "7010").as_deref(), Some("1065 L10"));
    assert_eq!(tag(&c, "7021").as_deref(), Some("1065 L21"));
    assert_eq!(tag(&c, "3010").as_deref(), Some("Sch L L21"));
    assert_eq!(tag(&c, "3110").as_deref(), Some("Sch K L19a"));
    assert_eq!(tag(&c, "2650").as_deref(), Some("Sch L L19a"));
    assert_eq!(tag(&c, "4700").as_deref(), Some("Sch K L5"));
    assert_eq!(tag(&c, "2000").as_deref(), Some("Sch L L15"));
}

#[test]
fn cp9_c_corp_tags() {
    let c = resolved("c_corp", &["invoicing", "bills"]);
    assert_eq!(tag(&c, "7012").as_deref(), Some("1120 L12"));
    assert_eq!(tag(&c, "7019").as_deref(), Some("1120 L19"));
    assert_eq!(tag(&c, "7030").as_deref(), Some("1120 L26"));
    assert_eq!(tag(&c, "8000").as_deref(), Some("Sch M-1 L2"));
    assert_eq!(tag(&c, "3200").as_deref(), Some("Sch L L25"));
    assert_eq!(tag(&c, "3300").as_deref(), Some("Sch M-2 L5a"));
    assert_eq!(tag(&c, "4700").as_deref(), Some("1120 L5"));
}

#[test]
fn cp10_rule_9_and_investment_overrides() {
    validate(&seed()).unwrap();
    let s = resolved("s_corp", &["investments"]);
    assert_eq!(tag(&s, "4710").as_deref(), Some("Sch K L5a"));
    assert_eq!(tag(&s, "4720").as_deref(), Some("Sch K L7/8a"));
    assert_eq!(tag(&s, "8850").as_deref(), Some("1120-S L20"));
    let c = resolved("c_corp", &["investments"]);
    assert_eq!(tag(&c, "4710").as_deref(), Some("1120 L4"));
    let p = resolved("personal_1040", &["investments"]);
    assert_eq!(tag(&p, "4720").as_deref(), Some("Sch D → 1040 L7a"));
}

#[test]
fn cp11_mutated_seeds_are_refused() {
    refused(
        |s| account_mut(s, "6000", "personal").tax_line = Some("Sch A L1".into()),
        "a header has no parent and no tax line",
    );
    refused(
        |s| account_mut(s, "6010", "personal").parent = Some("6020".into()),
        "is not a header",
    );
    refused(
        |s| account_mut(s, "1000", "core").code = "2100".into(),
        "outside the range",
    );
    refused(
        |s| account_mut(s, "1510", "personal").code = "1500".into(),
        "code 1500 is used twice",
    );
    refused(
        |s| s.presets[0].description = "x".repeat(150),
        "at least 200",
    );
    refused(
        |s| {
            let p = s.presets.iter_mut().find(|p| p.id == "s_corp").unwrap();
            p.tax_overrides.insert("1999".into(), "Sch L L1".into());
        },
        "tax override for missing code 1999",
    );
    refused(
        |s| account_mut(s, "6010", "personal").subtype = "credit-card".into(),
        "does not belong to type expense",
    );
}

#[test]
fn validation_rules_each_refuse() {
    // Rule 1.
    refused(|s| s.schema = 2, "schema is 2");
    // Rule 2: unknown pack, unknown module.
    refused(|s| s.presets[0].packs.push("nope".into()), "unknown pack");
    refused(
        |s| s.presets[0].modules.push("nope".into()),
        "unknown module",
    );
    refused(
        |s| {
            s.module_packs.insert("nope".into(), "sales".into());
        },
        "unknown module",
    );
    // Rule 3: code length.
    refused(
        |s| account_mut(s, "1000", "core").code = "100".into(),
        "exactly 4 digits",
    );
    // Rule 4: cost-of-goods outside 5000-5999.
    refused(
        |s| account_mut(s, "6010", "personal").subtype = "cost-of-goods".into(),
        "cost-of-goods",
    );
    // Rule 5: a parent of another type.
    refused(
        |s| account_mut(s, "6010", "personal").parent = Some("1500".into()),
        "is not a header",
    );
    // Rule 6: duplicate names and roles.
    refused(
        |s| account_mut(s, "1050", "core").name = "checking".into(),
        "used twice",
    );
    refused(
        |s| account_mut(s, "1050", "core").role = Some("receivable".into()),
        "held twice",
    );
    // Rule 7: a colon in a name, a long tax line.
    refused(
        |s| account_mut(s, "1050", "core").name = "Sav:ings".into(),
        "cannot contain ':'",
    );
    refused(
        |s| account_mut(s, "4700", "core").tax_line = Some("x".repeat(41)),
        "tax line",
    );
    // Rule 8: core roles.
    refused(
        |s| account_mut(s, "8999", "core").role = None,
        "uncategorized_expense",
    );
    refused(|s| s.presets[0].packs.retain(|p| p != "core"), "core pack");
    // Rule 9: an untagged business expense.
    refused(
        |s| account_mut(s, "7008", "schedc").tax_line = None,
        "has no tax line",
    );
}

#[test]
fn cp22_natural_order_of_tags() {
    let mut tags = vec!["Sch C L27b", "Sch C L16b", "Sch C L8", "Sch C L16a"];
    tags.sort_by(|a, b| tag_cmp(a, b));
    assert_eq!(tags, ["Sch C L8", "Sch C L16a", "Sch C L16b", "Sch C L27b"]);
    let mut groups = vec![
        "Form 2441",
        "Sch A L5b",
        "1040 L1a",
        "zzz",
        "Sch 1-A L13",
        "Sch 1 L1",
        "1120-S L7",
        "1120 L5",
    ];
    groups.sort_by(|a, b| tag_cmp(a, b));
    assert_eq!(
        groups,
        [
            "1040 L1a",
            "Sch 1-A L13",
            "Sch 1 L1",
            "Sch A L5b",
            "1120-S L7",
            "1120 L5",
            "Form 2441",
            "zzz"
        ]
    );
    let mut b = vec!["1040 L10", "1040 L2b"];
    b.sort_by(|x, y| tag_cmp(x, y));
    assert_eq!(b, ["1040 L2b", "1040 L10"]);
}

#[test]
fn seed_changes_from_the_owners_file() {
    let s = seed();
    // Renames and removal.
    let all: Vec<&seed::SeedAccount> = s.accounts.iter().collect();
    let name_of = |code: &str| all.iter().find(|a| a.code == code).map(|a| a.name.clone());
    assert_eq!(name_of("3900").as_deref(), Some("Opening Balances"));
    assert_eq!(name_of("1250").as_deref(), Some("Payments to Deposit"));
    assert_eq!(name_of("2270").as_deref(), Some("Reimbursements Payable"));
    assert_eq!(name_of("4720").as_deref(), Some("Realized Gains"));
    assert!(name_of("4730").is_none());
    assert!(!seed::SEED_TOML.contains("Opening Balance Equity"));
    assert!(!seed::SEED_TOML.contains("system = true"));
    assert!(!seed::SEED_TOML.contains("contra = true"));
    // Module ids.
    assert_eq!(s.module_pack("invoicing"), Some("sales"));
    assert_eq!(s.module_pack("sales-tax"), Some("sales_tax"));
    assert_eq!(s.module_pack("bills"), Some("purchasing"));
    assert_eq!(s.module_pack("expenses"), Some("expenses"));
    assert_eq!(s.module_pack("investments"), Some("investments"));
    // Preset modules.
    let mods = |id: &str| s.preset(id).unwrap().modules.clone();
    for id in [
        "personal",
        "sole_proprietor",
        "personal_1040",
        "personal_1040_schedule_c",
    ] {
        assert_eq!(mods(id), ["investments"], "{id}");
    }
    assert_eq!(mods("nonprofit"), ["classes", "funds"]);
    assert!(mods("small_business").is_empty());
    // Every Retained Earnings has the role.
    for a in s.accounts.iter().filter(|a| a.name == "Retained Earnings") {
        assert_eq!(a.role.as_deref(), Some("retained_earnings"));
    }
    // T5-T10.
    let acc = |code: &str, pack: &str| {
        s.accounts
            .iter()
            .find(|a| a.code == code && a.pack == pack)
            .unwrap()
            .clone()
    };
    assert_eq!(
        acc("4530", "f1120s").tax_line.as_deref(),
        Some("1120-S L4 / Sch K L9")
    );
    assert_eq!(
        acc("4530", "f1065").tax_line.as_deref(),
        Some("1065 L6 / Sch K L10")
    );
    assert_eq!(acc("4640", "f1120").tax_line.as_deref(), Some("1120 L9"));
    assert!(
        acc("4640", "f1120")
            .note
            .unwrap()
            .contains("Schedule D to L8")
    );
    assert_eq!(
        acc("7040", "f1120s").tax_line.as_deref(),
        Some("Sch K L12a/L12b")
    );
    assert_eq!(
        acc("7040", "f1065").tax_line.as_deref(),
        Some("Sch K L13a/L13b")
    );
    assert!(
        acc("4110", "form1040")
            .note
            .unwrap()
            .starts_with("Taxable only to the extent")
    );
    let sc = s.preset("schedule_c").unwrap();
    assert!(
        sc.description
            .contains("Choose \"Personal — Form 1040 + Schedule C\"")
    );
}
