// The page side of the MoneyKeep capture app, kept apart from start.js (which loads
// the wasm module) so Node can test it with fake browser globals
// (tests-js/shell.test.mjs). mobile-capture.md revisions 3, 9 and 15.
//
// - `window.mkCapture`: the browser's install offer for the app's "Install app"
//   button, and the hooks the app registers: `listen(fn)` (called when the offer
//   arrives or goes, so the app redraws at once) and `onResize(fn)` (called with
//   the canvas's CSS size and the safe-area insets whenever they change).
// - The canvas fills the visible viewport: the size goes into the CSS variables
//   `--mk-vw` / `--mk-vh` that index.html's canvas rule uses (with !important, so
//   a size Slint writes into the canvas's inline style cannot pin it), at load and
//   on every viewport change (window resize, rotation, the address bar, the
//   on-screen keyboard through `visualViewport`).
// - The service worker is registered at once (not after `load`), so the browser
//   can make its install offer sooner on a first visit.
// - Revision 15: the page knows its build (index.html's `moneykeep-capture-version`
//   meta tag, which the server fills in). When the service worker says it answered
//   the app from its cache of another build because the network answered with a
//   redirect (Cloudflare Access's sign-in), the page reloads once, so the browser
//   goes through the sign-in instead of showing a blank screen.
// - `startApp`: starts the app; when the module fails to start, a short message with
//   a Reload button instead of a blank screen.
// - Revision 18: the loading card (index.html's `#mk-loading`, shown before any
//   script runs). `startApp` downloads the program itself, so the card can show
//   "Downloading… 45 %" (from `X-MK-Program-Size`, else `Content-Length`), then
//   "Getting ready…" while it is prepared; the app says when it has drawn its first
//   frame (`mkCapture.ready()`) and the card fades out and is removed. On the Wi-Fi
//   address, no answer within 8 seconds shows "Can't reach your computer on this
//   Wi-Fi" with Try again (and Use Cloudflare when the computer has a Cloudflare
//   address). A start failure shows its message and Reload inside the card.
// - upgrade-resilience-1 Batch U2: every file the page loads says the page's build
//   (`?b=<build>`), so the service worker serves one build's files together (M8);
//   on the first visit the downloaded program is handed to the installing worker
//   (L9, `offerProgram`).

/** The visible viewport's CSS size: `visualViewport` when not pinch-zoomed (it
 * shrinks for the on-screen keyboard), else the window's inner size. */
export function viewportSize(win) {
  const vv = win.visualViewport;
  if (vv && vv.width > 0 && vv.height > 0 && Math.abs((vv.scale || 1) - 1) < 0.01) {
    return { width: Math.round(vv.width), height: Math.round(vv.height) };
  }
  return { width: Math.round(win.innerWidth), height: Math.round(win.innerHeight) };
}

/** The safe-area insets (CSS `env(safe-area-inset-*)`) in CSS pixels, read from a
 * probe element whose padding is set to them in index.html (`#mk-insets`). */
export function safeInsets(win, doc) {
  const probe = doc.getElementById("mk-insets");
  if (!probe || !win.getComputedStyle) return { top: 0, bottom: 0 };
  const cs = win.getComputedStyle(probe);
  const px = (v) => {
    const n = parseFloat(v);
    return Number.isFinite(n) && n > 0 ? n : 0;
  };
  return { top: px(cs.paddingTop), bottom: px(cs.paddingBottom) };
}

/** The install offer and the app's hooks (`window.mkCapture`). */
export function createCapture(win) {
  const state = {
    installEvent: null,
    standalone:
      (win.matchMedia ? win.matchMedia("(display-mode: standalone)").matches : false) ||
      (win.navigator && win.navigator.standalone === true),
    listener: null,
    resizer: null,
    size: null,
    install() {
      const e = state.installEvent;
      if (!e) return false;
      state.installEvent = null;
      e.prompt();
      state.changed();
      return true;
    },
    // The app's redraw: called whenever the offer arrives or goes.
    listen(fn) {
      state.listener = typeof fn === "function" ? fn : null;
    },
    changed() {
      if (state.listener) state.listener();
    },
    // The app's resize: called with (width, height, insetTop, insetBottom) in CSS
    // pixels; at once with the last size when one is known.
    onResize(fn) {
      state.resizer = typeof fn === "function" ? fn : null;
      if (state.resizer && state.size) {
        const s = state.size;
        state.resizer(s.width, s.height, s.top, s.bottom);
      }
    },
    // Revision 18: the app has drawn its first frame; the loading card goes.
    ready() {
      hideLoading(win);
    },
    resized(width, height, top, bottom) {
      const s = state.size;
      if (s && s.width === width && s.height === height && s.top === top && s.bottom === bottom) {
        return;
      }
      state.size = { width, height, top, bottom };
      if (state.resizer) state.resizer(width, height, top, bottom);
    },
  };
  win.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    state.installEvent = e;
    state.changed();
  });
  win.addEventListener("appinstalled", () => {
    state.installEvent = null;
    state.standalone = true;
    state.changed();
  });
  return state;
}

/** Keep the canvas the size of the visible viewport and tell the app its size.
 * Returns the function that applies the current size (called once here). */
export function fitCanvas(win, doc, canvas, state) {
  const root = doc.documentElement;
  const report = (width, height) => {
    const ins = safeInsets(win, doc);
    state.resized(width, height, ins.top, ins.bottom);
  };
  const apply = () => {
    const { width, height } = viewportSize(win);
    if (width <= 0 || height <= 0) return;
    root.style.setProperty("--mk-vw", `${width}px`);
    root.style.setProperty("--mk-vh", `${height}px`);
    // Where ResizeObserver is missing, report the viewport's size directly.
    if (!observer) report(width, height);
  };
  // The canvas's own CSS size, as laid out, whenever it changes.
  const observer =
    typeof win.ResizeObserver === "function"
      ? new win.ResizeObserver((entries) => {
          const r = entries[entries.length - 1].contentRect;
          if (r.width > 0 && r.height > 0) report(Math.round(r.width), Math.round(r.height));
        })
      : null;
  if (observer && canvas) observer.observe(canvas);
  win.addEventListener("resize", apply);
  win.addEventListener("orientationchange", apply);
  if (win.visualViewport) win.visualViewport.addEventListener("resize", apply);
  apply();
  return apply;
}

/** Register the service worker now (scope /). The registration's promise is kept
 * (`win.__mkWorkerRegistration`) for the program offer (L9). */
export function registerWorker(win) {
  const sw = win.navigator && win.navigator.serviceWorker;
  if (!sw) return false;
  const reg = Promise.resolve(sw.register("/sw.js", { scope: "/" })).catch(() => null);
  win.__mkWorkerRegistration = reg;
  return true;
}

/** The page's build: index.html's `<meta name="moneykeep-capture-version">`, or
 * null when it has none. */
export function pageBuild(doc) {
  const meta = doc && doc.querySelector ? doc.querySelector('meta[name="moneykeep-capture-version"]') : null;
  const v = meta && typeof meta.getAttribute === "function" ? meta.getAttribute("content") : null;
  return v ? v : null;
}

const RELOADED_KEY = "moneykeep-capture-reloaded";

/** Revision 15: a message from the service worker (of build `build`) that it served
 * the app from its cache because the network answered with a redirect, or
 * (upgrade-resilience-1 M8, reason `other-build`) that this page's files can't be
 * served together with it. Reloads the page (a navigation: through the sign-in, or
 * one complete build) when that build is not the page's, once for each pair of
 * builds. Returns whether it reloaded. */
export function onWorkerMessage(win, data) {
  if (!data || data.type !== "moneykeep-capture-shell") return false;
  if (data.reason !== "redirect" && data.reason !== "other-build") return false;
  const mine = pageBuild(win.document);
  if (!mine || typeof data.build !== "string" || data.build === mine) return false;
  const pair = `${data.build}>${mine}`;
  let store = null;
  try {
    store = win.sessionStorage || null;
    if (store && store.getItem(RELOADED_KEY) === pair) return false;
    if (store) store.setItem(RELOADED_KEY, pair);
  } catch (_) {
    // Without session storage the reload still happens once per page load.
    if (win.__mkCaptureReloaded === pair) return false;
  }
  win.__mkCaptureReloaded = pair;
  win.location.reload();
  return true;
}

/** Listen to the service worker's messages. */
export function watchWorker(win) {
  const sw = win.navigator && win.navigator.serviceWorker;
  if (!sw || typeof sw.addEventListener !== "function") return false;
  sw.addEventListener("message", (e) => onWorkerMessage(win, e && e.data));
  return true;
}

/** Everything start.js does before it loads the app. */
export function setup(win) {
  const doc = win.document;
  const state = createCapture(win);
  win.mkCapture = state;
  watchWorker(win);
  registerWorker(win);
  fitCanvas(win, doc, doc.getElementById("canvas"), state);
  return state;
}

/** The message shown when the app's module fails to start (revision 15). */
export const START_FAILED =
  "MoneyKeep couldn't start. Close the app and open it again while your computer is on.";

// Slint's web event loop (winit) leaves the app's start with this exception once
// the app runs; it is not a failure.
function isControlFlow(err) {
  const text = err && typeof err === "object" ? String(err.message || "") : String(err || "");
  return text.includes("Using exceptions for control flow");
}

// ----- the loading card (revision 18) -----------------------------------------------

/** The card's status while the program downloads ("Downloading… 45 %"). */
export const DOWNLOADING = "Downloading…";
/** … and while it is prepared. */
export const GETTING_READY = "Getting ready…";
/** The Wi-Fi address did not answer within UNREACHABLE_MS. */
export const WIFI_UNREACHABLE = "Can't reach your computer on this Wi-Fi";
export const WIFI_UNREACHABLE_DETAIL =
  "Make sure this phone is on the same Wi-Fi as your computer, and that MoneyKeep is running there.";
export const TRY_AGAIN = "Try again";
export const USE_CLOUDFLARE = "Use Cloudflare";
/** How long the Wi-Fi address may stay silent before the card says so. */
export const UNREACHABLE_MS = 8000;
/** The card's fade-out. */
export const FADE_MS = 150;
/** The card goes after this long even if the app never says it has drawn. */
export const READY_FALLBACK_MS = 20000;
/** The program (wasm module) start.js loads. */
export const PROGRAM_URL = "./moneykeep-capture_bg.wasm";
/** upgrade-resilience-1 M8: the header that says which build a file is of. */
export const BUILD_HEADER = "X-MK-Build";
/** L9: the message that hands the program's bytes to the installing worker. */
export const PROGRAM_OFFER = "moneykeep-capture-program";

/** The address of one of the page's files, with the page's build (`?b=<build>`),
 * so the service worker serves every file of one build (M8). */
export function withBuild(doc, url) {
  const b = pageBuild(doc);
  return b ? `${url}?b=${b}` : url;
}
/** What the app keeps in localStorage for this page (its way in, and the
 * computer's Cloudflare address when it has one). */
export const WAY_KEY = "moneykeep-capture-way";
export const CLOUDFLARE_KEY = "moneykeep-capture-cloudflare";

/** The loading card's parts, or null when the page has none. */
export function loadingCard(doc) {
  const get = (id) => (doc && doc.getElementById ? doc.getElementById(id) : null);
  const root = get("mk-loading");
  if (!root) return null;
  return {
    root,
    card: get("mk-loading-card") || root,
    status: get("mk-status"),
    bar: get("mk-bar"),
    fill: get("mk-fill"),
    detail: get("mk-detail"),
    actions: get("mk-actions"),
  };
}

function setClass(el, name, on) {
  if (!el) return;
  if (el.classList) {
    if (on) el.classList.add(name);
    else el.classList.remove(name);
  }
}

function setText(el, text) {
  if (el) el.textContent = text;
}

function setHidden(el, hidden) {
  if (!el) return;
  el.hidden = hidden;
  if (hidden) el.setAttribute("hidden", "");
  else if (el.removeAttribute) el.removeAttribute("hidden");
}

/** The bar: `percent` 0–100, or null for an indeterminate bar. */
export function setBar(card, percent) {
  if (!card) return;
  setHidden(card.bar, false);
  if (percent === null || percent === undefined) {
    setClass(card.bar, "mk-indeterminate", true);
    if (card.fill) card.fill.style.width = "";
    if (card.bar && card.bar.removeAttribute) card.bar.removeAttribute("aria-valuenow");
  } else {
    setClass(card.bar, "mk-indeterminate", false);
    if (card.fill) card.fill.style.width = `${percent}%`;
    if (card.bar) card.bar.setAttribute("aria-valuenow", String(percent));
  }
}

/** Downloading: `done` bytes of `total` (null or 0: not known). */
export function showProgress(card, done, total) {
  if (!card) return;
  if (total && total > 0) {
    const pct = Math.max(0, Math.min(100, Math.floor((done * 100) / total)));
    setText(card.status, `${DOWNLOADING} ${pct} %`);
    setBar(card, pct);
  } else {
    setText(card.status, DOWNLOADING);
    setBar(card, null);
  }
}

/** Getting ready (indeterminate bar). */
export function showGettingReady(card) {
  if (!card) return;
  setText(card.status, GETTING_READY);
  setBar(card, null);
}

/** The program's size from its answer: `X-MK-Program-Size` (uncompressed), else
 * `Content-Length` (compressed, when that is all there is), else null. */
export function programSize(res) {
  const h = res && res.headers;
  const get = (k) => (h && typeof h.get === "function" ? h.get(k) : null);
  for (const k of ["X-MK-Program-Size", "Content-Length"]) {
    const n = Number.parseInt(get(k) || "", 10);
    if (Number.isFinite(n) && n > 0) return n;
  }
  return null;
}

function timers(win) {
  return {
    set: (fn, ms) => (typeof win.setTimeout === "function" ? win.setTimeout(fn, ms) : setTimeout(fn, ms)),
    clear: (t) => (typeof win.clearTimeout === "function" ? win.clearTimeout(t) : clearTimeout(t)),
  };
}

function storageGet(win, key) {
  try {
    const s = win.localStorage;
    return s ? s.getItem(key) : null;
  } catch (_) {
    return null;
  }
}

/** Whether this page is the Wi-Fi address: the app said so when it paired, or
 * (before pairing) the address is an IPv4 address, as Wi-Fi capture's always is. */
export function isWifiPage(win) {
  const way = storageGet(win, WAY_KEY);
  if (way === "wifi") return true;
  if (way === "cloudflare") return false;
  const host = (win.location && win.location.hostname) || "";
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(host);
}

/** The computer's Cloudflare address (https only), when the app was told one. */
export function cloudflareAddress(win) {
  const v = storageGet(win, CLOUDFLARE_KEY);
  return typeof v === "string" && /^https:\/\/[^\s/]+/.test(v) ? v : null;
}

/** No answer at all from the computer (as opposed to an error answer). */
export class Unreachable extends Error {
  constructor(message) {
    super(message || WIFI_UNREACHABLE);
    this.name = "Unreachable";
  }
}

/** Fetch the program, showing its progress on the card. Resolves to its bytes
 * (Uint8Array). Rejects with `Unreachable` when there is no answer within
 * `timeoutMs` (only when `wifi`) or the fetch fails without one; with an Error for
 * an error answer. */
export async function loadProgram(win, card, { wifi = false, timeoutMs = UNREACHABLE_MS, info = null } = {}) {
  const t = timers(win);
  const controller = typeof win.AbortController === "function" ? new win.AbortController() : null;
  showProgress(card, 0, null);
  const sw = win.navigator && win.navigator.serviceWorker;
  if (info) info.controlled = !!(sw && sw.controller);
  const fetching = Promise.resolve()
    .then(() => win.fetch(withBuild(win.document, PROGRAM_URL), controller ? { signal: controller.signal } : {}))
    .then(
      (res) => ({ res }),
      (err) => ({ err }),
    );
  let timer = null;
  const silent = wifi
    ? new Promise((resolve) => {
        timer = t.set(() => resolve({ silent: true }), timeoutMs);
      })
    : new Promise(() => {});
  const first = await Promise.race([fetching, silent]);
  if (timer !== null) t.clear(timer);
  if (first.silent) {
    if (controller) controller.abort();
    throw new Unreachable();
  }
  if (first.err) throw new Unreachable(String(first.err && first.err.message));
  const res = first.res;
  if (!res || !res.ok) throw new Error(`the program answered ${res ? res.status : "nothing"}`);
  if (info) {
    const h = res.headers;
    info.build = h && typeof h.get === "function" ? h.get(BUILD_HEADER) : null;
  }
  const fromCache = res.headers && typeof res.headers.get === "function" && res.headers.get("X-MK-From-Cache") === "1";
  if (fromCache || !res.body || typeof res.body.getReader !== "function") {
    if (fromCache) showGettingReady(card);
    const buf = await res.arrayBuffer();
    showGettingReady(card);
    return new Uint8Array(buf);
  }
  const total = programSize(res);
  const reader = res.body.getReader();
  const parts = [];
  let done = 0;
  for (;;) {
    const { done: end, value } = await reader.read();
    if (end) break;
    if (value) {
      parts.push(value);
      done += value.length;
      showProgress(card, total ? Math.min(done, total) : done, total);
    }
  }
  showGettingReady(card);
  const bytes = new Uint8Array(done);
  let at = 0;
  for (const p of parts) {
    bytes.set(p, at);
    at += p.length;
  }
  return bytes;
}

function makeButton(doc, text, primary, onClick) {
  const b = doc.createElement("button");
  b.type = "button";
  b.textContent = text;
  if (b.setAttribute) b.setAttribute("class", primary ? "mk-button mk-primary" : "mk-button");
  b.className = primary ? "mk-button mk-primary" : "mk-button";
  b.addEventListener("click", onClick);
  return b;
}

/** The card says the Wi-Fi address doesn't answer, with Try again and (when the
 * computer has a Cloudflare address) Use Cloudflare. */
export function showUnreachable(win, card) {
  const doc = win.document;
  if (!card) return null;
  setText(card.status, WIFI_UNREACHABLE);
  setClass(card.status, "mk-strong", true);
  setHidden(card.bar, true);
  setText(card.detail, WIFI_UNREACHABLE_DETAIL);
  setHidden(card.detail, false);
  if (card.root) card.root.setAttribute("aria-busy", "false");
  if (card.actions) {
    if (card.actions.replaceChildren) card.actions.replaceChildren();
    card.actions.appendChild(makeButton(doc, TRY_AGAIN, true, () => win.location.reload()));
    const cf = cloudflareAddress(win);
    if (cf) {
      card.actions.appendChild(makeButton(doc, USE_CLOUDFLARE, false, () => win.location.assign(cf)));
    }
    setHidden(card.actions, false);
  }
  return card;
}

/** Fade the loading card out and remove it (the app has drawn). */
export function hideLoading(win) {
  const card = loadingCard(win.document);
  if (!card || card.root.__mkHiding) return false;
  card.root.__mkHiding = true;
  card.root.setAttribute("aria-busy", "false");
  setClass(card.root, "mk-gone", true);
  timers(win).set(() => {
    const r = card.root;
    if (typeof r.remove === "function") r.remove();
    else if (r.parentNode) r.parentNode.removeChild(r);
  }, FADE_MS);
  return true;
}

/** Show START_FAILED and a Reload button: inside the loading card when the page
 * has one (revision 18), else over the canvas. */
export function showStartFailure(win) {
  const doc = win.document;
  if (doc.getElementById("mk-start-failed")) return doc.getElementById("mk-start-failed");
  const card = loadingCard(doc);
  const box = doc.createElement("div");
  box.id = "mk-start-failed";
  box.setAttribute("role", "alert");
  const text = doc.createElement("p");
  text.textContent = START_FAILED;
  const reload = () => win.location.reload();
  if (card) {
    setHidden(card.status, true);
    setHidden(card.bar, true);
    setHidden(card.detail, true);
    card.root.setAttribute("aria-busy", "false");
    box.setAttribute("class", "mk-failed");
    box.className = "mk-failed";
    text.setAttribute("class", "mk-status mk-strong");
    text.className = "mk-status mk-strong";
    box.appendChild(text);
    box.appendChild(makeButton(doc, "Reload", true, reload));
    card.card.appendChild(box);
    return box;
  }
  const dark = win.matchMedia ? win.matchMedia("(prefers-color-scheme: dark)").matches : false;
  Object.assign(box.style, {
    position: "fixed",
    inset: "0",
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: "20px",
    padding: "24px",
    boxSizing: "border-box",
    textAlign: "center",
    font: "17px/1.4 system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif",
    background: dark ? "#1f1d1b" : "#f4f2ef",
    color: dark ? "#f4f2ef" : "#1f1d1b",
  });
  text.style.margin = "0";
  text.style.maxWidth = "320px";
  const button = doc.createElement("button");
  button.type = "button";
  button.textContent = "Reload";
  Object.assign(button.style, {
    font: "inherit",
    fontWeight: "600",
    minHeight: "44px",
    padding: "0 28px",
    border: "0",
    borderRadius: "8px",
    background: "#1f6f5c",
    color: "#ffffff",
  });
  button.addEventListener("click", reload);
  box.appendChild(text);
  box.appendChild(button);
  doc.body.appendChild(box);
  return box;
}

/** upgrade-resilience-1 L9: on the first visit (no worker answered the page) the
 * program the page downloaded is handed to the installing service worker, which
 * caches it instead of downloading it again. Only the page's own build's program
 * (its `X-MK-Build`) is offered. Resolves to whether it was offered. */
export function offerProgram(win, bytes, info) {
  const sw = win.navigator && win.navigator.serviceWorker;
  const mine = pageBuild(win.document);
  if (!sw || !bytes || !info || info.controlled || !mine || info.build !== mine) {
    return Promise.resolve(false);
  }
  const reg =
    win.__mkWorkerRegistration ||
    (typeof sw.getRegistration === "function" ? sw.getRegistration() : null);
  return Promise.resolve(reg)
    .then((r) => {
      const w = r && (r.installing || r.waiting);
      if (!w || typeof w.postMessage !== "function") return false;
      w.postMessage({ type: PROGRAM_OFFER, build: mine, bytes });
      return true;
    })
    .catch(() => false);
}

/** Start the app with `init` (the wasm-bindgen module's loader, given the
 * program's bytes, or nothing when this page cannot fetch it itself). Revision 18:
 * the program is downloaded here with its progress on the loading card. A failure
 * to start (fetching, compiling or instantiating the module, or its start
 * function throwing) shows START_FAILED with Reload; no answer from the Wi-Fi
 * address shows "Can't reach your computer on this Wi-Fi". Resolves to whether it
 * started. */
export function startApp(win, init) {
  const card = loadingCard(win.document);
  const canFetch = typeof win.fetch === "function";
  let started;
  try {
    const info = {};
    const program = canFetch
      ? loadProgram(win, card, { wifi: isWifiPage(win), info }).then((bytes) => {
          offerProgram(win, bytes, info);
          return bytes;
        })
      : Promise.resolve(undefined);
    started = program.then((bytes) => {
      showGettingReady(card);
      return init(bytes);
    });
  } catch (err) {
    started = Promise.reject(err);
  }
  const fallback = () => {
    if (card) timers(win).set(() => hideLoading(win), READY_FALLBACK_MS);
  };
  return started.then(
    () => {
      fallback();
      return true;
    },
    (err) => {
      if (isControlFlow(err)) {
        fallback();
        return true;
      }
      if (err instanceof Unreachable && isWifiPage(win) && card) {
        showUnreachable(win, card);
        return false;
      }
      showStartFailure(win);
      return false;
    },
  );
}
