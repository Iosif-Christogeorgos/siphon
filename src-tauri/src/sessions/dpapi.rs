//! Encryption at rest with Windows' Data Protection API: only the same
//! Windows user on the same computer can decrypt what this encrypts.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// Mixed into the key, so another program's DPAPI data can't be passed off as Siphon's.
const ENTROPY: &[u8] = b"Siphon sign-in cookies";

pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    run(data, |input, entropy, output| unsafe {
        CryptProtectData(
            input,
            ptr::null(),
            entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            output,
        )
    })
}

pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    run(data, |input, entropy, output| unsafe {
        CryptUnprotectData(
            input,
            ptr::null_mut(),
            entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            output,
        )
    })
}

fn run(
    data: &[u8],
    call: impl FnOnce(*const CRYPT_INTEGER_BLOB, *const CRYPT_INTEGER_BLOB, *mut CRYPT_INTEGER_BLOB) -> i32,
) -> io::Result<Vec<u8>> {
    let blob = |bytes: &[u8]| CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        // The API only reads its inputs.
        pbData: bytes.as_ptr().cast_mut(),
    };
    let input = blob(data);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    if call(&input, &entropy, &mut output) == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success the API hands over a buffer of `cbData` bytes, which
    // is ours to free with LocalFree.
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_hides_the_plain_text() {
        let secret = b".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfake-test-value";
        let sealed = protect(secret).unwrap();
        assert!(!sealed.windows(secret.len()).any(|window| window == secret));
        assert_eq!(unprotect(&sealed).unwrap(), secret);
    }

    #[test]
    fn refuses_tampered_data() {
        let mut sealed = protect(b"fake").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(unprotect(&sealed).is_err());
    }
}
