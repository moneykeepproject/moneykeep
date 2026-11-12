//! The capture server's inbox key sealed by Windows (upgrade-resilience-1 U1.6;
//! security-review-1 L5): DPAPI for this computer, through `CryptProtectData` and
//! `CryptUnprotectData` (Microsoft: "CryptProtectData function", "CryptUnprotectData
//! function", wincrypt.h), with `CRYPTPROTECT_LOCAL_MACHINE` so that the service's own
//! account can open what an administrator sealed, and `CRYPTPROTECT_UI_FORBIDDEN`
//! because a service has no desktop. A copy of the sealed file is useless on another
//! Windows installation.
//!
//! This is the workspace's only `unsafe` code: the two calls into `crypt32.dll` and
//! the `LocalFree` of the buffer DPAPI returns, in one Windows-only module. Outside
//! Windows every call answers [`WINDOWS_ONLY`].
//!
//! Compatible with the earlier PowerShell sealing (`[Security.Cryptography.
//! ProtectedData]::Protect($data, $entropy, 'LocalMachine')`, which calls the same
//! function with the same flags), so keys sealed before still open.

/// The answer outside Windows.
pub const WINDOWS_ONLY: &str = "Windows only";

/// Seal `plain` for this computer, with `entropy` as DPAPI's optional entropy.
pub fn protect(plain: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    imp::call(true, plain, entropy)
}

/// Open what [`protect`] sealed (the same `entropy`). The caller zeroizes the result.
pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    imp::call(false, sealed, entropy)
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn call(_protect: bool, _input: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, String> {
        Err(super::WINDOWS_ONLY.into())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CryptProtectData, CryptUnprotectData,
    };

    /// A blob DPAPI only reads (its input and entropy parameters are `*const`).
    fn blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB, String> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len()).map_err(|_| "the data is too large".to_owned())?,
            pbData: bytes.as_ptr().cast_mut(),
        })
    }

    pub(super) fn call(protect: bool, input: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let data_in = blob(input)?;
        let extra = blob(entropy)?;
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: core::ptr::null_mut(),
        };
        let flags = if protect {
            CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN
        } else {
            CRYPTPROTECT_UI_FORBIDDEN
        };
        // SAFETY: `data_in` and `extra` point at live slices of the lengths given and
        // DPAPI only reads them; the description, reserved and prompt pointers are
        // optional and passed as null; `out` is a valid blob DPAPI fills in.
        let ok = unsafe {
            if protect {
                CryptProtectData(
                    &data_in,
                    core::ptr::null(),
                    &extra,
                    core::ptr::null(),
                    core::ptr::null(),
                    flags,
                    &mut out,
                )
            } else {
                CryptUnprotectData(
                    &data_in,
                    core::ptr::null_mut(),
                    &extra,
                    core::ptr::null(),
                    core::ptr::null(),
                    flags,
                    &mut out,
                )
            }
        };
        let name = if protect {
            "CryptProtectData"
        } else {
            "CryptUnprotectData"
        };
        if ok == 0 {
            return Err(format!(
                "Windows could not {} the inbox key ({name}: {})",
                if protect { "seal" } else { "open" },
                std::io::Error::last_os_error()
            ));
        }
        if out.pbData.is_null() {
            return Err(format!("{name} gave back nothing"));
        }
        let len = usize::try_from(out.cbData).unwrap_or(0);
        // SAFETY: on success DPAPI sets `pbData` to a buffer of `cbData` bytes that it
        // allocated with LocalAlloc; it is read once, wiped and freed with LocalFree,
        // and never used afterwards.
        let bytes = unsafe {
            let v = core::slice::from_raw_parts(out.pbData, len).to_vec();
            core::ptr::write_bytes(out.pbData, 0, len);
            let _ = LocalFree(out.pbData.cast());
            v
        };
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn windows_only_elsewhere() {
        assert_eq!(protect(b"k", b"e").unwrap_err(), WINDOWS_ONLY);
        assert_eq!(unprotect(b"k", b"e").unwrap_err(), WINDOWS_ONLY);
    }

    /// On Windows (the CI job of upgrade-resilience-1 U3): a round trip, and a blob
    /// that was changed or opened with other entropy is refused.
    #[cfg(windows)]
    #[test]
    fn round_trip_on_windows() {
        let sealed = protect(b"key text", b"entropy").unwrap();
        assert_ne!(sealed.as_slice(), b"key text");
        assert_eq!(unprotect(&sealed, b"entropy").unwrap(), b"key text");
        assert!(unprotect(&sealed, b"other").is_err());
        assert!(unprotect(&[], b"entropy").is_err());
    }
}
