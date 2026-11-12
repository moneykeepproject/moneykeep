//! Entry point. In the browser (wasm32) it starts the web app on the page's canvas;
//! natively it only explains how to build it.

#[cfg(target_arch = "wasm32")]
fn main() {
    moneykeep_capture::web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "moneykeep-capture runs in a browser: build it with `trunk build --release` \
         (target wasm32-unknown-unknown); see docs/deploy/phone-capture.md"
    );
}
