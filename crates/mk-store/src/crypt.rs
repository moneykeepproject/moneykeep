//! The encrypted book file format (spec: book-encryption.md): Argon2id-derived
//! password key and a recovery key wrap one random data key in key slots; the data
//! key encrypts the SQLite image with XChaCha20-Poly1305. Pure functions over bytes;
//! [`crate::Store`] does the file IO.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

use crate::{StoreError, StoreResult};

/// First bytes of every encrypted book; never a valid SQLite header.
pub const MAGIC: &[u8; 8] = b"MCBOOKE1";
/// First bytes of a plain SQLite database file.
pub const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const VERSION: u16 = 1;
const ALG_ARGON2ID_13: u8 = 1;
const KDF_LEN: usize = 1 + 4 + 4 + 4 + 16;
const NONCE: usize = 24;
const WRAPPED: usize = 32 + 16;
const SLOT_LEN: usize = 1 + NONCE + WRAPPED;

/// A 256-bit key, zeroed when dropped.
pub type Key = Zeroizing<[u8; 32]>;

/// Argon2id parameters, stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// Defaults for new files: 64 MiB, 3 iterations, parallelism 1.
    pub const DEFAULT: Self = Self {
        memory_kib: 65_536,
        iterations: 3,
        parallelism: 1,
    };

    /// Cheap parameters for tests only.
    pub const TEST: Self = Self {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
}

impl KdfParams {
    /// The most memory a book may ask for: 1 GiB (security-review-1 L4).
    pub const MAX_MEMORY_KIB: u32 = 1024 * 1024;
    /// The most iterations and lanes: four times what MoneyKeep writes
    /// ([`KdfParams::DEFAULT`]).
    pub const MAX_ITERATIONS: u32 = 4 * Self::DEFAULT.iterations;
    pub const MAX_PARALLELISM: u32 = 4 * Self::DEFAULT.parallelism;

    /// Whether these are settings MoneyKeep could have made: at least
    /// [`KdfParams::TEST`]'s, at most 1 GiB of memory and four times the default
    /// iterations and lanes (security-review-1 L4).
    pub fn is_bounded(&self) -> bool {
        (Self::TEST.memory_kib..=Self::MAX_MEMORY_KIB).contains(&self.memory_kib)
            && (Self::TEST.iterations..=Self::MAX_ITERATIONS).contains(&self.iterations)
            && (Self::TEST.parallelism..=Self::MAX_PARALLELISM).contains(&self.parallelism)
            && self.memory_kib >= 8 * self.parallelism
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotKind {
    Password = 1,
    Recovery = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot {
    pub kind: SlotKind,
    pub nonce: [u8; NONCE],
    pub wrapped: Vec<u8>,
}

/// Everything before the payload nonce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub kdf: KdfParams,
    pub salt: [u8; 16],
    pub slots: Vec<Slot>,
}

/// What the user gives to open a book.
#[derive(Clone, PartialEq, Eq)]
pub enum Secret {
    Password(String),
    RecoveryKey(String),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Secret::Password(_) => "Secret::Password(…)",
            Secret::RecoveryKey(_) => "Secret::RecoveryKey(…)",
        })
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        match self {
            Secret::Password(s) | Secret::RecoveryKey(s) => s.zeroize(),
        }
    }
}

pub(crate) fn random<const N: usize>() -> StoreResult<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b)
        .map_err(|e| StoreError::Corrupt(format!("no OS random source: {e}")))?;
    Ok(b)
}

pub(crate) fn random_key() -> StoreResult<Key> {
    Ok(Zeroizing::new(random::<32>()?))
}

/// Outer whitespace trimmed, then Unicode NFC.
pub(crate) fn normalise_password(p: &str) -> Zeroizing<String> {
    Zeroizing::new(p.trim().nfc().collect())
}

/// At least 8 and at most 1,024 characters after trimming outer whitespace.
pub fn check_password_rules(p: &str) -> StoreResult<()> {
    let n = p.trim().chars().count();
    if n < 8 {
        return Err(StoreError::InvalidPassword(
            "a password needs at least 8 characters; a passphrase of several words is best".into(),
        ));
    }
    if n > 1024 {
        return Err(StoreError::InvalidPassword(
            "a password can have at most 1,024 characters".into(),
        ));
    }
    Ok(())
}

pub(crate) fn derive(password: &str, kdf: KdfParams, salt: &[u8; 16]) -> StoreResult<Key> {
    if !kdf.is_bounded() {
        return Err(StoreError::UnusualKdf);
    }
    let pw = normalise_password(password);
    let params =
        Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32)).map_err(|e| {
            StoreError::BookDamaged(format!("unusable key-derivation parameters ({e})"))
        })?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pw.as_bytes(), salt, out.as_mut())
        .map_err(|e| StoreError::Corrupt(format!("key derivation failed: {e}")))?;
    Ok(out)
}

fn kdf_block(h: &Header) -> Vec<u8> {
    let mut v = Vec::with_capacity(KDF_LEN);
    v.push(ALG_ARGON2ID_13);
    v.extend_from_slice(&h.kdf.memory_kib.to_le_bytes());
    v.extend_from_slice(&h.kdf.iterations.to_le_bytes());
    v.extend_from_slice(&h.kdf.parallelism.to_le_bytes());
    v.extend_from_slice(&h.salt);
    v
}

fn prefix() -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.extend_from_slice(&VERSION.to_le_bytes());
    v
}

/// Associated data of a slot: the password slot covers the KDF block; the recovery
/// slot does not (so a password change leaves it valid).
fn slot_aad(h: &Header, kind: SlotKind) -> Vec<u8> {
    let mut v = prefix();
    if kind == SlotKind::Password {
        v.extend_from_slice(&kdf_block(h));
    }
    v.push(kind as u8);
    v
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

/// Wrap the data key into a slot of `kind` with `wrapping` key.
pub(crate) fn wrap(
    h: &Header,
    kind: SlotKind,
    wrapping: &[u8; 32],
    data_key: &[u8; 32],
) -> StoreResult<Slot> {
    let nonce = random::<NONCE>()?;
    let aad = slot_aad(h, kind);
    let wrapped = cipher(wrapping)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: data_key,
                aad: &aad,
            },
        )
        .map_err(|_| StoreError::Corrupt("key wrapping failed".into()))?;
    Ok(Slot {
        kind,
        nonce,
        wrapped,
    })
}

fn unwrap_slot(h: &Header, slot: &Slot, wrapping: &[u8; 32]) -> Option<Key> {
    let aad = slot_aad(h, slot.kind);
    let plain = Zeroizing::new(
        cipher(wrapping)
            .decrypt(
                XNonce::from_slice(&slot.nonce),
                Payload {
                    msg: &slot.wrapped,
                    aad: &aad,
                },
            )
            .ok()?,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    if plain.len() != 32 {
        return None;
    }
    key.copy_from_slice(&plain);
    Some(key)
}

pub(crate) fn header_bytes(h: &Header) -> Vec<u8> {
    let mut v = prefix();
    v.extend_from_slice(&kdf_block(h));
    v.push(u8::try_from(h.slots.len()).unwrap_or(0));
    for s in &h.slots {
        v.push(s.kind as u8);
        v.extend_from_slice(&s.nonce);
        v.extend_from_slice(&s.wrapped);
    }
    v
}

/// The whole encrypted file for `image` (fresh payload nonce).
pub(crate) fn encode(h: &Header, data_key: &[u8; 32], image: &[u8]) -> StoreResult<Vec<u8>> {
    let mut out = header_bytes(h);
    out.extend_from_slice(&random::<NONCE>()?);
    let nonce_at = out.len() - NONCE;
    let ct = cipher(data_key)
        .encrypt(
            XNonce::from_slice(out.get(nonce_at..).unwrap_or(&[0u8; NONCE])),
            Payload {
                msg: image,
                aad: &out,
            },
        )
        .map_err(|_| StoreError::Corrupt("encryption failed".into()))?;
    out.extend_from_slice(&ct);
    Ok(out)
}

fn damaged() -> StoreError {
    StoreError::BookDamaged(String::new())
}

fn take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> StoreResult<&'a [u8]> {
    let s = b.get(*at..*at + n).ok_or_else(damaged)?;
    *at += n;
    Ok(s)
}

fn u32_at(b: &[u8], at: &mut usize) -> StoreResult<u32> {
    let s = take(b, at, 4)?;
    let mut a = [0u8; 4];
    a.copy_from_slice(s);
    Ok(u32::from_le_bytes(a))
}

/// Parse the header; returns it and the length of everything before the payload
/// ciphertext (header plus payload nonce).
pub(crate) fn parse(b: &[u8]) -> StoreResult<(Header, usize)> {
    let mut at = 0;
    if take(b, &mut at, 8)? != MAGIC {
        return Err(StoreError::NotABook);
    }
    let v = take(b, &mut at, 2)?;
    if v != VERSION.to_le_bytes() {
        return Err(damaged());
    }
    if take(b, &mut at, 1)? != [ALG_ARGON2ID_13] {
        return Err(damaged());
    }
    let kdf = KdfParams {
        memory_kib: u32_at(b, &mut at)?,
        iterations: u32_at(b, &mut at)?,
        parallelism: u32_at(b, &mut at)?,
    };
    // Bounded before anything is derived: a file asking for terabytes of memory is
    // refused, not attempted (security-review-1 L4).
    if !kdf.is_bounded() {
        return Err(StoreError::UnusualKdf);
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(take(b, &mut at, 16)?);
    let count = *take(b, &mut at, 1)?.first().ok_or_else(damaged)?;
    if !(1..=2).contains(&count) {
        return Err(damaged());
    }
    let mut slots = Vec::new();
    for _ in 0..count {
        let s = take(b, &mut at, SLOT_LEN)?;
        let kind = match s.first() {
            Some(1) => SlotKind::Password,
            Some(2) => SlotKind::Recovery,
            _ => return Err(damaged()),
        };
        let mut nonce = [0u8; NONCE];
        nonce.copy_from_slice(s.get(1..1 + NONCE).ok_or_else(damaged)?);
        slots.push(Slot {
            kind,
            nonce,
            wrapped: s.get(1 + NONCE..).ok_or_else(damaged)?.to_vec(),
        });
    }
    take(b, &mut at, NONCE)?;
    if b.len() < at + 16 {
        return Err(damaged());
    }
    Ok((Header { kdf, salt, slots }, at))
}

/// Find the data key the secret unlocks.
pub(crate) fn unlock(h: &Header, secret: &Secret) -> StoreResult<Key> {
    let (kind, wrapping) = match secret {
        Secret::Password(p) => (SlotKind::Password, derive(p, h.kdf, &h.salt)?),
        Secret::RecoveryKey(r) => (
            SlotKind::Recovery,
            Zeroizing::new(parse_recovery_key(r).ok_or(StoreError::WrongPassword)?),
        ),
    };
    h.slots
        .iter()
        .filter(|s| s.kind == kind)
        .find_map(|s| unwrap_slot(h, s, &wrapping))
        .ok_or(StoreError::WrongPassword)
}

/// Decrypt the payload of a parsed file.
pub(crate) fn decrypt(
    b: &[u8],
    header_len: usize,
    data_key: &[u8; 32],
) -> StoreResult<Zeroizing<Vec<u8>>> {
    let aad = b.get(..header_len).ok_or_else(damaged)?;
    let nonce = b.get(header_len - NONCE..header_len).ok_or_else(damaged)?;
    let ct = b.get(header_len..).ok_or_else(damaged)?;
    cipher(data_key)
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| damaged())
}

/// The payload nonce of a file (tests observe that it changes on every save).
pub fn payload_nonce(b: &[u8]) -> Option<Vec<u8>> {
    let (_, len) = parse(b).ok()?;
    b.get(len - NONCE..len).map(<[u8]>::to_vec)
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn crock(v: u32) -> u8 {
    CROCKFORD
        .get(usize::try_from(v).unwrap_or(0))
        .copied()
        .unwrap_or(b'0')
}

/// `MCRK-` and the 32 bytes in Crockford base32 (52 characters) in groups of four.
pub fn format_recovery_key(key: &[u8; 32]) -> String {
    let mut chars = Vec::with_capacity(52);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in key {
        acc = (acc << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(crock((acc >> bits) & 31));
        }
    }
    if bits > 0 {
        chars.push(crock((acc << (5 - bits)) & 31));
    }
    let groups: Vec<String> = chars
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    format!("MCRK-{}", groups.join("-"))
}

/// Parse a recovery key in any case, ignoring spaces and dashes, `O`→0, `I`/`L`→1.
pub fn parse_recovery_key(s: &str) -> Option<[u8; 32]> {
    let mut clean: Vec<u8> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .map(|c| match c {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .map(|c| c as u8)
        .collect();
    if clean.len() == 56 && clean.starts_with(b"MCRK") {
        clean.drain(..4);
    }
    if clean.len() != 52 {
        return None;
    }
    let mut out = Vec::with_capacity(33);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in clean {
        let v = CROCKFORD.iter().position(|x| *x == c)?;
        acc = (acc << 5) | u32::try_from(v).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(out.get(..32)?);
    out.zeroize();
    Some(key)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn production_kdf_defaults() {
        assert_eq!(
            KdfParams::DEFAULT,
            KdfParams {
                memory_kib: 65_536,
                iterations: 3,
                parallelism: 1
            }
        );
        assert_eq!(KdfParams::default(), KdfParams::DEFAULT);
    }

    #[test]
    fn recovery_key_round_trip_and_lenient_input() {
        let key = random::<32>().unwrap();
        let text = format_recovery_key(&key);
        assert!(text.starts_with("MCRK-"));
        let body = text.trim_start_matches("MCRK-");
        assert_eq!(body.replace('-', "").len(), 52);
        assert!(body.split('-').all(|g| g.len() == 4));
        assert_eq!(parse_recovery_key(&text), Some(key));
        let typed = text.to_lowercase().replace('-', " ");
        assert_eq!(parse_recovery_key(&typed), Some(key));
        let confusable = text.replace('0', "O").replace('1', "l");
        assert_eq!(parse_recovery_key(&confusable), Some(key));
        assert_eq!(parse_recovery_key("MCRK-ABCD"), None);
    }

    #[test]
    fn password_rules_and_normalisation() {
        assert!(check_password_rules("  short  ").is_err());
        assert!(check_password_rules("correct horse").is_ok());
        assert!(check_password_rules(&"x".repeat(1025)).is_err());
        // NFC: "é" composed and decomposed derive the same key; outer spaces trimmed.
        let salt = [7u8; 16];
        let a = derive("caf\u{e9} au lait", KdfParams::TEST, &salt).unwrap();
        let b = derive("  cafe\u{301} au lait ", KdfParams::TEST, &salt).unwrap();
        assert_eq!(*a, *b);
    }

    /// security-review-1 SR-B5 (L4): settings outside what MoneyKeep writes are
    /// refused when the header is read, before any key is derived.
    #[test]
    fn sr_b5_l4_unusual_kdf_settings_are_refused_before_deriving() {
        let data_key = random_key().unwrap();
        let mut h = Header {
            kdf: KdfParams::TEST,
            salt: random::<16>().unwrap(),
            slots: Vec::new(),
        };
        let pk = derive("correct horse battery", h.kdf, &h.salt).unwrap();
        h.slots
            .push(wrap(&h, SlotKind::Password, &pk, &data_key).unwrap());
        let file = encode(&h, &data_key, b"image bytes").unwrap();
        // memory_kib, iterations, parallelism follow the magic, version and algorithm.
        let with = |at: usize, v: u32| {
            let mut f = file.clone();
            f[at..at + 4].copy_from_slice(&v.to_le_bytes());
            f
        };
        let (mem, iters, lanes) = (11, 15, 19);
        // 4 TiB (the most a u32 of KiB can say).
        let huge = with(mem, u32::MAX);
        let e = parse(&huge).unwrap_err();
        assert!(matches!(e, StoreError::UnusualKdf), "{e:?}");
        assert_eq!(
            e.to_string(),
            "This book's password settings aren't ones MoneyKeep makes."
        );
        let over = KdfParams::MAX_MEMORY_KIB + 1;
        assert!(matches!(
            parse(&with(mem, over)),
            Err(StoreError::UnusualKdf)
        ));
        assert!(parse(&with(mem, KdfParams::MAX_MEMORY_KIB)).is_ok());
        assert!(matches!(
            parse(&with(iters, 13)),
            Err(StoreError::UnusualKdf)
        ));
        assert!(matches!(
            parse(&with(iters, 0)),
            Err(StoreError::UnusualKdf)
        ));
        assert!(parse(&with(iters, 12)).is_ok());
        assert!(matches!(
            parse(&with(lanes, 5)),
            Err(StoreError::UnusualKdf)
        ));
        assert!(matches!(
            parse(&with(lanes, 0)),
            Err(StoreError::UnusualKdf)
        ));
        assert!(parse(&with(lanes, 4)).is_ok());
        // Deriving refuses them too.
        let k = KdfParams {
            memory_kib: u32::MAX,
            ..KdfParams::TEST
        };
        assert!(matches!(
            derive("x", k, &h.salt),
            Err(StoreError::UnusualKdf)
        ));
        assert!(KdfParams::DEFAULT.is_bounded() && KdfParams::TEST.is_bounded());
    }

    #[test]
    fn encode_unlock_decrypt() {
        let data_key = random_key().unwrap();
        let mut h = Header {
            kdf: KdfParams::TEST,
            salt: random::<16>().unwrap(),
            slots: Vec::new(),
        };
        let pk = derive("correct horse battery", h.kdf, &h.salt).unwrap();
        h.slots
            .push(wrap(&h, SlotKind::Password, &pk, &data_key).unwrap());
        let rk = random::<32>().unwrap();
        h.slots
            .push(wrap(&h, SlotKind::Recovery, &rk, &data_key).unwrap());
        let file = encode(&h, &data_key, b"image bytes").unwrap();
        let (parsed, len) = parse(&file).unwrap();
        assert_eq!(parsed, h);
        let k = unlock(&parsed, &Secret::Password("correct horse battery".into())).unwrap();
        assert_eq!(&decrypt(&file, len, &k).unwrap()[..], b"image bytes");
        let k = unlock(&parsed, &Secret::RecoveryKey(format_recovery_key(&rk))).unwrap();
        assert_eq!(*k, *data_key);
        assert!(matches!(
            unlock(&parsed, &Secret::Password("wrong".into())),
            Err(StoreError::WrongPassword)
        ));
        // A new salt (password change) keeps the recovery slot valid.
        let mut h2 = parsed.clone();
        h2.salt = random::<16>().unwrap();
        assert!(unlock(&h2, &Secret::RecoveryKey(format_recovery_key(&rk))).is_ok());
        assert!(unlock(&h2, &Secret::Password("correct horse battery".into())).is_err());
    }
}
