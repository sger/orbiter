//! Read-only local identity inventory. No key export, signing, or portal access.
use serde::Serialize;

#[derive(Serialize)]
pub struct Identity {
    pub fingerprint: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct Inventory {
    pub identities: Vec<Identity>,
    pub available: bool,
    pub message: String,
}

#[cfg(any(target_os = "macos", test))]
fn parse(output: &str) -> Result<Vec<Identity>, String> {
    let mut identities = Vec::new();
    for line in output.lines() {
        let Some((index, rest)) = line.trim().split_once(')') else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let rest = rest.trim();
        let (fingerprint, label) = rest
            .split_once(' ')
            .ok_or("Unexpected identity response.")?;
        if fingerprint.len() != 40 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Unexpected identity response.".into());
        }
        let name = label
            .trim()
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("Unexpected identity response.")?;
        if name.len() > 1024 || name.chars().any(char::is_control) {
            return Err("Unexpected identity response.".into());
        }
        // Certificate names are labels, not verified team membership or profile compatibility.
        if [
            "Apple Development:",
            "Apple Distribution:",
            "iPhone Developer:",
            "iPhone Distribution:",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
            && !identities
                .iter()
                .any(|i: &Identity| i.fingerprint.eq_ignore_ascii_case(fingerprint))
        {
            identities.push(Identity {
                fingerprint: fingerprint.to_ascii_uppercase(),
                name: name.into(),
            });
        }
    }
    Ok(identities)
}

pub async fn discover() -> Result<Inventory, String> {
    #[cfg(not(target_os = "macos"))]
    return Ok(Inventory {
        identities: vec![],
        available: false,
        message: "Local signing identity discovery is currently available on macOS only.".into(),
    });

    #[cfg(target_os = "macos")]
    {
        use std::{process::Stdio, time::Duration};
        use tokio::io::AsyncReadExt;
        let operation = async {
            let mut child = tokio::process::Command::new("/usr/bin/security")
                .args(["find-identity", "-v", "-p", "codesigning"])
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| "Cannot start the Keychain identity check.")?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .ok_or("Cannot read the Keychain response.")?
                .take(65_537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "Cannot read the Keychain response.")?;
            if bytes.len() > 65_536 {
                return Err("Keychain response exceeds the supported limit.".into());
            }
            if !child
                .wait()
                .await
                .map_err(|_| "Keychain identity check stopped.")?
                .success()
            {
                return Err(
                    "Keychain identity check failed. Check Keychain Access and retry.".into(),
                );
            }
            let identities =
                parse(std::str::from_utf8(&bytes).map_err(|_| "Unexpected identity response.")?)?;
            let message = if identities.is_empty() {
                "No valid iOS signing identities found in the current Keychain search list. A signing certificate with its private key is required."
            } else {
                "Local iOS signing identities found. Private-key use, profile matching, and team authorization have not been tested."
            }.into();
            Ok(Inventory {
                identities,
                available: true,
                message,
            })
        };
        tokio::time::timeout(Duration::from_secs(10), operation)
            .await
            .map_err(|_| {
                "Keychain identity check timed out. Check Keychain Access and retry.".to_string()
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_desktop_identities_and_deduplicates() {
        let hash = "A".repeat(40);
        let output = format!(
            "  1) {hash} \"Apple Development: Test (TEAM)\"\n 2) {hash} \"Apple Development: Test (TEAM)\"\n 3) {} \"Developer ID Application: Test\"\n 3 valid identities found",
            "B".repeat(40)
        );
        let found = parse(&output).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fingerprint, hash);
    }
    #[test]
    fn empty_keychain_is_not_an_error() {
        assert!(parse("     0 valid identities found\n").unwrap().is_empty());
    }
    #[test]
    fn malformed_identity_is_an_error() {
        assert!(parse("1) BADHASH \"Apple Development: Test\"").is_err());
        assert!(parse(&format!("1) {} unquoted", "A".repeat(40))).is_err());
    }
}
