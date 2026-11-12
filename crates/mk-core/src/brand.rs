//! The product's names (branding.md "Name and tag line" and "Code and file names").

/// The product name, shown wherever a name is shown or written.
pub const PRODUCT: &str = "MoneyKeep";
/// The tag line under the name on the start screen and in the README.
pub const TAGLINE: &str = "Apps for Financial Management";
/// The project's page (release-prep.md §5): the workspace `repository`, shown in
/// About.
pub const PROJECT_PAGE: &str = env!("CARGO_PKG_REPOSITORY");
/// The book file extension (without the dot).
pub const BOOK_EXTENSION: &str = "moneykeep";
/// The definition file extension (without the dot).
pub const DEFINITION_EXTENSION: &str = "mkdef";
/// The format key that starts a definition file.
pub const DEFINITION_HEADER: &str = "moneykeep_definition";
/// The key that marks a bank CSV profile export.
pub const CSV_PROFILE_HEADER: &str = "moneykeep_csv_profile";
/// The per-user config and data folder name (`ProjectDirs`).
pub const FOLDER: &str = "MoneyKeep";
/// The OS credential store's service name.
pub const CREDENTIAL_SERVICE: &str = "MoneyKeep";

/// Whether `name` (a file name or path) is a definition file (`.mkdef`, any case).
pub fn is_definition_file(name: &str) -> bool {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && ext.eq_ignore_ascii_case(DEFINITION_EXTENSION)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_page() {
        assert_eq!(
            PROJECT_PAGE,
            "https://github.com/moneykeepproject/moneykeep"
        );
    }

    #[test]
    fn definition_files() {
        assert!(is_definition_file("x.mkdef"));
        assert!(is_definition_file("C:\\d\\X.MKDEF"));
        assert!(!is_definition_file("/a/.mkdef"));
        assert!(!is_definition_file("x.toml"));
    }
}
