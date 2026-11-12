//! MoneyKeep phone capture client (mobile-capture.md "WASM client").
//!
//! A web app for phones drawn with Slint (ui-slint.md revision 2: real `.slint`
//! screens from mk-views' `PhoneWindow`, pages built with mk-ui's page builder):
//! pair once with a code from the desktop, then
//! photograph receipts or enter expenses. Everything is saved on the phone first (the
//! outbox) and sent to the page's own origin (`/api/*`: the capture server serves
//! this app and its API on one address), so it works offline and never duplicates.
//!
//! The screens and the outbox build natively (and are tested natively); the browser
//! layer (`web`) builds for `wasm32-unknown-unknown`, or natively with the
//! `web-check` feature for type checking.

pub mod calendar;
pub mod keypad;
pub mod legacy;
pub mod outbox;
pub mod photo;
pub mod ui;

#[cfg(any(target_arch = "wasm32", feature = "web-check"))]
pub mod web;
