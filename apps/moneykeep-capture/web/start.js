// Starts the MoneyKeep capture app. Trunk builds the wasm-bindgen module with
// unhashed file names (Trunk.toml: filehash = false); this file loads it so the page
// needs no inline script under its Content-Security-Policy (script-src 'self').
//
// shell.js (mobile-capture.md revisions 3, 9 and 15) first sets up `window.mkCapture`
// (the browser's install offer and the app's redraw and resize hooks), registers
// the service worker at once, and keeps the canvas the size of the visible
// viewport; then the app starts on the canvas. If the module fails to start, the
// page shows a short message with a Reload button instead of a blank screen.
// The modules are imported here (not with static imports) so a file that cannot
// be loaded shows a message too.
//
// Revision 18: shell.js downloads the program (showing its progress on the loading
// card) and hands its bytes to the glue's loader; without them the glue fetches it
// itself.
//
// upgrade-resilience-1 M8: each file is asked for with the page's build
// (`?b=<build>`, from index.html's moneykeep-capture-version meta tag), so the
// service worker answers with one build's files.
const meta = document.querySelector('meta[name="moneykeep-capture-version"]');
const build = meta ? meta.getAttribute("content") : "";
const q = build ? `?b=${build}` : "";

import(`./shell.js${q}`).then(
  ({ setup, startApp }) => {
    setup(window);
    return startApp(window, (program) =>
      import(`./moneykeep-capture.js${q}`).then((glue) =>
        glue.default(program ? { module_or_path: program } : undefined),
      ),
    );
  },
  () => {
    const status = document.getElementById("mk-status");
    if (status) {
      status.textContent =
        "MoneyKeep couldn't start. Close the app and open it again while your computer is on.";
    }
  },
);
