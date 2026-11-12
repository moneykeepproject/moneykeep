//! The desktop UI acceptance tests (app-desktop.md and the screen specs), driving
//! the real `.slint` window on Slint's testing backend (ui-slint.md revision 2).
//! One test binary keeps the build small; each module holds one revision's tests
//! under their AT numbers.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod app_desktop;
mod app_desktop_r10;
mod app_desktop_r11;
mod app_desktop_r12;
mod app_desktop_r13;
mod app_desktop_r14;
mod app_desktop_r15;
mod app_desktop_r16;
mod app_desktop_r17;
mod app_desktop_r18;
mod app_desktop_r19;
mod app_desktop_r20;
mod app_desktop_r22;
mod app_desktop_r23;
mod app_desktop_r24;
mod app_desktop_r25;
mod app_desktop_r26;
mod app_desktop_r27;
mod app_desktop_r28;
mod app_desktop_r28_capture;
mod app_desktop_r29;
mod app_desktop_r30;
mod app_desktop_r31;
mod app_desktop_r32;
mod app_desktop_r33;
mod app_desktop_r34;
mod app_desktop_r35;
mod app_desktop_r36;
mod app_desktop_r37;
mod app_desktop_r38;
mod app_desktop_r39;
mod app_desktop_r4;
mod app_desktop_r40;
mod app_desktop_r41;
mod app_desktop_r42;
mod app_desktop_r6;
mod app_desktop_r7;
mod app_desktop_r8;
mod app_desktop_r9;
mod backups;
mod branding;
mod capture_r16;
mod capture_r17;
mod capture_sra;
mod capture_ur;
mod capture_wifi_r10;
mod capture_wifi_r12;
mod common;
mod dashboards_charts;
mod opening_balances;
mod options_bonds_crypto;
mod projects;
mod release_packaging;
mod security_review_b;
mod subscriptions;
mod table_columns;
mod ui_slint;
