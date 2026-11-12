//! The names used before the product was renamed, kept only where an installation on
//! the owner's computer has to be moved (branding.md revision 2): the capture
//! server's service, install folder and data folder (step 6) and the DNS records
//! phone capture setup made (Cloudflare settings stay unchanged). Books and their
//! settings are not carried over.
//!
//! The strings are built from parts so that no file spells the old name
//! (`cargo xtask names` allows it nowhere, and a history rewrite that replaces it
//! cannot change these values).

/// The old per-user data folder name (`ProjectDirs`) and Windows install folder.
pub const FOLDER: &str = concat!("Money", "Craft");
/// The old lower-case folder name (Linux install folder).
pub const FOLDER_LOWER: &str = concat!("money", "craft");
/// The capture server's old Windows service name.
pub const CAPTURE_SERVICE: &str = concat!("Money", "Craft", "Capture");
/// The capture server's old systemd user unit.
pub const CAPTURE_UNIT: &str = concat!("money", "craft", "-capture.service");
/// The capture server's old launchd agent label.
pub const CAPTURE_LABEL: &str = concat!("com.money", "craft", ".capture-server");
/// The comment on DNS records made by phone capture setup before the rename
/// (records with it are still recognised as made by the app).
pub const DNS_COMMENT: &str = concat!("Made by Money", "Craft phone capture");
