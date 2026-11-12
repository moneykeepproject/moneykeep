//! A small XML element tree for broker files (XML 1.0). Document type declarations
//! are rejected outright and only the predefined and numeric character entities are
//! expanded, so no external entity can ever be fetched or expanded. Namespaces are
//! ignored by using local names.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::{ImportError, ImportResult};

/// One element: local name, attributes (local name, value), text and children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Element>,
    pub line: usize,
}

impl Element {
    /// Elements at a `/`-separated path below this element.
    pub fn select<'a>(&'a self, path: &[&str]) -> Vec<&'a Element> {
        match path.split_first() {
            None => vec![self],
            Some((first, rest)) => self
                .children
                .iter()
                .filter(|c| c.name == *first)
                .flat_map(|c| c.select(rest))
                .collect(),
        }
    }

    /// Every value below this element as `path → text` (`child/grandchild`), with
    /// attributes as `@name` / `child/@name`.
    pub fn flatten(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .attrs
            .iter()
            .map(|(k, v)| (format!("@{k}"), v.clone()))
            .collect();
        for c in &self.children {
            out.push((c.name.clone(), c.text.clone()));
            for (k, v) in c.flatten() {
                out.push((format!("{}/{k}", c.name), v));
            }
        }
        out
    }
}

fn local(qname: &[u8]) -> String {
    let s = String::from_utf8_lossy(qname);
    s.rsplit(':').next().unwrap_or(&s).to_owned()
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        n if n.starts_with("#x") || n.starts_with("#X") => {
            char::from_u32(u32::from_str_radix(n.get(2..)?, 16).ok()?)
        }
        n if n.starts_with('#') => char::from_u32(n.get(1..)?.parse().ok()?),
        _ => None,
    }
}

/// Expand predefined and numeric entities in attribute text; others are errors.
fn unescape(s: &str) -> ImportResult<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 1..).unwrap_or("");
        let end = after
            .find(';')
            .ok_or_else(|| ImportError::Invalid("bad entity reference in XML".into()))?;
        let name = after.get(..end).unwrap_or("");
        let c = entity(name)
            .ok_or_else(|| ImportError::Invalid(format!("XML entity &{name}; is not allowed")))?;
        out.push(c);
        rest = after.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    Ok(out)
}

/// Parse a document into its root element.
pub(crate) fn parse(text: &str) -> ImportResult<Element> {
    let upper = text.to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err(ImportError::Invalid(
            "XML files with a DTD or entity declarations are rejected".into(),
        ));
    }
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = vec![Element::default()];
    let bad = |e: quick_xml::Error| ImportError::Invalid(format!("invalid XML: {e}"));
    let line_of = |pos: u64| -> usize {
        let p = usize::try_from(pos).unwrap_or(0).min(text.len());
        text.get(..p).map_or(1, |t| t.matches('\n').count() + 1)
    };
    loop {
        let pos = reader.buffer_position();
        match reader.read_event().map_err(bad)? {
            Event::DocType(_) => {
                return Err(ImportError::Invalid(
                    "XML files with a DTD are rejected".into(),
                ));
            }
            ev @ (Event::Start(_) | Event::Empty(_)) => {
                let (e, empty) = match &ev {
                    Event::Start(e) => (e, false),
                    Event::Empty(e) => (e, true),
                    _ => continue,
                };
                let mut el = Element {
                    name: local(e.name().as_ref()),
                    line: line_of(pos),
                    ..Element::default()
                };
                for a in e.attributes() {
                    let a = a.map_err(|x| ImportError::Invalid(format!("invalid XML: {x}")))?;
                    let raw = String::from_utf8_lossy(&a.value).into_owned();
                    el.attrs.push((local(a.key.as_ref()), unescape(&raw)?));
                }
                if empty {
                    if let Some(top) = stack.last_mut() {
                        top.children.push(el);
                    }
                } else {
                    if stack.len() > crate::safety::MAX_DEPTH {
                        return Err(ImportError::Invalid(format!(
                            "XML nesting deeper than {}",
                            crate::safety::MAX_DEPTH
                        )));
                    }
                    stack.push(el);
                }
            }
            Event::End(_) => {
                if stack.len() > 1
                    && let Some(mut el) = stack.pop()
                {
                    el.text = el.text.trim().to_owned();
                    if let Some(top) = stack.last_mut() {
                        top.children.push(el);
                    }
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                    crate::safety::check_field(&top.text, line_of(pos))?;
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                    crate::safety::check_field(&top.text, line_of(pos))?;
                }
            }
            Event::GeneralRef(r) => {
                let name = String::from_utf8_lossy(&r).into_owned();
                let c = entity(&name).ok_or_else(|| {
                    ImportError::Invalid(format!("XML entity &{name}; is not allowed"))
                })?;
                if let Some(top) = stack.last_mut() {
                    top.text.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack
        .pop()
        .and_then(|r| r.children.into_iter().next())
        .ok_or_else(|| ImportError::Invalid("the XML file has no root element".into()))?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn tree_attributes_entities_and_rejections() {
        let root = parse(
            "<?xml version=\"1.0\"?><a:R xmlns:a=\"x\"><T id=\"T&amp;1\"><N>A &lt; B</N><E /></T></a:R>",
        )
        .unwrap();
        assert_eq!(root.name, "R");
        let t = root.select(&["T"]);
        assert_eq!(t.len(), 1);
        let flat = t[0].flatten();
        assert!(flat.contains(&("@id".into(), "T&1".into())));
        assert!(flat.contains(&("N".into(), "A < B".into())));
        assert!(flat.contains(&("E".into(), String::new())));
        assert!(
            parse("<!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><r>&x;</r>").is_err()
        );
        assert!(parse("<r>&x;</r>").is_err());
    }
}
