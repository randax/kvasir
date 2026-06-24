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
    kvasir_data_dir_from(PathEnvironment::current())
}

struct PathEnvironment {
    data_dir: Option<PathBuf>,
    setup_settings_path: Option<PathBuf>,
    home: Option<PathBuf>,
    passwd_home: Option<PathBuf>,
}

impl PathEnvironment {
    fn current() -> Self {
        Self {
            data_dir: non_empty_path_env("KVASIR_DATA_DIR"),
            setup_settings_path: non_empty_path_env("KVASIR_SETUP_SETTINGS"),
            home: std::env::var_os("HOME").map(PathBuf::from),
            passwd_home: passwd_home_dir(),
        }
    }
}

fn kvasir_data_dir_from(environment: PathEnvironment) -> anyhow::Result<PathBuf> {
    if let Some(path) = non_empty_path(environment.data_dir) {
        return Ok(path);
    }
    let Some(home) = user_home_dir_from(environment.home, environment.passwd_home) else {
        anyhow::bail!(
            "HOME or passwd home directory must be available when KVASIR_DATA_DIR is not provided"
        );
    };
    Ok(home
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
    setup_settings_path_from(PathEnvironment::current())
}

fn setup_settings_path_from(environment: PathEnvironment) -> anyhow::Result<PathBuf> {
    if let Some(path) = non_empty_path(environment.setup_settings_path) {
        return Ok(path);
    }
    let Some(home) = user_home_dir_from(environment.home, environment.passwd_home) else {
        anyhow::bail!(
            "HOME or passwd home directory must be available when KVASIR_SETUP_SETTINGS is not provided"
        );
    };
    Ok(home.join(".claude").join("settings.json"))
}

fn user_home_dir_from(home: Option<PathBuf>, passwd_home: Option<PathBuf>) -> Option<PathBuf> {
    non_empty_path(home).or_else(|| non_empty_path(passwd_home))
}

fn non_empty_path_env(name: &str) -> Option<PathBuf> {
    non_empty_path(std::env::var_os(name).map(PathBuf::from))
}

fn non_empty_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
}

#[cfg(unix)]
fn passwd_home_dir() -> Option<PathBuf> {
    unix_user_home_dir()
}

#[cfg(not(unix))]
fn passwd_home_dir() -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn unix_user_home_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;
    use std::os::raw::{c_char, c_int};
    use std::ptr;

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Passwd {
        pw_name: *mut c_char,
        pw_passwd: *mut c_char,
        pw_uid: u32,
        pw_gid: u32,
        pw_change: i64,
        pw_class: *mut c_char,
        pw_gecos: *mut c_char,
        pw_dir: *mut c_char,
        pw_shell: *mut c_char,
        pw_expire: i64,
    }

    #[cfg(not(target_os = "macos"))]
    #[repr(C)]
    struct Passwd {
        pw_name: *mut c_char,
        pw_passwd: *mut c_char,
        pw_uid: u32,
        pw_gid: u32,
        pw_gecos: *mut c_char,
        pw_dir: *mut c_char,
        pw_shell: *mut c_char,
    }

    unsafe extern "C" {
        fn getuid() -> u32;
        fn getpwuid_r(
            uid: u32,
            pwd: *mut Passwd,
            buf: *mut c_char,
            buflen: usize,
            result: *mut *mut Passwd,
        ) -> c_int;
    }

    let mut passwd = MaybeUninit::<Passwd>::uninit();
    let mut result = ptr::null_mut();
    let mut buffer = vec![0 as c_char; 16 * 1024];

    // SAFETY: getpwuid_r writes at most buffer.len() bytes into the supplied
    // buffer and initializes passwd/result on success.
    let status = unsafe {
        getpwuid_r(
            getuid(),
            passwd.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }

    // SAFETY: result points to passwd initialized by a successful getpwuid_r
    // call, and pw_dir is a nul-terminated string owned by buffer.
    let directory = unsafe { (*result).pw_dir };
    if directory.is_null() {
        return None;
    }
    let directory = unsafe { CStr::from_ptr(directory) }.to_string_lossy();
    if directory.is_empty() {
        return None;
    }
    Some(PathBuf::from(directory.as_ref()))
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

    #[test]
    fn data_dir_prefers_explicit_override() -> anyhow::Result<()> {
        let data_dir = kvasir_data_dir_from(path_environment(
            Some("/tmp/kvasir-data"),
            None,
            Some("/Users/operator"),
            None,
        ))?;

        assert_eq!(data_dir, PathBuf::from("/tmp/kvasir-data"));
        Ok(())
    }

    #[test]
    fn data_dir_uses_home_when_override_is_missing() -> anyhow::Result<()> {
        let data_dir = kvasir_data_dir_from(path_environment(
            None,
            None,
            Some("/Users/operator"),
            Some("/Users/passwd"),
        ))?;

        assert_eq!(
            data_dir,
            PathBuf::from("/Users/operator")
                .join("Library")
                .join("Application Support")
                .join("dev.kvasir")
        );
        Ok(())
    }

    #[test]
    fn data_dir_falls_back_to_passwd_home_when_home_is_empty() -> anyhow::Result<()> {
        let data_dir = kvasir_data_dir_from(path_environment(
            None,
            None,
            Some(""),
            Some("/Users/passwd"),
        ))?;

        assert_eq!(
            data_dir,
            PathBuf::from("/Users/passwd")
                .join("Library")
                .join("Application Support")
                .join("dev.kvasir")
        );
        Ok(())
    }

    #[test]
    fn data_dir_ignores_empty_explicit_override() -> anyhow::Result<()> {
        let data_dir = kvasir_data_dir_from(path_environment(
            Some(""),
            None,
            Some("/Users/operator"),
            None,
        ))?;

        assert_eq!(
            data_dir,
            PathBuf::from("/Users/operator")
                .join("Library")
                .join("Application Support")
                .join("dev.kvasir")
        );
        Ok(())
    }

    #[test]
    fn setup_settings_prefers_explicit_override() -> anyhow::Result<()> {
        let settings_path = setup_settings_path_from(path_environment(
            None,
            Some("/tmp/settings.json"),
            Some("/Users/operator"),
            None,
        ))?;

        assert_eq!(settings_path, PathBuf::from("/tmp/settings.json"));
        Ok(())
    }

    #[test]
    fn setup_settings_ignores_empty_explicit_override() -> anyhow::Result<()> {
        let settings_path = setup_settings_path_from(path_environment(
            None,
            Some(""),
            Some("/Users/operator"),
            None,
        ))?;

        assert_eq!(
            settings_path,
            PathBuf::from("/Users/operator")
                .join(".claude")
                .join("settings.json")
        );
        Ok(())
    }

    #[test]
    fn setup_settings_uses_passwd_home_when_home_is_unavailable() -> anyhow::Result<()> {
        let settings_path =
            setup_settings_path_from(path_environment(None, None, None, Some("/Users/passwd")))?;

        assert_eq!(
            settings_path,
            PathBuf::from("/Users/passwd")
                .join(".claude")
                .join("settings.json")
        );
        Ok(())
    }

    #[test]
    fn path_resolution_requires_home_or_passwd_home_when_overrides_are_missing() {
        let error = kvasir_data_dir_from(path_environment(None, None, None, None))
            .expect_err("missing home should fail");

        assert!(
            error
                .to_string()
                .contains("HOME or passwd home directory must be available")
        );
    }

    fn path_environment(
        data_dir: Option<&str>,
        setup_settings_path: Option<&str>,
        home: Option<&str>,
        passwd_home: Option<&str>,
    ) -> PathEnvironment {
        PathEnvironment {
            data_dir: data_dir.map(PathBuf::from),
            setup_settings_path: setup_settings_path.map(PathBuf::from),
            home: home.map(PathBuf::from),
            passwd_home: passwd_home.map(PathBuf::from),
        }
    }
}
