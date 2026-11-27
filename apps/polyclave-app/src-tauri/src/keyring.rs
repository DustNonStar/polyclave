use crate::shell::Daemon;
use serde::Serialize;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct KeyStatus {
    configured: bool,
    masked_tail: Option<String>,
}

fn entry(daemon: &Daemon, profile: &str) -> Result<(String, String), String> {
    if profile.is_empty()
        || profile.len() > 100
        || !profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid provider identity".into());
    }
    let binding = daemon.get(&format!("/api/profiles/{profile}/credential-binding"))?;
    Ok((
        binding["service"]
            .as_str()
            .ok_or("Provider has no keychain binding")?
            .to_owned(),
        binding["account"]
            .as_str()
            .ok_or("Provider has no keychain binding")?
            .to_owned(),
    ))
}

fn masked_from_result(res: std::io::Result<String>) -> Result<KeyStatus, String> {
    match res.map(Zeroizing::new) {
        Ok(secret) if !secret.is_empty() => {
            let last: String = secret
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let masked = (secret.chars().count() > 4).then(|| format!("••••{last}"));
            Ok(KeyStatus {
                configured: true,
                masked_tail: masked,
            })
        }
        Ok(_) => Ok(KeyStatus {
            configured: false,
            masked_tail: None,
        }),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || (e.kind() == std::io::ErrorKind::Other
                    && e.to_string() == "native credential unavailable") =>
        {
            Ok(KeyStatus {
                configured: false,
                masked_tail: None,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
            Err("Native keychain is not supported on this platform".into())
        }
        Err(e) => Err(format!("Keychain read failed: {e}")),
    }
}

fn masked(service: &str, account: &str) -> Result<KeyStatus, String> {
    masked_from_result(polyclave_os::read_provider_credential(service, account))
}

fn normalize_secret(raw: &str) -> Result<Zeroizing<String>, String> {
    let value = Zeroizing::new(raw.trim().to_owned());
    if value.len() < 8 || value.len() > 4096 || value.bytes().any(|b| !(33..=126).contains(&b)) {
        return Err("Invalid provider credential format".into());
    }
    Ok(value)
}

#[tauri::command]
pub async fn key_status(
    daemon: tauri::State<'_, Daemon>,
    profile: String,
) -> Result<KeyStatus, String> {
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (service, account) = entry(&daemon, &profile)?;
        masked(&service, &account)
    })
    .await
    .map_err(|_| "Keychain request was interrupted".to_owned())?
}

#[tauri::command]
pub async fn save_key(
    daemon: tauri::State<'_, Daemon>,
    profile: String,
    value: String,
) -> Result<KeyStatus, String> {
    let value = normalize_secret(&value)?;
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (service, account) = entry(&daemon, &profile)?;
        polyclave_os::write_provider_credential(&service, &account, &value)
            .map_err(|e| format!("Keychain could not save the credential: {e}"))?;
        masked(&service, &account)
    })
    .await
    .map_err(|_| "Keychain request was interrupted".to_owned())?
}

#[tauri::command]
pub async fn delete_key(
    daemon: tauri::State<'_, Daemon>,
    profile: String,
) -> Result<KeyStatus, String> {
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (service, account) = entry(&daemon, &profile)?;
        polyclave_os::write_provider_credential(&service, &account, "")
            .map_err(|e| format!("Keychain could not clear the credential: {e}"))?;
        Ok(KeyStatus {
            configured: false,
            masked_tail: None,
        })
    })
    .await
    .map_err(|_| "Keychain request was interrupted".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn normalize_secret_trims_whitespace_and_rejects_invalid_chars() {
        assert_eq!(
            &*normalize_secret("  sk-12345678 \n").unwrap(),
            "sk-12345678"
        );
        assert!(normalize_secret("  short  ").is_err());
        assert!(normalize_secret("sk-1234 5678").is_err());
    }

    #[test]
    fn masked_from_result_distinguishes_unconfigured_from_real_errors() {
        assert_eq!(
            masked_from_result(Err(io::Error::new(io::ErrorKind::NotFound, "missing"))).unwrap(),
            KeyStatus {
                configured: false,
                masked_tail: None
            }
        );
        assert_eq!(
            masked_from_result(Err(io::Error::other("native credential unavailable"))).unwrap(),
            KeyStatus {
                configured: false,
                masked_tail: None
            }
        );
        assert!(masked_from_result(Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "locked"
        )))
        .is_err());
        assert_eq!(
            masked_from_result(Ok("sk-abcdef".into())).unwrap(),
            KeyStatus {
                configured: true,
                masked_tail: Some("••••cdef".into())
            }
        );
    }
}
