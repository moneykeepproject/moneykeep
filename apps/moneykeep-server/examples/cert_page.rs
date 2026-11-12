//! Writes Wi-Fi capture's certificate page as an Android phone and as an iPhone see
//! it (mobile-capture.md revision 10, test 76), light and dark, as HTML files for
//! review (render them with any browser at 390 px wide).
//! `cargo run -p moneykeep-server --example cert_page -- <dir>`.
#![allow(clippy::unwrap_used, clippy::print_stdout)]

use moneykeep_server::wifi;

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let target = wifi::Target {
        ip: "192.168.1.20".parse().unwrap(),
        name: None,
    };
    let (der, _) =
        wifi::new_ca("MoneyKeep Capture CA (OFFICE-PC)", &target, 1_793_000_000).unwrap();
    let info = wifi::Info {
        ca_name: "MoneyKeep Capture CA (OFFICE-PC)".into(),
        address: "192.168.1.20".into(),
        fingerprint: wifi::fingerprint(&der),
        ..wifi::Info::default()
    };
    for (who, ua) in [
        ("android", "Mozilla/5.0 (Linux; Android 14; Pixel 8)"),
        (
            "iphone",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)",
        ),
    ] {
        let page = wifi::page_for(&info, 14443, ua);
        // The dark page: the colour scheme's dark rules applied unconditionally.
        let dark = page.replace("@media (prefers-color-scheme: dark)", "@media all");
        for (word, html) in [("light", page), ("dark", dark)] {
            let p = out.join(format!("phone10-certpage-{who}-{word}.html"));
            std::fs::write(&p, html).unwrap();
            println!("{}", p.display());
        }
    }
}
