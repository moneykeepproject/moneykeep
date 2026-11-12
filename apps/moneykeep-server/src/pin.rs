//! The Capture PIN (mobile-capture.md revision 10): 6–12 digits, kept only as an
//! Argon2id hash (RFC 9106) with the parameters book passwords use
//! (book-encryption.md: 64 MiB, 3 iterations, parallelism 1) and a random 16-byte
//! salt. The stored record is JSON: the parameters, the salt and the 32-byte hash,
//! Base64. The PIN itself is never stored or logged.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::store::{self, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

/// The parameters of new PINs.
pub const COST: Cost = Cost {
    m: 65_536,
    t: 3,
    p: 1,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    alg: String,
    #[serde(flatten)]
    cost: Cost,
    salt: String,
    hash: String,
}

fn derive(pin: &str, salt: &[u8], cost: Cost) -> StoreResult<Zeroizing<[u8; 32]>> {
    let params = Params::new(cost.m, cost.t, cost.p, Some(32))
        .map_err(|e| format!("PIN parameters: {e}"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pin.as_bytes(), salt, out.as_mut())
        .map_err(|e| format!("PIN hash: {e}"))?;
    Ok(out)
}

/// The stored form of a PIN (already checked to be 6–12 digits).
pub fn hash(pin: &str) -> StoreResult<String> {
    hash_with(pin, COST)
}

pub fn hash_with(pin: &str, cost: Cost) -> StoreResult<String> {
    let salt = store::random::<16>()?;
    let h = derive(pin, &salt, cost)?;
    let b64 = base64::engine::general_purpose::STANDARD;
    serde_json::to_string(&Record {
        alg: "argon2id".into(),
        cost,
        salt: b64.encode(salt),
        hash: b64.encode(h.as_ref()),
    })
    .map_err(|e| e.to_string())
}

/// Whether `pin` matches the stored record (compared in constant time).
pub fn verify(pin: &str, stored: &str) -> bool {
    let Ok(r) = serde_json::from_str::<Record>(stored) else {
        return false;
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let (Ok(salt), Ok(want)) = (b64.decode(&r.salt), b64.decode(&r.hash)) else {
        return false;
    };
    if r.alg != "argon2id" {
        return false;
    }
    derive(pin, &salt, r.cost).is_ok_and(|got| store::ct_eq(got.as_ref(), &want))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn only_a_hash_is_kept() {
        let cheap = Cost { m: 64, t: 1, p: 1 };
        let rec = hash_with("123456", cheap).unwrap();
        assert!(!rec.contains("123456"));
        assert!(rec.contains("\"alg\":\"argon2id\""));
        assert!(verify("123456", &rec));
        assert!(!verify("123457", &rec));
        assert!(!verify("123456", "not json"));
        // Each hash has its own salt.
        assert_ne!(rec, hash_with("123456", cheap).unwrap());
    }
}
