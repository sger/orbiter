//! Signing-key storage in the macOS Keychain.
//!
//! The key is stored under an account name derived from the Apple account and team, so different
//! accounts and teams never share one. It is marked as this device only and not synchronised, so
//! it does not reach iCloud or another Mac. Orbiter stores exactly one secret: the development
//! signing key it generated. Nothing else — no password, token, or device identifier — is written.
use sha2::{Digest, Sha256};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn orbiter_keychain_store(account: *const u8, bytes: *const u8, length: usize) -> i32;
    fn orbiter_keychain_load(
        account: *const u8,
        buffer: *mut u8,
        capacity: usize,
        written: *mut usize,
    ) -> i32;
    fn orbiter_keychain_delete(account: *const u8) -> i32;
}

/// Keychain account name for one Apple account and team. The address is hashed rather than
/// stored, so reading the Keychain does not disclose which Apple account Orbiter signed in with.
pub fn account(email: &str, team_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(email.trim().to_ascii_lowercase().as_bytes());
    digest.update(b"\0");
    digest.update(team_id.as_bytes());
    format!("orbiter-signing-key-{:x}", digest.finalize())
}

fn terminated(account: &str) -> Vec<u8> {
    let mut bytes = account.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

#[cfg(not(target_os = "macos"))]
pub fn store(_: &str, _: &[u8]) -> Result<(), String> {
    Err("Signing keys can only be stored on macOS.".into())
}
#[cfg(not(target_os = "macos"))]
pub fn load(_: &str) -> Result<Option<Vec<u8>>, String> {
    Err("Signing keys can only be read on macOS.".into())
}
#[cfg(not(target_os = "macos"))]
pub fn forget(_: &str) -> Result<(), String> {
    Err("Signing keys can only be removed on macOS.".into())
}

/// Store `key` (PKCS#8 DER), replacing any key previously stored for this account.
#[cfg(target_os = "macos")]
pub fn store(account: &str, key: &[u8]) -> Result<(), String> {
    let name = terminated(account);
    // SAFETY: both pointers refer to live slices for the duration of the call.
    match unsafe { orbiter_keychain_store(name.as_ptr(), key.as_ptr(), key.len()) } {
        0 => Ok(()),
        1 => Err("The signing key could not be prepared for the Keychain.".into()),
        _ => Err(
            "macOS did not store the signing key in the Keychain. Unlock the Mac and allow Orbiter access, then try again."
                .into(),
        ),
    }
}

/// Read this account's stored signing key, if one exists.
#[cfg(target_os = "macos")]
pub fn load(account: &str) -> Result<Option<Vec<u8>>, String> {
    let name = terminated(account);
    let mut bytes = zeroize::Zeroizing::new(vec![0_u8; 64 * 1024]);
    let mut written = 0_usize;
    // SAFETY: the buffer is live and its capacity is passed honestly.
    let result = unsafe {
        orbiter_keychain_load(name.as_ptr(), bytes.as_mut_ptr(), bytes.len(), &mut written)
    };
    match result {
        0 if written > 0 && written <= bytes.len() => Ok(Some(bytes[..written].to_vec())),
        3 => Ok(None),
        1 => Err("The signing key could not be requested from the Keychain.".into()),
        _ => Err(
            "macOS did not return the stored signing key. Unlock the Mac and allow Orbiter access, then try again."
                .into(),
        ),
    }
}

/// Remove this account's stored signing key. Absent is success.
#[cfg(target_os = "macos")]
pub fn forget(account: &str) -> Result<(), String> {
    let name = terminated(account);
    // SAFETY: the name is a live, NUL-terminated buffer.
    match unsafe { orbiter_keychain_delete(name.as_ptr()) } {
        0 => Ok(()),
        1 => Err("The signing key could not be identified for removal.".into()),
        _ => Err("macOS did not remove the stored signing key.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_derived_per_apple_account_and_team_and_disclose_neither() {
        let one = account("Person@example.invalid", "T8B3X5UL5W");
        // Case and surrounding space are not a different account.
        assert_eq!(one, account(" person@example.invalid ", "T8B3X5UL5W"));
        // A different team, or a different account, is a different key.
        assert_ne!(one, account("person@example.invalid", "OTHERTEAM1"));
        assert_ne!(one, account("other@example.invalid", "T8B3X5UL5W"));
        // The stored name reveals neither the address nor the team.
        assert!(!one.contains("person") && !one.contains("example"));
        assert!(!one.contains("T8B3X5UL5W"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Writes and removes one item in this Mac's login Keychain"]
    fn a_stored_key_survives_and_can_be_forgotten() {
        let account = account("orbiter-test@example.invalid", "TESTTEAM01");
        let key = b"synthetic-key-material".to_vec();
        store(&account, &key).expect("stored");
        assert_eq!(load(&account).expect("read"), Some(key.clone()));
        // Storing again replaces rather than duplicating.
        store(&account, b"replacement").expect("replaced");
        assert_eq!(load(&account).expect("read"), Some(b"replacement".to_vec()));
        forget(&account).expect("removed");
        assert_eq!(load(&account).expect("read"), None);
        // Removing an absent key is not an error.
        forget(&account).expect("absent is success");
    }
}
