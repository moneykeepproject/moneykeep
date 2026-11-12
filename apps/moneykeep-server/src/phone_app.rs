//! The phone app's files, served at `/` on the same hostname as the API
//! (mobile-capture.md revision 2, section A). They are embedded at build time when
//! `MK_CAPTURE_DIST` names the built app (`cargo xtask dist-capture`); otherwise the
//! server answers every page with a short explanation.
//!
//! Revision 15: the service worker's cache name and the page carry the build's
//! capture version. `sw.js` and `index.html` hold the placeholder
//! [`BUILD_PLACEHOLDER`]; the server replaces it with its capture version (the
//! phone app is built into the same program, so that is the app's build) when it
//! takes the files in, so every answer carries it.
//!
//! Revision 18: each compressible file is kept with Brotli and gzip copies
//! ([`crate::precompress`]); [`PhoneApp::get_encoded`] picks the one the phone
//! accepts.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// An embedded file: its path, its bytes, its Brotli and gzip copies.
type EmbeddedFile = (
    &'static str,
    &'static [u8],
    Option<&'static [u8]>,
    Option<&'static [u8]>,
);

mod embedded {
    use super::EmbeddedFile;
    include!(concat!(env!("OUT_DIR"), "/phone_app.rs"));
}

/// The page served when the phone app was not built into this program.
pub const NOT_BUILT: &str = "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>MoneyKeep capture</title></head>\n<body>\n<h1>MoneyKeep capture</h1>\n\
<p>The phone app wasn't built into this server — run cargo xtask dist-capture</p>\n</body>\n</html>\n";

/// The text `sw.js` and `index.html` carry in place of the build's capture version.
pub const BUILD_PLACEHOLDER: &str = "__MK_CAPTURE_VERSION__";

/// The files the server writes the capture version into.
const STAMPED: [&str; 2] = ["/sw.js", "/index.html"];

/// The capture version as it goes into a JS string and an HTML attribute (it is
/// `x.y.z+hash`; anything else is replaced, so the files can never be broken).
pub(crate) fn build_text(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `bytes` with the placeholder replaced by `version` (unchanged without one).
fn stamp(bytes: Cow<'static, [u8]>, version: &str) -> Cow<'static, [u8]> {
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return bytes;
    };
    if !text.contains(BUILD_PLACEHOLDER) {
        return bytes;
    }
    Cow::Owned(
        text.replace(BUILD_PLACEHOLDER, &build_text(version))
            .into_bytes(),
    )
}

/// The phone program (the wasm module): its answers carry `X-MK-Program-Size`.
pub const PROGRAM: &str = "/moneykeep-capture_bg.wasm";

/// A file and its compressed copies.
#[derive(Clone, Default)]
struct Entry {
    identity: Cow<'static, [u8]>,
    br: Option<Cow<'static, [u8]>>,
    gzip: Option<Cow<'static, [u8]>>,
}

/// A file as it is sent: its bytes in the chosen coding.
#[derive(Debug, Clone, Copy)]
pub struct Encoded<'a> {
    pub content_type: &'static str,
    pub body: &'a [u8],
    pub coding: crate::precompress::Coding,
    /// The file's own (uncompressed) size.
    pub size: usize,
    /// Other codings exist (the answer varies with `Accept-Encoding`).
    pub varies: bool,
}

/// The phone app's files by path (`/index.html`, `/moneykeep-capture_bg.wasm`, …).
#[derive(Clone, Default)]
pub struct PhoneApp {
    files: BTreeMap<String, Entry>,
}

impl std::fmt::Debug for PhoneApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneApp")
            .field("files", &self.files.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PhoneApp {
    /// The files embedded at build time, or `None` when none were.
    pub fn embedded() -> Option<Self> {
        let files: BTreeMap<String, Entry> = embedded::FILES
            .iter()
            .map(|(p, b, br, gz)| {
                (
                    (*p).to_owned(),
                    Entry {
                        identity: Cow::Borrowed(*b),
                        br: br.map(Cow::Borrowed),
                        gzip: gz.map(Cow::Borrowed),
                    },
                )
            })
            .collect();
        files
            .contains_key("/index.html")
            .then(|| Self::stamped(files, crate::CAPTURE_VERSION))
    }

    /// An app made of these files (tests), stamped with this program's capture
    /// version; the compressed copies are made here.
    pub fn from_files(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self::stamped(
            files
                .into_iter()
                .map(|(p, b)| {
                    (
                        p,
                        Entry {
                            identity: Cow::Owned(b),
                            ..Entry::default()
                        },
                    )
                })
                .collect(),
            crate::CAPTURE_VERSION,
        )
    }

    /// The files with the build's capture version written into `sw.js` and
    /// `index.html` (revision 15), and the compressed copies of every compressible
    /// file that has none yet (revision 18; a copy made before the stamp is
    /// dropped and made again).
    fn stamped(mut files: BTreeMap<String, Entry>, version: &str) -> Self {
        for path in STAMPED {
            if let Some(e) = files.remove(path) {
                let before = e.identity.len();
                let identity = stamp(e.identity, version);
                let changed = matches!(identity, Cow::Owned(_)) || identity.len() != before;
                files.insert(
                    path.to_owned(),
                    if changed {
                        Entry {
                            identity,
                            ..Entry::default()
                        }
                    } else {
                        Entry { identity, ..e }
                    },
                );
            }
        }
        for (path, e) in &mut files {
            if crate::precompress::compressible(path) && e.br.is_none() && e.gzip.is_none() {
                e.br = crate::precompress::brotli(&e.identity).map(Cow::Owned);
                e.gzip = crate::precompress::gzip(&e.identity).map(Cow::Owned);
            }
        }
        Self { files }
    }

    /// The file for a request path (`/` is `/index.html`) and its content type.
    pub fn get(&self, path: &str) -> Option<(&'static str, &[u8])> {
        let p = if path == "/" || path.is_empty() {
            "/index.html"
        } else {
            path
        };
        if p.contains("..") {
            return None;
        }
        self.files
            .get(p)
            .map(|e| (content_type(p), e.identity.as_ref()))
    }

    /// The file for a request path in the best coding `accept` (the request's
    /// `Accept-Encoding`) allows: Brotli, then gzip, else the file itself.
    pub fn get_encoded(&self, path: &str, accept: Option<&str>) -> Option<Encoded<'_>> {
        use crate::precompress::{Coding, choose};
        let p = if path == "/" || path.is_empty() {
            "/index.html"
        } else {
            path
        };
        if p.contains("..") {
            return None;
        }
        let e = self.files.get(p)?;
        let coding = choose(accept, e.br.is_some(), e.gzip.is_some());
        let body = match coding {
            Coding::Brotli => e.br.as_deref(),
            Coding::Gzip => e.gzip.as_deref(),
            Coding::Identity => None,
        }
        .unwrap_or(&e.identity);
        Some(Encoded {
            content_type: content_type(p),
            body,
            coding,
            size: e.identity.len(),
            varies: e.br.is_some() || e.gzip.is_some(),
        })
    }
}

/// The content type of a file by its extension.
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_types() {
        let app = PhoneApp::from_files([
            ("/index.html".to_owned(), b"<html>".to_vec()),
            ("/app_bg.wasm".to_owned(), vec![0, 97, 115, 109]),
        ]);
        assert_eq!(
            app.get("/").map(|(t, b)| (t, b.len())),
            Some(("text/html; charset=utf-8", 6))
        );
        assert_eq!(
            app.get("/app_bg.wasm").map(|f| f.0),
            Some("application/wasm")
        );
        assert!(app.get("/../secret").is_none());
        assert!(app.get("/missing.js").is_none());
        assert!(NOT_BUILT.contains("run cargo xtask dist-capture"));
    }

    /// mobile-capture.md test 84 (the stamp): `sw.js` and `index.html` get the
    /// capture version in place of the placeholder; other files are left alone; an
    /// odd character can never break the JS string.
    #[test]
    fn at84_build_is_written_into_the_worker_and_the_page() {
        let sw = format!("const VERSION = \"moneykeep-capture-{BUILD_PLACEHOLDER}\";");
        let page =
            format!("<meta name=\"moneykeep-capture-version\" content=\"{BUILD_PLACEHOLDER}\">");
        let app = PhoneApp::from_files([
            ("/index.html".to_owned(), page.into_bytes()),
            ("/sw.js".to_owned(), sw.clone().into_bytes()),
            ("/start.js".to_owned(), sw.into_bytes()),
        ]);
        let text = |p: &str| {
            String::from_utf8(app.get(p).map(|f| f.1.to_vec()).unwrap_or_default())
                .unwrap_or_default()
        };
        assert_eq!(
            text("/sw.js"),
            format!(
                "const VERSION = \"moneykeep-capture-{}\";",
                crate::CAPTURE_VERSION
            )
        );
        assert!(text("/").contains(&format!("content=\"{}\"", crate::CAPTURE_VERSION)));
        assert!(
            text("/start.js").contains(BUILD_PLACEHOLDER),
            "only sw.js and index.html"
        );
        let odd = PhoneApp::stamped(
            [(
                "/sw.js".to_owned(),
                Entry {
                    identity: Cow::Owned(BUILD_PLACEHOLDER.as_bytes().to_vec()),
                    ..Entry::default()
                },
            )]
            .into(),
            "1.0.0+ab\"c</x>",
        );
        assert_eq!(
            odd.get("/sw.js").map(|f| f.1.to_vec()),
            Some(b"1.0.0+ab-c--x-".to_vec())
        );
    }
}
