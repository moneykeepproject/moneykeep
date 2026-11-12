//! The built-in chart seed (chart-presets.md "Seed format"): presets, packs and
//! accounts compiled into the program, with the seed validation rules.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use mk_core::{AccountSubtype, AccountType};
use serde::Deserialize;

/// The shipped seed.
pub(crate) const SEED_TOML: &str = include_str!("../../charts/default_charts.toml");

/// Presets whose every postable income and expense account needs a tax line
/// (validation rule 9).
const FULLY_TAGGED: &[&str] = &["schedule_c", "s_corp", "partnership", "c_corp"];
/// A preset in which only the accounts of one pack need a tax line.
const PACK_TAGGED: &[(&str, &str)] = &[("personal_1040_schedule_c", "schedc")];
/// Packs rule 9 does not check in a fully tagged preset: in a sole proprietor's
/// book the investments pack holds personal (Form 1040) items, and the spec's seed
/// changes give it no Schedule C tags (8850 Investment Fees is not deductible on
/// Schedule C). The corporate presets tag it through T2-T4.
const RULE9_EXEMPT: &[(&str, &str)] = &[("schedule_c", "investments")];
/// Roles the `core` pack must hold (rule 8).
const CORE_ROLES: &[&str] = &[
    super::roles::OPENING_BALANCES,
    super::roles::UNCATEGORIZED_INCOME,
    super::roles::UNCATEGORIZED_EXPENSE,
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Seed {
    pub schema: i64,
    #[serde(rename = "preset", default)]
    pub presets: Vec<Preset>,
    #[serde(default)]
    pub module_packs: BTreeMap<String, String>,
    #[serde(rename = "account", default)]
    pub accounts: Vec<SeedAccount>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preset {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub description: String,
    pub packs: Vec<String>,
    #[serde(default)]
    pub tax_year: Option<i64>,
    #[serde(default)]
    pub tax_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeedAccount {
    pub code: String,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: String,
    pub subtype: String,
    pub pack: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub header: bool,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tax_line: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// One account of a resolved chart (a preset plus module packs), with the preset's
/// tax-line overrides applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChartAccount {
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
    pub subtype: AccountSubtype,
    pub pack: String,
    pub parent: Option<String>,
    pub header: bool,
    pub role: Option<String>,
    pub tax_line: Option<String>,
    pub note: Option<String>,
}

impl Seed {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("the chart seed does not parse: {e}"))
    }

    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The pack a module adds, if any.
    pub fn module_pack(&self, module: &str) -> Option<&str> {
        self.module_packs.get(module).map(String::as_str)
    }

    /// Accounts of `packs` (in seed order), typed. Unknown types or subtypes are
    /// reported by [`validate`]; here they are skipped.
    fn accounts_of<'s>(
        &'s self,
        packs: &[&str],
    ) -> Vec<(&'s SeedAccount, AccountType, AccountSubtype)> {
        self.accounts
            .iter()
            .filter(|a| packs.contains(&a.pack.as_str()))
            .filter_map(|a| {
                let t = a.account_type.parse().ok()?;
                let s = a.subtype.parse().ok()?;
                Some((a, t, s))
            })
            .collect()
    }

    /// The resolved chart of `preset` plus the packs of `modules`, in code order
    /// (headers first among equal codes), with tax overrides applied.
    pub fn resolve(&self, preset: &Preset, modules: &[&str]) -> Vec<ChartAccount> {
        let mut packs: Vec<&str> = preset.packs.iter().map(String::as_str).collect();
        for m in modules {
            if let Some(p) = self.module_pack(m)
                && !packs.contains(&p)
            {
                packs.push(p);
            }
        }
        let mut out: Vec<ChartAccount> = self
            .accounts_of(&packs)
            .into_iter()
            .map(|(a, t, s)| {
                let tax_line = match preset.tax_overrides.get(&a.code) {
                    Some(o) if o.is_empty() => None,
                    Some(o) => Some(o.clone()),
                    None => a.tax_line.clone(),
                };
                ChartAccount {
                    code: a.code.clone(),
                    name: a.name.clone(),
                    account_type: t,
                    subtype: s,
                    pack: a.pack.clone(),
                    parent: a.parent.clone(),
                    header: a.header,
                    role: a.role.clone(),
                    tax_line,
                    note: a.note.clone(),
                }
            })
            .collect();
        out.sort_by(|a, b| a.code.cmp(&b.code).then(b.header.cmp(&a.header)));
        out
    }

    /// Every module id that has a pack.
    pub fn pack_modules(&self) -> Vec<&str> {
        self.module_packs.keys().map(String::as_str).collect()
    }
}

/// The shipped seed, parsed and validated once. A bad seed is refused: every chart
/// command reports the problem instead of using it.
pub(crate) fn shipped() -> Result<&'static Seed, String> {
    static SEED: OnceLock<Result<Seed, String>> = OnceLock::new();
    SEED.get_or_init(|| {
        let seed = Seed::parse(SEED_TOML)?;
        validate(&seed)?;
        Ok(seed)
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn code_range_ok(t: AccountType, code: u32) -> bool {
    match t {
        AccountType::Asset => (1000..=1999).contains(&code),
        AccountType::Liability => (2000..=2999).contains(&code),
        AccountType::Equity => (3000..=3999).contains(&code),
        AccountType::Income => (4000..=4999).contains(&code),
        AccountType::Expense => (5000..=8999).contains(&code),
    }
}

fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
    let n = s.trim().chars().count();
    if n == 0 || n > max || s.chars().any(char::is_control) {
        return Err(format!(
            "{what} {s:?} must be 1-{max} characters of printable text"
        ));
    }
    Ok(())
}

/// Check the seed against the rules of chart-presets.md ("Validation"). The first
/// problem found is returned.
pub(crate) fn validate(seed: &Seed) -> Result<(), String> {
    // Rule 1.
    if seed.schema != 1 {
        return Err(format!("schema is {}, expected 1", seed.schema));
    }
    let packs: HashSet<&str> = seed.accounts.iter().map(|a| a.pack.as_str()).collect();
    // Rule 2.
    let mut ids = HashSet::new();
    for p in &seed.presets {
        if p.id.is_empty()
            || !p
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "preset id {:?} must use lowercase letters, digits and _",
                p.id
            ));
        }
        if !ids.insert(p.id.as_str()) {
            return Err(format!("preset id {:?} is used twice", p.id));
        }
        check_text(&format!("preset {} name", p.id), &p.name, 100)?;
        if p.blurb.trim().is_empty() || p.blurb.contains('\n') {
            return Err(format!("preset {}: the blurb is one line", p.id));
        }
        let len = p.description.trim().chars().count();
        if len < 200 {
            return Err(format!(
                "preset {}: the description has {len} characters; at least 200 are needed",
                p.id
            ));
        }
        for pk in &p.packs {
            if !packs.contains(pk.as_str()) {
                return Err(format!("preset {} names unknown pack {pk:?}", p.id));
            }
        }
        for m in &p.modules {
            if crate::modules::find(m).is_none() {
                return Err(format!("preset {} turns on unknown module {m:?}", p.id));
            }
        }
    }
    for (m, pk) in &seed.module_packs {
        if crate::modules::find(m).is_none() {
            return Err(format!("module pack for unknown module {m:?}"));
        }
        if !packs.contains(pk.as_str()) {
            return Err(format!("module {m} names unknown pack {pk:?}"));
        }
    }
    // Rules 3, 4, 7 and the header part of 5, per account.
    for a in &seed.accounts {
        let at = format!("account {} {:?}", a.code, a.name);
        let t: AccountType = a
            .account_type
            .parse()
            .map_err(|_| format!("{at}: unknown type {:?}", a.account_type))?;
        let s: AccountSubtype = a
            .subtype
            .parse()
            .map_err(|_| format!("{at}: unknown subtype {:?}", a.subtype))?;
        if a.code.len() != 4 || !a.code.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("{at}: a code is exactly 4 digits"));
        }
        let n: u32 = a.code.parse().map_err(|_| format!("{at}: bad code"))?;
        if !code_range_ok(t, n) {
            return Err(format!(
                "{at}: code {} is outside the range for {t}",
                a.code
            ));
        }
        if s.account_type() != t {
            return Err(format!("{at}: subtype {s} does not belong to type {t}"));
        }
        if s == AccountSubtype::CostOfGoods && !(5000..=5999).contains(&n) {
            return Err(format!("{at}: cost-of-goods is only for codes 5000-5999"));
        }
        if a.header && (a.parent.is_some() || a.tax_line.is_some()) {
            return Err(format!("{at}: a header has no parent and no tax line"));
        }
        if a.name.contains(':') {
            return Err(format!("{at}: a name cannot contain ':'"));
        }
        check_text(&format!("{at}: name"), &a.name, 100)?;
        if let Some(tl) = &a.tax_line {
            check_text(&format!("{at}: tax line"), tl, 40)?;
        }
        if let Some(note) = &a.note
            && note.chars().count() > 1000
        {
            return Err(format!("{at}: the note is longer than 1000 characters"));
        }
        if let Some(r) = &a.role
            && super::roles::purpose(r).is_none()
        {
            return Err(format!("{at}: unknown role {r:?}"));
        }
    }
    let core: Vec<&SeedAccount> = seed.accounts.iter().filter(|a| a.pack == "core").collect();
    for r in CORE_ROLES {
        if !core.iter().any(|a| a.role.as_deref() == Some(r)) {
            return Err(format!("the core pack does not hold the role {r}"));
        }
    }
    let all_modules = seed.pack_modules();
    for p in &seed.presets {
        // Rule 8.
        if !p.packs.iter().any(|x| x == "core") {
            return Err(format!("preset {} does not include the core pack", p.id));
        }
        for (code, tag) in &p.tax_overrides {
            if !tag.is_empty() {
                check_text(&format!("preset {} override {code}", p.id), tag, 40)?;
            }
        }
        for with_modules in [false, true] {
            let mods: &[&str] = if with_modules { &all_modules } else { &[] };
            let chart = seed.resolve(p, mods);
            check_chart(seed, p, &chart, with_modules)?;
        }
    }
    Ok(())
}

/// Rules 5, 6 and 9 for one resolved chart.
fn check_chart(
    seed: &Seed,
    p: &Preset,
    chart: &[ChartAccount],
    with_modules: bool,
) -> Result<(), String> {
    let ctx = if with_modules {
        format!("preset {} with all module packs", p.id)
    } else {
        format!("preset {}", p.id)
    };
    let by_code: HashMap<&str, &ChartAccount> =
        chart.iter().map(|a| (a.code.as_str(), a)).collect();
    // Rule 6.
    let mut codes = HashSet::new();
    let mut names = HashSet::new();
    let mut roles = HashSet::new();
    for a in chart {
        if !codes.insert(a.code.as_str()) {
            return Err(format!("{ctx}: code {} is used twice", a.code));
        }
        if !names.insert(a.name.to_lowercase()) {
            return Err(format!("{ctx}: the name {:?} is used twice", a.name));
        }
        if let Some(r) = &a.role
            && !roles.insert(r.as_str())
        {
            return Err(format!("{ctx}: the role {r} is held twice"));
        }
    }
    if with_modules {
        for code in p.tax_overrides.keys() {
            if !by_code.contains_key(code.as_str()) {
                return Err(format!("{ctx}: tax override for missing code {code}"));
            }
        }
    }
    // Rule 5 (parents resolve within the chart).
    for a in chart {
        if let Some(pc) = &a.parent {
            let parent = by_code.get(pc.as_str()).ok_or_else(|| {
                format!(
                    "{ctx}: account {} names parent {pc}, which is not in the chart",
                    a.code
                )
            })?;
            if !parent.header {
                return Err(format!(
                    "{ctx}: account {}'s parent {pc} is not a header",
                    a.code
                ));
            }
            if parent.account_type != a.account_type {
                return Err(format!(
                    "{ctx}: account {}'s parent {pc} has a different type",
                    a.code
                ));
            }
        }
    }
    // Rule 9.
    if with_modules {
        let only_pack = PACK_TAGGED
            .iter()
            .find(|(id, _)| *id == p.id)
            .map(|(_, pk)| *pk);
        if FULLY_TAGGED.contains(&p.id.as_str()) || only_pack.is_some() {
            for a in chart {
                let postable = !a.header
                    && matches!(a.account_type, AccountType::Income | AccountType::Expense);
                let uncategorised = matches!(
                    a.role.as_deref(),
                    Some(super::roles::UNCATEGORIZED_INCOME | super::roles::UNCATEGORIZED_EXPENSE)
                );
                if !postable || uncategorised || a.tax_line.is_some() {
                    continue;
                }
                if only_pack.is_some_and(|pk| pk != a.pack)
                    || RULE9_EXEMPT.contains(&(p.id.as_str(), a.pack.as_str()))
                {
                    continue;
                }
                return Err(format!(
                    "{ctx}: account {} {:?} has no tax line",
                    a.code, a.name
                ));
            }
        }
    }
    let _ = seed;
    Ok(())
}
