//! Opening a PDF receipt in the system's PDF viewer (expenses.md: desktop only, after
//! a confirmation naming the file). MoneyKeep never renders or runs the PDF itself:
//! it writes a temporary, unencrypted copy and hands its path to the platform's
//! opener as a plain argument (no shell).
//!
//! Revision 2: the copies live in a per-session folder
//! (`<temp>/moneykeep-receipts/session-<pid>`) that only the user can read
//! (folders 0700, files 0600 on Unix), deleted when the book closes or the app
//! exits. Folders left by earlier sessions (a crash, a viewer that held a file
//! open on Windows) are removed on start. On Linux a folder whose process is still
//! running (a second MoneyKeep window) is left alone; elsewhere every other
//! session's folder counts as a leftover.

use std::io;
use std::path::{Path, PathBuf};

const ROOT: &str = "moneykeep-receipts";
const PREFIX: &str = "session-";

/// A safe file name: the last path component, without characters file systems
/// reject, ending in `.pdf`.
fn safe_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("receipt.pdf");
    let mut clean: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    if clean.trim().is_empty() || clean == "." || clean == ".." {
        clean = "receipt.pdf".into();
    }
    if !clean.to_lowercase().ends_with(".pdf") {
        clean.push_str(".pdf");
    }
    clean
}

/// This session's temporary PDF copies.
#[derive(Debug)]
pub(crate) struct TempCopies {
    root: PathBuf,
    dir: PathBuf,
}

impl TempCopies {
    /// The session for this process, under the system temporary folder. Removes
    /// earlier sessions' leftovers.
    pub(crate) fn start() -> Self {
        Self::in_root(std::env::temp_dir().join(ROOT), std::process::id())
    }

    fn in_root(root: PathBuf, pid: u32) -> Self {
        remove_leftovers(&root, pid);
        let dir = root.join(format!("{PREFIX}{pid}"));
        Self { root, dir }
    }

    /// Write a copy of the PDF into the session folder and return its path.
    fn write(&self, name: &str, data: &[u8]) -> Result<PathBuf, String> {
        if !data.starts_with(b"%PDF-") {
            return Err("it is not a PDF file".into());
        }
        private_dir(&self.root).map_err(|e| e.to_string())?;
        private_dir(&self.dir).map_err(|e| e.to_string())?;
        let path = self.dir.join(safe_name(name));
        write_private(&path, data).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Write the PDF and open it with the system viewer.
    pub(crate) fn open(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.write(name, data)?;
        let program = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(program)
            .arg(path.as_os_str())
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Delete this session's copies (book closed, app exit). Files a viewer still
    /// holds open on Windows stay until the next start.
    pub(crate) fn clear(&self) {
        if is_private_dir(&self.root) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Drop for TempCopies {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Create `dir` (owner-only), or check an existing one is a real, private folder of
/// this user. A symlink or a folder others can enter is refused: in a shared
/// temporary folder someone else could have created it first.
fn private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Owner-only on Unix; elsewhere the user's temporary folder is private.
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new().mode(0o700).create(dir)
            };
            #[cfg(not(unix))]
            let created = std::fs::DirBuilder::new().create(dir);
            match created {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    }
    if is_private_dir(dir) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private folder", dir.display()),
        ))
    }
}

/// A real folder (not a symlink); on Unix also owned by this user and closed to
/// group and others.
fn is_private_dir(dir: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !m.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.mode() & 0o077 != 0 || Some(m.uid()) != my_uid() {
            return false;
        }
    }
    true
}

/// This user's id without libc: the owner of our own process entry on Linux, or of
/// the home folder elsewhere.
#[cfg(unix)]
fn my_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    if let Ok(m) = std::fs::metadata("/proc/self") {
        return Some(m.uid());
    }
    std::env::var_os("HOME")
        .and_then(|h| std::fs::metadata(h).ok())
        .map(|m| m.uid())
}

/// Write a file only the user can read, replacing an earlier copy.
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    // An earlier copy (or anything else at that name) is replaced, never followed.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(data)
}

/// Whether the session with this process id is still running (Linux only; other
/// platforms report none, so every other folder is a leftover).
fn alive(pid: u32) -> bool {
    cfg!(target_os = "linux") && Path::new("/proc").join(pid.to_string()).exists()
}

/// Remove what earlier sessions left in `root`: session folders of processes no
/// longer running (and this pid's own, from an earlier process), and loose files
/// from versions that wrote straight into `root`. Nothing is touched unless `root`
/// is a private folder of this user.
fn remove_leftovers(root: &Path, pid: u32) {
    if !is_private_dir(root) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let session = name
            .to_str()
            .and_then(|n| n.strip_prefix(PREFIX))
            .and_then(|n| n.parse::<u32>().ok());
        if session.is_some_and(|p| p != pid && alive(p)) {
            continue;
        }
        let path = entry.path();
        let _ = match entry.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join(ROOT);
        (t, root)
    }

    #[test]
    fn names_are_reduced_to_a_pdf_file_name() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd.pdf");
        assert_eq!(safe_name("Invoice: March?.pdf"), "Invoice March.pdf");
        assert_eq!(safe_name(""), "receipt.pdf");
        assert_eq!(safe_name(".."), "receipt.pdf");
        assert_eq!(safe_name("scan.PDF"), "scan.PDF");
    }

    #[test]
    fn copies_go_in_a_private_session_folder_and_are_cleared() {
        let (_t, root) = root();
        let c = TempCopies::in_root(root.clone(), 4242);
        assert!(c.write("x.pdf", b"not a pdf").is_err());
        let p = c.write("Invoice.pdf", b"%PDF-1.4 one").unwrap();
        assert_eq!(p, root.join("session-4242").join("Invoice.pdf"));
        assert_eq!(std::fs::read(&p).unwrap(), b"%PDF-1.4 one");
        // A second open of the same name replaces the copy.
        c.write("Invoice.pdf", b"%PDF-1.4 two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"%PDF-1.4 two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(&root.join("session-4242")), 0o700);
            assert_eq!(mode(&p), 0o600);
        }
        c.clear();
        assert!(!p.exists());
        assert!(!root.join("session-4242").exists());
        // Writing again after a book closed works (the folder comes back).
        c.write("b.pdf", b"%PDF-1.4").unwrap();
        drop(c);
        assert!(!root.join("session-4242").exists(), "deleted on exit");
    }

    #[test]
    fn leftovers_of_earlier_sessions_are_removed_on_start() {
        let (_t, root) = root();
        private_dir(&root).unwrap();
        let stale = root.join("session-4000000000");
        private_dir(&stale).unwrap();
        std::fs::write(stale.join("old.pdf"), b"%PDF").unwrap();
        // A loose file from the earlier layout, and this pid's own old folder.
        std::fs::write(root.join("loose.pdf"), b"%PDF").unwrap();
        private_dir(&root.join("session-77")).unwrap();
        // A session still running (Linux knows pid 1 is alive) is kept there.
        private_dir(&root.join("session-1")).unwrap();
        let _c = TempCopies::in_root(root.clone(), 77);
        assert!(!stale.exists());
        assert!(!root.join("loose.pdf").exists());
        assert!(!root.join("session-77").exists());
        assert_eq!(
            root.join("session-1").exists(),
            cfg!(target_os = "linux"),
            "a running session is left alone on Linux"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_others_can_enter_or_a_symlink_is_not_used() {
        use std::os::unix::fs::PermissionsExt;
        let (t, root) = root();
        // Someone else's (here: open) folder at the root path: nothing is written
        // or deleted there.
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::write(root.join("keep.txt"), b"x").unwrap();
        let c = TempCopies::in_root(root.clone(), 5);
        assert!(root.join("keep.txt").exists());
        assert!(c.write("a.pdf", b"%PDF-1.4").is_err());
        drop(c);
        // A symlink pointing elsewhere is refused too, and its target untouched.
        let elsewhere = t.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(elsewhere.join("precious"), b"x").unwrap();
        let link = t.path().join("linked");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let c = TempCopies::in_root(link.clone(), 6);
        assert!(elsewhere.join("precious").exists());
        assert!(c.write("a.pdf", b"%PDF-1.4").is_err());
    }
}
