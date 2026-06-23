use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use kvasir_core::{BearerToken, KvasirEndpoint, RawBodyDirectory};
use kvasird::{DaemonConfig, SetupSecretSource, start};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = kvasir_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    set_private_dir_permissions(&data_dir)?;

    let otlp_bind = std::env::var("KVASIR_OTLP_BIND")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| SocketAddr::from((Ipv4Addr::LOCALHOST, 4318)));
    let rpc_socket_path = std::env::var_os("KVASIR_RPC_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("kvasird.sock"));
    let database_path = std::env::var_os("KVASIR_DATABASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("usage.sqlite3"));
    let bearer_token =
        bearer_token_from_environment_or_setup(std::env::var("KVASIR_BEARER_TOKEN").ok(), || {
            daemon_setup_bearer_token(otlp_bind, &data_dir)
        })?;

    let daemon = start(DaemonConfig::new(
        otlp_bind,
        rpc_socket_path,
        database_path,
        bearer_token,
    ))
    .await?;
    eprintln!("kvasird listening for OTLP on {}", daemon.otlp_addr());

    tokio::signal::ctrl_c().await?;
    drop(daemon);
    Ok(())
}

fn kvasir_data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("KVASIR_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    let Some(home) = std::env::var_os("HOME") else {
        anyhow::bail!("HOME must be set when KVASIR_DATA_DIR is not provided");
    };
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("dev.kvasir"))
}

fn bearer_token_from_environment_or_setup(
    environment_token: Option<String>,
    setup_token: impl FnOnce() -> anyhow::Result<BearerToken>,
) -> anyhow::Result<BearerToken> {
    match environment_token {
        Some(token) if !token.trim().is_empty() => Ok(BearerToken::new(token)),
        _ => setup_token(),
    }
}

fn daemon_setup_bearer_token(
    otlp_bind: SocketAddr,
    data_dir: &std::path::Path,
) -> anyhow::Result<BearerToken> {
    let settings_path = setup_settings_path()?;
    let config = SetupSecretSource::claude_code_keychain(&settings_path).resolve(
        KvasirEndpoint::from_otlp_addr(otlp_bind),
        RawBodyDirectory::new(data_dir.join("raw-bodies")),
    )?;
    Ok(config.bearer_token().clone())
}

fn setup_settings_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("KVASIR_SETUP_SETTINGS") {
        return Ok(PathBuf::from(path));
    }
    let Some(home) = std::env::var_os("HOME") else {
        anyhow::bail!("HOME must be set when KVASIR_SETUP_SETTINGS is not provided");
    };
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_from_environment_prefers_non_empty_override() -> anyhow::Result<()> {
        let token =
            bearer_token_from_environment_or_setup(Some("operator-token".to_owned()), || {
                Ok(BearerToken::new("setup-token"))
            })?;

        assert_eq!(token.as_str(), "operator-token");
        Ok(())
    }

    #[test]
    fn bearer_token_from_environment_uses_setup_token_when_override_is_missing_or_empty()
    -> anyhow::Result<()> {
        let missing =
            bearer_token_from_environment_or_setup(None, || Ok(BearerToken::new("setup-token")))?;
        let empty = bearer_token_from_environment_or_setup(Some("  ".to_owned()), || {
            Ok(BearerToken::new("setup-token"))
        })?;

        assert_eq!(missing.as_str(), "setup-token");
        assert_eq!(empty.as_str(), "setup-token");
        Ok(())
    }
}
