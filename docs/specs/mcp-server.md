# Spec: MCP server

- **Area / module:** front end (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18 (revision 2: updated for MCP 2026-07-28; revision 3, 2026-11-04:
  separate `keep-mcp` program, per-book AI access, access levels and permission
  groups, the app's local control channel)
- **Status:** revision 3 implemented (2026-11-04)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | MCP specification overview, revision 2026-07-28 — https://modelcontextprotocol.io/specification (text supplied by the project owner, 2026-10-18) | Current revision is 2026-07-28 |
| 2 | Standard | MCP 2026-07-28 Key Changes — https://modelcontextprotocol.io/specification/2026-07-28/changelog.md | Handshake removed, per-request `_meta`, `server/discover`, `subscriptions/listen`, `resultType`, cache fields, `ping` removed, error-code renumbering |
| 3 | Standard | MCP 2026-07-28 Versioning — https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning.md | Version negotiation, `UnsupportedProtocolVersionError`, dual-era servers |
| 4 | Standard | MCP 2026-07-28 Base protocol — https://modelcontextprotocol.io/specification/2026-07-28/basic/index.md | Required `_meta` keys, `resultType`, error-code ranges, statelessness, JSON Schema 2020-12 |
| 5 | Standard | MCP 2026-07-28 Discovery — https://modelcontextprotocol.io/specification/2026-07-28/server/discover.md | `server/discover` request and `DiscoverResult` |
| 6 | Standard | MCP 2026-07-28 Tools — https://modelcontextprotocol.io/specification/2026-07-28/server/tools.md | `tools/list`, `tools/call`, tool names, error handling, `structuredContent` |
| 7 | Standard | MCP 2026-07-28 Subscriptions — https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions.md | `subscriptions/listen`, acknowledgment, `subscriptionId`, cancellation, graceful closure |
| 8 | Standard | MCP 2026-07-28 stdio transport — https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio.md | Framing, no server-to-client requests, shutdown, backward-compatibility probe |
| 9 | Standard | MCP 2025-11-25 and earlier lifecycle (`initialize` handshake) — https://modelcontextprotocol.io/specification/2025-11-25 | Legacy era support |
| 10 | Standard | JSON Schema 2020-12 — https://json-schema.org/ | Tool input schemas |
| 11 | Design | MoneyKeep plan ("one command registry"), Phase 0/1 `mk-commands` | |
| 12 | Owner decision | 2026-11-04: the MCP server becomes a separate, optional program (`keep-mcp`) that a future keep-launcher installs on request; every book has an AI access switch, off by default; access is read-only unless the owner chooses read and write; risky command groups need their own tick; password-protected books are used only through the running app after the owner allows the session; stdio only | |
| 13 | Specs | `book-encryption.md` (exclusive use, secrets never over MCP), `ledger-audit-undo.md` (actors, per-actor undo, session records), `input-safety.md` (R6 data notes), `connectivity.md` and `mobile-capture.md` (`NOT_OVER_MCP`), `app-desktop.md` (Settings → Security, History; revision 33) | |
| 14 | Standard | RFC 4648 (base16 for the control-channel token); Rust `std::net` loopback TCP | Control channel transport |

## Purpose

AI agents and scripts drive MoneyKeep through the same commands as the GUI and CLI —
but only when the owner of a book has said so, only as far as the owner has said, and
in a way the owner can see and review (revision 3).

## Behaviour

### General

- `moneykeep mcp <book> [--read-only]` (revision 3: now `keep-mcp <book>`, see below)
  runs an MCP server over the stdio transport for one book file. Messages are newline-delimited JSON-RPC 2.0, one per line, with no
  embedded newlines. Nothing but MCP messages is written to stdout; logs go to stderr.
- The server is **dual-era**: it serves modern clients (revision `2026-07-28`) and
  legacy clients (revisions `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`) from
  the same process, possibly interleaved.
- The server never sends JSON-RPC requests to the client.
- The server exits promptly when stdin reaches end-of-file. Before exiting it ends each
  open subscription gracefully (see Subscriptions).
- `--read-only` exposes only non-mutating commands.

### Tools (both eras)

- Tools are exactly the registry commands available in the book: disabled modules'
  commands are absent, and in read-only mode mutating commands are absent.
- Tool name = command name with `.` replaced by `_` (e.g. `txn_create`). This is kept
  for both eras so names are stable for clients.
- `tools/list` returns tools sorted by name (deterministic order). The server returns
  all tools in one page (no `nextCursor`).
- Each tool has `name`, `description` (the command summary), and `inputSchema`
  generated from the command's argument type. Schemas use JSON Schema 2020-12 (no
  `$schema` needed). A command with no arguments uses
  `{"type":"object","additionalProperties":false}`. Schemas contain no network `$ref`.
- `tools/call` with an unknown tool name, a tool hidden by a disabled module, or a
  mutating tool in read-only mode is a **protocol error**: JSON-RPC error `-32602` with
  a message naming the tool. (This replaces revision 1's `isError` behaviour.)
- A call whose arguments are not an object, or that is otherwise malformed, is a
  protocol error `-32602`.
- A command that runs and fails (validation, business rule, busy book) is a **tool
  execution error**: a result with `isError: true`, a text content item holding
  `{"error":{"code":"…","message":"…"}}`, and the same JSON as `structuredContent`.
  Codes are the CLI's codes.
- A successful call returns a text content item with the command's JSON result and the
  same JSON as `structuredContent`, with `isError: false`.
- Error codes: the server uses standard JSON-RPC codes and the MCP-defined codes only.
  It never emits codes in `-32000`…`-32019`, nor any code in `-32020`…`-32099` that MCP
  does not define, nor `-32002`.

### Modern era (revision 2026-07-28)

A request is modern when its `params._meta` contains
`io.modelcontextprotocol/protocolVersion`.

- **Required metadata.** Every modern request must carry
  `io.modelcontextprotocol/protocolVersion` and
  `io.modelcontextprotocol/clientCapabilities` in `_meta`. If either is missing, the
  server rejects the request with `-32602` (Invalid params).
- **Version check.** If the requested version is not supported, the server replies
  with error `-32022`, message "Unsupported protocol version", and
  `data: {"supported": [<all supported versions, modern first>], "requested": "<v>"}`.
- **Statelessness.** Each modern request is handled using only its own metadata. No
  state from earlier requests (including any legacy `initialize` on the same process)
  affects it.
- **Every result** includes `"resultType": "complete"` and
  `_meta: {"io.modelcontextprotocol/serverInfo": {"name": "moneykeep", "version": "<crate version>"}}`.
- **Audit actor** for a modern mutating call is `mcp:<clientInfo.name>` from that
  request's `io.modelcontextprotocol/clientInfo`, or `mcp:unknown` if absent.
- **`server/discover`** returns `supportedVersions` (all supported versions, modern
  first), `capabilities: {"tools": {"listChanged": true}}`, `serverInfo` in `_meta`,
  a short `instructions` string (one or two sentences: what the server does, that
  amounts are decimal strings, and that changes are audited and undoable),
  `ttlMs: 3600000` and `cacheScope: "private"`.
- **`tools/list`** results also carry `ttlMs: 60000` and `cacheScope: "private"`
  (the list depends on the book's modules, so it is not shareable).
- **Methods removed in this revision** (`initialize` aside, see Legacy era), such as
  `ping` and `logging/setLevel`, are answered with `-32601` (Method not found) on
  modern requests.
- **Logging.** The server does not emit `notifications/message`.

### Subscriptions (modern era)

- `subscriptions/listen` with `params.notifications` opens a subscription. The only
  supported notification type is `toolsListChanged`. Other requested types are
  omitted from the acknowledgment.
- The server first sends `notifications/subscriptions/acknowledged` with
  `params.notifications` = the honoured subset and
  `params._meta["io.modelcontextprotocol/subscriptionId"]` = the JSON-RPC id of the
  `subscriptions/listen` request. It sends nothing on that subscription before the
  acknowledgment.
- While a subscription with `toolsListChanged: true` is open, every change to the
  available tool set (a module enabled or disabled by any command, including undo or
  redo, through this server) sends `notifications/tools/list_changed` carrying that
  subscription's `subscriptionId` in `_meta`. A tool-set change is a change in the
  sorted list of tool names, not merely a module toggle.
- The `subscriptions/listen` request has no immediate response; it stays open. It ends
  when the client sends `notifications/cancelled` with that request id (the server
  then sends nothing more for it), or when the server shuts down, in which case the
  server first sends the graceful-closure result for that id:
  `{"resultType":"complete","_meta":{"io.modelcontextprotocol/subscriptionId": <id>}}`.
- Multiple concurrent subscriptions are allowed; each receives its own tagged copy.
- Modern clients never receive untagged or unsolicited notifications.

### Legacy era (revisions 2025-11-25 and earlier)

- An `initialize` request without modern `_meta` starts a legacy session scoped to the
  process. The server negotiates the version as those revisions define: it echoes the
  client's requested version if supported, otherwise replies with its latest legacy
  version (`2025-11-25`). It declares `capabilities: {"tools": {"listChanged": true}}`
  and `serverInfo`.
- Legacy clients send `notifications/initialized`, may use `ping`, `tools/list` and
  `tools/call`, and receive untagged `notifications/tools/list_changed` when the tool
  set changes (revision 1 behaviour).
- The legacy audit actor is `mcp:<clientInfo.name>` from `initialize`.
- Legacy results do not need `resultType`; including it is harmless and allowed.
- A request that has neither modern `_meta` nor a prior legacy `initialize` on this
  process is rejected with `-32602`, with a message stating that this server supports
  `2026-07-28` (per-request `_meta`) and legacy `initialize`.

### Single writer

The server holds the book open. SQLite locking makes a second concurrent writer fail
with the `busy` tool execution error, as in revision 1.

## Ledger effect

As the invoked command.

## Acceptance tests

1. **Modern discover.** `server/discover` with modern `_meta` returns
   `supportedVersions` starting with `2026-07-28` and including `2025-11-25`,
   `capabilities.tools.listChanged = true`, `resultType: "complete"`, `serverInfo` in
   `_meta`, `ttlMs` and `cacheScope: "private"`.
2. **Modern without handshake.** A first-ever `tools/list` carrying modern `_meta`
   (no `initialize`) succeeds, tools sorted by name, with `resultType`, `ttlMs`,
   `cacheScope`.
3. **Missing metadata.** A modern-looking request with `protocolVersion` but no
   `clientCapabilities` gets `-32602`. A request with no `_meta` and no prior
   `initialize` gets `-32602`.
4. **Unsupported version.** `protocolVersion: "1900-01-01"` gets `-32022` with
   `data.supported` (containing `2026-07-28`) and `data.requested = "1900-01-01"`.
5. **Unknown tool.** `tools/call` for `no_such_tool`, and for `txn_create` in
   `--read-only`, both get protocol error `-32602` (both eras).
6. **Tool execution error.** An unbalanced `txn_create` returns `isError: true`, a
   `structuredContent.error.code` of `invalid_args`, and the book is unchanged.
7. **Subscriptions.** (No shipped module has commands yet, so tests register a
   test-only command in a module, e.g. `estimates`, to make the tool set change.)
   `subscriptions/listen {toolsListChanged: true}` with id 7 gets
   an acknowledgment tagged `subscriptionId: 7` before anything else for it. A tool
   call that enables a module whose commands change the tool set triggers a
   `notifications/tools/list_changed` tagged `7`. A module toggle that does not change
   the tool set sends nothing. After `notifications/cancelled` for id 7, further
   changes send nothing. On stdin EOF with an open subscription, the server sends the
   graceful-closure result for that id and exits.
8. **No unsolicited modern notifications.** A modern client with no subscription that
   enables a module receives only the tool result.
9. **Legacy client.** `initialize` with `2025-06-18`, `notifications/initialized`,
   `ping`, `tools/list`, `tools/call` all work; the negotiated version is echoed;
   tool-set changes send untagged `notifications/tools/list_changed`.
10. **Modern `ping`.** `ping` with modern `_meta` gets `-32601`.
11. **Dual-era in one process.** A legacy `initialize` followed by a modern
    `tools/call` from a different `clientInfo` audits the modern call as
    `mcp:<modern client name>`.
12. **Audit actor.** Modern mutating calls are audited as `mcp:<clientInfo.name>`, or
    `mcp:unknown` when `clientInfo` is absent.
13. **Schemas.** Every tool's `inputSchema` validates as JSON Schema 2020-12, contains
    no network `$ref`, and no-argument tools use
    `{"type":"object","additionalProperties":false}`.
14. **Success shape.** A successful call has `isError: false`, one text content item
    whose text parses to the same JSON as `structuredContent`.
15. **Error-code hygiene.** A test drives every error path above and asserts no code in
    `-32000`…`-32019`, no `-32002`, and only `-32022` from the MCP-reserved range.

## Revision 3 — keep-mcp, per-book AI access, control channel (2026-11-04)

### Summary

1. The server is no longer part of the `moneykeep` CLI. It ships as its own program,
   **`keep-mcp`** (`apps/keep-mcp`), built from the same workspace so its tools always
   match the command registry. People who do not want it never install it.
2. Each book has **AI access** settings, stored in the book, **off by default**.
3. With access on, the owner picks a **level** (read only, or read and write) and may
   tick **permission groups** for risky commands.
4. `keep-mcp` reaches a book either **directly** (plain books, when the owner allows
   it) or **through the running MoneyKeep app** over a local **control channel**,
   after the owner allows the session in the app. Password-protected books are only
   ever reached through the app.
5. The app shows when an assistant is connected, and History can show only AI changes.

Everything in revision 2 about the protocol (both eras, tools, errors, subscriptions)
still holds, except where this revision says otherwise.

### Threat model (what the gate is and is not)

- The AI access settings are a **consent and safety control**: they stop honest
  software and an assistant's mistakes from reading or changing more than the owner
  allowed. They are not a defence against malicious programs already running as the
  same user, which can read a plain book file directly.
- Whatever an assistant reads is sent to the AI service that runs it. The app says so
  where the switch is (wording below).
- No secret (book password, recovery key, bank credential, capture token) is ever
  available to `keep-mcp` or sent over MCP or the control channel.

### Behaviour

#### AI access settings (per book)

Stored in the book's settings table under module `core`, keys starting `ai_access.`:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `ai_access.enabled` | bool | `false` | AI assistants may use this book at all |
| `ai_access.level` | `"read_only"` \| `"read_write"` | `"read_only"` | Which commands are available (below) |
| `ai_access.groups` | array of group names | `[]` | Permission groups the owner ticked (below) |
| `ai_access.app_only` | bool | `true` | Only through the running app, with approval each session. Always treated as `true` for a password-protected book |

- These keys are changed only by a **session operation**, `set_ai_access`
  (like the encryption operations: not a registry command, never a tool, never on the
  control channel). It is available in the app (Settings → Security) and the CLI
  (`moneykeep ai-access <book> [--on|--off] [--level read-only|read-write]
  [--groups delete,settings,files,connections] [--app-only|--direct]`; with no flags it
  prints the current settings as JSON).
- `set_ai_access` writes a session audit record `book.ai_access_changed` with the old
  and new settings. `history.undo` / `history.redo` skip it (it is a session record).
- `setting.set` and `module.set_setting` refuse any key starting `ai_access.` with
  `invalid_args` ("AI access is changed in Settings → Security"), for every actor.
  `setting.get` may read them. Definition files cannot set them.
- A book whose keys are missing or hold invalid values is treated as the defaults
  (access off).

#### Levels, groups and the exposed command set

The **exposed set** for a book is computed in `mk-commands` (one function, used by both
`keep-mcp` and the app's control channel, so the rules cannot drift):

1. Start from the registry commands available in the book (disabled modules removed).
2. Remove `NOT_OVER_MCP` (unchanged: credential, capture-token and receipt-file
   commands are never offered).
3. If access is off: the set is empty.
4. **Read only:** keep commands with `mutates = false`.
   **Read and write:** keep all commands.
5. Remove every command in a permission group the owner has not ticked.

Permission groups (a command may be in more than one; it is kept only if every group
it belongs to is ticked). Membership is by rule, so future commands are covered:

| Group | App label | Members (rule) |
|---|---|---|
| `delete` | Delete and void | mutating commands whose name ends in `.delete` or `.void`, plus `budget.remove_line` and `definition.uninstall` |
| `settings` | Change features and book settings | mutating commands whose name starts with `module.` or `book.`, plus `setting.set` and `processor.set` |
| `files` | Read files on this computer | any command whose arguments have a property named `path` (today: `import.stage`, `definition.install`, `definition.inspect`, `definition.validate`) |
| `connections` | Use bank feeds and connections | every command `connect::is_network` reports, every mutating `connect.*` or `capture.*` command, and `capture.revoke_device`, `capture.publish_lists` |

- Read only never offers a `connections` command, including the non-mutating ones
  that reach the network (`capture.status`, `capture.test`, `capture.setup_plan`,
  `capture.devices`): read only means no network side effects. This is intended
  (confirmed by the spec writer, 2026-11-04).
- `history.undo` / `history.redo` are ordinary write commands (no group). Undo is per
  actor, so an assistant can only undo its own changes.
- A unit test lists, for the current registry, exactly which commands fall in each
  group, so a reviewer sees any change.
- `tools/call` for a command outside the exposed set is the revision 2 protocol error
  `-32602`, with a message that says why and where to change it, e.g.
  "txn_delete is not available: this book allows read and write but not 'Delete and
  void' (MoneyKeep → Settings → Security → AI assistant access)", or "AI access is
  turned off for this book". The message never reveals secrets.
- The exposed set is recomputed on **every** request (the owner may change settings
  while a client is connected). If the sorted tool names differ from those last seen
  on this connection, list-changed notifications go out as in revision 2 (tagged per
  subscription for modern clients; untagged for a legacy session), right after the
  response to the request that noticed it.
- `instructions` (discover / initialize) state the level and ticked groups in one
  sentence, e.g. "Access: read and write; delete and void allowed." When access is off
  or the book cannot be reached, they say so and what the owner must do.

#### `keep-mcp` program

- `apps/keep-mcp`, binary `keep-mcp`, depends on `mk-mcp`, `mk-commands`, `mk-store`.
  The `mcp` subcommand is **removed** from the `moneykeep` CLI (and its tests move to
  `keep-mcp`). `mk-mcp` stays a library; the desktop app does not depend on it.
- Usage:
  - `keep-mcp <book>` — serve that book.
  - `keep-mcp --app` — serve whichever book the running app has open (through the
    app only).
  - `keep-mcp config [--client claude-desktop|claude-code|generic] <book|--app>` —
    print the JSON snippet to paste into that client's MCP configuration (command =
    this executable's absolute path, args), and exit. It writes nothing.
  - `--version`, `--help`.
  - There is no `--read-only` flag any more (the book decides) and **no password
    option**: `keep-mcp` ignores `MONEYKEEP_PASSWORD`, has no `--password-file`, and
    never prompts.
- Server name in `serverInfo` stays `moneykeep`; version is the workspace version.
- **Choosing the route**, done at start and again on every request until a route
  works (so the owner can open the app after the client started, without restarting
  it):
  1. If the app's control channel is advertised (discovery file below) and its book
     is the requested book (same canonical path) — or `--app` was given — use the
     **app route**.
  2. Else, if the book is plain, `ai_access.enabled` is true and `ai_access.app_only`
     is false — use the **direct route**: open the file itself (as revision 2 did),
     actor `mcp:<client name>`.
  3. Else **no route**: answer `server/discover` / `initialize` normally (with
     instructions explaining the situation), `tools/list` with an empty list, and
     `tools/call` with `-32602` and one of these messages:
     - password-protected: "This book is password protected. Open it in MoneyKeep,
       turn on AI assistant access, and allow this assistant when asked."
     - app only: "This book allows AI access only through MoneyKeep. Open it in
       MoneyKeep and allow this assistant when asked."
     - access off: "AI access is turned off for this book (MoneyKeep → Settings →
       Security)."
     - not a book / missing file: the store's error message.
- On the direct route the settings are read from the book on every request; if the
  owner turns direct use off or access off, the next request finds no route.
- On the app route, if the app closes the channel (book closed or locked, owner
  pressed Disconnect, access turned off, app exited), `keep-mcp` goes back to choosing
  a route on the next request; the tool set becomes empty and list-changed
  notifications go out.
- Errors from the control channel become tool execution errors (`isError: true`) for
  command failures and `-32602` for "not available" refusals, exactly as on the direct
  route.

#### Control channel (the app side)

The "JSON control channel" from the plan: commands as JSON over a local socket. In
this revision it serves only `keep-mcp` (client kind `keep-mcp`); other clients may be
added by later specs.

- **When it listens.** Only while the app has a book open **and** that book has
  `ai_access.enabled = true` (any `app_only` value). It stops listening, and closes
  every connection, when the book closes or locks, or access is turned off.
- **Transport.** TCP on `127.0.0.1`, port chosen by the OS. Never any other address.
- **Discovery file.** `<app config dir>/control.json`, written when listening starts
  and deleted when it stops (and on clean exit; a stale file whose `pid` is not
  running is ignored and replaced). Contents:
  `{"version":1,"port":<u16>,"token":"<64 hex chars>","pid":<u32>,"book_path":"<canonical path>"}`.
  The token is 32 random bytes from the OS, base16, new each time listening starts.
  On Unix the file is created with mode 0600; on Windows it lives in the per-user
  config folder.
- **Framing.** Newline-delimited JSON, one object per line, at most 4 MiB per line
  (longer → the app closes the connection). A first line that is not a JSON object
  (for example an HTTP request line) closes the connection with no reply.
- **Hello.** The client's first message:
  `{"hello":"moneykeep-control/1","token":"…","client":{"name":"<MCP client name>","kind":"keep-mcp"},"book_path":"<canonical path>"|null}`.
  The app compares the token in constant time. Replies, then (except `ok`) closes:
  - `{"ok":true,"book":{"name":"…","path":"…"},"level":"…","groups":[…]}`
  - `{"error":{"code":"bad_token"|"wrong_book"|"access_off"|"denied"|"approval_timeout"|"busy","message":"…"}}`
- **Approval.** On a valid hello the app shows a dialog (wording below) unless this
  (book, client name) pair was already allowed since the book was opened. The reply
  waits for the answer; no answer within 120 seconds → `approval_timeout`. Allowing
  writes a session audit record `ai.session_allowed` (client name, level, groups).
  Denying sends `denied`. Approval is forgotten when the book closes or locks, when
  the owner presses Disconnect, or when AI access settings change.
- **Requests** after `ok`:
  - `{"id":<n>,"op":"list"}` → `{"id":<n>,"commands":[<CommandSpec JSON>…],"level":"…","groups":[…]}` — the exposed set.
  - `{"id":<n>,"op":"call","command":"txn.create","args":{…}}` →
    `{"id":<n>,"result":<command JSON>}` or
    `{"id":<n>,"error":{"code":"<CLI error code>","message":"…"}}`; a command outside
    the exposed set → `{"id":<n>,"error":{"code":"not_available","message":"…"}}`.
  - Calls run one at a time on the app's worker, on the open session, with actor
    `mcp:<client name>`, through the registry like any GUI action, and the app's
    screens refresh afterwards as for any change.
- **Events** the app may send at any time after `ok`:
  `{"event":"tools_changed"}` (settings or modules changed) and
  `{"event":"closed","reason":"book_closed"|"locked"|"disconnected"|"access_off"|"app_exit"}`
  (then it closes the connection).
- At most 4 connections; a fifth hello gets `busy`.
- The channel never carries secrets and has no operation for session operations
  (encryption, `set_ai_access`, credentials).

#### App: Settings → Security → AI assistant access

A new group at the end of Settings → Security (per book):

- Switch **Allow AI assistants to use this book** (off by default). Under it, always
  visible, the note: "An AI assistant on this computer can use this book through the
  keep-mcp program. What the assistant reads is sent to the AI service you use, under
  that service's terms. Its changes are recorded in History and can be undone."
- When on:
  - **Access**: radio buttons **Read only** / **Read and write**.
  - **Also allow** (enabled only with Read and write, except "Read files on this
    computer", which also applies to read-only commands): four tick boxes with the
    group labels above.
  - **Only through MoneyKeep, and ask me each time** (ticked by default). For a
    password-protected book it is ticked and disabled, with the note "Password-protected
    books are only used through MoneyKeep."
  - When `keep-mcp` is not installed next to the app (no `keep-mcp` executable found
    in the app's folder or on `PATH`): the line "keep-mcp is not installed. Install it
    (for example from keep-launcher) to connect an assistant." (informational only).
  - Button **Copy assistant setup…**: shows the `keep-mcp config` snippet for this
    book (chosen client) with a Copy button, when `keep-mcp` is found.
- Every change calls `set_ai_access` immediately.

#### App: approval dialog, indicator, History

- **Approval dialog** (modal, shown even if the window is in the background; the app
  requests attention from the OS): title "Allow AI assistant?", text "<client name>
  wants to use <book name> through keep-mcp. Access: <Read only | Read and write>
  <; also: group labels>." Buttons **Allow** (default) and **Deny**.
- **Indicator**: while at least one assistant is connected over the control channel, a
  small icon button in the top bar, accessible name "AI assistant connected", tooltip
  "Connected: <client names>". Clicking it opens a pop-up listing each client with
  its access, a **Disconnect** button per client, and **Open AI history**.
- **History** gains a filter **Changes by AI assistants** (entries whose actor starts
  `mcp:`), also reached from **Open AI history**. Session records
  `book.ai_access_changed` and `ai.session_allowed` appear in History like the
  encryption records.

#### Packaging for keep-launcher

- `apps/keep-mcp/keep-app.toml` describes the component for a future keep-launcher:
  `id = "keep-mcp"`, `name = "keep-mcp"`, `description = "Lets AI assistants use
  MoneyKeep books you allow"`, `optional = true`, `default_install = false`,
  `requires = ["moneykeep"]`, `binary = "keep-mcp"`, and `install_note` with the
  privacy sentence from Settings. keep-launcher's own behaviour is out of scope here.
- `cargo xtask ci` builds `keep-mcp`. The default build of the desktop app and the
  `moneykeep` CLI must not link `mk-mcp` (the layering check enforces it).

#### Unchanged from revision 2

General stdio rules, dual-era handling, tool naming and schemas, success and error
shapes, error-code hygiene, subscriptions, legacy era, single writer (direct route).
Revision 2's `--read-only` behaviour is replaced by `ai_access.level`.

### Ledger effect

As the invoked command. `set_ai_access` and approvals write session records only.

### Edge cases

- The owner turns access off while a direct-route client is mid-session: the next
  request gets no route; a command already running finishes.
- Read only with `files` ticked: `definition.inspect` and `definition.validate` are
  exposed, `import.stage` is not (it mutates).
- A module enabled by the owner in the app adds tools; a connected client gets a
  list-changed notification on its next request (direct) or on `tools_changed` (app).
- Two MoneyKeep app instances: the second sees a live `control.json` and does not
  listen; Settings shows "Another MoneyKeep window is serving AI assistants."
- `keep-mcp <book>` where the app has a *different* book open: the app route is not
  used; the direct-route rules apply to the requested book.
- An empty or whitespace client name is `unknown` (actor `mcp:unknown`), as before.

### Acceptance tests

Revision 2's AT1–AT15 keep passing, run against `keep-mcp` (direct route) on a plain
test book with access on, level read and write, all groups, `app_only = false`, except
that AT5's read-only case is now set up with `ai_access.level = "read_only"`.

16. **Off by default.** A new book: `set_ai_access` defaults are off/read
    only/no groups/app only. `keep-mcp` on it: `tools/list` is empty and
    `tools/call account_list` gets `-32602` whose message says access is off.
17. **Read only.** Access on, read only, direct: only non-mutating tools are listed;
    `txn_create` gets `-32602` naming read only.
18. **Groups.** Read and write with no groups: `txn_create` works, `txn_delete`,
    `module_enable`, `import_stage` and `connect_sync` are absent. Ticking each group
    in turn adds exactly that group's commands. The group-membership unit test lists
    each group's commands for the current registry.
19. **Live settings change.** With a modern subscription open, changing the level
    from read only to read and write through `set_ai_access` (another `Store` handle on
    the same plain book) makes the next request's response followed by one tagged
    `notifications/tools/list_changed`.
20. **Settings cannot be changed by commands.** `setting.set` with key
    `ai_access.enabled` is refused with `invalid_args` for actor `cli` and over MCP;
    `set_ai_access` records `book.ai_access_changed`, which `history.undo` skips.
21. **Encrypted book.** `keep-mcp` on an encrypted test book with access on and no app
    running lists no tools and refuses calls with the password-protected message;
    `MONEYKEEP_PASSWORD` set in its environment changes nothing.
22. **App only.** A plain book with access on and `app_only = true`: no app → the
    app-only message; with a test control-channel server (the real app-side handler,
    driven without a window, approval answered Allow by the test) advertising this
    book → tools work and changes are audited `mcp:<client name>`.
23. **Control channel hello.** Wrong token → `bad_token` and close; other book →
    `wrong_book`; access off → `access_off`; Deny → `denied`; no answer in 120 s
    (injected clock) → `approval_timeout`; an HTTP request line → closed with no
    reply; a fifth connection → `busy`; a line over 4 MiB → closed.
24. **Control channel scope.** After `ok`, `list` equals the exposed set; a call
    outside it → `not_available`; locking the book sends `closed`/`locked`, removes
    `control.json`, and `keep-mcp` then lists no tools.
25. **Discovery file.** Listening writes `control.json` with a 64-hex token and the
    canonical book path (mode 0600 on Unix); stopping deletes it; a stale file with a
    dead `pid` is replaced.
26. **No secrets.** Neither `keep-mcp` nor the control channel offers any
    `NOT_OVER_MCP` command or session operation; a scan of every control-channel and
    MCP response in the test suite finds no password, recovery key or token value.
27. **CLI.** `moneykeep mcp` no longer exists; `moneykeep ai-access <book> --on
    --level read-write --groups delete --direct` then `moneykeep ai-access <book>`
    prints those settings. `keep-mcp config --client claude-desktop <book>` prints
    JSON whose command is an absolute path to `keep-mcp`.
28. **App UI** (Slint testing backend): Settings → Security shows the AI group with
    the switch off and the privacy note; turning it on shows Access, Also allow and
    the app-only box (ticked; disabled for an encrypted book); the approval dialog
    shows the client name and access, and Allow/Deny answer the test hello; while
    connected the top-bar indicator is present with accessible name "AI assistant
    connected"; Disconnect sends `closed`/`disconnected`; History's "Changes by AI
    assistants" filter shows only `mcp:` entries.
29. **Layering.** `moneykeep-app` and `moneykeep-cli` do not depend on `mk-mcp`.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
