// MoneyKeep capture service worker (mobile-capture.md revisions 3, 6, 14, 15 and 18;
// upgrade-resilience-1 Batch U2; W3C Service Workers). Caches the app shell so the
// app opens offline and its outbox keeps working; never caches the API: /api/
// requests always go to the network.
//
// One build at a time (upgrade-resilience-1 M8). The cache name carries the build's
// capture version, which the server writes into this file when it serves it (it
// replaces the placeholder below). A build's cache is filled once, at install, from
// answers of that build (the server marks each file with `X-MK-Build`), and never
// written to afterwards; activation deletes every other cache. The page, start.js,
// shell.js, the JS glue and the program always come from one source:
//
// - A page load is network-first (revision 6): the network's answer as it is, a
//   redirect to Cloudflare Access's sign-in and a 4xx included; this build's cached
//   page when the network fails, answers with status 500 or higher (revision 14) or
//   gives no answer within 6 seconds (revision 18).
// - When the network's page is of another build, the rest of its set (start.js,
//   shell.js, the glue, the program) is asked for from the network at once, within
//   what is left of the same 6 seconds. Only when every one answers ok, from that
//   same build, is the page used, and the page's files are then served from those
//   answers. Otherwise everything comes from this build's cache: its page and its
//   files, one complete build.
// - The page says its build in the address of each file it loads (`?b=<build>`;
//   the server fills it into index.html). A file for this build's page comes from
//   this build's cache. A file for a page of another build that has no answer held
//   for it can't be served from any cache; the page is told (a message), and
//   reloads once.
//
// First install (L9): the page downloads the program itself (with the loading
// card's progress) and hands the bytes to the installing worker, which caches them
// instead of downloading the program a second time; it waits up to two minutes for
// them, then fetches the program itself.
const PREFIX = "moneykeep-capture-";
const VERSION = "moneykeep-capture-__MK_CAPTURE_VERSION__";
const BUILD = VERSION.slice(PREFIX.length);
const PROGRAM = "/moneykeep-capture_bg.wasm";
const CORE = ["/moneykeep-capture.js", PROGRAM];
// The files of one build that are served together with its page (M8).
const SET = ["/start.js", "/shell.js", ...CORE];
const PAGES = ["/", "/index.html"];
const SHELL = [
  ...PAGES,
  ...SET,
  "/manifest.webmanifest",
  "/icons/icon-192.png",
  "/icons/icon-512.png",
  "/icons/icon-512-maskable.png",
  "/icons/apple-touch-icon.png",
];
// The header the server puts on each of the phone app's files: their build.
const BUILD_HEADER = "X-MK-Build";
// The page's message offering its copy of the program (L9).
const PROGRAM_OFFER = "moneykeep-capture-program";
// Answers held for another build's page are used within this time.
const ROUND_MS = 30000;
// Revision 18: a page load and its set that get no answer at all in this time count
// as a failed fetch (this build's cache is used), so a Wi-Fi address that doesn't
// answer opens the cached app instead of a blank screen. Shorter than the page's
// own 8 s, after which it says the computer can't be reached. M8: one budget for
// the page and its whole set.
const NO_ANSWER_MS = 6000;
// L9: how long a first install waits for the page's copy of the program.
const PROGRAM_WAIT_MS = 120000;

// `fetch`, failing when no answer comes within `ms` (where timers exist).
function fetchOrSilent(input, init, ms = NO_ANSWER_MS) {
  if (ms <= 0) return Promise.reject(new TypeError("no answer"));
  const answer = fetch(input, init);
  if (typeof setTimeout !== "function") return answer;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new TypeError("no answer")), ms);
    answer.then(
      (res) => {
        clearTimeout(t);
        resolve(res);
      },
      (err) => {
        clearTimeout(t);
        reject(err);
      },
    );
  });
}

// Revision 18: a core file answered from this build's cache says so
// (`X-MK-From-Cache: 1`), so the loading screen goes straight to "Getting ready…".
function fromCache(hit) {
  if (!hit || typeof Response !== "function" || typeof Headers !== "function" || !hit.headers) {
    return hit;
  }
  try {
    const headers = new Headers(hit.headers);
    headers.set("X-MK-From-Cache", "1");
    return new Response(hit.body, { status: hit.status, statusText: hit.statusText, headers });
  } catch (_) {
    return hit;
  }
}

function href(path) {
  return new URL(path, self.location.origin).href;
}

function pathOf(url) {
  return new URL(url, self.location.origin).pathname;
}

function isApi(url) {
  return pathOf(url).startsWith("/api/");
}

// The build a request's page said (`?b=<build>`, read as written: a build has `+`
// in it), or null when it said none.
function askedBuild(url) {
  const m = /[?&]b=([^&#]*)/.exec(new URL(url, self.location.origin).search);
  return m && m[1] ? m[1] : null;
}

// The build an answer is of (`X-MK-Build`), or null when it doesn't say.
function buildOf(res) {
  const h = res && res.headers;
  const v = h && typeof h.get === "function" ? h.get(BUILD_HEADER) : null;
  return v ? v : null;
}

// This build's cached copy (never another build's cache).
function cached(path) {
  return caches.open(VERSION).then((cache) => cache.match(path));
}

function cachedPage() {
  return cached("/index.html").then((hit) => hit || cached("/"));
}

// An answer usable as a shell file: status 200, not opaque, not a redirect.
function usable(res) {
  return (
    !!res &&
    res.ok &&
    res.status === 200 &&
    res.type !== "opaque" &&
    res.type !== "opaqueredirect" &&
    res.redirected !== true
  );
}

// ----- install: this build's cache, filled once ---------------------------------------

// L9: the page's copy of the program, when it offers one.
let offered = null;
const offerWaiters = [];

self.addEventListener("message", (event) => {
  const d = event && event.data;
  if (!d || d.type !== PROGRAM_OFFER || d.build !== BUILD || !d.bytes) return;
  offered = d;
  for (const w of offerWaiters.splice(0)) w(d);
});

function waitForOffer(ms) {
  if (offered) return Promise.resolve(offered);
  if (typeof setTimeout !== "function") return Promise.resolve(null);
  return new Promise((resolve) => {
    offerWaiters.push(resolve);
    setTimeout(() => resolve(null), ms);
  });
}

// The program from the page's bytes, as the server would answer it.
function programFrom(d) {
  const bytes = d.bytes;
  const size = bytes.byteLength !== undefined ? bytes.byteLength : bytes.length;
  return new Response(bytes, {
    status: 200,
    headers: {
      "Content-Type": "application/wasm",
      [BUILD_HEADER]: BUILD,
      "X-MK-Program-Size": String(size),
    },
  });
}

// The first install (no worker of an earlier build is active): the page is
// downloading the program itself.
function firstInstall() {
  const reg = self.registration;
  return !!reg && !reg.active && typeof Response === "function";
}

// One shell file from the network for the cache: an ok answer of this build.
function fetchForCache(path) {
  return fetch(href(path), { cache: "reload" }).then((res) => {
    const b = buildOf(res);
    if (!usable(res) || (b !== null && b !== BUILD)) {
      throw new TypeError(`${path} is not this build's (${res ? res.status : "no answer"})`);
    }
    return res;
  });
}

// Every shell file of this build fetched first, then all put in the cache; any
// failure fails the install (the browser tries again later) and nothing is used.
async function fill() {
  const first = firstInstall();
  const paths = SHELL.filter((p) => !(first && p === PROGRAM));
  const answers = await Promise.all(paths.map(fetchForCache));
  const files = new Map(paths.map((p, i) => [p, answers[i]]));
  if (first) {
    const d = await waitForOffer(PROGRAM_WAIT_MS);
    files.set(PROGRAM, d ? programFrom(d) : await fetchForCache(PROGRAM));
  }
  offered = null;
  const cache = await caches.open(VERSION);
  await Promise.all([...files].map(([p, res]) => cache.put(p, res)));
}

self.addEventListener("install", (event) => {
  event.waitUntil(fill().then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== VERSION).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

// ----- answers ----------------------------------------------------------------------

self.addEventListener("fetch", (event) => {
  const request = event.request;
  // The API (and anything not a plain GET) goes to the network, uncached.
  if (request.method !== "GET" || isApi(request.url)) {
    return;
  }
  const path = pathOf(request.url);
  if (!SHELL.includes(path)) {
    return;
  }
  if (request.mode === "navigate") {
    event.respondWith(page(request));
  } else if (SET.includes(path)) {
    event.respondWith(setFile(event, request, path));
  } else {
    event.respondWith(otherFile(request, path));
  }
});

// The answers held for a page of another build: {build, at, files}.
let round = null;

// The rest of `build`'s set from the network, within `ms`: the answers when every
// one is ok and of that build, else null (and the downloads stopped).
function setRound(build, ms) {
  if (ms <= 0) return Promise.resolve(null);
  const controller = typeof AbortController === "function" ? new AbortController() : null;
  const q = build ? `?b=${build}` : "";
  const init = { redirect: "manual" };
  if (controller) init.signal = controller.signal;
  return Promise.all(
    SET.map((p) =>
      fetchOrSilent(href(p + q), init, ms).then(
        (res) => res,
        () => null,
      ),
    ),
  ).then((list) => {
    if (!list.every((res) => usable(res) && buildOf(res) === build)) {
      if (controller) controller.abort();
      return null;
    }
    return { build, at: Date.now(), files: new Map(SET.map((p, i) => [p, list[i]])) };
  });
}

// A page load (see the top of this file).
async function page(request) {
  const started = Date.now();
  let res;
  try {
    res = await fetchOrSilent(request);
  } catch (err) {
    const hit = await cachedPage();
    if (hit) return hit;
    throw err;
  }
  if (res.type !== "opaqueredirect" && res.status >= 500) {
    return (await cachedPage()) || res;
  }
  if (!usable(res)) return res;
  const build = buildOf(res);
  if (build === BUILD) {
    round = null;
    return res;
  }
  const r = await setRound(build, NO_ANSWER_MS - (Date.now() - started));
  if (r) {
    round = r;
    return res;
  }
  return (await cachedPage()) || res;
}

// A file of a page's set: from the answers held for its build, from this build's
// cache for this build's page, else from the network when its answer is of the
// page's build. A page that said no build (from before upgrade-resilience-1) gets
// the network's answer when it is ok, else this build's copy.
async function setFile(event, request, path) {
  const want = askedBuild(request.url);
  const r = round;
  if (r && Date.now() - r.at <= ROUND_MS && r.files.has(path) && (want === null || want === r.build)) {
    const res = r.files.get(path);
    r.files.delete(path);
    return res;
  }
  if (want === BUILD) {
    const hit = await cached(path);
    if (hit) return CORE.includes(path) ? fromCache(hit) : hit;
    // This build's cache is still being made: the network's copy of this build.
    const res = await fetchOrSilent(request);
    const b = buildOf(res);
    if (usable(res) && (b === null || b === BUILD)) return res;
    throw new TypeError(`${path}: not this build's`);
  }
  if (want === null) {
    let res = null;
    let err = null;
    try {
      res = await fetchOrSilent(request);
    } catch (e) {
      err = e;
    }
    if (usable(res)) return res;
    const hit = await cached(path);
    if (hit) return CORE.includes(path) ? fromCache(hit) : hit;
    if (res) return res;
    throw err;
  }
  // A page of another build with nothing held for it (a worker of a newer build took
  // over while it loaded, or the held answers were used): only that build's own
  // files will do.
  let res = null;
  try {
    res = await fetchOrSilent(request);
  } catch (_) {
    res = null;
  }
  if (usable(res) && buildOf(res) === want) return res;
  tell(event.clientId, want);
  throw new TypeError(`${path}: build ${want} is not available`);
}

// Tell a page of another build that its files can't be served together; it reloads
// once (the reload gets one complete build).
const told = new Set();
function tell(clientId, build) {
  if (!clientId || told.has(clientId)) return;
  if (!self.clients || typeof self.clients.get !== "function") return;
  told.add(clientId);
  Promise.resolve(self.clients.get(clientId))
    .then((client) => {
      if (client) {
        client.postMessage({ type: "moneykeep-capture-shell", build: BUILD, page: build, reason: "other-build" });
      }
    })
    .catch(() => {});
}

// The manifest and the icons: this build's copy, or the network's answer when this
// build has none.
function otherFile(request, path) {
  return cached(path).then((hit) => hit || fetch(request));
}
