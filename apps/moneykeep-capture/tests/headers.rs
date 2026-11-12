//! The page itself has no inline script and loads nothing from other hosts, so the
//! Content-Security-Policy the capture server sends (mobile-capture.md AT19, tested
//! in apps/moneykeep-server/tests/phone_app.rs) can forbid both.
#![allow(clippy::unwrap_used)]

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn page_has_no_inline_script_or_other_hosts() {
    let html = read("index.html");
    for tag in html.split("<script").skip(1) {
        let open = tag.split('>').next().unwrap();
        assert!(open.contains("src="), "inline script: <script{open}>");
    }
    assert!(
        !html.contains("http://") && !html.contains("https://"),
        "{html}"
    );
    // Slint draws on the canvas with id "canvas" (ui-slint.md).
    assert!(html.contains(r#"<canvas id="canvas">"#));
    assert!(read("web/start.js").contains("./moneykeep-capture.js"));
    assert!(read("Trunk.toml").contains("filehash = false"));
}

/// Revision 2: the Pages Function and `_headers` are gone (the server sends the
/// headers).
#[test]
fn no_pages_files() {
    let dir = env!("CARGO_MANIFEST_DIR");
    assert!(!std::path::Path::new(&format!("{dir}/web/functions")).exists());
    assert!(!std::path::Path::new(&format!("{dir}/tests-js/proxy.test.mjs")).exists());
    assert!(!std::path::Path::new(&format!("{dir}/web/_headers")).exists());
    assert!(!read("index.html").contains("_headers"));
}

/// mobile-capture.md AT31 (manifest): the fields browsers need to install the app,
/// both icon sizes and a maskable icon; index.html links the manifest, the
/// apple-touch-icon and the iPhone metas; Trunk copies the service worker and icons.
#[test]
fn at31_manifest_and_page_links() {
    let m: serde_json::Value = serde_json::from_str(&read("web/manifest.webmanifest")).unwrap();
    assert_eq!(m["name"], "MoneyKeep capture");
    assert_eq!(m["short_name"], "Capture");
    for (k, v) in [
        ("id", "/"),
        ("start_url", "/"),
        ("scope", "/"),
        ("display", "standalone"),
        ("orientation", "portrait"),
    ] {
        assert_eq!(m[k], v, "{k}");
    }
    assert!(m["theme_color"].is_string() && m["background_color"].is_string());
    let icons = m["icons"].as_array().unwrap();
    let has = |size: &str, purpose: &str| {
        icons
            .iter()
            .any(|i| i["sizes"] == size && i["type"] == "image/png" && i["purpose"] == purpose)
    };
    assert!(has("192x192", "any") && has("512x512", "any") && has("512x512", "maskable"));
    for i in icons {
        let file = i["src"].as_str().unwrap().trim_start_matches('/');
        assert!(
            std::path::Path::new(&format!("{}/web/{file}", env!("CARGO_MANIFEST_DIR"))).is_file(),
            "{file}"
        );
    }
    let html = read("index.html");
    for needle in [
        r#"<link rel="manifest" href="manifest.webmanifest">"#,
        r#"<link rel="apple-touch-icon" href="icons/apple-touch-icon.png">"#,
        r#"<meta name="apple-mobile-web-app-capable" content="yes">"#,
        r#"<meta name="apple-mobile-web-app-title" content="Capture">"#,
        r#"<link data-trunk rel="copy-file" href="web/sw.js">"#,
        r#"<link data-trunk rel="copy-dir" href="web/icons">"#,
    ] {
        assert!(html.contains(needle), "{needle}");
    }
    // Registered from the page script at once (revision 9: not after `load`).
    assert!(read("web/shell.js").contains(r#"register("/sw.js", { scope: "/" })"#));
    // upgrade-resilience-1 M8: shell.js and the module are asked for with the
    // page's build (`?b=`), the module through startApp, so a failure to start
    // shows a message; the page and the worker carry the build placeholder the
    // server fills in.
    assert!(read("web/start.js").contains(r#"import(`./shell.js${q}`)"#));
    assert!(read("web/start.js").contains(r#"({ setup, startApp }) =>"#));
    assert!(read("web/start.js").contains(r#"import(`./moneykeep-capture.js${q}`)"#));
    assert!(html.contains(r#"src="start.js?b=__MK_CAPTURE_VERSION__""#));
    assert!(
        read("web/sw.js")
            .contains(r#"const VERSION = "moneykeep-capture-__MK_CAPTURE_VERSION__";"#)
    );
    assert!(
        html.contains(
            r#"<meta name="moneykeep-capture-version" content="__MK_CAPTURE_VERSION__">"#
        )
    );
    assert!(
        read("web/sw.js").contains(r#""/shell.js","#),
        "shell.js is cached"
    );
}
