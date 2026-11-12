//! OFX 1.x (SGML) and 2.x (XML) parser, also used for QFX and QBO (spec:
//! import-ofx.md). One small tolerant tokenizer serves both versions; the 2.x path
//! rejects DOCTYPE and entity declarations before anything else, and no entity is
//! ever resolved except the predefined character entities.

use mk_core::{CommodityCode, Date, Decimal};
use sha2::{Digest, Sha256};

use crate::decode::{decode_cp1252, decode_latin1};
use crate::statement::{ClosingBalance, DateKind, Parser, RawRow, Section, Statement};
use crate::{ImportError, ImportResult};

/// The OFX/QFX/QBO parser.
///
/// By default a `STMTTRN` takes its transaction date (`DTUSER`) when the file gives
/// one, else `DTPOSTED`; `posted_dates` always takes `DTPOSTED` (import-ofx.md
/// revision 3, core setting `import.date_source`).
#[derive(Debug, Clone, Copy, Default)]
pub struct OfxParser {
    pub posted_dates: bool,
}

impl Parser for OfxParser {
    fn format(&self) -> &'static str {
        "ofx"
    }

    fn parse(&self, bytes: &[u8]) -> ImportResult<Statement> {
        let (body, xml) = decode_body(bytes)?;
        let root = build_tree(&tokenize(&body, xml)?, xml)?;
        extract(&root, self.posted_dates)
    }
}

fn not_ofx() -> ImportError {
    ImportError::Invalid("not an OFX file".into())
}

// ----- header and decoding -----------------------------------------------------

fn trim_start_bytes(b: &[u8]) -> &[u8] {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    let n = b.iter().take_while(|c| c.is_ascii_whitespace()).count();
    b.get(n..).unwrap_or(&[])
}

/// Decode the file and return the body text (from `<OFX>` on for 1.x, the whole
/// document for 2.x) and whether it is XML.
fn decode_body(bytes: &[u8]) -> ImportResult<(String, bool)> {
    let b = trim_start_bytes(bytes);
    if b.starts_with(b"OFXHEADER") {
        return decode_v1(b).map(|s| (s, false));
    }
    if b.starts_with(b"<?") {
        return decode_v2(b).map(|s| (s, true));
    }
    Err(not_ofx())
}

fn decode_v1(b: &[u8]) -> ImportResult<String> {
    // Header lines up to the first `<`.
    let body_start = b.iter().position(|&c| c == b'<').ok_or_else(not_ofx)?;
    let header = String::from_utf8_lossy(b.get(..body_start).unwrap_or(&[])).into_owned();
    let mut fields = std::collections::HashMap::new();
    for line in header.lines() {
        if let Some((k, v)) = line.split_once(':') {
            fields.insert(k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
        }
    }
    if fields.get("OFXHEADER").map(String::as_str) != Some("100") {
        return Err(not_ofx());
    }
    let body = b.get(body_start..).unwrap_or(&[]);
    let encoding = fields.get("ENCODING").map_or("", String::as_str);
    let charset = fields.get("CHARSET").map_or("", String::as_str);
    Ok(if encoding == "UTF-8" {
        match std::str::from_utf8(body) {
            Ok(s) => s.to_owned(),
            Err(_) => decode_cp1252(body),
        }
    } else if charset == "ISO-8859-1" || charset == "8859-1" {
        decode_latin1(body)
    } else {
        // 1252, NONE or missing.
        decode_cp1252(body)
    })
}

fn xml_encoding(b: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(b.get(..b.len().min(512)).unwrap_or(&[])).into_owned();
    let decl = head.strip_prefix("<?xml")?;
    let end = decl.find("?>")?;
    let decl = decl.get(..end)?;
    let i = decl.find("encoding")?;
    let rest = decl
        .get(i + "encoding".len()..)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let q = rest.chars().next()?;
    let rest = rest.get(1..)?;
    let close = rest.find(q)?;
    Some(rest.get(..close)?.to_ascii_uppercase())
}

fn decode_v2(b: &[u8]) -> ImportResult<String> {
    let text = match xml_encoding(b).as_deref() {
        None | Some("UTF-8" | "US-ASCII" | "ASCII") => std::str::from_utf8(b)
            .map(str::to_owned)
            .map_err(|_| ImportError::Invalid("OFX 2.x file is not valid UTF-8".into()))?,
        Some("WINDOWS-1252" | "CP1252") => decode_cp1252(b),
        Some("ISO-8859-1" | "LATIN1" | "LATIN-1") => decode_latin1(b),
        Some(other) => {
            return Err(ImportError::Invalid(format!(
                "unsupported OFX encoding {other:?}"
            )));
        }
    };
    let upper = text.to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err(ImportError::Invalid(
            "OFX 2.x files with DOCTYPE or entity declarations are rejected".into(),
        ));
    }
    // The OFX processing instruction must be present.
    let pi = upper.find("<?OFX").ok_or_else(not_ofx)?;
    let pi_end = upper
        .get(pi..)
        .and_then(|s| s.find("?>"))
        .ok_or_else(not_ofx)?;
    let pi_text = upper.get(pi..pi + pi_end).unwrap_or("");
    if !pi_text.contains("OFXHEADER=\"200\"") && !pi_text.contains("OFXHEADER='200'") {
        return Err(not_ofx());
    }
    Ok(text)
}

// ----- tokenizer ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Start(String, usize),
    End(String),
    Text(String),
}

pub(crate) fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i..).unwrap_or("");
        let end = after.find(';').filter(|e| *e <= 12);
        let decoded = end.and_then(|e| {
            let name = after.get(1..e)?;
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{A0}',
                n if n.starts_with("#x") || n.starts_with("#X") => {
                    char::from_u32(u32::from_str_radix(n.get(2..)?, 16).ok()?)?
                }
                n if n.starts_with('#') => char::from_u32(n.get(1..)?.parse().ok()?)?,
                _ => return None,
            };
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = after.get(e + 1..).unwrap_or("");
            }
            None => {
                out.push('&');
                rest = after.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(text: &str, xml: bool) -> ImportResult<Vec<Token>> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut line = 1usize;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut out, rest);
            break;
        };
        let before = rest.get(..lt).unwrap_or("");
        push_text(&mut out, before);
        line += before.matches('\n').count();
        let tag_start = rest.get(lt..).unwrap_or("");
        if let Some(c) = tag_start.strip_prefix("<!--") {
            let end = c
                .find("-->")
                .ok_or_else(|| ImportError::at(line, "unterminated comment"))?;
            line += c.get(..end).unwrap_or("").matches('\n').count();
            rest = c.get(end + 3..).unwrap_or("");
            continue;
        }
        if let Some(c) = tag_start.strip_prefix("<![CDATA[") {
            let end = c
                .find("]]>")
                .ok_or_else(|| ImportError::at(line, "unterminated CDATA"))?;
            let t = c.get(..end).unwrap_or("");
            if !t.trim().is_empty() {
                out.push(Token::Text(t.trim().to_owned()));
            }
            rest = c.get(end + 3..).unwrap_or("");
            continue;
        }
        if tag_start.starts_with("<!") {
            // Declarations are never processed.
            return Err(ImportError::at(
                line,
                "declarations are not allowed in OFX files",
            ));
        }
        if let Some(c) = tag_start.strip_prefix("<?") {
            let end = c
                .find("?>")
                .ok_or_else(|| ImportError::at(line, "unterminated processing instruction"))?;
            rest = c.get(end + 2..).unwrap_or("");
            continue;
        }
        let gt = tag_start
            .find('>')
            .ok_or_else(|| ImportError::at(line, "unterminated tag"))?;
        let inner = tag_start.get(1..gt).unwrap_or("").trim();
        rest = tag_start.get(gt + 1..).unwrap_or("");
        if let Some(name) = inner.strip_prefix('/') {
            out.push(Token::End(name.trim().to_ascii_uppercase()));
        } else {
            let self_closing = xml && inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            // Attributes are not used in OFX; keep the name only.
            let name = inner
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_uppercase();
            if name.is_empty() {
                return Err(ImportError::at(line, "empty tag"));
            }
            out.push(Token::Start(name.clone(), line));
            if self_closing {
                out.push(Token::End(name));
            }
        }
    }
    Ok(out)
}

fn push_text(out: &mut Vec<Token>, s: &str) {
    let t = s.trim();
    if !t.is_empty() {
        out.push(Token::Text(decode_entities(t)));
    }
}

// ----- tree --------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) text: Option<String>,
    pub(crate) children: Vec<Node>,
    pub(crate) line: usize,
}

impl Node {
    pub(crate) fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub(crate) fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub(crate) fn text_of(&self, name: &str) -> Option<String> {
        self.child(name)
            .and_then(|c| c.text.clone())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
    }

    pub(crate) fn path(&self, names: &[&str]) -> Option<&Node> {
        let mut cur = self;
        for n in names {
            cur = cur.child(n)?;
        }
        Some(cur)
    }
}

/// Aggregates this parser reads. In 1.x files any other tag directly followed by a
/// tag is an element with empty content and is closed at the next tag.
const AGGREGATES: &[&str] = &[
    "OFX",
    "SIGNONMSGSRSV1",
    "SONRS",
    "STATUS",
    "FI",
    "BANKMSGSRSV1",
    "STMTTRNRS",
    "STMTRS",
    "BANKACCTFROM",
    "BANKACCTTO",
    "BANKTRANLIST",
    "STMTTRN",
    "STMTTRNP",
    "PAYEE",
    "LEDGERBAL",
    "AVAILBAL",
    "CREDITCARDMSGSRSV1",
    "CCSTMTTRNRS",
    "CCSTMTRS",
    "CCACCTFROM",
    "CCACCTTO",
    "CURRENCY",
    "ORIGCURRENCY",
    "BALLIST",
    "BAL",
    "INVSTMTMSGSRSV1",
    "INVSTMTTRNRS",
    "INVSTMTRS",
    "INVACCTFROM",
    "INVACCTTO",
    "INVTRANLIST",
    "INVBANKTRAN",
    "BUYDEBT",
    "BUYMF",
    "BUYOPT",
    "BUYOTHER",
    "BUYSTOCK",
    "SELLDEBT",
    "SELLMF",
    "SELLOPT",
    "SELLOTHER",
    "SELLSTOCK",
    "INVBUY",
    "INVSELL",
    "INVTRAN",
    "SECID",
    "INCOME",
    "REINVEST",
    "INVEXPENSE",
    "MARGININTEREST",
    "RETOFCAP",
    "SPLIT",
    "TRANSFER",
    "JRNLFUND",
    "JRNLSEC",
    "CLOSUREOPT",
    "INVPOSLIST",
    "POSDEBT",
    "POSMF",
    "POSOPT",
    "POSOTHER",
    "POSSTOCK",
    "INVPOS",
    "INVBAL",
    "INVOOLIST",
    "INV401K",
    "INV401KBAL",
    "SECLISTMSGSRSV1",
    "SECLIST",
    "SECINFO",
    "STOCKINFO",
    "MFINFO",
    "DEBTINFO",
    "OPTINFO",
    "OTHERINFO",
    "MFASSETCLASS",
    "FIMFASSETCLASS",
    "PORTION",
    "FIPORTION",
    "LOANMSGSRSV1",
    "LOANSTMTTRNRS",
    "LOANSTMTRS",
    "MKTGINFO",
    "IMAGEDATA",
    // Direct Connect replies (connectivity-brokerage.md): signup, profile, challenge.
    "SIGNUPMSGSRSV1",
    "ACCTINFOTRNRS",
    "ACCTINFORS",
    "ACCTINFO",
    "BANKACCTINFO",
    "CCACCTINFO",
    "INVACCTINFO",
    "BPACCTINFO",
    "HOLDERINFO",
    "PRIMARYHOLDER",
    "SECONDARYHOLDER",
    "MFACHALLENGETRNRS",
    "MFACHALLENGERS",
    "MFACHALLENGE",
    "PROFMSGSRSV1",
    "PROFTRNRS",
    "PROFRS",
    "MSGSETLIST",
    "SIGNONINFOLIST",
    "SIGNONINFO",
    // Requests (MoneyKeep reads its own requests back in tests).
    "SIGNONMSGSRQV1",
    "SONRQ",
    "MFACHALLENGETRNRQ",
    "MFACHALLENGERQ",
    "MFACHALLENGEANSWER",
    "SIGNUPMSGSRQV1",
    "ACCTINFOTRNRQ",
    "ACCTINFORQ",
    "BANKMSGSRQV1",
    "STMTTRNRQ",
    "STMTRQ",
    "INCTRAN",
    "CREDITCARDMSGSRQV1",
    "CCSTMTTRNRQ",
    "CCSTMTRQ",
    "INVSTMTMSGSRQV1",
    "INVSTMTTRNRQ",
    "INVSTMTRQ",
    "INCPOS",
    "PROFMSGSRQV1",
    "PROFTRNRQ",
    "PROFRQ",
];

fn build_tree(tokens: &[Token], xml: bool) -> ImportResult<Node> {
    let mut stack: Vec<Node> = vec![Node {
        name: "#root".into(),
        ..Node::default()
    }];
    let close_top = |stack: &mut Vec<Node>| {
        if stack.len() > 1
            && let Some(n) = stack.pop()
            && let Some(parent) = stack.last_mut()
        {
            parent.children.push(n);
        }
    };
    for t in tokens {
        match t {
            Token::Start(name, line) => {
                if !xml {
                    // An SGML element ends at the next tag.
                    let top_is_element = stack.last().is_some_and(|n| {
                        n.name != "#root"
                            && n.children.is_empty()
                            && (n.text.is_some() || !AGGREGATES.contains(&n.name.as_str()))
                    });
                    if top_is_element {
                        close_top(&mut stack);
                    }
                }
                if stack.len() > crate::safety::MAX_DEPTH {
                    return Err(ImportError::at(
                        *line,
                        "elements nest deeper than 64 levels",
                    ));
                }
                stack.push(Node {
                    name: name.clone(),
                    line: *line,
                    ..Node::default()
                });
            }
            Token::Text(s) => {
                if let Some(top) = stack.last_mut() {
                    match &mut top.text {
                        Some(existing) => {
                            existing.push(' ');
                            existing.push_str(s);
                        }
                        None => top.text = Some(s.clone()),
                    }
                    if let Some(t) = &top.text {
                        crate::safety::check_field(t, top.line)?;
                    }
                }
            }
            Token::End(name) => {
                // Close up to the matching open tag; ignore stray end tags.
                if let Some(pos) = stack.iter().rposition(|n| n.name == *name)
                    && pos > 0
                {
                    while stack.len() > pos {
                        close_top(&mut stack);
                    }
                }
            }
        }
    }
    while stack.len() > 1 {
        close_top(&mut stack);
    }
    Ok(stack.pop().unwrap_or_default())
}

// ----- values ------------------------------------------------------------------

/// OFX date: the calendar date as written (first eight digits).
pub(crate) fn parse_ofx_date(text: &str) -> Option<Date> {
    let t = text.trim();
    let digits = t.get(..8)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest = t.get(8..)?;
    let time: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let tail = rest.get(time.len()..)?.trim();
    if !(tail.is_empty() || (tail.starts_with('[') && tail.ends_with(']'))) {
        return None;
    }
    Date::from_ymd(
        digits.get(..4)?.parse().ok()?,
        digits.get(4..6)?.parse().ok()?,
        digits.get(6..8)?.parse().ok()?,
    )
    .ok()
}

/// OFX amount: optional sign, `.` or `,` decimal separator, no grouping, ≤ 32 chars.
pub(crate) fn parse_ofx_amount(text: &str) -> Option<Decimal> {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 32 {
        return None;
    }
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let body = body.replace(',', ".");
    if body.is_empty()
        || body.starts_with('.') && body.len() == 1
        || !body.chars().all(|c| c.is_ascii_digit() || c == '.')
        || body.matches('.').count() > 1
    {
        return None;
    }
    let d: Decimal = body.parse().ok()?;
    Some(if neg { -d } else { d })
}

pub(crate) fn status_error(trnrs: &Node) -> Option<String> {
    let status = trnrs.child("STATUS")?;
    let code = status.text_of("CODE")?;
    if code == "0" {
        return None;
    }
    let meaning = match code.as_str() {
        "2000" => "general error",
        "2003" => "account not found",
        _ => "error",
    };
    let msg = status
        .text_of("MESSAGE")
        .map(|m| format!(": {m}"))
        .unwrap_or_default();
    Some(format!("statement error {code} ({meaning}){msg}"))
}

pub(crate) fn last_four(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(4);
    chars
        .get(start..)
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn balance(node: Option<&Node>) -> Option<ClosingBalance> {
    let n = node?;
    Some(ClosingBalance {
        date: parse_ofx_date(&n.text_of("DTASOF")?)?,
        amount: parse_ofx_amount(&n.text_of("BALAMT")?)?,
    })
}

/// The section key of a bank (`bank`) or card (`cc`) statement.
pub(crate) fn bank_key(
    org: &str,
    fid: &str,
    bankid: &str,
    branchid: &str,
    acctid: &str,
    kind: &str,
) -> String {
    let key_material = format!("{org}|{fid}|{bankid}|{branchid}|{acctid}|{kind}");
    format!("ofx:{}", hex(&Sha256::digest(key_material.as_bytes())))
}

/// The section key of an investment statement.
pub(crate) fn inv_key(org: &str, broker: &str, acctid: &str) -> String {
    format!(
        "ofx:{}",
        hex(&Sha256::digest(
            format!("{org}|{broker}|{acctid}|investment").as_bytes()
        ))
    )
}

/// The element tree of an OFX document (1.x or 2.x).
pub(crate) fn parse_tree(bytes: &[u8]) -> ImportResult<Node> {
    let (body, xml) = decode_body(bytes)?;
    build_tree(&tokenize(&body, xml)?, xml)
}

fn extract(root: &Node, posted_dates: bool) -> ImportResult<Statement> {
    let ofx = root.child("OFX").ok_or_else(not_ofx)?;
    let fi = ofx.path(&["SIGNONMSGSRSV1", "SONRS", "FI"]);
    let org = fi.and_then(|f| f.text_of("ORG")).unwrap_or_default();
    let fid = fi.and_then(|f| f.text_of("FID")).unwrap_or_default();
    let mut st = Statement::default();
    let mut pending = 0usize;
    let mut groups: Vec<(&str, &Node)> = Vec::new();
    for m in ofx.all("BANKMSGSRSV1") {
        groups.extend(m.all("STMTTRNRS").map(|t| ("bank", t)));
    }
    for m in ofx.all("CREDITCARDMSGSRSV1") {
        groups.extend(m.all("CCSTMTTRNRS").map(|t| ("cc", t)));
    }
    let invst: usize = ofx
        .all("INVSTMTMSGSRSV1")
        .map(|m| m.all("INVSTMTTRNRS").count())
        .sum();
    let loans: usize = ofx
        .all("LOANMSGSRSV1")
        .map(|m| m.all("LOANSTMTTRNRS").count())
        .sum();
    for (n, (kind, trnrs)) in groups.into_iter().enumerate() {
        if let Some(err) = status_error(trnrs) {
            st.warnings.push(format!("statement {}: {err}", n + 1));
            continue;
        }
        let rs = match kind {
            "bank" => trnrs.child("STMTRS"),
            _ => trnrs.child("CCSTMTRS"),
        };
        let Some(rs) = rs else {
            st.warnings
                .push(format!("statement {}: no statement in the response", n + 1));
            continue;
        };
        let (from, bankid, branchid, accttype) = match kind {
            "bank" => {
                let f = rs.child("BANKACCTFROM");
                (
                    f,
                    f.and_then(|x| x.text_of("BANKID")).unwrap_or_default(),
                    f.and_then(|x| x.text_of("BRANCHID")).unwrap_or_default(),
                    f.and_then(|x| x.text_of("ACCTTYPE"))
                        .unwrap_or_else(|| "BANK".into()),
                )
            }
            _ => (
                rs.child("CCACCTFROM"),
                String::new(),
                String::new(),
                "CREDITCARD".into(),
            ),
        };
        let acctid = from.and_then(|x| x.text_of("ACCTID")).unwrap_or_default();
        let key = bank_key(&org, &fid, &bankid, &branchid, &acctid, kind);
        let hint = last_four(&acctid);
        let name = [org.as_str(), accttype.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let curdef = rs.text_of("CURDEF").map(|c| c.to_ascii_uppercase());
        let currency = curdef
            .as_deref()
            .map(CommodityCode::new)
            .transpose()
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
        let list = rs.child("BANKTRANLIST");
        let period = list.and_then(|l| {
            Some((
                parse_ofx_date(&l.text_of("DTSTART")?)?,
                parse_ofx_date(&l.text_of("DTEND")?)?,
            ))
        });
        let mut rows: Vec<RawRow> = Vec::new();
        if let Some(list) = list {
            pending += list.all("STMTTRNP").count();
            for (i, t) in list.all("STMTTRN").enumerate() {
                let at = (n + 1, i + 1);
                if let Some(row) =
                    transaction(t, curdef.as_deref(), &mut st.warnings, at, posted_dates)
                {
                    apply_in_file_correction(&mut rows, row, &mut st.warnings);
                }
            }
        }
        st.sections.push(Section {
            account_key: Some(key),
            account_name: Some(if hint.is_empty() {
                name
            } else {
                format!("{name} ending {hint}").trim().to_owned()
            }),
            account_hint: (!hint.is_empty()).then_some(hint),
            currency,
            period,
            closing: balance(rs.child("LEDGERBAL")),
            available: balance(rs.child("AVAILBAL")),
            rows,
            ..Section::default()
        });
    }
    if pending > 0 {
        st.warnings
            .push(format!("pending transactions not imported ({pending})"));
    }
    if invst > 0 {
        crate::ofx_inv::extract_investments(ofx, &org, &mut st, posted_dates)?;
    }
    if loans > 0 {
        st.warnings
            .push(format!("skipped: loan statements ({loans})"));
    }
    Ok(st)
}

pub(crate) fn transaction(
    t: &Node,
    curdef: Option<&str>,
    warnings: &mut Vec<String>,
    (stmt, pos): (usize, usize),
    posted_dates: bool,
) -> Option<RawRow> {
    let at = format!("statement {stmt}, transaction {pos} (line {})", t.line);
    let (Some(fitid), Some(posted), Some(amt)) = (
        t.text_of("FITID"),
        t.text_of("DTPOSTED"),
        t.text_of("TRNAMT"),
    ) else {
        warnings.push(format!(
            "{at} skipped: FITID, DTPOSTED and TRNAMT are required"
        ));
        return None;
    };
    let Some(posted_date) = parse_ofx_date(&posted) else {
        warnings.push(format!("{at} skipped: invalid date {posted:?}"));
        return None;
    };
    // Revision 3: the transaction date (DTUSER) when given and wanted.
    let user = if posted_dates {
        None
    } else {
        t.text_of("DTUSER").filter(|u| !u.trim().is_empty())
    };
    let (date, date_kind) = match user.as_deref().map(|u| (u, parse_ofx_date(u))) {
        Some((_, Some(d))) => (d, DateKind::Transaction),
        Some((u, None)) => {
            warnings.push(format!(
                "{at}: invalid DTUSER {u:?}; the posting date is used"
            ));
            (posted_date, DateKind::Posted)
        }
        None => (posted_date, DateKind::Posted),
    };
    let Some(amount) = parse_ofx_amount(&amt) else {
        warnings.push(format!("{at} skipped: invalid amount {amt:?}"));
        return None;
    };
    let trntype = t.text_of("TRNTYPE").map(|s| s.to_ascii_uppercase());
    if trntype.as_deref() == Some("DEBIT") && amount > Decimal::ZERO {
        warnings.push(format!(
            "{at}: TRNTYPE DEBIT with a positive amount; the amount's sign is used"
        ));
    }
    let payee = t
        .text_of("EXTDNAME")
        .or_else(|| t.text_of("NAME"))
        .or_else(|| t.child("PAYEE").and_then(|p| p.text_of("NAME")))
        .unwrap_or_default();
    let foreign = ["CURRENCY", "ORIGCURRENCY"].iter().find_map(|agg| {
        let sym = t.child(agg)?.text_of("CURSYM")?.to_ascii_uppercase();
        (Some(sym.as_str()) != curdef).then_some(sym)
    });
    let mut memo = t.text_of("MEMO").unwrap_or_default();
    // Revision 2: a memo that only repeats the name is left empty.
    let same = |name: &str| memo.trim().to_lowercase() == name.trim().to_lowercase();
    if !memo.trim().is_empty() && (same(&payee) || t.text_of("NAME").is_some_and(|n| same(&n))) {
        memo = String::new();
    }
    let number = t
        .text_of("CHECKNUM")
        .or_else(|| t.text_of("REFNUM"))
        .unwrap_or_default();
    let raw = format!(
        "STMTTRN {} {} {} {} {}",
        trntype.as_deref().unwrap_or(""),
        posted,
        amt,
        fitid,
        payee
    );
    Some(RawRow {
        date_kind,
        cleaned: Vec::new(),
        class_hint: None,
        pending: false,
        invoices: Vec::new(),
        external_id: Some(fitid),
        date,
        amount,
        payee,
        memo,
        number,
        category_hint: String::new(),
        cleared: None,
        splits: Vec::new(),
        trntype,
        review: foreign.map(|c| format!("foreign-currency transaction ({c})")),
        correct_fitid: t.text_of("CORRECTFITID"),
        correct_action: t.text_of("CORRECTACTION").map(|a| a.to_ascii_uppercase()),
        line: t.line,
        raw,
    })
}

/// A correction of a FITID earlier in the same file replaces or drops that row.
fn apply_in_file_correction(rows: &mut Vec<RawRow>, row: RawRow, warnings: &mut Vec<String>) {
    let Some(target) = row.correct_fitid.clone() else {
        rows.push(row);
        return;
    };
    let Some(pos) = rows
        .iter()
        .position(|r| r.external_id.as_deref() == Some(target.as_str()))
    else {
        rows.push(row);
        return;
    };
    let delete = row.correct_action.as_deref() == Some("DELETE");
    rows.remove(pos);
    if delete {
        warnings.push(format!(
            "transaction {target} was deleted by a correction later in the file"
        ));
    } else {
        let mut replacement = row;
        replacement.correct_fitid = None;
        replacement.correct_action = None;
        rows.insert(pos, replacement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgml_elements_close_at_next_tag() {
        let toks = tokenize(
            "<STATUS><CODE>0<SEVERITY>INFO</STATUS><DTSERVER>x<EMPTY><TAIL>z",
            false,
        )
        .unwrap();
        let root = build_tree(&toks, false).unwrap();
        let st = root.child("STATUS").unwrap();
        assert_eq!(st.text_of("CODE").as_deref(), Some("0"));
        assert_eq!(st.text_of("SEVERITY").as_deref(), Some("INFO"));
        assert_eq!(root.text_of("DTSERVER").as_deref(), Some("x"));
        // An unknown tag directly followed by a tag is an empty element.
        assert!(root.child("EMPTY").is_some());
        assert_eq!(root.text_of("TAIL").as_deref(), Some("z"));
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("A &amp; B &lt;&#65;&#x42;&nbsp;&bogus;"),
            "A & B <AB\u{A0}&bogus;"
        );
    }

    #[test]
    fn dates_and_amounts() {
        for s in [
            "20261005",
            "20261005120000",
            "20261005235959.000[-5:EST]",
            "20261005120000[+5.75]",
        ] {
            assert_eq!(parse_ofx_date(s).unwrap().to_string(), "2026-10-05", "{s}");
        }
        assert!(parse_ofx_date("20261305").is_none());
        assert!(parse_ofx_date("2026105").is_none());
        let d = |s: &str| parse_ofx_amount(s).unwrap().to_string();
        assert_eq!(d("550"), "550");
        assert_eq!(d("-12,34"), "-12.34");
        assert_eq!(d("+7.5"), "7.5");
        assert!(parse_ofx_amount("1,234.56").is_none());
        assert!(parse_ofx_amount(&"1".repeat(33)).is_none());
    }

    #[test]
    fn declarations_rejected() {
        assert!(tokenize("<!DOCTYPE x>", true).is_err());
        assert!(
            decode_v2(b"<?xml version=\"1.0\"?><?OFX OFXHEADER=\"200\"?><!DOCTYPE ofx><OFX/>")
                .is_err()
        );
    }
}
