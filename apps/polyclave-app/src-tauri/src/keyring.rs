use crate::shell::Daemon;
use serde::Serialize;
use zeroize::Zeroizing;

#[derive(Serialize)]
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
    let binding = daemon
        .get(&format!("/api/profiles/{profile}/credential-binding"))
        .map_err(str::to_owned)?;
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
fn masked(service: &str, account: &str) -> KeyStatus {
    match polyclave_os::read_provider_credential(service, account).map(Zeroizing::new) {
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
            KeyStatus {
                configured: true,
                masked_tail: masked,
            }
        }
        _ => KeyStatus {
            configured: false,
            masked_tail: None,
        },
    }
}
#[tauri::command]
pub async fn key_status(
    daemon: tauri::State<'_, Daemon>,
    profile: String,
) -> Result<KeyStatus, String> {
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (service, account) = entry(&daemon, &profile)?;
        Ok(masked(&service, &account))
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
    let value = Zeroizing::new(value);
    if value.len() < 8 || value.len() > 4096 || value.bytes().any(|b| !(33..=126).contains(&b)) {
        return Err("Invalid provider credential format".into());
    }
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (service, account) = entry(&daemon, &profile)?;
        polyclave_os::write_provider_credential(&service, &account, &value)
            .map_err(|_| "Keychain could not save the credential")?;
        Ok(masked(&service, &account))
    })
    .await
    .map_err(|_| "Keychain request was interrupted".to_owned())?
}
