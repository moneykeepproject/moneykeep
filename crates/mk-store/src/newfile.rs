//! New files that never replace one (security-review-1 B1): a restored copy and an
//! unencrypted copy are written under a temporary name and moved into a name that
//! was reserved with `create_new`. The move refuses to replace a file: a hard link
//! (which fails when the name exists) and then the temporary name removed. Where the
//! file system has no hard links, the bytes are copied into a file created with
//! `create_new`, which refuses an existing name just the same. If the name was taken
//! in the meantime, the next free name is used (`<name> (2).moneykeep`, …).
//!
//! On Windows the spec names `MoveFileEx` without `MOVEFILE_REPLACE_EXISTING`; the
//! workspace forbids `unsafe` code (and so calls into the Windows API), so the
//! spec's other method, a hard link and then unlink, is used everywhere.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// The most numbered names tried before giving up.
const MAX_NUMBER: u32 = 999;

/// `<stem> (n).<ext>` beside `path` (`path` itself for `n` = 1).
pub fn numbered(path: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({n})"),
    };
    path.with_file_name(name)
}

/// What identifies the empty placeholder this process created.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    dev_ino: (u64, u64),
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

fn identity(meta: &std::fs::Metadata) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Identity {
            dev_ino: (meta.dev(), meta.ino()),
        }
    }
    #[cfg(not(unix))]
    {
        Identity {
            modified: meta.modified().ok(),
        }
    }
}

/// A name reserved by creating it, empty, with `create_new`.
#[derive(Debug)]
pub struct Reserved {
    /// The name asked for (numbered names are made from it).
    base: PathBuf,
    /// Its number (1 = the name itself).
    n: u32,
    path: PathBuf,
    id: Identity,
}

/// Reserve `path`: create it, empty, refusing an existing file (`AlreadyExists`).
pub fn reserve(path: &Path) -> io::Result<Reserved> {
    reserve_numbered(path, 1, false)
}

/// Reserve `path`, or the first free numbered name after it.
pub fn reserve_free(path: &Path) -> io::Result<Reserved> {
    reserve_numbered(path, 1, true)
}

fn reserve_numbered(base: &Path, from: u32, next_free: bool) -> io::Result<Reserved> {
    let mut n = from;
    loop {
        let path = numbered(base, n);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => {
                let id = identity(&f.metadata()?);
                return Ok(Reserved {
                    base: base.to_path_buf(),
                    n,
                    path,
                    id,
                });
            }
            Err(e) if next_free && e.kind() == io::ErrorKind::AlreadyExists && n < MAX_NUMBER => {
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

impl Reserved {
    /// The reserved name.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the name still holds the empty placeholder made here.
    fn still_ours(&self) -> bool {
        std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.file_type().is_file() && m.len() == 0 && identity(&m) == self.id)
    }

    /// Give the name back: the placeholder is removed if it is still the empty file
    /// made here (anything else there is left alone).
    pub fn release(self) {
        if self.still_ours() {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Move the finished file `tmp` into the reserved name, never replacing a file:
    /// when the name was taken in the meantime (the placeholder replaced or written
    /// to, or a file appearing between removing the placeholder and the move), the
    /// next free numbered name is reserved and used. Returns where the file is.
    pub fn fill_from(self, tmp: &Path) -> io::Result<PathBuf> {
        let mut r = self;
        loop {
            if r.still_ours() {
                std::fs::remove_file(&r.path)?;
                match move_no_replace(tmp, &r.path) {
                    Ok(()) => return Ok(r.path),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            if r.n >= MAX_NUMBER {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("no free name for {}", r.base.display()),
                ));
            }
            r = reserve_numbered(&r.base, r.n + 1, true)?;
        }
    }
}

/// Move `from` to `to`, refusing to replace an existing `to` (`AlreadyExists`): a
/// hard link then `from` removed; where hard links are not supported, a copy into a
/// file created with `create_new` (flushed), then `from` removed.
pub fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists || to.exists() => {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, e.to_string()));
        }
        Err(_) => {
            let mut src = File::open(from)?;
            let mut dst = OpenOptions::new().write(true).create_new(true).open(to)?;
            let copied = io::copy(&mut src, &mut dst).and_then(|_| dst.sync_all());
            if let Err(e) = copied {
                drop(dst);
                let _ = std::fs::remove_file(to);
                return Err(e);
            }
        }
    }
    std::fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn numbered_names() {
        let p = Path::new("/b/Household restored 2026-11-07 1420.moneykeep");
        assert_eq!(numbered(p, 1), p);
        assert_eq!(
            numbered(p, 2),
            Path::new("/b/Household restored 2026-11-07 1420 (2).moneykeep")
        );
        assert_eq!(numbered(Path::new("/b/x"), 3), Path::new("/b/x (3)"));
    }

    #[test]
    fn the_move_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"other").unwrap();
        let e = move_no_replace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"other");
        assert_eq!(std::fs::read(&a).unwrap(), b"new");
        let c = dir.path().join("c");
        move_no_replace(&a, &c).unwrap();
        assert_eq!(std::fs::read(&c).unwrap(), b"new");
        assert!(!a.exists());
    }

    #[test]
    fn a_name_taken_meanwhile_gets_the_next_number() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("x.moneykeep");
        let r = reserve(&target).unwrap();
        assert!(reserve(&target).is_err(), "reserved");
        // Someone writes into the name while the copy is made.
        std::fs::write(&target, b"theirs").unwrap();
        let tmp = dir.path().join(".x.part");
        std::fs::write(&tmp, b"ours").unwrap();
        let at = r.fill_from(&tmp).unwrap();
        assert_eq!(at, dir.path().join("x (2).moneykeep"));
        assert_eq!(std::fs::read(&at).unwrap(), b"ours");
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
        assert!(!tmp.exists());
        // Released placeholders go; other files stay.
        let r = reserve(&dir.path().join("y")).unwrap();
        r.release();
        assert!(!dir.path().join("y").exists());
        let r = reserve(&dir.path().join("z")).unwrap();
        std::fs::write(dir.path().join("z"), b"kept").unwrap();
        r.release();
        assert_eq!(std::fs::read(dir.path().join("z")).unwrap(), b"kept");
    }
}
