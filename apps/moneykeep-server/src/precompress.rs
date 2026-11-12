//! Compressed copies of the phone app's files (mobile-capture.md revision 18):
//! Brotli (RFC 7932) and gzip (RFC 1952), made once — at build time for the files
//! `build.rs` embeds, and when the server takes the files in for `index.html` and
//! `sw.js` (they carry the build's capture version, written in then). The server
//! answers with the best copy the phone accepts (RFC 9110 `Accept-Encoding`).
//!
//! `build.rs` includes this file too (`#[path]`), so it uses nothing else of the
//! crate.

/// Whether a file of this path is worth compressing (text and the wasm program;
/// pictures are compressed already).
pub fn compressible(path: &str) -> bool {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "html" | "js" | "mjs" | "wasm" | "css" | "json" | "webmanifest" | "svg" | "txt"
    )
}

/// The Brotli copy (quality 11, 4 MiB window), or `None` when it is not smaller.
pub fn brotli(data: &[u8]) -> Option<Vec<u8>> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        lgwin: 22,
        size_hint: data.len(),
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut input = data;
    brotli::BrotliCompress(&mut input, &mut out, &params).ok()?;
    (out.len() < data.len()).then_some(out)
}

/// The gzip copy (best compression), or `None` when it is not smaller.
pub fn gzip(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).ok()?;
    let out = e.finish().ok()?;
    (out.len() < data.len()).then_some(out)
}

/// The content codings the server can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    Brotli,
    Gzip,
    Identity,
}

impl Coding {
    /// The `Content-Encoding` value (`None` for identity).
    pub fn header(self) -> Option<&'static str> {
        match self {
            Coding::Brotli => Some("br"),
            Coding::Gzip => Some("gzip"),
            Coding::Identity => None,
        }
    }
}

/// The q-value of `coding` in an `Accept-Encoding` header (RFC 9110 §12.5.3):
/// the coding's own entry, else `*`'s, else `None` (not listed).
fn quality(accept: &str, coding: &str) -> Option<u16> {
    let mut star = None;
    for part in accept.split(',') {
        let mut it = part.split(';');
        let name = it.next().unwrap_or("").trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let mut q: u16 = 1000;
        for param in it {
            let (k, v) = param.split_once('=').unwrap_or((param, ""));
            if k.trim().eq_ignore_ascii_case("q") {
                q = parse_q(v.trim());
            }
        }
        if name == coding {
            return Some(q);
        }
        if name == "*" {
            star = Some(q);
        }
    }
    star
}

/// A q-value ("0", "0.5", "1.000") in thousandths; anything else counts as 0.
fn parse_q(v: &str) -> u16 {
    let (int, frac) = v.split_once('.').unwrap_or((v, ""));
    let digits: String = frac.chars().take(3).collect();
    if !int.chars().all(|c| c.is_ascii_digit())
        || int.is_empty()
        || !digits.chars().all(|c| c.is_ascii_digit())
    {
        return 0;
    }
    let whole: u16 = int.parse().unwrap_or(0);
    let mut thousandths: u16 = 0;
    for (i, c) in digits.chars().enumerate() {
        let d = u16::try_from(c.to_digit(10).unwrap_or(0)).unwrap_or(0);
        thousandths += d * [100, 10, 1].get(i).copied().unwrap_or(0);
    }
    whole
        .saturating_mul(1000)
        .saturating_add(thousandths)
        .min(1000)
}

/// The best coding for this `Accept-Encoding` header among those available:
/// Brotli, then gzip, else identity.
pub fn choose(accept: Option<&str>, has_br: bool, has_gzip: bool) -> Coding {
    let Some(accept) = accept else {
        return Coding::Identity;
    };
    if has_br && quality(accept, "br").is_some_and(|q| q > 0) {
        return Coding::Brotli;
    }
    if has_gzip && quality(accept, "gzip").is_some_and(|q| q > 0) {
        return Coding::Gzip;
    }
    Coding::Identity
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 9110 §12.5.3: q-values, `*`, and q=0 as "not acceptable".
    #[test]
    fn accept_encoding() {
        let c = |a: Option<&str>| choose(a, true, true);
        assert_eq!(c(None), Coding::Identity);
        assert_eq!(c(Some("")), Coding::Identity);
        assert_eq!(c(Some("gzip, deflate, br, zstd")), Coding::Brotli);
        assert_eq!(c(Some("gzip")), Coding::Gzip);
        assert_eq!(c(Some("BR;Q=1.0")), Coding::Brotli);
        assert_eq!(c(Some("br;q=0, gzip")), Coding::Gzip);
        assert_eq!(c(Some("br;q=0.000, gzip;q=0")), Coding::Identity);
        assert_eq!(c(Some("*")), Coding::Brotli);
        assert_eq!(c(Some("*;q=0")), Coding::Identity);
        assert_eq!(c(Some("br;q=0, *;q=0.5")), Coding::Gzip);
        assert_eq!(c(Some("identity")), Coding::Identity);
        assert_eq!(choose(Some("br"), false, true), Coding::Identity);
        assert_eq!(choose(Some("br, gzip"), false, true), Coding::Gzip);
        assert_eq!(parse_q("0.5"), 500);
        assert_eq!(parse_q("1"), 1000);
        assert_eq!(parse_q("x"), 0);
        assert!(compressible("/a.wasm") && compressible("/index.html"));
        assert!(!compressible("/icons/icon-192.png"));
        assert!(gzip(b"x").is_none(), "not smaller");
    }
}
