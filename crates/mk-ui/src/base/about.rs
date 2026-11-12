//! About MoneyKeep: the third-party list and the build.

/// Third-party components with licences other than MoneyKeep's own, as listed in
/// NOTICE and ATTRIBUTION.md.
pub(crate) const THIRD_PARTY: &[(&str, &str)] = &[
    ("Slint", "Slint Royalty-free License 2.0"),
    (
        "Archivo, Inter and Geist Mono (fonts of the start screen)",
        "SIL Open Font License 1.1",
    ),
    (
        "Inter (the font Slint embeds in the phone app)",
        "SIL Open Font License 1.1",
    ),
    (
        "Book pictures (from photos on Pexels, Pixabay and Unsplash)",
        "Pexels, Pixabay and Unsplash licences (see ATTRIBUTION.md)",
    ),
    (
        "Other Rust libraries",
        "MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0, BSL-1.0 or Unicode-3.0 (each with its licence text under Third-party licences)",
    ),
];

/// The build: version and target.
pub(crate) fn build_text() -> String {
    format!(
        "Version {} ({} build)",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    )
}

/// Longest book description, in characters (the command refuses longer ones).
pub(crate) const MAX_DESCRIPTION: usize = 200;
/// Longest book tagline, in characters (revision 40).
pub(crate) const MAX_TAGLINE: usize = 60;
