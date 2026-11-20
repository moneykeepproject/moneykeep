//! The built phone page without Trunk's own loader (capture 0.1.3 investigation,
//! 2026-11-19).
//!
//! Trunk adds to `dist/index.html` an inline module script that imports the glue and
//! starts the program, and `modulepreload` / `preload` links for the glue and the
//! program. MoneyKeep starts the app from `start.js` instead (the page's
//! Content-Security-Policy blocks inline scripts), asking for each file with the page's
//! build (`?b=<build>`), so:
//!
//! - the inline script only produced a blocked-script error in every browser console;
//! - the preloads asked for the program without `?b=`, so the phone downloaded the
//!   12 MB program twice and the browser warned that the preload was not used.
//!
//! [`strip`] removes both from the built page.

use std::path::Path;

/// The glue and the program as Trunk links them.
const PRELOADED: [&str; 2] = ["/moneykeep-capture.js", "/moneykeep-capture_bg.wasm"];

/// What [`strip`] removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub scripts: usize,
    pub preloads: usize,
}

/// The end of the tag that starts at `from` (just past its `>`).
fn tag_end(html: &str, from: usize) -> Option<usize> {
    html[from..].find('>').map(|i| from + i + 1)
}

/// `html` without Trunk's inline loader scripts (inline `<script>` elements that
/// import the glue) and without its `preload` / `modulepreload` links to the glue or
/// the program.
pub fn strip(html: &str) -> (String, Removed) {
    let mut out = String::with_capacity(html.len());
    let mut removed = Removed::default();
    let mut rest = html;
    loop {
        let lower = rest.to_ascii_lowercase();
        let next_script = lower.find("<script");
        let next_link = lower.find("<link");
        let start = match (next_script, next_link) {
            (Some(s), Some(l)) => s.min(l),
            (Some(s), None) => s,
            (None, Some(l)) => l,
            (None, None) => break,
        };
        let Some(open_end) = tag_end(rest, start) else {
            break;
        };
        let open = &lower[start..open_end];
        if open.starts_with("<script") {
            let Some(close) = lower[open_end..].find("</script>") else {
                break;
            };
            let close_end = open_end + close + "</script>".len();
            let body = &rest[open_end..open_end + close];
            let inline_loader = !open.contains("src=")
                && PRELOADED.iter().any(|p| body.contains(p))
                && body.contains("import");
            out.push_str(&rest[..start]);
            if inline_loader {
                removed.scripts += 1;
            } else {
                out.push_str(&rest[start..close_end]);
            }
            rest = &rest[close_end..];
        } else {
            let preload = (open.contains("rel=\"preload\"")
                || open.contains("rel=\"modulepreload\""))
                && PRELOADED
                    .iter()
                    .any(|p| open.contains(&format!("href=\"{p}\"")));
            out.push_str(&rest[..start]);
            if preload {
                removed.preloads += 1;
            } else {
                out.push_str(&rest[start..open_end]);
            }
            rest = &rest[open_end..];
        }
    }
    out.push_str(rest);
    (out, removed)
}

/// Strip `dist/index.html` in place.
pub fn strip_file(dist: &Path) -> Result<Removed, String> {
    let page = dist.join("index.html");
    let html = std::fs::read_to_string(&page)
        .map_err(|e| format!("cannot read {}: {e}", page.display()))?;
    let (html, removed) = strip(&html);
    std::fs::write(&page, html).map_err(|e| format!("cannot write {}: {e}", page.display()))?;
    println!(
        "==> phone page: removed Trunk's loader ({} inline script, {} preload links)",
        removed.scripts, removed.preloads
    );
    Ok(removed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// The shape of Trunk 0.21's output around the page's own start script.
    const BUILT: &str = r#"<!DOCTYPE html><html><head>
  <title>MoneyKeep capture</title>
<script type="module">
import init, * as bindings from '/moneykeep-capture.js';
const wasm = await init({ module_or_path: '/moneykeep-capture_bg.wasm' });
window.wasmBindings = bindings;
</script>
  <link rel="manifest" href="manifest.webmanifest" crossorigin="use-credentials">
<link rel="modulepreload" href="/moneykeep-capture.js" crossorigin="anonymous" integrity="sha384-x"><link rel="preload" href="/moneykeep-capture_bg.wasm" crossorigin="anonymous" integrity="sha384-y" as="fetch" type="application/wasm"></head>
<body><canvas id="canvas"></canvas>
  <script type="module" src="start.js?b=__MK_CAPTURE_VERSION__"></script>
</body></html>"#;

    #[test]
    fn trunks_loader_and_preloads_go_and_the_rest_stays() {
        let (html, removed) = strip(BUILT);
        assert_eq!(
            removed,
            Removed {
                scripts: 1,
                preloads: 2
            }
        );
        assert!(!html.contains("import init"), "{html}");
        assert!(!html.contains("preload"), "{html}");
        assert!(html.contains(
            r#"<script type="module" src="start.js?b=__MK_CAPTURE_VERSION__"></script>"#
        ));
        assert!(html.contains(
            r#"<link rel="manifest" href="manifest.webmanifest" crossorigin="use-credentials">"#
        ));
        assert!(html.contains("<canvas id=\"canvas\"></canvas>"));
        // Stripping again changes nothing.
        let (again, none) = strip(&html);
        assert_eq!(again, html);
        assert_eq!(none, Removed::default());
    }

    #[test]
    fn other_scripts_and_links_are_kept() {
        let page = r#"<script>let a = 1;</script><link rel="preload" href="/other.wasm"><link rel="icon" href="x.png">"#;
        let (html, removed) = strip(page);
        assert_eq!(html, page);
        assert_eq!(removed, Removed::default());
    }

    /// The real built page, when given (`MK_PHONE_PAGE`: a Trunk-built index.html).
    #[test]
    fn a_real_built_page_when_given() {
        let Some(p) = std::env::var_os("MK_PHONE_PAGE") else {
            eprintln!("MK_PHONE_PAGE not set: NOT CHECKED");
            return;
        };
        let (html, removed) = strip(&std::fs::read_to_string(p).unwrap());
        assert_eq!(removed.scripts, 1, "{html}");
        assert_eq!(removed.preloads, 2, "{html}");
        assert!(html.contains("start.js?b="));
    }
}
