// mobile-capture.md revision 9, tests 61 and 66: the page fills the visible
// viewport (index.html's canvas rules and shell.js's resize helper), and the
// browser's install offer reaches the app at once. Revision 15, tests 85 (the page's
// side: reload once on the worker's word) and 86 (a failing start shows a message
// and Reload). Runs web/shell.js with minimal
// fakes of the browser globals (Node 22 test runner, no packages).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  PROGRAM_OFFER,
  START_FAILED,
  createCapture,
  offerProgram,
  withBuild,
  fitCanvas,
  onWorkerMessage,
  pageBuild,
  registerWorker,
  safeInsets,
  setup,
  startApp,
  viewportSize,
} from "../web/shell.js";

const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");

// A minimal event target.
function target(extra = {}) {
  const handlers = {};
  return {
    handlers,
    addEventListener(type, fn) {
      (handlers[type] ||= []).push(fn);
    },
    fire(type, ev = {}) {
      for (const fn of handlers[type] || []) fn(ev);
    },
    ...extra,
  };
}

// A fake ResizeObserver whose observations the test triggers.
function observers() {
  const made = [];
  class FakeObserver {
    constructor(cb) {
      this.cb = cb;
      this.observed = [];
      made.push(this);
    }
    observe(el) {
      this.observed.push(el);
    }
    // The canvas laid out at `width` × `height` CSS px.
    report(width, height) {
      this.cb([{ contentRect: { width, height } }], this);
    }
  }
  return { FakeObserver, made };
}

// A fake window: `inner` its inner size, `vv` its visualViewport (or null).
// A minimal element (for the start-failure message).
function element(tag) {
  const el = target({
    tag,
    children: [],
    attrs: {},
    style: {},
    textContent: "",
    setAttribute(k, v) {
      el.attrs[k] = v;
    },
    appendChild(child) {
      el.children.push(child);
      return child;
    },
    click() {
      el.fire("click", {});
    },
  });
  return el;
}

function fakeWindow({
  inner = [412, 915],
  vv = null,
  insets = ["0px", "0px"],
  sw = true,
  build = "0.9.1+bbbbbbbbbbbb",
  dark = false,
} = {}) {
  const vars = {};
  const registered = [];
  const { FakeObserver, made } = observers();
  const canvas = { id: "canvas" };
  const probe = { id: "mk-insets" };
  const body = element("body");
  const findById = (el, id) => {
    if (el.id === id) return el;
    for (const c of el.children || []) {
      const hit = findById(c, id);
      if (hit) return hit;
    }
    return null;
  };
  const doc = {
    documentElement: {
      style: {
        setProperty(name, value) {
          vars[name] = value;
        },
      },
    },
    body,
    createElement: element,
    querySelector(sel) {
      if (sel !== 'meta[name="moneykeep-capture-version"]' || build === null) return null;
      return { getAttribute: (k) => (k === "content" ? build : null) };
    },
    getElementById(id) {
      return id === "canvas" ? canvas : id === "mk-insets" ? probe : findById(body, id);
    },
  };
  const stored = new Map();
  const win = target({
    innerWidth: inner[0],
    innerHeight: inner[1],
    visualViewport: vv ? target(vv) : null,
    document: doc,
    ResizeObserver: FakeObserver,
    getComputedStyle(el) {
      assert.equal(el, probe);
      return { paddingTop: insets[0], paddingBottom: insets[1] };
    },
    matchMedia: (q) => ({ matches: q === "(prefers-color-scheme: dark)" ? dark : false }),
    location: {
      reloads: 0,
      reload() {
        this.reloads++;
      },
    },
    sessionStorage: {
      getItem: (k) => (stored.has(k) ? stored.get(k) : null),
      setItem: (k, v) => stored.set(k, String(v)),
    },
    navigator: sw
      ? {
          serviceWorker: target({
            register(url, opts) {
              registered.push([url, opts]);
              return Promise.resolve();
            },
          }),
        }
      : {},
  });
  return { win, doc, canvas, vars, registered, observers: made, stored };
}

test("AT61: index.html gives the canvas no outline or tap highlight and dynamic viewport size", () => {
  const style = html.slice(html.indexOf("<style>"), html.indexOf("</style>"));
  const canvasRule = style.slice(style.indexOf("canvas#canvas {"));
  assert.match(canvasRule, /outline:\s*none/);
  assert.match(canvasRule, /-webkit-tap-highlight-color:\s*transparent/);
  assert.match(style, /canvas#canvas:focus, canvas#canvas:focus-visible \{ outline: none; \}/);
  // Dynamic viewport units (with a fallback), and !important so the inline size
  // Slint writes cannot pin the canvas.
  assert.match(style, /height: var\(--mk-vh, 100dvh\) !important/);
  assert.match(style, /width: var\(--mk-vw, 100dvw\) !important/);
  assert.match(style, /height: var\(--mk-vh, 100%\) !important/);
  assert.match(style, /min-width: 0 !important; min-height: 0 !important/);
  assert.match(style, /overscroll-behavior: none/);
  assert.match(style, /overflow: hidden/);
  // The page background is the app's window background in both themes.
  assert.match(style, /background: #f4f2ef/);
  assert.match(style, /prefers-color-scheme: dark\)[^}]*background: #1f1d1b/);
  assert.match(html, /viewport-fit=cover/);
  assert.match(html, /env\(safe-area-inset-bottom/);
  assert.match(html, /<link data-trunk rel="copy-file" href="web\/shell.js">/);
});

test("AT61: the viewport's size goes to the canvas, and its CSS size to the app", () => {
  const { win, doc, canvas, vars, observers: obs } = fakeWindow({ insets: ["24px", "34px"] });
  const state = createCapture(win);
  fitCanvas(win, doc, canvas, state);
  assert.equal(vars["--mk-vw"], "412px");
  assert.equal(vars["--mk-vh"], "915px");
  assert.deepEqual(obs[0].observed, [canvas], "observes the canvas");
  const sizes = [];
  state.onResize((...a) => sizes.push(a));
  obs[0].report(412, 915);
  assert.deepEqual(sizes, [[412, 915, 24, 34]]);
  // The same size again changes nothing.
  obs[0].report(412, 915);
  assert.equal(sizes.length, 1);
  // Rotation: the window's resize sets the new size; the canvas's observer tells
  // the app.
  win.innerWidth = 915;
  win.innerHeight = 412;
  win.fire("resize");
  assert.equal(vars["--mk-vw"], "915px");
  assert.equal(vars["--mk-vh"], "412px");
  obs[0].report(915, 412);
  assert.deepEqual(sizes.at(-1), [915, 412, 24, 34]);
  // A late listener gets the last size at once.
  const late = [];
  state.onResize((...a) => late.push(a));
  assert.deepEqual(late, [[915, 412, 24, 34]]);
});

test("AT61: the on-screen keyboard (visualViewport) shrinks the canvas; pinch zoom does not", () => {
  const { win, doc, canvas, vars } = fakeWindow({
    inner: [390, 844],
    vv: { width: 390, height: 844, scale: 1 },
  });
  const state = createCapture(win);
  fitCanvas(win, doc, canvas, state);
  assert.equal(vars["--mk-vh"], "844px");
  win.visualViewport.height = 500;
  win.visualViewport.fire("resize");
  assert.equal(vars["--mk-vh"], "500px");
  // Zoomed in: the window's size, not the zoomed view's.
  win.visualViewport.scale = 2;
  win.visualViewport.width = 195;
  win.visualViewport.height = 250;
  win.visualViewport.fire("resize");
  assert.equal(vars["--mk-vw"], "390px");
  assert.equal(vars["--mk-vh"], "844px");
  assert.deepEqual(viewportSize({ innerWidth: 360.4, innerHeight: 640.6 }), {
    width: 360,
    height: 641,
  });
});

test("AT61: without ResizeObserver the viewport's size goes to the app directly", () => {
  const { win, doc, canvas } = fakeWindow({ inner: [360, 740] });
  win.ResizeObserver = undefined;
  const state = createCapture(win);
  const sizes = [];
  state.onResize((...a) => sizes.push(a));
  fitCanvas(win, doc, canvas, state);
  assert.deepEqual(sizes, [[360, 740, 0, 0]]);
});

test("safe-area insets read from the probe; missing probe or values give 0", () => {
  const { win, doc } = fakeWindow({ insets: ["47px", "34px"] });
  assert.deepEqual(safeInsets(win, doc), { top: 47, bottom: 34 });
  const bare = fakeWindow({ insets: ["", "auto"] });
  assert.deepEqual(safeInsets(bare.win, bare.doc), { top: 0, bottom: 0 });
  assert.deepEqual(safeInsets(win, { getElementById: () => null }), { top: 0, bottom: 0 });
});

test("AT66: an install offer arriving after start calls the app's callback once and is kept", () => {
  const { win } = fakeWindow();
  const state = setup(win);
  assert.equal(win.mkCapture, state, "window.mkCapture (renamed from mcCapture)");
  assert.equal(win.mcCapture, undefined);
  let calls = 0;
  state.listen(() => calls++);
  let prevented = false;
  let prompted = 0;
  const offer = {
    preventDefault() {
      prevented = true;
    },
    prompt() {
      prompted++;
    },
  };
  win.fire("beforeinstallprompt", offer);
  assert.equal(calls, 1, "the app is told once");
  assert.equal(state.installEvent, offer, "the offer is kept");
  assert.ok(prevented, "the browser's own banner is held back");
  // Install app: the offer is used once and the app redraws without it.
  assert.equal(state.install(), true);
  assert.equal(prompted, 1);
  assert.equal(state.installEvent, null);
  assert.equal(calls, 2);
  assert.equal(state.install(), false);
  // Installed: the app is told, and knows it runs installed.
  win.fire("beforeinstallprompt", offer);
  win.fire("appinstalled");
  assert.equal(state.installEvent, null);
  assert.equal(state.standalone, true);
  assert.equal(calls, 4);
});

test("AT66: an offer before the app listens is kept for its first draw", () => {
  const { win } = fakeWindow();
  const state = setup(win);
  const offer = { preventDefault() {}, prompt() {} };
  win.fire("beforeinstallprompt", offer);
  assert.equal(state.installEvent, offer);
  let calls = 0;
  state.listen(() => calls++);
  assert.equal(calls, 0);
});

test("the service worker is registered at once, not after load", () => {
  const { win, registered } = fakeWindow();
  setup(win);
  assert.deepEqual(registered, [["/sw.js", { scope: "/" }]]);
  assert.equal(win.handlers.load, undefined, "no load handler");
  const none = fakeWindow({ sw: false });
  assert.equal(registerWorker(none.win), false);
});

const shellMessage = (build) => ({ type: "moneykeep-capture-shell", build, reason: "redirect" });

test("AT85: the page reloads once when the worker served another build because of a redirect", () => {
  const { win } = fakeWindow({ build: "0.9.1+bbbbbbbbbbbb" });
  setup(win);
  assert.equal(pageBuild(win.document), "0.9.1+bbbbbbbbbbbb");
  // The worker's message arrives through navigator.serviceWorker.
  win.navigator.serviceWorker.fire("message", { data: shellMessage("0.9.0+aaaaaaaaaaaa") });
  assert.equal(win.location.reloads, 1, "reloaded (a navigation, through the sign-in)");
  // The same again (after the reload, in the same tab): not again.
  win.navigator.serviceWorker.fire("message", { data: shellMessage("0.9.0+aaaaaaaaaaaa") });
  assert.equal(win.location.reloads, 1, "only once");
});

test("AT85: no reload for the page's own build, other messages, or an unknown page build", () => {
  const { win } = fakeWindow({ build: "0.9.0+aaaaaaaaaaaa" });
  assert.equal(onWorkerMessage(win, shellMessage("0.9.0+aaaaaaaaaaaa")), false);
  assert.equal(onWorkerMessage(win, { type: "other", build: "x", reason: "redirect" }), false);
  assert.equal(onWorkerMessage(win, null), false);
  assert.equal(win.location.reloads, 0);
  const bare = fakeWindow({ build: null });
  assert.equal(pageBuild(bare.doc), null);
  assert.equal(onWorkerMessage(bare.win, shellMessage("0.9.0+aaaaaaaaaaaa")), false);
  // Session storage refused (private mode): still once per page load.
  const locked = fakeWindow();
  locked.win.sessionStorage = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.equal(onWorkerMessage(locked.win, shellMessage("0.9.0+aaaaaaaaaaaa")), true);
  assert.equal(onWorkerMessage(locked.win, shellMessage("0.9.0+aaaaaaaaaaaa")), false);
  assert.equal(locked.win.location.reloads, 1);
});

test("AT85: index.html carries the build placeholder the server fills in", () => {
  assert.match(html, /<meta name="moneykeep-capture-version" content="__MK_CAPTURE_VERSION__">/);
});

test("AT86: a module that fails to start shows the message and Reload", async () => {
  for (const init of [
    () => Promise.reject(new WebAssembly.LinkError("import object field is not a Function")),
    () => {
      throw new TypeError("Failed to fetch dynamically imported module");
    },
    () => Promise.reject(new WebAssembly.CompileError("expected magic word")),
  ]) {
    for (const dark of [false, true]) {
      const { win, doc } = fakeWindow({ dark });
      const started = await startApp(win, init);
      assert.equal(started, false);
      const box = doc.getElementById("mk-start-failed");
      assert.ok(box, "the message shows instead of a blank screen");
      assert.equal(box.attrs.role, "alert");
      const [text, button] = box.children;
      assert.equal(text.textContent, START_FAILED);
      assert.equal(
        START_FAILED,
        "MoneyKeep couldn't start. Close the app and open it again while your computer is on.",
      );
      assert.equal(button.textContent, "Reload");
      assert.equal(box.style.background, dark ? "#1f1d1b" : "#f4f2ef");
      button.click();
      assert.equal(win.location.reloads, 1, "Reload reloads the page");
    }
  }
});

test("AT86: a module that starts shows nothing (Slint's control-flow exception included)", async () => {
  for (const init of [
    () => Promise.resolve({}),
    () =>
      Promise.reject(
        new Error("Using exceptions for control flow, don't mind me. This isn't actually an error!"),
      ),
  ]) {
    const { win, doc } = fakeWindow();
    assert.equal(await startApp(win, init), true);
    assert.equal(doc.getElementById("mk-start-failed"), null);
  }
});

test("AT86: start.js starts the app through startApp", () => {
  const start = readFileSync(new URL("../web/start.js", import.meta.url), "utf8");
  // upgrade-resilience-1 M8: shell.js and the glue are asked for with the page's
  // build (from its meta tag), so the worker serves one build's files.
  assert.match(start, /meta\[name="moneykeep-capture-version"\]/);
  assert.match(start, /import\(`\.\/shell\.js\$\{q\}`\)\.then\(/);
  assert.match(start, /\(\{ setup, startApp \}\) =>/);
  // Revision 18: the loader gets the program's bytes from shell.js.
  assert.match(start, /startApp\(window, \(program\) =>\s+import\(`\.\/moneykeep-capture\.js\$\{q\}`\)/);
  assert.match(start, /module_or_path: program/);
  assert.ok(!/^import /m.test(start), "no static import that could fail before the message");
});

test("UR-7 (M8): the page asks for start.js, and every file, with its build", () => {
  assert.match(html, /<script type="module" src="start\.js\?b=__MK_CAPTURE_VERSION__"><\/script>/);
  const { doc } = fakeWindow({ build: "0.9.1+bbbbbbbbbbbb" });
  assert.equal(withBuild(doc, "./moneykeep-capture_bg.wasm"), "./moneykeep-capture_bg.wasm?b=0.9.1+bbbbbbbbbbbb");
  const bare = fakeWindow({ build: null });
  assert.equal(withBuild(bare.doc, "./x.js"), "./x.js");
});

test("UR-7 (M8): the page reloads once when its files can't be served with the worker's build", () => {
  const { win } = fakeWindow({ build: "0.9.0+aaaaaaaaaaaa" });
  setup(win);
  const msg = { type: "moneykeep-capture-shell", build: "0.9.1+bbbbbbbbbbbb", page: "0.9.0+aaaaaaaaaaaa", reason: "other-build" };
  win.navigator.serviceWorker.fire("message", { data: msg });
  win.navigator.serviceWorker.fire("message", { data: msg });
  assert.equal(win.location.reloads, 1);
  assert.equal(onWorkerMessage(win, { ...msg, reason: "other" }), false);
});

// A window whose service worker registration has an installing worker.
function installingWindow({ build = "0.9.1+bbbbbbbbbbbb", controller = null } = {}) {
  const made = fakeWindow({ build });
  const posted = [];
  const reg = { installing: { postMessage: (m) => posted.push(m) }, waiting: null, active: null };
  made.win.navigator.serviceWorker.register = () => Promise.resolve(reg);
  made.win.navigator.serviceWorker.controller = controller;
  registerWorker(made.win);
  return { ...made, posted, reg };
}

test("UR-7 (L9): on the first visit the downloaded program is handed to the installing worker", async () => {
  const bytes = new Uint8Array([0, 97, 115, 109]);
  const { win, posted } = installingWindow();
  assert.equal(await offerProgram(win, bytes, { controlled: false, build: "0.9.1+bbbbbbbbbbbb" }), true);
  assert.deepEqual(posted, [{ type: PROGRAM_OFFER, build: "0.9.1+bbbbbbbbbbbb", bytes }]);
  // Not when a worker answered the page (it has its copy), not another build's
  // program, not without a page build or an installing worker.
  for (const [w, info] of [
    [installingWindow(), { controlled: true, build: "0.9.1+bbbbbbbbbbbb" }],
    [installingWindow(), { controlled: false, build: "0.9.0+aaaaaaaaaaaa" }],
    [installingWindow(), { controlled: false, build: null }],
    [installingWindow({ build: null }), { controlled: false, build: null }],
  ]) {
    assert.equal(await offerProgram(w.win, bytes, info), false);
    assert.deepEqual(w.posted, []);
  }
  const done = installingWindow();
  done.reg.installing = null;
  assert.equal(await offerProgram(done.win, bytes, { controlled: false, build: "0.9.1+bbbbbbbbbbbb" }), false);
});

test("UR-7 (L9): startApp downloads the program once, with the page's build, and offers it", async () => {
  const { win, posted } = installingWindow();
  const asked = [];
  win.fetch = async (url) => {
    asked.push(url);
    return {
      ok: true,
      status: 200,
      headers: { get: (k) => (k === "X-MK-Build" ? "0.9.1+bbbbbbbbbbbb" : null) },
      body: null,
      arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer,
    };
  };
  let given = null;
  assert.equal(await startApp(win, (program) => ((given = program), Promise.resolve())), true);
  assert.deepEqual(asked, ["./moneykeep-capture_bg.wasm?b=0.9.1+bbbbbbbbbbbb"], "one download");
  assert.deepEqual([...given], [1, 2, 3]);
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(posted.length, 1);
  assert.deepEqual([...posted[0].bytes], [1, 2, 3]);
});
