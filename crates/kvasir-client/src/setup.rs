use std::fs;
use std::path::{Path, PathBuf};

use kvasir_core::{
    CodexConfigToml, KvasirEndpoint, RawBodyDirectory, SetupConfig, SetupError, SetupSecretSource,
};
#[cfg(test)]
use kvasir_core::{SetupCredential, prepare_setup_config};

use crate::client::KvasirClient;
use crate::error::KvasirClientError;
use crate::types::{
    KvasirBearerToken, KvasirContentQuery, KvasirContentReplay, KvasirContentReplayQuery,
    KvasirSocketPath,
};

mod fs_atomic;
mod generated_files;
mod managed_state;
mod uninstall;

use fs_atomic::{
    PreviousFile, read_previous_file, replace_file, replacement_permissions, restore_previous_file,
    sync_parent_directory, write_replacement_file,
};
use generated_files::{
    generated_harness_files_are_current, install_generated_harness_files,
    prepare_generated_harness_files,
};
use managed_state::{
    ensure_installable_state, ensure_refreshable_state, managed_file_is_current,
    write_installed_state,
};
use uninstall::{setup_managed_paths, uninstall_managed_file};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KvasirHarnessTelemetrySetup {
    pub codex_config_path: KvasirCodexConfigPath,
    pub claude_settings_path: KvasirClaudeSettingsPath,
    pub copilot_profile_path: KvasirShellProfilePath,
    pub opencode_config_path: KvasirOpenCodeConfigPath,
    pub opencode_env_path: KvasirOpenCodeEnvPath,
    pub zsh_profile_path: KvasirShellProfilePath,
    pub bash_profile_path: KvasirShellProfilePath,
    pub zsh_repo_hook_path: KvasirRepoHookPath,
    pub bash_repo_hook_path: KvasirRepoHookPath,
    pub raw_body_directory: KvasirRawBodyDirectory,
    pub otlp_endpoint: KvasirOtlpEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirCodexConfigPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirClaudeSettingsPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirShellProfilePath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirOpenCodeConfigPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirOpenCodeEnvPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirRepoHookPath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirRawBodyDirectory(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvasirOtlpEndpoint(String);

uniffi::custom_type!(KvasirCodexConfigPath, String);
uniffi::custom_type!(KvasirClaudeSettingsPath, String);
uniffi::custom_type!(KvasirShellProfilePath, String);
uniffi::custom_type!(KvasirOpenCodeConfigPath, String);
uniffi::custom_type!(KvasirOpenCodeEnvPath, String);
uniffi::custom_type!(KvasirRepoHookPath, String);
uniffi::custom_type!(KvasirRawBodyDirectory, String);
uniffi::custom_type!(KvasirOtlpEndpoint, String);

#[uniffi::export]
pub fn configure_kvasir_harness_telemetry(
    config: KvasirHarnessTelemetrySetup,
) -> Result<(), KvasirClientError> {
    let setup_secret_source =
        SetupSecretSource::claude_code_keychain(config.claude_settings_path.as_path());
    let pending_setup_config = setup_secret_source
        .prepare(
            config.otlp_endpoint.to_core(),
            config.raw_body_directory.to_core(),
        )
        .map_err(setup_error_to_client_error)?;
    let setup_config = pending_setup_config.config().clone();
    let prepared_config = prepare_codex_telemetry_config(config.clone(), &setup_config)?;
    let generated_files = prepare_generated_harness_files(&config, &setup_config)?;
    if pending_setup_config.credential_is_unchanged()
        && prepared_config.is_current()?
        && generated_harness_files_are_current(&generated_files)?
    {
        prepared_config.discard();
        return Ok(());
    }
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
    if install_generated_harness_files(generated_files).is_err() {
        let uninstall_result = uninstall_kvasir_harness_telemetry(config);
        return match setup_secret_source.rollback(committed_setup_config) {
            Ok(()) if uninstall_result.is_ok() => Err(KvasirClientError::Filesystem),
            Err(_) => Err(KvasirClientError::HarnessTelemetryRollback),
            Ok(()) => Err(KvasirClientError::HarnessTelemetryRollback),
        };
    }
    Ok(())
}

#[uniffi::export]
pub fn load_kvasir_content_replay(
    socket_path: KvasirSocketPath,
    config: KvasirHarnessTelemetrySetup,
    query: KvasirContentReplayQuery,
) -> Result<KvasirContentReplay, KvasirClientError> {
    let bearer_token =
        resolve_kvasir_bearer_token_from_source(config, bearer_token_from_environment)?;
    let client = KvasirClient::connect(socket_path)?;
    client.content_replay(KvasirContentQuery {
        harness: query.harness,
        session_id: query.session_id,
        prompt_id: query.prompt_id,
        bearer_token,
    })
}

#[uniffi::export]
pub fn uninstall_kvasir_harness_telemetry(
    config: KvasirHarnessTelemetrySetup,
) -> Result<(), KvasirClientError> {
    for path in setup_managed_paths(&config) {
        uninstall_managed_file(&path)?;
    }
    Ok(())
}

fn resolve_kvasir_bearer_token_from_source(
    config: KvasirHarnessTelemetrySetup,
    environment_token: impl FnOnce() -> Option<String>,
) -> Result<KvasirBearerToken, KvasirClientError> {
    if let Some(token) = environment_token() {
        return KvasirBearerToken::try_from(token);
    }

    let setup_secret_source =
        SetupSecretSource::claude_code_keychain(config.claude_settings_path.as_path());
    let setup_config = setup_secret_source
        .resolve(
            config.otlp_endpoint.to_core(),
            config.raw_body_directory.to_core(),
        )
        .map_err(setup_error_to_client_error)?;
    KvasirBearerToken::try_from(setup_config.bearer_token().as_str().to_owned())
}

fn bearer_token_from_environment() -> Option<String> {
    std::env::var("KVASIR_BEARER_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
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
        config.otlp_endpoint.to_core(),
        config.raw_body_directory.to_core(),
    )
    .map_err(setup_error_to_client_error)?;
    let setup_config = pending_setup_config.config().clone();
    let prepared_config = prepare_codex_telemetry_config(config.clone(), &setup_config)?;
    let generated_files = prepare_generated_harness_files(&config, &setup_config)?;
    if pending_setup_config.credential_is_unchanged()
        && prepared_config.is_current()?
        && generated_harness_files_are_current(&generated_files)?
    {
        prepared_config.discard();
        return Ok(());
    }
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
    if install_generated_harness_files(generated_files).is_err() {
        let uninstall_result = uninstall_kvasir_harness_telemetry(config);
        return match committed_setup_config.rollback(credential) {
            Ok(()) if uninstall_result.is_ok() => Err(KvasirClientError::Filesystem),
            Err(_) => Err(KvasirClientError::HarnessTelemetryRollback),
            Ok(()) => Err(KvasirClientError::HarnessTelemetryRollback),
        };
    }
    Ok(())
}

#[cfg(test)]
fn resolve_kvasir_bearer_token_with_credential(
    config: KvasirHarnessTelemetrySetup,
    credential: &dyn SetupCredential,
    environment_token: Option<String>,
) -> Result<KvasirBearerToken, KvasirClientError> {
    if let Some(token) = environment_token.filter(|token| !token.trim().is_empty()) {
        return KvasirBearerToken::try_from(token);
    }

    let setup_config = prepare_setup_config(
        credential,
        config.otlp_endpoint.to_core(),
        config.raw_body_directory.to_core(),
    )
    .map_err(setup_error_to_client_error)?
    .commit(credential)
    .map_err(setup_error_to_client_error)?
    .into_config();
    KvasirBearerToken::try_from(setup_config.bearer_token().as_str().to_owned())
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

pub(super) fn setup_error_to_client_error(error: SetupError) -> KvasirClientError {
    match error {
        SetupError::SettingsNotObject
        | SetupError::EnvNotObject
        | SetupError::InvalidSettingsJson(_) => {
            KvasirClientError::HarnessTelemetryInvalidClaudeSettings
        }
        SetupError::OpenCodeConfigNotObject
        | SetupError::OpenCodeExperimentalNotObject
        | SetupError::OpenCodeManagedBlockNotObject
        | SetupError::InvalidOpenCodeConfigJson(_)
        | SetupError::InvalidOpenCodeOtlpEndpointEnvValue
        | SetupError::InvalidOpenCodeOtlpHeadersEnvValue => {
            KvasirClientError::HarnessTelemetryInvalidOpenCodeConfig
        }
        SetupError::InvalidSetupSecretJson(_) | SetupError::SetupSecretSerialization(_) => {
            KvasirClientError::HarnessTelemetryInvalidStoredSecret
        }
        SetupError::SetupKeychain(_)
        | SetupError::BearerTokenGeneration(_)
        | SetupError::SetupCredentialRead(_)
        | SetupError::SetupCredentialWrite(_)
        | SetupError::MalformedManagedBlock
        | SetupError::ConflictingCodexOtelKeys => KvasirClientError::HarnessTelemetrySetup,
        _ => KvasirClientError::HarnessTelemetrySetup,
    }
}

fn codex_setup_error_to_client_error(error: SetupError) -> KvasirClientError {
    match error {
        SetupError::MalformedManagedBlock | SetupError::ConflictingCodexOtelKeys => {
            KvasirClientError::HarnessTelemetryInvalidCodexConfig
        }
        other => setup_error_to_client_error(other),
    }
}

enum PreparedCodexTelemetryConfig {
    Unchanged {
        target_path: PathBuf,
        desired_contents: String,
    },
    Replacement {
        target_path: PathBuf,
        temp_path: PathBuf,
        previous_config: PreviousFile,
        desired_contents: String,
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
            Self::Unchanged {
                target_path,
                desired_contents,
            } => ensure_installable_state(&target_path)
                .and_then(|_| write_installed_state(&target_path, &desired_contents))
                .map_err(|_| ConfigInstallError::ConfigPreserved),
            Self::Replacement {
                target_path,
                temp_path,
                previous_config,
                desired_contents,
            } => {
                if ensure_refreshable_state(&target_path).is_err() {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ConfigInstallError::ConfigPreserved);
                }
                let staged_contents = match fs::read_to_string(&temp_path) {
                    Ok(contents) => contents,
                    Err(_) => {
                        let _ = fs::remove_file(&temp_path);
                        return Err(ConfigInstallError::ConfigPreserved);
                    }
                };
                let _ = fs::remove_file(&temp_path);
                if replace_file(&target_path, &staged_contents).is_err() {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ConfigInstallError::ConfigPreserved);
                }
                if write_installed_state(&target_path, &desired_contents).is_err() {
                    return match restore_previous_file(&target_path, previous_config, sync_parent) {
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

    fn is_current(&self) -> Result<bool, KvasirClientError> {
        match self {
            Self::Unchanged {
                target_path,
                desired_contents,
            }
            | Self::Replacement {
                target_path,
                desired_contents,
                ..
            } => managed_file_is_current(target_path, desired_contents),
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
    let codex_config_path = config.codex_config_path.into_path_buf();
    if let Some(parent) = codex_config_path.parent() {
        fs::create_dir_all(parent).map_err(|_| KvasirClientError::Filesystem)?;
    }

    let existing_config =
        read_previous_file(&codex_config_path).map_err(|_| KvasirClientError::Filesystem)?;
    let generated = CodexConfigToml::generate(existing_config.contents(), setup_config)
        .map_err(codex_setup_error_to_client_error)?;

    if generated.as_str() == existing_config.contents() {
        return Ok(PreparedCodexTelemetryConfig::Unchanged {
            target_path: codex_config_path,
            desired_contents: generated.as_str().to_owned(),
        });
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
        desired_contents: generated.as_str().to_owned(),
    })
}

impl KvasirCodexConfigPath {
    fn into_path_buf(self) -> PathBuf {
        PathBuf::from(self.0)
    }

    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    #[cfg(test)]
    fn backup_path(&self) -> PathBuf {
        managed_state::backup_path(self.as_path())
    }
}

impl From<String> for KvasirCodexConfigPath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<KvasirCodexConfigPath> for String {
    fn from(value: KvasirCodexConfigPath) -> Self {
        value.0
    }
}

impl AsRef<Path> for KvasirCodexConfigPath {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl KvasirClaudeSettingsPath {
    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    #[cfg(test)]
    fn backup_path(&self) -> PathBuf {
        managed_state::backup_path(self.as_path())
    }
}

impl From<String> for KvasirClaudeSettingsPath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<KvasirClaudeSettingsPath> for String {
    fn from(value: KvasirClaudeSettingsPath) -> Self {
        value.0
    }
}

macro_rules! setup_path_type {
    ($name:ident) => {
        impl $name {
            fn as_path(&self) -> &Path {
                Path::new(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                self.as_path()
            }
        }
    };
}

setup_path_type!(KvasirShellProfilePath);
setup_path_type!(KvasirOpenCodeConfigPath);
setup_path_type!(KvasirOpenCodeEnvPath);
setup_path_type!(KvasirRepoHookPath);

#[cfg(test)]
impl KvasirOpenCodeEnvPath {
    fn missing_backup_path(&self) -> PathBuf {
        managed_state::missing_backup_path(self.as_path())
    }
}

impl KvasirRawBodyDirectory {
    fn to_core(&self) -> RawBodyDirectory {
        RawBodyDirectory::new(PathBuf::from(&self.0))
    }
}

impl From<String> for KvasirRawBodyDirectory {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<KvasirRawBodyDirectory> for String {
    fn from(value: KvasirRawBodyDirectory) -> Self {
        value.0
    }
}

impl KvasirOtlpEndpoint {
    fn to_core(&self) -> KvasirEndpoint {
        KvasirEndpoint::new(self.0.clone())
    }
}

impl From<String> for KvasirOtlpEndpoint {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<KvasirOtlpEndpoint> for String {
    fn from(value: KvasirOtlpEndpoint) -> Self {
        value.0
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
    fn configure_harness_telemetry_applies_all_managed_files_with_backups_idempotently()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::create_dir_all(temp.path().join(".claude"))?;
        fs::create_dir_all(temp.path().join(".config/opencode"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;
        fs::write(
            config.claude_settings_path.as_path(),
            "{\n  \"theme\": \"dark\"\n}\n",
        )?;
        fs::write(
            config.copilot_profile_path.as_path(),
            "alias gs='git status'\n",
        )?;
        fs::write(
            config.opencode_config_path.as_path(),
            "{\n  \"theme\": \"system\"\n}\n",
        )?;
        fs::write(config.zsh_profile_path.as_path(), "export EDITOR='vim'\n")?;
        fs::write(config.bash_profile_path.as_path(), "alias ll='ls -la'\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        assert!(fs::read_to_string(config.codex_config_path.as_path())?.contains("[otel]"));
        assert!(
            fs::read_to_string(config.claude_settings_path.as_path())?
                .contains("CLAUDE_CODE_ENABLE_TELEMETRY")
        );
        assert!(
            fs::read_to_string(config.copilot_profile_path.as_path())?
                .contains("BEGIN KVASIR MANAGED COPILOT OTEL")
        );
        assert!(
            fs::read_to_string(config.opencode_config_path.as_path())?
                .contains("\"openTelemetry\": true")
        );
        assert!(
            fs::read_to_string(config.opencode_env_path.as_path())?
                .contains("OTEL_EXPORTER_OTLP_ENDPOINT")
        );
        assert!(
            fs::read_to_string(config.zsh_profile_path.as_path())?
                .contains("BEGIN KVASIR MANAGED REPO OTEL")
        );
        assert!(
            fs::read_to_string(config.bash_profile_path.as_path())?
                .contains("BEGIN KVASIR MANAGED REPO OTEL")
        );
        assert!(fs::read_to_string(config.zsh_repo_hook_path.as_path())?.contains("add-zsh-hook"));
        assert!(
            fs::read_to_string(config.bash_repo_hook_path.as_path())?.contains("PROMPT_COMMAND")
        );

        assert_eq!(
            fs::read_to_string(config.codex_config_path.backup_path())?,
            "model = \"gpt-5\"\n"
        );
        assert_eq!(
            fs::read_to_string(config.claude_settings_path.backup_path())?,
            "{\n  \"theme\": \"dark\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(config.opencode_env_path.missing_backup_path())?,
            ""
        );

        let first_apply_snapshot = managed_setup_snapshot(&config)?;
        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        assert_eq!(managed_setup_snapshot(&config)?, first_apply_snapshot);
        assert_eq!(*credential.write_count.borrow(), 1);

        Ok(())
    }

    #[test]
    fn resolve_bearer_token_uses_existing_setup_secret() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        let first = resolve_kvasir_bearer_token_with_credential(config.clone(), &credential, None)?;
        let second = resolve_kvasir_bearer_token_with_credential(config, &credential, None)?;

        assert_eq!(String::from(first.clone()).len(), 64);
        assert_eq!(first, second);
        assert_eq!(*credential.write_count.borrow(), 2);
        Ok(())
    }

    #[test]
    fn resolve_bearer_token_prefers_environment_override() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        let token = resolve_kvasir_bearer_token_with_credential(
            config,
            &credential,
            Some("operator-token".to_owned()),
        )?;

        assert_eq!(String::from(token), "operator-token");
        assert_eq!(*credential.write_count.borrow(), 0);
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_restores_backups_and_removes_created_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::create_dir_all(temp.path().join(".claude"))?;
        fs::create_dir_all(temp.path().join(".config/opencode"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;
        fs::write(
            config.claude_settings_path.as_path(),
            "{\n  \"theme\": \"dark\"\n}\n",
        )?;
        fs::write(
            config.copilot_profile_path.as_path(),
            "alias gs='git status'\n",
        )?;
        fs::write(
            config.opencode_config_path.as_path(),
            "{\n  \"theme\": \"system\"\n}\n",
        )?;
        fs::write(config.zsh_profile_path.as_path(), "export EDITOR='vim'\n")?;
        fs::write(config.bash_profile_path.as_path(), "alias ll='ls -la'\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        uninstall_kvasir_harness_telemetry(config.clone())?;

        assert_eq!(
            fs::read_to_string(config.codex_config_path.as_path())?,
            "model = \"gpt-5\"\n"
        );
        assert_eq!(
            fs::read_to_string(config.claude_settings_path.as_path())?,
            "{\n  \"theme\": \"dark\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(config.copilot_profile_path.as_path())?,
            "alias gs='git status'\n"
        );
        assert_eq!(
            fs::read_to_string(config.opencode_config_path.as_path())?,
            "{\n  \"theme\": \"system\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(config.zsh_profile_path.as_path())?,
            "export EDITOR='vim'\n"
        );
        assert_eq!(
            fs::read_to_string(config.bash_profile_path.as_path())?,
            "alias ll='ls -la'\n"
        );
        assert!(!config.opencode_env_path.as_path().exists());
        assert!(!config.zsh_repo_hook_path.as_path().exists());
        assert!(!config.bash_repo_hook_path.as_path().exists());
        assert!(!config.codex_config_path.backup_path().exists());
        assert!(!config.opencode_env_path.missing_backup_path().exists());

        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_refuses_to_overwrite_user_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::write(
            config.codex_config_path.as_path(),
            "model = \"gpt-5\"\n# user edit after setup\n",
        )?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(
            fs::read_to_string(config.codex_config_path.as_path())?
                .contains("user edit after setup")
        );
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_errors_when_restore_state_is_incomplete()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::remove_file(config.codex_config_path.backup_path())?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(fs::read_to_string(config.codex_config_path.as_path())?.contains("[otel]"));
        assert!(managed_state::installed_path(config.codex_config_path.as_path()).exists());
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_refuses_when_installed_state_is_missing_after_user_edit()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::remove_file(managed_state::installed_path(
            config.codex_config_path.as_path(),
        ))?;
        fs::write(
            config.codex_config_path.as_path(),
            "model = \"gpt-5\"\n# user edit after setup\n",
        )?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(
            fs::read_to_string(config.codex_config_path.as_path())?
                .contains("user edit after setup")
        );
        assert!(config.codex_config_path.backup_path().exists());
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_refuses_to_delete_created_file_when_installed_state_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::remove_file(managed_state::installed_path(
            config.opencode_env_path.as_path(),
        ))?;
        fs::write(
            config.opencode_env_path.as_path(),
            "OTEL_EXPORTER_OTLP_ENDPOINT='http://edited.example/v1/metrics'\n",
        )?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(fs::read_to_string(config.opencode_env_path.as_path())?.contains("edited.example"));
        assert!(config.opencode_env_path.missing_backup_path().exists());
        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_refuses_incomplete_existing_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::remove_file(config.codex_config_path.backup_path())?;

        let error = configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)
            .unwrap_err();

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert!(!config.codex_config_path.backup_path().exists());
        assert!(managed_state::installed_path(config.codex_config_path.as_path()).exists());
        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_preserves_codex_user_edits_on_rerun()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::write(
            config.codex_config_path.as_path(),
            "model = \"gpt-5\"\n# user edit after setup\n",
        )?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let generated = fs::read_to_string(config.codex_config_path.as_path())?;

        assert!(generated.contains("user edit after setup"));
        assert!(generated.contains("# BEGIN KVASIR MANAGED CODEX OTEL"));
        assert!(managed_file_is_current(
            config.codex_config_path.as_path(),
            &generated
        )?);
        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_refuses_contradictory_restore_markers()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::write(
            managed_state::missing_backup_path(config.codex_config_path.as_path()),
            "",
        )?;

        let error = configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)
            .unwrap_err();

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert!(config.codex_config_path.backup_path().exists());
        assert!(managed_state::missing_backup_path(config.codex_config_path.as_path()).exists());
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_refuses_contradictory_restore_markers()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::write(
            managed_state::missing_backup_path(config.codex_config_path.as_path()),
            "",
        )?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(fs::read_to_string(config.codex_config_path.as_path())?.contains("[otel]"));
        assert!(config.codex_config_path.backup_path().exists());
        assert!(managed_state::missing_backup_path(config.codex_config_path.as_path()).exists());
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_refuses_deleted_empty_original_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::remove_file(config.codex_config_path.as_path())?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(!config.codex_config_path.as_path().exists());
        assert!(config.codex_config_path.backup_path().exists());
        Ok(())
    }

    #[test]
    fn uninstall_harness_telemetry_retry_cleans_state_after_prior_restore()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        uninstall_kvasir_harness_telemetry(config.clone())?;

        assert_eq!(
            fs::read_to_string(config.codex_config_path.as_path())?,
            "model = \"gpt-5\"\n"
        );
        assert!(!config.codex_config_path.backup_path().exists());
        assert!(!managed_state::installed_path(config.codex_config_path.as_path()).exists());
        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_restores_files_when_late_apply_write_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::create_dir_all(temp.path().join(".claude"))?;
        fs::create_dir_all(temp.path().join(".config/opencode"))?;
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;
        fs::write(
            config.claude_settings_path.as_path(),
            "{\n  \"theme\": \"dark\"\n}\n",
        )?;
        fs::write(
            config.copilot_profile_path.as_path(),
            "alias gs='git status'\n",
        )?;
        fs::write(
            config.opencode_config_path.as_path(),
            "{\n  \"theme\": \"system\"\n}\n",
        )?;
        fs::create_dir(config.opencode_env_path.as_path())?;

        let error = configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)
            .unwrap_err();

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert_eq!(*credential.write_count.borrow(), 1);
        assert!(credential.password.borrow().is_none());
        assert_eq!(
            fs::read_to_string(config.codex_config_path.as_path())?,
            "model = \"gpt-5\"\n"
        );
        assert_eq!(
            fs::read_to_string(config.claude_settings_path.as_path())?,
            "{\n  \"theme\": \"dark\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(config.copilot_profile_path.as_path())?,
            "alias gs='git status'\n"
        );
        assert_eq!(
            fs::read_to_string(config.opencode_config_path.as_path())?,
            "{\n  \"theme\": \"system\"\n}\n"
        );
        assert!(config.opencode_env_path.as_path().is_dir());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn configure_and_uninstall_harness_telemetry_preserve_symlinked_managed_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir_all(&dotfiles)?;
        fs::create_dir_all(temp.path().join(".codex"))?;
        let zsh_target = dotfiles.join("zshrc");
        fs::write(&zsh_target, "export EDITOR='vim'\n")?;
        symlink(&zsh_target, config.zsh_profile_path.as_path())?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        assert!(
            fs::symlink_metadata(config.zsh_profile_path.as_path())?
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(&zsh_target)?.contains("BEGIN KVASIR MANAGED REPO OTEL"),
            "setup should edit the symlink target"
        );

        uninstall_kvasir_harness_telemetry(config.clone())?;

        assert!(
            fs::symlink_metadata(config.zsh_profile_path.as_path())?
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&zsh_target)?, "export EDITOR='vim'\n");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_harness_telemetry_refuses_retargeted_symlink()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir_all(&dotfiles)?;
        let original_target = dotfiles.join("zshrc");
        let other_target = dotfiles.join("other-zshrc");
        fs::write(&original_target, "export EDITOR='vim'\n")?;
        symlink(&original_target, config.zsh_profile_path.as_path())?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let installed_contents = fs::read_to_string(managed_state::installed_path(
            config.zsh_profile_path.as_path(),
        ))?;
        fs::write(&other_target, installed_contents)?;
        fs::remove_file(config.zsh_profile_path.as_path())?;
        symlink(&other_target, config.zsh_profile_path.as_path())?;

        let error = uninstall_kvasir_harness_telemetry(config.clone()).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryUninstallConflict
        ));
        assert!(
            fs::read_to_string(&other_target)?.contains("BEGIN KVASIR MANAGED REPO OTEL"),
            "uninstall must not restore backup into the retargeted symlink target"
        );
        assert!(fs::read_to_string(&original_target)?.contains("BEGIN KVASIR MANAGED REPO OTEL"));
        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_writes_codex_config_with_persisted_setup_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let generated = fs::read_to_string(config.codex_config_path.as_path())?;

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
        assert_eq!(
            fs::read_to_string(config.codex_config_path.as_path())?,
            generated
        );

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_does_not_commit_token_when_config_generation_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(
            config.codex_config_path.as_path(),
            r#"  # BEGIN KVASIR MANAGED CODEX OTEL
[otel]
metrics_exporter = { otlp-http = { endpoint = "http://old.example/v1/metrics", protocol = "binary" } }
# END KVASIR MANAGED CODEX OTEL
"#,
        )?;

        let error =
            configure_kvasir_harness_telemetry_with_credential(config, &credential).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryInvalidCodexConfig
        ));
        assert_eq!(*credential.write_count.borrow(), 0);
        assert!(credential.password.borrow().is_none());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_adopts_existing_codex_otel_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(temp.path().join(".codex"))?;
        fs::write(
            config.codex_config_path.as_path(),
            "[otel]\nexporter = \"none\"\n",
        )?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let generated = fs::read_to_string(config.codex_config_path.as_path())?;

        assert!(generated.contains("# BEGIN KVASIR MANAGED CODEX OTEL"));
        assert!(generated.contains("http://127.0.0.1:4318/v1/logs"));
        assert!(!generated.contains("exporter = \"none\""));
        assert_eq!(*credential.write_count.borrow(), 1);
        assert!(credential.password.borrow().is_some());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_reports_invalid_claude_settings()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(config.claude_settings_path.as_path().parent().unwrap())?;
        fs::write(config.claude_settings_path.as_path(), "{not json")?;

        let error =
            configure_kvasir_harness_telemetry_with_credential(config, &credential).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryInvalidClaudeSettings
        ));
        assert_eq!(*credential.write_count.borrow(), 0);
        assert!(credential.password.borrow().is_none());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_reports_invalid_opencode_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(config.opencode_config_path.as_path().parent().unwrap())?;
        fs::write(config.opencode_config_path.as_path(), "{not json")?;

        let error =
            configure_kvasir_harness_telemetry_with_credential(config, &credential).unwrap_err();

        assert!(matches!(
            error,
            KvasirClientError::HarnessTelemetryInvalidOpenCodeConfig
        ));
        assert_eq!(*credential.write_count.borrow(), 0);
        assert!(credential.password.borrow().is_none());

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_keeps_shell_profile_errors_generic()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::default();
        let config = full_harness_setup_config(temp.path());
        fs::create_dir_all(config.zsh_profile_path.as_path().parent().unwrap())?;
        fs::write(
            config.zsh_profile_path.as_path(),
            "# BEGIN KVASIR MANAGED REPO OTEL\n",
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
            codex_config_path: codex_config_path(temp.path().join(".codex")),
            ..full_harness_setup_config(temp.path())
        };
        fs::create_dir(config.codex_config_path.as_path())?;

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
            codex_config_path: codex_config_path(codex_dir.join("config.toml")),
            ..full_harness_setup_config(temp.path())
        };
        let existing_config = "model = \"gpt-5\"\n";
        fs::write(config.codex_config_path.as_path(), existing_config)?;

        let error = configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)
            .unwrap_err();

        assert!(matches!(error, KvasirClientError::HarnessTelemetrySetup));
        assert_eq!(
            fs::read_to_string(config.codex_config_path.as_path())?,
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
        let config = full_harness_setup_config(temp.path());
        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;
        let previous_password = credential.password.borrow().clone();
        let previous_config = fs::read_to_string(config.codex_config_path.as_path())?;

        let updated_config = KvasirHarnessTelemetrySetup {
            otlp_endpoint: otlp_endpoint("http://127.0.0.1:9999"),
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
            fs::read_to_string(config.codex_config_path.as_path())?,
            previous_config
        );

        Ok(())
    }

    #[test]
    fn configure_harness_telemetry_surfaces_rollback_failure_after_staged_install_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let credential = MemorySetupCredential::failing_on_write_number(3);
        let config = full_harness_setup_config(temp.path());
        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let updated_config = KvasirHarnessTelemetrySetup {
            otlp_endpoint: otlp_endpoint("http://127.0.0.1:9999"),
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
            codex_config_path: codex_config_path(codex_dir.join("config.toml")),
            ..full_harness_setup_config(temp.path())
        };
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let generated = fs::read_to_string(config.codex_config_path.as_path())?;
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
        let config = full_harness_setup_config(temp.path());

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let mode = fs::metadata(config.codex_config_path.as_path())?
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
            codex_config_path: codex_config_path(codex_dir.join("config.toml")),
            ..full_harness_setup_config(temp.path())
        };
        fs::write(config.codex_config_path.as_path(), "model = \"gpt-5\"\n")?;
        fs::set_permissions(
            config.codex_config_path.as_path(),
            fs::Permissions::from_mode(0o600),
        )?;

        configure_kvasir_harness_telemetry_with_credential(config.clone(), &credential)?;

        let mode = fs::metadata(config.codex_config_path.as_path())?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        Ok(())
    }

    fn codex_config_path(path: impl AsRef<Path>) -> KvasirCodexConfigPath {
        KvasirCodexConfigPath(path.as_ref().display().to_string())
    }

    fn claude_settings_path(path: impl AsRef<Path>) -> KvasirClaudeSettingsPath {
        KvasirClaudeSettingsPath(path.as_ref().display().to_string())
    }

    fn raw_body_directory(path: impl AsRef<Path>) -> KvasirRawBodyDirectory {
        KvasirRawBodyDirectory(path.as_ref().display().to_string())
    }

    fn otlp_endpoint(endpoint: &str) -> KvasirOtlpEndpoint {
        KvasirOtlpEndpoint(endpoint.to_owned())
    }

    fn full_harness_setup_config(root: &Path) -> KvasirHarnessTelemetrySetup {
        KvasirHarnessTelemetrySetup {
            codex_config_path: codex_config_path(root.join(".codex/config.toml")),
            claude_settings_path: claude_settings_path(root.join(".claude/settings.json")),
            copilot_profile_path: shell_profile_path(root.join(".profile")),
            opencode_config_path: opencode_config_path(root.join(".config/opencode/opencode.json")),
            opencode_env_path: opencode_env_path(root.join(".config/opencode/kvasir.env")),
            zsh_profile_path: shell_profile_path(root.join(".zshrc")),
            bash_profile_path: shell_profile_path(root.join(".bashrc")),
            zsh_repo_hook_path: repo_hook_path(root.join(".kvasir/repo-hook.zsh")),
            bash_repo_hook_path: repo_hook_path(root.join(".kvasir/repo-hook.bash")),
            raw_body_directory: raw_body_directory(root.join("raw-bodies")),
            otlp_endpoint: otlp_endpoint("http://127.0.0.1:4318"),
        }
    }

    fn managed_setup_snapshot(
        config: &KvasirHarnessTelemetrySetup,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let paths = [
            config.codex_config_path.as_path(),
            config.claude_settings_path.as_path(),
            config.copilot_profile_path.as_path(),
            config.opencode_config_path.as_path(),
            config.opencode_env_path.as_path(),
            config.zsh_profile_path.as_path(),
            config.bash_profile_path.as_path(),
            config.zsh_repo_hook_path.as_path(),
            config.bash_repo_hook_path.as_path(),
        ];
        paths
            .into_iter()
            .map(|path| {
                Ok((
                    path.display().to_string(),
                    fs::read_to_string(path).unwrap_or_default(),
                ))
            })
            .collect()
    }

    fn shell_profile_path(path: impl AsRef<Path>) -> KvasirShellProfilePath {
        KvasirShellProfilePath(path.as_ref().display().to_string())
    }

    fn opencode_config_path(path: impl AsRef<Path>) -> KvasirOpenCodeConfigPath {
        KvasirOpenCodeConfigPath(path.as_ref().display().to_string())
    }

    fn opencode_env_path(path: impl AsRef<Path>) -> KvasirOpenCodeEnvPath {
        KvasirOpenCodeEnvPath(path.as_ref().display().to_string())
    }

    fn repo_hook_path(path: impl AsRef<Path>) -> KvasirRepoHookPath {
        KvasirRepoHookPath(path.as_ref().display().to_string())
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
