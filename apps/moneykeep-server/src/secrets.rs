//! Private keys of Wi-Fi capture (mobile-capture.md revision 10): the certificate
//! authority's key and the server certificate's key.
//!
//! They are kept like the inbox key: in the operating system's credential store when
//! there is one ([`OsCredentials`], the `keyring` crate), otherwise — and always with
//! `--key-file`, which the Windows service uses because it may run under another
//! account — in a protected file in the data folder. Unlike the inbox key file, that
//! file is never plain text: it is sealed with XChaCha20-Poly1305 under the inbox
//! key. A key is never sent to a phone, put in `server.toml` or written to a log.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use mk_core::brand;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::store::{self, KeyHome, StoreResult};

/// A credential store: named secrets (text).
pub trait CredentialStore: Send + Sync {
    fn get(&self, name: &str) -> StoreResult<Option<Zeroizing<String>>>;
    fn set(&self, name: &str, value: &str) -> StoreResult<()>;
    fn delete(&self, name: &str) -> StoreResult<()>;
}

/// The operating system's credential store (Windows Credential Manager, macOS
/// Keychain, Secret Service), under MoneyKeep's service name.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsCredentials;

impl CredentialStore for OsCredentials {
    fn get(&self, name: &str) -> StoreResult<Option<Zeroizing<String>>> {
        let e = keyring::Entry::new(brand::CREDENTIAL_SERVICE, name)
            .map_err(|_| "the credential store is not available".to_owned())?;
        match e.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("the credential store could not be read".into()),
        }
    }
    fn set(&self, name: &str, value: &str) -> StoreResult<()> {
        let e = keyring::Entry::new(brand::CREDENTIAL_SERVICE, name)
            .map_err(|_| "the credential store is not available".to_owned())?;
        e.set_password(value)
            .map_err(|_| "the credential store refused the key".to_owned())
    }
    fn delete(&self, name: &str) -> StoreResult<()> {
        let e = keyring::Entry::new(brand::CREDENTIAL_SERVICE, name)
            .map_err(|_| "the credential store is not available".to_owned())?;
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("the credential store could not remove the key".into()),
        }
    }
}

/// The operating system's credential store under another service name
/// (macos-release.md C4: [`KEYCHAIN_SERVICE`]).
#[derive(Debug, Clone, Copy)]
pub struct OsService(pub &'static str);

impl CredentialStore for OsService {
    fn get(&self, name: &str) -> StoreResult<Option<Zeroizing<String>>> {
        let e = keyring::Entry::new(self.0, name)
            .map_err(|_| "the keychain is not available".to_owned())?;
        match e.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("the keychain could not be read".into()),
        }
    }
    fn set(&self, name: &str, value: &str) -> StoreResult<()> {
        let e = keyring::Entry::new(self.0, name)
            .map_err(|_| "the keychain is not available".to_owned())?;
        e.set_password(value)
            .map_err(|_| "the keychain refused the key".to_owned())
    }
    fn delete(&self, name: &str) -> StoreResult<()> {
        let e = keyring::Entry::new(self.0, name)
            .map_err(|_| "the keychain is not available".to_owned())?;
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("the keychain could not remove the key".into()),
        }
    }
}

/// macos-release.md C4: the keychain item's service name.
pub const KEYCHAIN_SERVICE: &str = "MoneyKeep capture server";

/// What a sealed inbox key file starts with (C4), so it is never taken for a
/// Windows DPAPI blob or the plain key.
const KEYCHAIN_MAGIC: &[u8; 4] = b"MKK1";

/// macOS (C4): the inbox key file sealed with a key kept in the user's login
/// keychain ([`KEYCHAIN_SERVICE`], account: the data folder's path), in place of
/// Windows' DPAPI. `inbox.key.sealed` is the inbox key encrypted with
/// XChaCha20-Poly1305 under that keychain key, so a copy of the data folder taken
/// elsewhere, or by another user, cannot be opened. The keychain key is made the
/// first time a key is sealed; opening never makes one. The U1.2 rules
/// ([`crate::store::load_key_with`]) apply unchanged.
pub struct KeychainSealer {
    account: String,
    store: Arc<dyn CredentialStore>,
}

impl std::fmt::Debug for KeychainSealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeychainSealer")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl KeychainSealer {
    /// The sealer of the data folder `dir`, keeping its key in `store` (the
    /// keychain: [`OsService`] with [`KEYCHAIN_SERVICE`]; tests: a fake).
    pub fn new(dir: &Path, store: Arc<dyn CredentialStore>) -> Self {
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        Self {
            account: canon.display().to_string(),
            store,
        }
    }

    /// The keychain account: the data folder's path.
    pub fn account(&self) -> &str {
        &self.account
    }

    fn cipher(&self, make: bool) -> StoreResult<XChaCha20Poly1305> {
        let decode = |t: &str| -> StoreResult<Zeroizing<[u8; 32]>> {
            let raw = Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(t.trim())
                    .map_err(|_| "the keychain key is damaged".to_owned())?,
            );
            let arr: [u8; 32] = raw
                .as_slice()
                .try_into()
                .map_err(|_| "the keychain key is damaged".to_owned())?;
            Ok(Zeroizing::new(arr))
        };
        let key = match self.store.get(&self.account)? {
            Some(text) => decode(&text)?,
            None if make => {
                let key = Zeroizing::new(store::random::<32>()?);
                let text = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(*key));
                self.store.set(&self.account, &text)?;
                // Read back: a keychain that took the key keeps it.
                match self.store.get(&self.account)? {
                    Some(back) if back.as_str() == text.as_str() => key,
                    _ => return Err("the keychain did not keep the key".into()),
                }
            }
            None => return Err("the keychain has no key for this data folder".into()),
        };
        Ok(XChaCha20Poly1305::new(key.as_ref().into()))
    }
}

impl store::KeySealer for KeychainSealer {
    fn seal(&self, plain: &[u8]) -> StoreResult<Vec<u8>> {
        let nonce = store::random::<24>()?;
        let ct = self
            .cipher(true)?
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: store::SEALED_KEY_FILE.as_bytes(),
                },
            )
            .map_err(|_| "encryption failed".to_owned())?;
        let mut out = KEYCHAIN_MAGIC.to_vec();
        out.extend(nonce);
        out.extend(ct);
        Ok(out)
    }
    fn unseal(&self, sealed: &[u8]) -> StoreResult<Zeroizing<Vec<u8>>> {
        let rest = sealed
            .strip_prefix(KEYCHAIN_MAGIC.as_slice())
            .ok_or("inbox.key.sealed is damaged")?;
        let (nonce, ct) = rest
            .split_at_checked(24)
            .ok_or("inbox.key.sealed is damaged")?;
        self.cipher(false)?
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: store::SEALED_KEY_FILE.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| "inbox.key.sealed is damaged or the keychain key is wrong".into())
    }
}

/// A credential store in memory (tests: the "fake credential store" of test 68).
#[derive(Debug, Default)]
pub struct MemoryCredentials {
    map: Mutex<BTreeMap<String, String>>,
}

impl MemoryCredentials {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// The names held.
    pub fn names(&self) -> Vec<String> {
        self.map
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
    /// The values held (tests compare them with what else was written).
    pub fn values(&self) -> Vec<String> {
        self.map
            .lock()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }
}

impl CredentialStore for MemoryCredentials {
    fn get(&self, name: &str) -> StoreResult<Option<Zeroizing<String>>> {
        Ok(self
            .map
            .lock()
            .map_err(|_| "lock".to_owned())?
            .get(name)
            .map(|v| Zeroizing::new(v.clone())))
    }
    fn set(&self, name: &str, value: &str) -> StoreResult<()> {
        self.map
            .lock()
            .map_err(|_| "lock".to_owned())?
            .insert(name.to_owned(), value.to_owned());
        Ok(())
    }
    fn delete(&self, name: &str) -> StoreResult<()> {
        self.map.lock().map_err(|_| "lock".to_owned())?.remove(name);
        Ok(())
    }
}

/// The named private keys of one data folder.
pub struct Secrets {
    dir: PathBuf,
    store: Option<Arc<dyn CredentialStore>>,
    key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("dir", &self.dir)
            .field("credential_store", &self.store.is_some())
            .finish_non_exhaustive()
    }
}

/// The file a sealed secret lives in.
pub fn sealed_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.sealed"))
}

impl Secrets {
    /// The keys of `dir`: in `creds` when given (tests), else the operating system's
    /// store for [`KeyHome::Auto`], else sealed files.
    pub fn open(
        dir: &Path,
        home: KeyHome,
        creds: Option<Arc<dyn CredentialStore>>,
    ) -> StoreResult<Self> {
        let key = store::load_key(dir, home)?;
        let store = creds.or_else(|| match home {
            KeyHome::Auto => Some(Arc::new(OsCredentials) as Arc<dyn CredentialStore>),
            KeyHome::File => None,
        });
        Ok(Self {
            dir: dir.to_path_buf(),
            store,
            key,
        })
    }

    /// The credential-store name of `name` for this data folder.
    fn entry(&self, name: &str) -> String {
        let canon = self.dir.canonicalize().unwrap_or_else(|_| self.dir.clone());
        let h = Sha256::digest(canon.to_string_lossy().as_bytes());
        format!("capture-{name}:{}", store::hex(h.get(..8).unwrap_or(&[])))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(self.key.as_ref().into())
    }

    /// Where `name` is kept now, in words (Details; no value).
    pub fn location(&self, name: &str) -> &'static str {
        if sealed_file(&self.dir, name).exists() {
            "a sealed file in the data folder"
        } else {
            "the credential store"
        }
    }

    pub fn get(&self, name: &str) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        let file = sealed_file(&self.dir, name);
        if file.exists() {
            let blob =
                std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let (nonce, ct) = blob
                .split_at_checked(24)
                .ok_or_else(|| format!("{name} is damaged"))?;
            let plain = self
                .cipher()
                .decrypt(
                    XNonce::from_slice(nonce),
                    Payload {
                        msg: ct,
                        aad: name.as_bytes(),
                    },
                )
                .map_err(|_| format!("{name} is damaged or the inbox key is wrong"))?;
            return Ok(Some(Zeroizing::new(plain)));
        }
        let Some(s) = &self.store else {
            return Ok(None);
        };
        match s.get(&self.entry(name)) {
            Ok(Some(text)) => base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map(|v| Some(Zeroizing::new(v)))
                .map_err(|_| format!("{name} is damaged")),
            Ok(None) | Err(_) => Ok(None),
        }
    }

    /// Keep `bytes` as `name`: the credential store when it takes it (read back to
    /// make sure), else a sealed file.
    pub fn set(&self, name: &str, bytes: &[u8]) -> StoreResult<()> {
        let text = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(bytes));
        let entry = self.entry(name);
        let in_store = self.store.as_ref().is_some_and(|s| {
            s.set(&entry, &text).is_ok()
                && s.get(&entry)
                    .ok()
                    .flatten()
                    .is_some_and(|v| v.as_str() == text.as_str())
        });
        let file = sealed_file(&self.dir, name);
        if in_store {
            let _ = std::fs::remove_file(&file);
            return Ok(());
        }
        let nonce = store::random::<24>()?;
        let ct = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: bytes,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| "encryption failed".to_owned())?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        store::write_private(&file, &out)
    }

    pub fn delete(&self, name: &str) -> StoreResult<()> {
        let file = sealed_file(&self.dir, name);
        if file.exists() {
            std::fs::remove_file(&file)
                .map_err(|e| format!("cannot remove {}: {e}", file.display()))?;
        }
        if let Some(s) = &self.store {
            s.delete(&self.entry(name))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn sealed_files_and_the_credential_store() {
        let dir = tempfile::tempdir().unwrap();
        // --key-file: a sealed file, never the plain bytes.
        let s = Secrets::open(dir.path(), KeyHome::File, None).unwrap();
        s.set("wifi-ca-key", b"secret key bytes").unwrap();
        let on_disk = std::fs::read(sealed_file(dir.path(), "wifi-ca-key")).unwrap();
        assert!(!on_disk.windows(6).any(|w| w == b"secret"));
        assert_eq!(
            s.get("wifi-ca-key").unwrap().unwrap().as_slice(),
            b"secret key bytes"
        );
        s.delete("wifi-ca-key").unwrap();
        assert!(s.get("wifi-ca-key").unwrap().is_none());
        // A credential store: nothing in the data folder.
        let creds = MemoryCredentials::new();
        let s = Secrets::open(dir.path(), KeyHome::File, Some(creds.clone())).unwrap();
        s.set("wifi-tls-key", b"other").unwrap();
        assert!(!sealed_file(dir.path(), "wifi-tls-key").exists());
        assert_eq!(creds.names().len(), 1);
        assert!(creds.names()[0].starts_with("capture-wifi-tls-key:"));
        assert_eq!(s.get("wifi-tls-key").unwrap().unwrap().as_slice(), b"other");
        assert_eq!(s.location("wifi-tls-key"), "the credential store");
        s.delete("wifi-tls-key").unwrap();
        assert!(creds.names().is_empty());
    }
}
