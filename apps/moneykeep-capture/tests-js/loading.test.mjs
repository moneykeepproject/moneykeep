// mobile-capture.md revision 18, tests 99–101 and 105: the loading card in
// index.html (static check), the download progress, the card's removal after the
// app's first frame, a start failure inside the card, and the Wi-Fi address that
// never answers. Runs web/shell.js with small fakes of the browser (Node 22 test
// runner, no packages).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  CLOUDFLARE_KEY,
  DOWNLOADING,
  FADE_MS,
  GETTING_READY,
  START_FAILED,
  TRY_AGAIN,
  UNREACHABLE_MS,
  USE_CLOUDFLARE,
  WAY_KEY,
  WIFI_UNREACHABLE,
  WIFI_UNREACHABLE_DETAIL,
  createCapture,
  isWifiPage,
  programSize,
  startApp,
} from "../web/shell.js";

const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const ui = readFileSync(new URL("../src/ui.rs", import.meta.url), "utf8");

// ----- a small fake DOM ------------------------------------------------------------------

function element(tag, id = "") {
  const handlers = {};
  const el = {
    tag,
    id,
    children: [],
    attrs: {},
    style: {},
    hidden: false,
    textContent: "",
    parentNode: null,
    classes: new Set(),
    classList: {
      add: (c) => el.classes.add(c),
      remove: (c) => el.classes.delete(c),
      contains: (c) => el.classes.has(c),
    },
    setAttribute(k, v) {
      el.attrs[k] = String(v);
    },
    removeAttribute(k) {
      delete el.attrs[k];
    },
    appendChild(c) {
      c.parentNode = el;
      el.children.push(c);
      return c;
    },
    replaceChildren() {
      el.children = [];
    },
    remove() {
      if (el.parentNode) el.parentNode.children = el.parentNode.children.filter((c) => c !== el);
      el.parentNode = null;
    },
    addEventListener(type, fn) {
      (handlers[type] ||= []).push(fn);
    },
    click() {
      for (const fn of handlers.click || []) fn({});
    },
  };
  return el;
}

// The page as index.html builds it (its loading card's parts).
function page({ host = "192.168.1.20", stored = {}, fetch } = {}) {
  const body = element("body");
  const root = body.appendChild(element("div", "mk-loading"));
  root.setAttribute("role", "status");
  root.setAttribute("aria-busy", "true");
  const card = root.appendChild(element("div", "mk-loading-card"));
  const status = card.appendChild(element("p", "mk-status"));
  status.textContent = "Downloading…";
  const bar = card.appendChild(element("div", "mk-bar"));
  bar.classes.add("mk-indeterminate");
  bar.appendChild(element("div", "mk-fill"));
  const detail = card.appendChild(element("p", "mk-detail"));
  detail.hidden = true;
  const actions = card.appendChild(element("div", "mk-actions"));
  actions.hidden = true;
  const find = (el, id) => {
    if (el.id === id) return el;
    for (const c of el.children) {
      const hit = find(c, id);
      if (hit) return hit;
    }
    return null;
  };
  const doc = {
    body,
    createElement: (tag) => element(tag),
    getElementById: (id) => find(body, id),
    querySelector: () => null,
  };
  // Timers the test runs by hand.
  const pending = [];
  let now = 0;
  const win = {
    document: doc,
    location: {
      hostname: host,
      reloads: 0,
      assigned: [],
      reload() {
        this.reloads++;
      },
      assign(u) {
        this.assigned.push(u);
      },
    },
    localStorage: {
      getItem: (k) => (k in stored ? stored[k] : null),
    },
    fetch,
    setTimeout(fn, ms) {
      const t = { at: now + ms, fn };
      pending.push(t);
      return t;
    },
    clearTimeout(t) {
      const i = pending.indexOf(t);
      if (i >= 0) pending.splice(i, 1);
    },
    addEventListener() {},
    matchMedia: () => ({ matches: false }),
    navigator: {},
  };
  const advance = async (ms) => {
    now += ms;
    for (;;) {
      pending.sort((a, b) => a.at - b.at);
      const t = pending[0];
      if (!t || t.at > now) break;
      pending.shift();
      t.fn();
      await settle();
    }
  };
  return { win, doc, root, status, bar, detail, actions, advance };
}

const settle = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

// A streamed answer: the body arrives in `chunks`, each when the test says.
function streamed(chunks, headers = {}) {
  const gates = [];
  let i = 0;
  const reader = {
    read() {
      if (i >= chunks.length) return Promise.resolve({ done: true });
      const chunk = chunks[i++];
      return new Promise((resolve) => gates.push(() => resolve({ done: false, value: chunk })));
    },
  };
  const h = new Map(Object.entries(headers).map(([k, v]) => [k.toLowerCase(), v]));
  return {
    res: {
      ok: true,
      status: 200,
      headers: { get: (k) => (h.has(k.toLowerCase()) ? h.get(k.toLowerCase()) : null) },
      body: { getReader: () => reader },
    },
    // Let the next chunk arrive.
    async next() {
      while (!gates.length) await Promise.resolve();
      gates.shift()();
      await settle();
    },
  };
}

// ----- test 99 -----------------------------------------------------------------------------

test("test 99: index.html has the centered loading card, shown without a script", () => {
  const body = html.slice(html.indexOf("<body>"), html.indexOf("</body>"));
  const card = body.slice(body.indexOf('<div id="mk-loading"'), body.indexOf("<script"));
  assert.match(card, /<div id="mk-loading" role="status" aria-busy="true"/);
  assert.match(card, /<img class="mk-icon" src="icons\/icon-192\.png" width="96" height="96" alt="">/);
  assert.match(card, /MoneyKeep Capture/);
  assert.match(card, /id="mk-status"/);
  assert.match(card, /id="mk-bar"/);
  // Shown by the markup and the style sheet alone: not hidden, no inline script.
  assert.doesNotMatch(card, /<div id="mk-loading"[^>]*hidden/);
  assert.doesNotMatch(html, /<script(?![^>]*src=)/, "no inline script");
  const style = html.slice(html.indexOf("<style>"), html.indexOf("</style>"));
  const rule = style.slice(style.indexOf("#mk-loading {"));
  assert.match(rule, /position: fixed; inset: 0/);
  assert.match(rule, /align-items: center; justify-content: center/);
  assert.match(rule, /env\(safe-area-inset-top/);
  assert.match(rule, /transition: opacity 150ms/);
  assert.match(style, /#mk-bar \{[^}]*width: 240px[^}]*border-radius: 2px/);
  assert.match(style, /#mk-loading \.mk-icon \{ width: 96px; height: 96px/);
  // The app's page colours in both themes (no white flash).
  assert.match(rule, /--mk-page: #f4f2ef/);
  assert.match(style, /prefers-color-scheme: dark\)[\s\S]*--mk-page: #1f1d1b/);
  // The old "Loading…" is gone: from the page and from the app's first screen.
  assert.doesNotMatch(html, /Loading…/);
  assert.doesNotMatch(ui, /"Loading…"/);
});

// ----- test 100 ----------------------------------------------------------------------------

test("test 100: the download shows its progress, then Getting ready…", async () => {
  const half = new Uint8Array(500).fill(1);
  const s = streamed([half, half], { "X-MK-Program-Size": "1000", "Content-Length": "300" });
  const p = page({ fetch: async () => s.res });
  let given = null;
  const started = startApp(p.win, async (bytes) => {
    given = bytes;
  });
  await settle();
  assert.equal(p.status.textContent, DOWNLOADING);
  await s.next();
  assert.equal(p.status.textContent, "Downloading… 50 %");
  assert.equal(p.doc.getElementById("mk-fill").style.width, "50%");
  assert.ok(!p.bar.classes.has("mk-indeterminate"), "a filled bar");
  // The last part arrives: getting ready.
  await s.next();
  assert.equal(await started, true);
  assert.equal(p.status.textContent, GETTING_READY);
  assert.ok(p.bar.classes.has("mk-indeterminate"), "getting ready: indeterminate");
  assert.equal(given.length, 1000, "the glue gets the whole program");
});

test("test 100: without a size the bar is indeterminate; the compressed size is used when alone", async () => {
  const s = streamed([new Uint8Array(400), new Uint8Array(400)]);
  const p = page({ fetch: async () => s.res });
  const started = startApp(p.win, async () => {});
  await s.next();
  assert.equal(p.status.textContent, DOWNLOADING);
  assert.ok(p.bar.classes.has("mk-indeterminate"));
  await s.next();
  assert.equal(await started, true);
  assert.equal(p.status.textContent, GETTING_READY);
  const h = (o) => ({ headers: { get: (k) => o[k] ?? null } });
  assert.equal(programSize(h({ "X-MK-Program-Size": "1000", "Content-Length": "300" })), 1000);
  assert.equal(programSize(h({ "Content-Length": "300" })), 300);
  assert.equal(programSize(h({})), null);
});

test("test 100: from the service worker's cache, straight to Getting ready…", async () => {
  const seen = [];
  const res = {
    ok: true,
    status: 200,
    headers: { get: (k) => (k === "X-MK-From-Cache" ? "1" : null) },
    body: { getReader: () => assert.fail("not streamed") },
    arrayBuffer: async () => {
      seen.push(p.status.textContent);
      return new ArrayBuffer(8);
    },
  };
  const p = page({ fetch: async () => res });
  assert.equal(await startApp(p.win, async (b) => seen.push(b.length)), true);
  assert.deepEqual(seen, [GETTING_READY, 8]);
});

// ----- test 101 ----------------------------------------------------------------------------

test("test 101: the card fades and is removed after the app's first frame", async () => {
  const p = page({
    fetch: async () => ({ ok: true, status: 200, headers: { get: () => null }, arrayBuffer: async () => new ArrayBuffer(4) }),
  });
  const capture = createCapture(p.win);
  assert.equal(await startApp(p.win, async () => {}), true);
  assert.ok(p.doc.getElementById("mk-loading"), "still shown until the app has drawn");
  capture.ready();
  assert.equal(p.root.attrs["aria-busy"], "false");
  assert.ok(p.root.classes.has("mk-gone"), "fading");
  assert.ok(p.doc.getElementById("mk-loading"));
  await p.advance(FADE_MS);
  assert.equal(p.doc.getElementById("mk-loading"), null, "removed from the page");
  capture.ready();
});

test("test 101: a start failure shows the message and Reload inside the card", async () => {
  const p = page({ host: "capture.example.com", fetch: async () => ({ ok: false, status: 404, headers: { get: () => null } }) });
  assert.equal(await startApp(p.win, async () => {}), false);
  const box = p.doc.getElementById("mk-start-failed");
  assert.ok(box);
  assert.equal(box.parentNode.id, "mk-loading-card", "inside the centered card");
  assert.equal(box.attrs.role, "alert");
  const [text, button] = box.children;
  assert.equal(text.textContent, START_FAILED);
  assert.equal(button.textContent, "Reload");
  assert.ok(p.status.hidden && p.bar.hidden, "status and bar make way");
  button.click();
  assert.equal(p.win.location.reloads, 1);
  // A module that does not start (after the download) does the same.
  const q = page({
    fetch: async () => ({ ok: true, status: 200, headers: { get: () => null }, arrayBuffer: async () => new ArrayBuffer(4) }),
  });
  assert.equal(await startApp(q.win, () => Promise.reject(new WebAssembly.CompileError("bad"))), false);
  assert.equal(q.doc.getElementById("mk-start-failed").parentNode.id, "mk-loading-card");
});

// ----- test 105 ----------------------------------------------------------------------------

test("test 105: a Wi-Fi address that never answers says so within 8 seconds, with Try again", async () => {
  const p = page({ fetch: () => new Promise(() => {}) });
  let inits = 0;
  const started = startApp(p.win, async () => inits++);
  await settle();
  await p.advance(UNREACHABLE_MS - 1);
  assert.equal(p.status.textContent, DOWNLOADING, "still waiting");
  await p.advance(1);
  assert.equal(await started, false);
  assert.equal(inits, 0);
  assert.equal(p.status.textContent, WIFI_UNREACHABLE);
  assert.equal(WIFI_UNREACHABLE, "Can't reach your computer on this Wi-Fi");
  assert.equal(p.detail.textContent, WIFI_UNREACHABLE_DETAIL);
  assert.equal(
    WIFI_UNREACHABLE_DETAIL,
    "Make sure this phone is on the same Wi-Fi as your computer, and that MoneyKeep is running there.",
  );
  assert.equal(p.detail.hidden, false);
  assert.equal(p.bar.hidden, true);
  assert.equal(p.root.attrs.role, "status");
  const labels = p.actions.children.map((b) => b.textContent);
  assert.deepEqual(labels, [TRY_AGAIN], "no Cloudflare address: Try again only");
  p.actions.children[0].click();
  assert.equal(p.win.location.reloads, 1);
});

test("test 105: with a Cloudflare address, Use Cloudflare is offered too", async () => {
  const p = page({
    host: "192.168.1.20",
    stored: { [WAY_KEY]: "wifi", [CLOUDFLARE_KEY]: "https://capture.example.com" },
    fetch: () => new Promise(() => {}),
  });
  const started = startApp(p.win, async () => {});
  await settle();
  await p.advance(UNREACHABLE_MS);
  assert.equal(await started, false);
  const labels = p.actions.children.map((b) => b.textContent);
  assert.deepEqual(labels, [TRY_AGAIN, USE_CLOUDFLARE]);
  p.actions.children[1].click();
  assert.deepEqual(p.win.location.assigned, ["https://capture.example.com"]);
});

test("test 105: a failed fetch on Wi-Fi is the same message; the Cloudflare address waits", async () => {
  const p = page({ fetch: async () => Promise.reject(new TypeError("Failed to fetch")) });
  assert.equal(await startApp(p.win, async () => {}), false);
  assert.equal(p.status.textContent, WIFI_UNREACHABLE);
  // A Cloudflare phone: no 8-second rule (Cloudflare always answers).
  const q = page({
    host: "capture.example.com",
    stored: { [WAY_KEY]: "cloudflare" },
    fetch: () => new Promise(() => {}),
  });
  assert.equal(isWifiPage(q.win), false);
  startApp(q.win, async () => {});
  await settle();
  await q.advance(UNREACHABLE_MS * 2);
  assert.equal(q.status.textContent, DOWNLOADING);
  // The way the app stored wins over the address's look.
  assert.equal(isWifiPage(page({ host: "10.0.0.5", stored: { [WAY_KEY]: "cloudflare" } }).win), false);
  assert.equal(isWifiPage(page({ host: "moneykeep.home.arpa", stored: { [WAY_KEY]: "wifi" } }).win), true);
  assert.equal(isWifiPage(page({ host: "192.168.1.20" }).win), true);
});
