# Spec: Move the user interface to Slint (owner request 2026-11-02)

- **Area / module:** front ends — desktop app (`mk-ui`, `moneykeep-app`) and phone app
  (`moneykeep-capture`)
- **Spec writer:** Claude session 2026-11-02 (spec-writer role)
- **Date:** 2026-11-02
- **Status:** revision 2 (2026-11-02)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Owner request | 2026-11-02: change the UI toolkit to Slint for a modern look. Owner decisions: Slint **Royalty-free License 2.0**; both the desktop and the phone app move; the switch happens **all at once** (one release replaces the old UI); **Fluent** style | |
| 2 | Library docs | Slint documentation (language, Rust API, styles incl. `fluent`/`fluent-dark`, accessibility properties, `Path`, images incl. SVG, web/wasm support, testing backend `i-slint-backend-testing`) — https://docs.slint.dev | The testing backend is preliminary and does not follow semver: pin it with `=x.y.z` |
| 3 | Licence | Slint LICENSE.md (GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0) and LicenseRef-Slint-Royalty-free-2.0 §2 (attribution: show the `AboutSlint` widget in an About screen reachable from the top-level menu, or the attribution badge on the public download page), §3 (no embedded systems), no stand-alone redistribution of Slint, no applications that expose Slint's APIs | MoneyKeep's own code stays MIT OR Apache-2.0 |
| 4 | Specs | `app-desktop.md` (all revisions), `dashboards.md`, `expenses.md`, `mobile-capture.md`, `branding.md` and every spec that defines a screen | They remain the behaviour reference |

## Principle

This is a **toolkit change, not a behaviour change**. Every screen, field, button,
message, keyboard shortcut, accessible name and acceptance test in the existing specs
keeps its meaning. Where a spec names an egui mechanism (immediate-mode panels, the
egui test harness), read it as the equivalent Slint mechanism. Business logic stays in
the command registry; the UI still talks to books only through registry commands.

## Licence and attribution

- Slint is used under the **Royalty-free License 2.0** (owner decision). `deny.toml`
  gets an owner-approved exception allowing `LicenseRef-Slint-Royalty-free-2.0` for
  the `slint` crate family only (`slint`, `slint-build`, `i-slint-*`), with a comment
  naming the decision date; the general allowlist is unchanged.
- **About MoneyKeep** dialog (new), reachable from the top bar (a small "i" icon button
  after Close book, and the start screen's footer link "About"): app name, tag line
  "Apps for Financial Management", version and build, MoneyKeep's licence (MIT OR
  Apache-2.0), a "Third-party licences" list, and Slint's **`AboutSlint`** widget
  (licence §2(a)). The phone app shows the same in its menu's **About** screen.
- NOTICE and ATTRIBUTION record the Slint licence choice. MoneyKeep never exposes
  Slint's API to scripts or plugins.

## Desktop app

- `mk-ui` is rebuilt on Slint: `.slint` files for the views and components (compiled
  with `slint-build`), Rust code for state and command calls. The `Backend` trait,
  config handling and the app shell keep their roles; `moneykeep-app` creates the
  Slint window instead of an eframe window.
- **Style:** Slint's `fluent` style, following the operating system's light/dark
  setting, with MoneyKeep's accent colour (the existing theme's accent) and the
  existing number formatting. The visual rules of `app-desktop.md` (layout rules,
  focus ring, amounts right-aligned, no clipping at the supported text sizes) apply.
- **Text size and fonts** (revision 7): text size maps to the window scale factor
  (saved per user as before); interface and number fonts chosen from installed fonts
  map to Slint's default font family and a number-font property.
- **Icons:** the painted icons become SVG documents (MoneyKeep's own drawings, same
  shapes) shown with Slint `Image`; the app icon stays generated in code.
- **Charts:** drawn with Slint `Path` elements or rendered by Rust into images; same
  data and labels as now.
- **Window state** (size, position, panel widths) persists as before.
- **Accessibility:** every interactive element sets `accessible-role`,
  `accessible-label` (the same names the egui version used, so the specs' accessible
  names hold), and value/checked/selected where relevant.
- **Performance:** registers use virtualised lists (`ListView`) so 100,000 rows scroll
  smoothly (revision 2c requirement).

## Phone app

- `moneykeep-capture` is rebuilt on Slint's web (wasm) target with the same screens
  (Pair, Home tiles, form, Outbox, Sent, About), offline outbox, IndexedDB storage,
  file inputs for camera and photos (through web-sys, as now), PWA manifest, service
  worker and CSP (revisions 2–5). The CSP must still need no `unsafe-eval` beyond
  `wasm-unsafe-eval`.

## Tests

- UI acceptance tests are ported from the egui test harness to Slint's testing
  backend (pinned exact version): find elements by accessible label, invoke actions,
  check values. Every existing UI acceptance test keeps an equivalent (same AT number)
  unless the test only checked an egui implementation detail — those are listed in the
  implementer's report with the reason.
- Layout checks that measured rectangles (text sizes, no clipping) use element
  geometry from the testing backend; screenshot-based checks, where needed, use
  Slint's software renderer.
- `cargo xtask ci` stays the gate; the wasm build step remains (skipped with a clear
  message where the target can't be installed).

## Delivery

All at once (owner decision): the work happens on a branch; the desktop and phone
apps switch together in one update, and the egui code and dependencies are removed in
that same update. `egui`, `eframe`, `egui_kittest` and their licence exceptions (the
egui default fonts) are removed from `deny.toml` when no longer used.

## Acceptance tests (new)

1. `cargo deny` passes with the Slint exception limited to the Slint crates.
2. The About dialog shows name, tag line, version, licence and the AboutSlint widget,
   and is reachable from the top bar and the start screen.
3. The app follows light/dark from the operating system and uses the Fluent style.
4. No egui crate remains in `Cargo.lock`.
5. The phone app builds for wasm32 (where the target is available) and its logic
   tests pass natively.
6. Every previously numbered UI acceptance test has a Slint counterpart or a listed
   reason.

## Revision 2 (owner decision 2026-11-02): real Slint screens

The first pass (branch `slint`) kept the egui-era view code and drew it through an
immediate-mode layer (`mk-gui`, `mk-gui-slint`) in one Slint window. It works and its
tests pass, but the look barely changed. The owner chose to **rebuild the screens as real
Slint components** for a fully modern look. This revision replaces the "Desktop app"
and "Phone app" sections above where they differ.

### Structure

- **Views are declared in `.slint` files**, using Slint's layouts (`VerticalLayout`,
  `HorizontalLayout`, `GridLayout`) and the Fluent std-widgets (`Button`, `LineEdit`,
  `TextEdit`, `ComboBox`, `CheckBox`, `Switch`, `SpinBox`, `Slider`, `TabWidget`,
  `ScrollView`, `ListView`, `StandardTableView`, `PopupWindow`, `ProgressIndicator`,
  `DatePickerPopup` where it fits) plus MoneyKeep's own components built from them.
  Rust supplies data through properties, models (`ModelRc`/`VecModel` or custom
  `Model` implementations) and global singletons, and receives user actions through
  callbacks. No screen is drawn by an immediate-mode layer.
- **Crates:**
  - `mk-ui` becomes the **toolkit-free controller**: application state, view models
    (plain Rust structs and lists), formatting, validation messages and registry
    command calls. It does not depend on Slint and its logic is unit-tested natively.
  - **`mk-views` (new)** holds the `.slint` files, the SVG icons and the glue that binds
    controller view models to Slint models and callbacks. It may depend on `slint`
    (and `slint-build` at build time), `mk-ui` and `mk-core`. It holds no business
    logic: anything that decides something lives in `mk-ui` or the registry.
  - `moneykeep-app` creates the main window from `mk-views`.
  - `mk-gui` and `mk-gui-slint` are **removed** when the last screen has moved. They
    may serve as a temporary bridge while screens move, but not in the delivered
    result.
  - The layering check is updated to this structure; `slint` remains allowed only in
    `mk-views` and `moneykeep-app` (and the phone app).
- **Phone app:** its screens are `.slint` components too (same structure: logic in
  plain Rust, views in `.slint`).

### Look and feel

The goal is a clean, modern Fluent look, not a copy of the old drawing:

- An 8 px spacing grid (4 px for tight places), generous padding, aligned edges.
- A type scale: title, subtitle, body, caption, with the number font for amounts.
- **Cards**: book tiles, dashboard blocks, phone tiles, settings groups and report
  panels are cards with rounded corners (8 px), a subtle border or shadow, and hover
  and pressed states. Tile labels are centred under or inside their tile.
- **Icons**: MoneyKeep's own SVG drawings in one consistent line style, tinted to the
  theme; the same meaning and accessible names as the painted icons.
- **Motion**: Slint `animate` for the accounts slide-out pane, pop-ups and menus
  (short, 120-200 ms); nothing animates when the OS asks for reduced motion if Slint
  exposes it.
- Light and dark follow the OS (revision 1). The accent colour, focus ring, right-
  aligned amounts and no-clipping rules still apply.
- **Registers** use `StandardTableView` or a `ListView` of row components:
  virtualised, so 100,000 rows scroll smoothly; sorting and column widths as specified.
- Behaviour, field names, messages, shortcuts and accessible names stay as the specs
  define them (Principle above). Where Slint's standard widget behaves slightly
  differently from the egui version (for example the caret or selection details),
  the Slint behaviour is acceptable unless a spec states otherwise.

### Phone app

- Phone About screen with the `AboutSlint` widget (still missing after the first pass).
- The wasm build cannot be compiled in the implementer's environment (the target
  cannot be downloaded there). Keep the browser-only code small and behind a clear
  module boundary, check it with `cargo check` on native where possible, and follow
  Slint's documented wasm setup exactly. The owner's PC performs the first real wasm
  build (`cargo xtask dist-capture`); the delivery notes say so.

### Tests

- Controller logic in `mk-ui` is tested natively without Slint.
- UI acceptance tests drive the real `.slint` UI through Slint's testing backend
  (pinned exact version): find elements by accessible label, click, type, check
  values. Every numbered AT keeps a counterpart, as in the first pass.
- **Screenshots for review:** the implementer renders the main screens (start, book
  tiles, register, transaction pop-up, accounts pane, dashboard, settings, a report,
  calculator, About, phone home and phone form) in light and dark with Slint's
  software renderer or a virtual display, and reports where they are. They are
  review material, not committed.

### Acceptance tests (revision 2)

7. No crate named `mk-gui` or `mk-gui-slint` remains; `mk-ui` has no Slint
   dependency; `slint` appears only where the layering check allows it.
8. Every screen listed in `app-desktop.md`, `dashboards.md`, `expenses.md` and
   `mobile-capture.md` exists as a `.slint` component.
9. Book tile labels are centred; cards, spacing and type scale follow this revision
   (checked by element geometry in tests where practical, and by the screenshots).
10. The phone app has an About screen with the `AboutSlint` widget.
