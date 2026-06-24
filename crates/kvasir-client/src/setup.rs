use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use kvasir_core::{
    CodexConfigToml, KvasirEndpoint, RawBodyDirectory, SetupConfig, SetupSecretSource,
};
#[cfg(test)]
use kvasir_core::{SetupCredential, prepare_setup_config};

use crate::error::KvasirClientError;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirHarnessTelemetrySetup {
    pub codex_config_path: String,
    pub claude_settings_path: String,
    pub raw_body_directory: String,
    pub otlp_endpoint: String,
}

#[uniffi::export]
pub fn configure_kvasir_harness_telemetry(
    config: KvasirHarnessTelemetrySetup,
) -> Result<(), KvasirClientError> {
    let setup_secret_source =
        SetupSecretSource::claude_code_keychain(&PathBuf::from(&config.claude_settings_path));
    let pending_setup_config = setup_secret_source
        .prepare(
            KvasirEndpoint::new(&config.otlp_endpoint),
            RawBodyDirectory::new(PathBuf::from(&config.raw_body_directory)),
        )
        .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let prepared_config = prepare_codex_telemetry_config(config, pending_setup_config.config())?;
    let committed_setup_config = match setup_secret_source.commit(pending_setup_config) {
        Ok(committed) => committed,
        Err(_) => {
            prepared_config.discard();
            return Err(KvasirClientError::HarnessTelemetrySetup);
        }
    };
    if let Err(error) = prepared_config.install() {
        return handle_install_error(error, || {
            setup_secret_source.rollback(committed_setup_config)
        });
    }
    Ok(())
}

#[cfg(test)]
fn configure_kvasir_harness_telemetry_with_credential(
    config: KvasirHarnessTelemetrySetup,
    credential: &dyn SetupCredential,
) -> Result<(), KvasirClientError> {
    configure_kvasir_harness_telemetry_with_credential_and_install_hook(config, credential, |_| {
        Ok(())
    })
}

#[cfg(test)]
fn configure_kvasir_harness_telemetry_with_credential_and_install_hook(
    config: KvasirHarnessTelemetrySetup,
    credential: &dyn SetupCredential,
    before_install: impl FnOnce(&PreparedCodexTelemetryConfig) -> std::io::Result<()>,
) -> Result<(), KvasirClientError> {
    let pending_setup_config = prepare_setup_config(
        credential,
        KvasirEndpoint::new(&config.otlp_endpoint),
        RawBodyDirectory::new(PathBuf::from(&config.raw_body_directory)),
    )
    .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let prepared_config = prepare_codex_telemetry_config(config, pending_setup_config.config())?;
    let committed_setup_config = match pending_setup_config.commit(credential) {
        Ok(committed) => committed,
        Err(_) => {
            prepared_config.discard();
            return Err(KvasirClientError::HarnessTelemetrySetup);
        }
    };
    if before_install(&prepared_config).is_err() {
        let rollback_result = committed_setup_config.rollback(credential);
        prepared_config.discard();
        return match rollback_result {
            Ok(()) => Err(KvasirClientError::Filesystem),
            Err(_) => Err(KvasirClientError::HarnessTelemetryRollback),
        };
    }
    if let Err(error) = prepared_config.install() {
        return handle_install_error(error, || committed_setup_config.rollback(credential));
    }
    Ok(())
}

fn handle_install_error(
    error: ConfigInstallError,
    rollback: impl FnOnce() -> Result<(), kvasir_core::SetupError>,
) -> Result<(), KvasirClientError> {
    match error {
        ConfigInstallError::ConfigPreserved => match rollback() {
            Ok(()) => Err(KvasirClientError::Filesystem),
            Err(_) => Err(KvasirClientError::HarnessTelemetryRollback),
        },
        ConfigInstallError::ConfigStateUnknown => {
            Err(KvasirClientError::HarnessTelemetryStateUnknown)
        }
    }
}

enum PreparedCodexTelemetryConfig {
    Unchanged,
    Replacement {
        target_path: PathBuf,
        temp_path: PathBuf,
        previous_config: PreviousCodexConfig,
    },
}

enum PreviousCodexConfig {
    Missing,
    Present {
        contents: String,
        permissions: Option<fs::Permissions>,
    },
}

impl PreparedCodexTelemetryConfig {
    fn install(self) -> Result<(), ConfigInstallError> {
        self.install_with_sync(sync_parent_directory)
    }

    fn install_with_sync(
        self,
        sync_parent: impl Fn(&Path) -> std::io::Result<()>,
    ) -> Result<(), ConfigInstallError> {
        match self {
            Self::Unchanged => Ok(()),
            Self::Replacement {
                target_path,
                temp_path,
                previous_config,
            } => {
                if fs::rename(&temp_path, &target_path).is_err() {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ConfigInstallError::ConfigPreserved);
                }
                if sync_parent(&target_path).is_err() {
                    return match restore_previous_config(&target_path, previous_config, sync_parent)
                    {
                        Ok(()) => Err(ConfigInstallError::ConfigPreserved),
                        Err(()) => Err(ConfigInstallError::ConfigStateUnknown),
                    };
                }
                Ok(())
            }
        }
    }

    fn discard(self) {
        if let Self::Replacement { temp_path, .. } = self {
            let _ = fs::remove_file(temp_path);
        }
    }
}

enum ConfigInstallError {
    ConfigPreserved,
    ConfigStateUnknown,
}

fn prepare_codex_telemetry_config(
    config: KvasirHarnessTelemetrySetup,
    setup_config: &SetupConfig,
) -> Result<PreparedCodexTelemetryConfig, KvasirClientError> {
    let codex_config_path = PathBuf::from(config.codex_config_path);
    if let Some(parent) = codex_config_path.parent() {
        fs::create_dir_all(parent).map_err(|_| KvasirClientError::Filesystem)?;
    }

    let existing_config = match fs::read_to_string(&codex_config_path) {
        Ok(contents) => PreviousCodexConfig::Present {
            contents,
            permissions: current_permissions(&codex_config_path),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PreviousCodexConfig::Missing,
        Err(_) => return Err(KvasirClientError::Filesystem),
    };
    let generated = CodexConfigToml::generate(existing_config.contents(), setup_config)
        .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;

    if generated.as_str() == existing_config.contents() {
        return Ok(PreparedCodexTelemetryConfig::Unchanged);
    }

    let temp_path = write_replacement_file(
        &codex_config_path,
        generated.as_str(),
        replacement_permissions(&existing_config),
    )
    .map_err(|_| KvasirClientError::Filesystem)?;
    Ok(PreparedCodexTelemetryConfig::Replacement {
        target_path: codex_config_path,
        temp_path,
        previous_config: existing_config,
    })
}

impl PreviousCodexConfig {
    fn contents(&self) -> &str {
        match self {
            Self::Missing => "",
            Self::Present { contents, .. } => contents,
        }
    }
}

fn restore_previous_config(
    path: &Path,
    previous_config: PreviousCodexConfig,
    sync_parent: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<(), ()> {
    match previous_config {
        PreviousCodexConfig::Missing => fs::remove_file(path)
            .map_err(|_| ())
            .and_then(|_| sync_parent(path).map_err(|_| ())),
        PreviousCodexConfig::Present {
            contents,
            permissions,
        } => {
            let temp_path = write_replacement_file(path, &contents, permissions).map_err(|_| ())?;
            let restore_result = fs::rename(&temp_path, path)
                .map_err(|_| ())
                .and_then(|_| sync_parent(path).map_err(|_| ()));
            if restore_result.is_err() {
                let _ = fs::remove_file(&temp_path);
            }
            restore_result
        }
    }
}

fn write_replacement_file(
    path: &Path,
    contents: &str,
    permissions: Option<fs::Permissions>,
) -> std::io::Result<PathBuf> {
    let (temp_path, mut temp_file) = create_temp_file(path)?;
    let write_result = write_temp_file(&mut temp_file, contents);
    drop(temp_file);
    let write_result = write_result.and_then(|_| {
        if let Some(permissions) = permissions {
            fs::set_permissions(&temp_path, permissions)?;
        }
        Ok(())
    });
    if let Err(err) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(temp_path)
}

fn write_temp_file(file: &mut File, contents: &str) -> std::io::Result<()> {
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn create_temp_file(path: &Path) -> std::io::Result<(PathBuf, File)> {
    let parent = writable_parent(path);
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path has no file name",
        )
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..16 {
        let candidate = parent.join(format!(
            ".{}.kvasir-tmp-{}-{nonce}-{attempt}",
            file_name.to_string_lossy(),
            std::process::id(),
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "temporary config file name collision",
    ))
}

fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = writable_parent(path);
    File::open(parent)?.sync_all()
}

fn writable_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn current_permissions(path: &Path) -> Option<fs::Permissions> {
    fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .ok()
}

#[cfg(not(unix))]
fn current_permissions(path: &Path) -> Option<fs::Permissions> {
    fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .ok()
}

#[cfg(unix)]
fn replacement_permissions(previous_config: &PreviousCodexConfig) -> Option<fs::Permissions> {
    let mode = match previous_config {
        PreviousCodexConfig::Missing => 0o600,
        PreviousCodexConfig::Present {
            permissions: Some(permissions),
            ..
        } => permissions.mode() & 0o700,
        PreviousCodexConfig::Present {
            permissions: None, ..
        } => 0o600,
    };
    Some(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn replacement_permissions(previous_config: &PreviousCodexConfig) -> Option<fs::Permissions> {
    match previous_config {
        PreviousCodexConfig::Missing => None,
        PreviousCodexConfig::Present { permissions, .. } => permissions.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn configure_harness_telemetry_writes_codex_config_with_persisted_setup_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex/config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let generated = fs::read_to_string(&config.codex_config_path)?;

        assert!(generated.contains("[otel]"));
        assert!(generated.contains("http://127.0.0.1:4318/v1/metrics"));
        assert!(generated.contains("Authorization\" = \"Bearer "));
        let token = credential
            .password
            .borrow()
            .clone()
            .expect("setup token persisted");
        let bearer_token: serde_json::Value = serde_json::from_str(&token)?;
        let token = bearer_token["bearer_token"]
            .as_str()
            .expect("bearer token serialized");
        assert!(generated.contains(&format!("Bearer {token}")));

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        assert_eq!(fs::read_to_string(&config.codex_config_path)?, generated);

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_does_not_commit_token_when_config_generation_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex/config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(
            &config.codex_config_path,
            r#"  # BEGIN KVASIR MANAGED CODEX OTEL
[otel]
metrics_exporter = { otlp-http = { endpoint = "http://old.example/v1/metrics", protocol = "binary" } }
# END KVASIR MANAGED CODEX OTEL
"#,
        )?;

        let error =
            configure_kvasir_harness_telemetry_with_credential(config, &credential).unwrap_err();

        assert!(matches!(error, KvasirClientError::HarnessTelemetrySetup));
        assert_eq!(*credential.write_count.borrow(), 0);
        assert!(credential.password.borrow().is_none());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_does_not_commit_token_when_config_write_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        fs::create_dir(&config.codex_config_path)?;

        let error =
            configure_kvasir_harness_telemetry_with_credential(config, &credential).unwrap_err();

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert_eq!(*credential.write_count.borrow(), 0);
        assert!(credential.password.borrow().is_none());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_does_not_replace_config_when_token_commit_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::failing_writes();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir)?;
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: codex_dir.join("config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        let existing_config = "model = \"gpt-5\"\n";
        fs::write(&config.codex_config_path, existing_config)?;

        let error = configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)
            .unwrap_err();

        assert!(matches!(error, KvasirClientError::HarnessTelemetrySetup));
        assert_eq!(
            fs::read_to_string(&config.codex_config_path)?,
            existing_config
        );
        assert_eq!(*credential.write_count.borrow(), 1);
        assert!(credential.password.borrow().is_none());
        assert_eq!(temporary_file_count(&codex_dir)?, 0);

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_rolls_back_token_when_staged_install_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex/config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let previous_password = credential.password.borrow().clone();
        let previous_config = fs::read_to_string(&config.codex_config_path)?;

        let updated_config = KvasirHarnessTelemetrySetup {
            otlp_endpoint: "http://127.0.0.1:9999".to_owned(),
            ..config.clone()
        };
        let error = configure_kvasir_harness_telemetry_with_credential_and_install_hook(
            updated_config,
            &credential,
            |prepared_config| {
                if let PreparedCodexTelemetryConfig::Replacement { temp_path, .. } = prepared_config
                {
                    fs::remove_file(temp_path)?;
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert_eq!(*credential.password.borrow(), previous_password);
        assert_eq!(
            fs::read_to_string(&config.codex_config_path)?,
            previous_config
        );

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_surfaces_rollback_failure_after_staged_install_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::failing_on_write_number(3);
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex/config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let updated_config = KvasirHarnessTelemetrySetup {
            otlp_endpoint: "http://127.0.0.1:9999".to_owned(),
            ..config
        };
        let error = configure_kvasir_harness_telemetry_with_credential_and_install_hook(
            updated_config,
            &credential,
            |prepared_config| {
                if let PreparedCodexTelemetryConfig::Replacement { temp_path, .. } = prepared_config
                {
                    fs::remove_file(temp_path)?;
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert!(matches!(error, KvasirClientError::HarnessTelemetryRollback));
        assert_eq!(*credential.write_count.borrow(), 3);

        Ok(())
    }

    #[test]
    fn setup_state_unknown_error_does_not_roll_back_credentials() {
        let rollback_called = Cell::new(false);

        let error = handle_install_error(ConfigInstallError::ConfigStateUnknown, || {
            rollback_called.set(true);
            Ok(())
        })
        .unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryStateUnknown
        ));
        assert!(!rollback_called.get());
    }

    #[test]
    fn configure_harness_telemetry_replaces_codex_config_without_leaving_temp_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir)?;
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: codex_dir.join("config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        fs::write(&config.codex_config_path, "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let generated = fs::read_to_string(&config.codex_config_path)?;
        assert!(generated.contains("model = \"gpt-5\""));
        assert!(generated.contains("[otel]"));
        assert_eq!(temporary_file_count(&codex_dir)?, 0);
        assert_eq!(*credential.write_count.borrow(), 1);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn configure_harness_telemetry_creates_private_codex_config_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: temp.path().join(".codex/config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let mode = fs::metadata(&config.codex_config_path)?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn configure_harness_telemetry_keeps_replaced_codex_config_private()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir)?;
        let config = KvasirHarnessTelemetrySetup {
            codex_config_path: codex_dir.join("config.toml").display().to_string(),
            claude_settings_path: temp
                .path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            raw_body_directory: temp.path().join("raw-bodies").display().to_string(),
            otlp_endpoint: "http://127.0.0.1:4318".to_owned(),
        };
        fs::write(&config.codex_config_path, "model = \"gpt-5\"\n")?;
        fs::set_permissions(&config.codex_config_path, fs::Permissions::from_mode(0o600))?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let mode = fs::metadata(&config.codex_config_path)?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        Ok(())
    }

    fn temporary_file_count(directory: &Path) -> Result<usize, Box<dyn std::error::Error>> {
        Ok(fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".kvasir-tmp-"))
            .count())
    }

    #[derive(Clone, Default)]
    struct MemorySetupCredential {
        password: Rc<RefCell<Option<String>>>,
        write_count: Rc<RefCell<usize>>,
        fail_write_number: Option<usize>,
    }

    impl MemorySetupCredential {
        fn failing_writes() -> Self {
            Self::failing_on_write_number(1)
        }

        fn failing_on_write_number(write_number: usize) -> Self {
            Self {
                fail_write_number: Some(write_number),
                ..Self::default()
            }
        }
    }

    impl SetupCredential for MemorySetupCredential {
        fn read(&self) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.password.borrow().clone())
        }

        fn write(&self, password: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            *self.write_count.borrow_mut() += 1;
            if self.fail_write_number == Some(*self.write_count.borrow()) {
                return Err(Box::new(std::io::Error::other("credential write failed")));
            }
            self.password.replace(Some(password.to_owned()));
            Ok(())
        }

        fn delete(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.password.replace(None);
            Ok(())
        }
    }
}
