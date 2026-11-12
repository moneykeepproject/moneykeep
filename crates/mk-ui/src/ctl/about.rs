//! The About MoneyKeep dialog (ui-slint.md): name, tag line, version and build,
//! MoneyKeep's licence, the project's page (release-prep.md §5), the third-party
//! components with a link to `THIRD-PARTY-LICENSES.txt` (security-review-1 B4), and Slint's AboutSlint widget
//! (Slint Royalty-free License 2.0, section 2(a)), and Clean up (upgrade-resilience-1
//! U1.3: an old copy of the capture server's data). Reachable from the top bar's
//! "i" button and the start screen's "About" link.

use super::ui::Ui;
use super::{App, AppEvent};
use crate::base::about::{THIRD_PARTY, build_text};

pub(crate) fn open(app: &mut App) {
    app.about_open = true;
}

pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    if !app.about_open {
        return;
    }
    let mut close = ui.dialog_closed("about");
    let mut open_page = false;
    let mut open_licences = false;
    // upgrade-resilience-1 U1.3: the old copy of the capture server's data, once
    // the Status card's 30 days are over.
    let clean_up = super::capture::old_copy_to_clean(app);
    ui.dialog("about", "About MoneyKeep", 460, |ui| {
        ui.group(|ui| {
            ui.next().bold();
            ui.big_heading(mk_core::brand::PRODUCT);
            ui.secondary(mk_core::brand::TAGLINE);
            ui.label(build_text());
            ui.label("MoneyKeep is licensed under MIT OR Apache-2.0.");
            ui.label(format!("Project page: {}", mk_core::brand::PROJECT_PAGE));
            if ui.link("Open the project page").clicked() {
                open_page = true;
            }
        });
        ui.card("Third-party components", |ui| {
            for (name, licence) in THIRD_PARTY {
                ui.label(format!("{name}: {licence}"));
            }
            // Every crate with its licence text, installed beside the programs
            // (security-review-1 B4).
            if ui.link("Third-party licences").clicked() {
                open_licences = true;
            }
        });
        if let Some(path) = &clean_up {
            ui.card(super::capture::CLEAN_UP, |ui| {
                super::capture::old_copy_banner(app, ui, path);
            });
        }
        ui.group(|ui| ui.about_slint());
        ui.actions(|ui| {
            if ui.primary("Close").clicked() {
                close = true;
            }
        });
    });
    if open_licences {
        app.emit(AppEvent::OpenThirdPartyLicences);
    }
    if open_page {
        app.emit(AppEvent::OpenLink {
            url: mk_core::brand::PROJECT_PAGE.to_owned(),
        });
    }
    if close {
        app.about_open = false;
    }
}
