// mobile-capture.md AT32, revision 6 (AT44, AT45, AT47), revision 14 (AT80–AT81),
// revision 15 (test 84) and revision 18, and upgrade-resilience-1 UR-7 (M8, L9):
// the service worker caches only the app shell, one cache per build filled once at
// install and never written afterwards, sends /api/ requests to the network, serves
// page loads network-first, and serves a page and its files (start.js, shell.js,
// the glue, the program) from one build: all from the network when the page is of
// another build and its whole set answers, else all from this build's cache.
// (upgrade-resilience-1 replaces AT46, the shell files' cache refresh, and AT85's
// pair rule with the whole-set rule.) Runs web/sw.js in a minimal fake of the
// service worker globals (Node 22 test runner, no packages).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../web/sw.js", import.meta.url), "utf8");
const ORIGIN = "https://capture.example.com";

function pathOf(req) {
  return new URL(typeof req === "string" ? req : req.url, ORIGIN).pathname;
}

class FakeHeaders {
  constructor(init) {
    this.map = new Map();
    const entries = init instanceof FakeHeaders ? init.map : Object.entries(init || {});
    for (const [k, v] of entries) this.map.set(k.toLowerCase(), String(v));
  }
  set(k, v) {
    this.map.set(k.toLowerCase(), String(v));
  }
  get(k) {
    const v = this.map.get(k.toLowerCase());
    return v === undefined ? null : v;
  }
}

class FakeResponse {
  constructor(body, init = {}) {
    this.body = body;
    this.status = init.status === undefined ? 200 : init.status;
    this.statusText = init.statusText || "";
    this.headers = new FakeHeaders(init.headers);
    this.type = "basic";
    this.ok = this.status >= 200 && this.status <= 299;
  }
  clone() {
    return this;
  }
}

// A fake response. `type` is "basic", "opaque" or "opaqueredirect"; `build` its
// X-MK-Build (none when null).
function response(body, { status = 200, type = "basic", build = null, headers = {} } = {}) {
  const h = { ...headers };
  if (build) h["X-MK-Build"] = build;
  return {
    body,
    status,
    type,
    headers: new FakeHeaders(h),
    ok: status >= 200 && status <= 299,
    clone() {
      return response(body, { status, type, build, headers });
    },
  };
}

// The build the tests' worker is (the server writes it into sw.js), and newer ones.
const BUILD = "0.9.0+aaaaaaaaaaaa";
const NEW = "0.9.1+bbbbbbbbbbbb";
const CACHE = `moneykeep-capture-${BUILD}`;
const PAGE_FILES = ["/start.js", "/shell.js", "/moneykeep-capture.js", "/moneykeep-capture_bg.wasm"];

// A capture server of `build`: each file says its build and which build it is.
const server = (build) => (path) => response(`${build} ${path}`, { build });

const offline = () => {
  throw new TypeError("Failed to fetch");
};

// `network(path, init, url)` answers a fetch: a response, or throws (offline).
// `first`: the first install (no active worker yet).
function load(network = server(BUILD), build = BUILD, globals = {}, { first = false } = {}) {
  const handlers = {};
  const cached = new Map(); // cache name -> Map of path -> response
  const fetched = [];
  const writes = [];
  const self = {
    location: { origin: ORIGIN },
    registration: { active: first ? null : {} },
    addEventListener: (type, fn) => {
      handlers[type] = fn;
    },
    skipWaiting: async () => {},
    clients: { claim: async () => {} },
  };
  const caches = {
    open: async (name) => {
      if (!cached.has(name)) cached.set(name, new Map());
      const entries = cached.get(name);
      return {
        put: async (req, res) => {
          writes.push([name, pathOf(req)]);
          entries.set(pathOf(req), res);
        },
        match: async (req) => entries.get(pathOf(req)),
      };
    },
    keys: async () => [...cached.keys()],
    delete: async (name) => cached.delete(name),
  };
  const sw = { handlers, cached, fetched, writes, network, told: [] };
  const fetch = async (req, init) => {
    const url = typeof req === "string" ? req : req.url;
    fetched.push(url);
    return sw.network(pathOf(req), init, url);
  };
  self.clients.get = async (id) => ({ postMessage: (data) => sw.told.push([id, data]) });
  const stamped = source.replaceAll("__MK_CAPTURE_VERSION__", build);
  vm.runInNewContext(stamped, {
    self,
    caches,
    fetch,
    URL,
    Promise,
    Date,
    Map,
    Set,
    Headers: FakeHeaders,
    Response: FakeResponse,
    ...globals,
  });
  return sw;
}

async function install(sw) {
  let wait;
  sw.handlers.install({ waitUntil: (p) => (wait = p) });
  await wait;
}

async function activate(sw) {
  let wait;
  sw.handlers.activate({ waitUntil: (p) => (wait = p) });
  await wait;
}

// An installed, active worker of BUILD whose cache came from a server of BUILD;
// the network is then `network`.
async function installed(network, globals) {
  const sw = load(server(BUILD), BUILD, globals);
  await install(sw);
  await activate(sw);
  sw.writes.length = 0;
  sw.fetched.length = 0;
  sw.network = network;
  return sw;
}

function fetchEvent(sw, url, { method = "GET", mode = "no-cors", clientId = "page-1" } = {}) {
  let responded = null;
  sw.handlers.fetch({
    clientId,
    request: { url, method, mode },
    respondWith: (p) => (responded = p),
  });
  return responded;
}

// A page load and the files its page asks for (with the page's build, as start.js
// and shell.js ask). Resolves to the bodies by path ("/" for the page), or the
// error for a file that failed.
async function open(sw, { clientId = "page-1" } = {}) {
  const pageRes = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate", clientId });
  const pageBuild = pageRes.headers ? pageRes.headers.get("X-MK-Build") : null;
  const out = { "/": pageRes.body, build: pageBuild };
  for (const p of PAGE_FILES) {
    const url = pageBuild ? `${ORIGIN}${p}?b=${pageBuild}` : `${ORIGIN}${p}`;
    try {
      out[p] = (await fetchEvent(sw, url, { clientId })).body;
    } catch (err) {
      out[p] = err;
    }
  }
  return out;
}

// Every file of one complete build (by the bodies' build prefix).
function oneBuild(out) {
  const builds = new Set(["/", ...PAGE_FILES].map((p) => String(out[p]).split(" ")[0]));
  return builds.size === 1 ? [...builds][0] : null;
}

test("install caches only shell files, never the API", async () => {
  const sw = load();
  await install(sw);
  assert.equal(sw.cached.size, 1);
  const [paths] = [...sw.cached.values()];
  for (const p of ["/", "/index.html", "/start.js", "/shell.js", "/moneykeep-capture_bg.wasm", "/manifest.webmanifest", "/icons/icon-512.png"]) {
    assert.ok(paths.has(p), p);
  }
  for (const p of paths.keys()) assert.ok(!p.startsWith("/api/"), p);
  for (const p of paths.keys()) assert.equal(paths.get(p).body, `${BUILD} ${p}`, "from this build's server");
});

test("UR-7 (M8): install refuses another build's files, a redirect or an error; nothing is used", async () => {
  for (const bad of [
    (p) => (p === "/shell.js" ? response(`${NEW} ${p}`, { build: NEW }) : server(BUILD)(p)),
    (p) => (p === "/start.js" ? response(null, { status: 0, type: "opaqueredirect" }) : server(BUILD)(p)),
    (p) => (p === "/moneykeep-capture_bg.wasm" ? response("err", { status: 503 }) : server(BUILD)(p)),
  ]) {
    const sw = load(bad);
    await assert.rejects(install(sw));
    assert.deepEqual(sw.writes, [], "nothing put in the cache");
  }
  // Each file is asked for past the browser's HTTP cache.
  const seen = [];
  const sw = load((p, init) => {
    seen.push(init && init.cache);
    return server(BUILD)(p);
  });
  await install(sw);
  assert.ok(seen.length > 0 && seen.every((c) => c === "reload"));
});

test("/api/ requests go to the network and are not cached", async () => {
  const sw = await installed(server(BUILD));
  for (const [url, method] of [
    [`${ORIGIN}/api/v1/lists`, "GET"],
    [`${ORIGIN}/api/v1/expenses`, "POST"],
    [`${ORIGIN}/api/v1/health`, "GET"],
  ]) {
    assert.equal(fetchEvent(sw, url, { method }), null, url);
  }
  assert.ok(![...sw.cached.get(CACHE).keys()].some((p) => p.startsWith("/api/")));
});

test("non-shell pages and non-GET requests are left to the browser", async () => {
  const sw = await installed(server(BUILD));
  assert.equal(fetchEvent(sw, `${ORIGIN}/other`), null);
  assert.equal(fetchEvent(sw, `${ORIGIN}/other`, { mode: "navigate" }), null);
  assert.equal(fetchEvent(sw, `${ORIGIN}/start.js`, { method: "POST" }), null);
  assert.equal(sw.fetched.length, 0);
});

test("AT44: a page load uses the network's answer, an opaque redirect too", async () => {
  let answer = response("network page", { build: BUILD });
  const sw = await installed(() => answer);
  const online = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
  assert.equal(online.body, "network page");
  assert.deepEqual(sw.fetched, [`${ORIGIN}/`]);
  // Cloudflare Access answers with a redirect to its sign-in.
  answer = response(null, { status: 0, type: "opaqueredirect" });
  const redirect = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
  assert.equal(redirect, answer, "the redirect itself, not the cached page");
});

test("AT45: a page load while the network fails answers with the cached page", async () => {
  const sw = await installed(offline);
  const res = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
  assert.equal(res.body, `${BUILD} /index.html`);
  // With only "/" cached, that is used.
  sw.cached.get(CACHE).delete("/index.html");
  const res2 = await fetchEvent(sw, `${ORIGIN}/index.html`, { mode: "navigate" });
  assert.equal(res2.body, `${BUILD} /`);
});

test("UR-7 (M8): the cache is never written after install", async () => {
  for (const network of [server(BUILD), server(NEW), offline]) {
    const sw = await installed(network);
    await open(sw);
    await open(sw);
    assert.equal((await fetchEvent(sw, `${ORIGIN}/manifest.webmanifest`)).body, `${BUILD} /manifest.webmanifest`);
    assert.deepEqual(sw.writes, []);
    for (const [p, r] of sw.cached.get(CACHE)) assert.equal(r.body, `${BUILD} ${p}`);
  }
});

test("UR-7 (M8): this build's page gets its files from this build's cache", async () => {
  const sw = await installed(server(BUILD));
  const out = await open(sw);
  assert.equal(out["/"], `${BUILD} /`, "the page from the network");
  for (const p of PAGE_FILES) assert.equal(out[p], `${BUILD} ${p}`);
  assert.deepEqual(sw.fetched, [`${ORIGIN}/`], "the program isn't downloaded again");
  // Revision 18: the program from the cache says so.
  const wasm = await fetchEvent(sw, `${ORIGIN}/moneykeep-capture_bg.wasm?b=${BUILD}`);
  assert.equal(wasm.headers.get("X-MK-From-Cache"), "1");
});

test("UR-7 (M8): old worker plus a new server, then offline: one complete old build", async () => {
  const sw = await installed(server(NEW));
  // Online: the new build's page and its whole set, all from the network.
  const online = await open(sw);
  assert.equal(online.build, NEW);
  assert.equal(oneBuild(online), NEW);
  for (const p of PAGE_FILES) assert.ok(sw.fetched.includes(`${ORIGIN}${p}?b=${NEW}`), p);
  // Offline: the old build's page and files, every one from its cache.
  sw.network = offline;
  const off = await open(sw);
  assert.equal(off.build, BUILD);
  assert.equal(oneBuild(off), BUILD);
  // And a server answering 530 (the computer off) the same.
  sw.network = () => response("Error 1033", { status: 530 });
  assert.equal(oneBuild(await open(sw)), BUILD);
});

// Revision 18 / M8: fake timers the test runs by hand.
function clock() {
  const pending = [];
  return {
    setTimeout(fn, ms) {
      const t = { fn, ms };
      pending.push(t);
      return t;
    },
    clearTimeout(t) {
      const i = pending.indexOf(t);
      if (i >= 0) pending.splice(i, 1);
    },
    pending,
    fire() {
      for (const t of pending.splice(0)) t.fn();
    },
  };
}

test("UR-7 (M8): a new page with a slow program: all from the network, or all from the cache", async () => {
  // The program answers in time: everything from the network.
  const quick = await installed(server(NEW));
  assert.equal(oneBuild(await open(quick)), NEW);
  // The program never answers: after the 6 s the page and every file come from
  // this build's cache, never the new page with old files.
  const c = clock();
  const slow = await installed(
    (p) => (p === "/moneykeep-capture_bg.wasm" ? new Promise(() => {}) : server(NEW)(p)),
    { setTimeout: c.setTimeout, clearTimeout: c.clearTimeout },
  );
  const page = fetchEvent(slow, `${ORIGIN}/`, { mode: "navigate" });
  for (let i = 0; i < 10; i++) await Promise.resolve();
  assert.ok(c.pending.length >= 1);
  assert.ok(c.pending.every((t) => t.ms <= 6000), "one 6 s budget for the page and its set");
  c.fire();
  const res = await page;
  assert.equal(res.body, `${BUILD} /index.html`, "the cached page");
  for (const p of PAGE_FILES) {
    assert.equal((await fetchEvent(slow, `${ORIGIN}${p}?b=${BUILD}`)).body, `${BUILD} ${p}`);
  }
  // A set file of another build, or a redirect (Access) in the set: the cached build.
  for (const odd of [
    (p) => (p === "/shell.js" ? response(`${BUILD} ${p}`, { build: BUILD }) : server(NEW)(p)),
    (p) => (p === "/moneykeep-capture.js" ? response(null, { status: 0, type: "opaqueredirect" }) : server(NEW)(p)),
    (p) => (p === "/start.js" ? offline() : server(NEW)(p)),
  ]) {
    const sw = await installed(odd);
    assert.equal(oneBuild(await open(sw)), BUILD);
  }
});

test("UR-7 (M8): a page of another build with nothing held for it is told to reload", async () => {
  // A newer worker took over while an older page loaded: the older build's files
  // aren't in any cache, and the network now has the newer build.
  const sw = await installed(server(NEW));
  await assert.rejects(fetchEvent(sw, `${ORIGIN}/shell.js?b=0.8.0+cccccccccccc`, { clientId: "old-page" }));
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(JSON.parse(JSON.stringify(sw.told)), [
    ["old-page", { type: "moneykeep-capture-shell", build: BUILD, page: "0.8.0+cccccccccccc", reason: "other-build" }],
  ]);
  // The network has that build: its own file.
  sw.network = server("0.8.0+cccccccccccc");
  assert.equal(
    (await fetchEvent(sw, `${ORIGIN}/shell.js?b=0.8.0+cccccccccccc`)).body,
    "0.8.0+cccccccccccc /shell.js",
  );
});

test("UR-7 (M8): a page that says no build (before upgrade-resilience-1) gets the network's ok answer, else the cache", async () => {
  const sw = await installed(() => response("plain", {}));
  assert.equal((await fetchEvent(sw, `${ORIGIN}/start.js`)).body, "plain");
  sw.network = offline;
  assert.equal((await fetchEvent(sw, `${ORIGIN}/start.js`)).body, `${BUILD} /start.js`);
  assert.deepEqual(sw.writes, []);
});

test("AT47/AT84: install uses this build's cache; activate deletes the old caches", async () => {
  const sw = load();
  sw.cached.set("moneykeep-capture-v3", new Map([["/", response("old")]]));
  sw.cached.set("mc-capture-v1", new Map([["/old.js", response("old")]]));
  await install(sw);
  assert.ok(sw.cached.has(CACHE));
  await activate(sw);
  assert.deepEqual([...sw.cached.keys()], [CACHE]);
});

// Revision 14: Cloudflare answers for the computer while it is off.
const CF_1033 = "<!DOCTYPE html><html><head><title>Cloudflare Tunnel error</title></head><body>Error 1033</body></html>";

test("AT80: a page load answered with 5xx uses the cached shell; 4xx and redirects pass through", async () => {
  let answer = response(CF_1033, { status: 530 });
  const sw = await installed(() => answer);
  const r530 = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
  assert.equal(r530.body, `${BUILD} /index.html`, "530: the cached app, not Cloudflare's page");
  for (const status of [500, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527]) {
    answer = response(`error ${status}`, { status });
    const r = await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
    assert.equal(r.body, `${BUILD} /index.html`, `${status}`);
  }
  answer = response("not found", { status: 404 });
  assert.equal(await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" }), answer);
  answer = response(null, { status: 0, type: "opaqueredirect" });
  assert.equal(await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" }), answer);
  answer = response("network page", { build: BUILD });
  assert.equal((await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" })).body, "network page");
  // Nothing cached: the 530 answer is shown.
  sw.cached.clear();
  answer = response(CF_1033, { status: 530 });
  assert.equal(await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" }), answer);
});

test("AT81/AT84: activation deletes -v3 and -v4 and other builds' caches", async () => {
  const sw = load();
  sw.cached.set("moneykeep-capture-v3", new Map([["/", response("old")]]));
  sw.cached.set("moneykeep-capture-v4", new Map([["/", response("old")]]));
  sw.cached.set("moneykeep-capture-0.8.0+bbbbbbbbbbbb", new Map([["/", response("old")]]));
  await install(sw);
  await activate(sw);
  assert.deepEqual([...sw.cached.keys()], [CACHE]);
});

test("AT84: the cache name is the build the server wrote into sw.js", async () => {
  assert.match(source, /const VERSION = "moneykeep-capture-__MK_CAPTURE_VERSION__";/);
  assert.ok(!source.includes("moneykeep-capture-v4"), "no fixed cache name any more");
  const a = load(server("1.0.0+111111111111"), "1.0.0+111111111111");
  await install(a);
  assert.deepEqual([...a.cached.keys()], ["moneykeep-capture-1.0.0+111111111111"]);
  const b = load(server("1.0.1+222222222222"), "1.0.1+222222222222");
  b.cached.set("moneykeep-capture-1.0.0+111111111111", a.cached.get("moneykeep-capture-1.0.0+111111111111"));
  await install(b);
  assert.equal(b.cached.size, 2);
  await activate(b);
  assert.deepEqual([...b.cached.keys()], ["moneykeep-capture-1.0.1+222222222222"]);
});

test("revision 18: a page load that gets no answer opens the cached app after 6 s", async () => {
  const c = clock();
  const sw = await installed(() => new Promise(() => {}), { setTimeout: c.setTimeout, clearTimeout: c.clearTimeout });
  const answer = fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" });
  await Promise.resolve();
  assert.equal(c.pending.length, 1);
  assert.equal(c.pending[0].ms, 6000, "before the page's own 8 s");
  c.fire();
  assert.equal((await answer).body, `${BUILD} /index.html`);
  // An answer in time clears the timer.
  c.pending.length = 0;
  sw.network = () => response("network page", { build: BUILD });
  assert.equal((await fetchEvent(sw, `${ORIGIN}/`, { mode: "navigate" })).body, "network page");
  assert.equal(c.pending.length, 0);
});

// ----- L9: the first install takes the page's copy of the program -------------------

test("UR-7 (L9): the first install fetches the program once (the page's copy)", async () => {
  const c = clock();
  const sw = load(server(BUILD), BUILD, { setTimeout: c.setTimeout, clearTimeout: c.clearTimeout }, { first: true });
  const done = install(sw);
  for (let i = 0; i < 20; i++) await Promise.resolve();
  assert.ok(!sw.fetched.some((u) => pathOf(u) === "/moneykeep-capture_bg.wasm"), "waits for the page");
  // Another build's bytes are not taken.
  sw.handlers.message({ data: { type: "moneykeep-capture-program", build: NEW, bytes: new Uint8Array([9]) } });
  for (let i = 0; i < 20; i++) await Promise.resolve();
  assert.equal(sw.cached.get(CACHE), undefined, "still waiting");
  // The page downloaded the program (with its progress) and hands it over.
  const bytes = new Uint8Array([0, 97, 115, 109, 1]);
  sw.handlers.message({ data: { type: "moneykeep-capture-program", build: BUILD, bytes } });
  await done;
  assert.equal(sw.fetched.filter((u) => pathOf(u) === "/moneykeep-capture_bg.wasm").length, 0, "fetched once: by the page only");
  const wasm = sw.cached.get(CACHE).get("/moneykeep-capture_bg.wasm");
  assert.equal(wasm.body, bytes);
  assert.equal(wasm.headers.get("Content-Type"), "application/wasm");
  assert.equal(wasm.headers.get("X-MK-Build"), BUILD);
  assert.equal(wasm.headers.get("X-MK-Program-Size"), "5");
  assert.equal(c.pending[0].ms, 120000, "it would have waited two minutes");
});

test("UR-7 (L9): no copy from the page in time: the worker fetches the program itself", async () => {
  const c = clock();
  const sw = load(server(BUILD), BUILD, { setTimeout: c.setTimeout, clearTimeout: c.clearTimeout }, { first: true });
  const done = install(sw);
  for (let i = 0; i < 20; i++) await Promise.resolve();
  c.fire();
  await done;
  assert.equal(sw.fetched.filter((u) => pathOf(u) === "/moneykeep-capture_bg.wasm").length, 1);
  assert.equal(sw.cached.get(CACHE).get("/moneykeep-capture_bg.wasm").body, `${BUILD} /moneykeep-capture_bg.wasm`);
  // An update (a worker already active) never waits.
  const update = load(server(BUILD));
  await install(update);
  assert.equal(update.fetched.filter((u) => pathOf(u) === "/moneykeep-capture_bg.wasm").length, 1);
});
