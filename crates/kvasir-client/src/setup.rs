use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use kvasir_core::{
    ClaudeCodeSettings, CodexConfigToml, CopilotShellProfile, KvasirEndpoint, OpenCodeSetup,
    RawBodyDirectory, RepoInjectionShell, RepoInjectionShellHook, RepoInjectionShellProfile,
    SetupConfig, SetupSecretSource,
};
#[cfg(test)]
use kvasir_core::{SetupCredential, prepare_setup_config};

use crate::error::KvasirClientError;

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
        .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
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
pub fn uninstall_kvasir_harness_telemetry(
    config: KvasirHarnessTelemetrySetup,
) -> Result<(), KvasirClientError> {
    for path in setup_managed_paths(&config) {
        uninstall_managed_file(&path)?;
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
        config.otlp_endpoint.to_core(),
        config.raw_body_directory.to_core(),
    )
    .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
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
    Unchanged {
        target_path: PathBuf,
        desired_contents: String,
    },
    Replacement {
        target_path: PathBuf,
        temp_path: PathBuf,
        previous_config: PreviousCodexConfig,
        desired_contents: String,
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
            Self::Unchanged {
                target_path,
                desired_contents,
            } => ensure_backup_state(&target_path)
                .and_then(|_| write_installed_state(&target_path, &desired_contents))
                .map_err(|_| ConfigInstallError::ConfigPreserved),
            Self::Replacement {
                target_path,
                temp_path,
                previous_config,
                desired_contents,
            } => {
                if ensure_backup_state(&target_path).is_err() {
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

struct GeneratedHarnessFile {
    target_path: PathBuf,
    contents: String,
}

fn prepare_generated_harness_files(
    config: &KvasirHarnessTelemetrySetup,
    setup_config: &SetupConfig,
) -> Result<Vec<GeneratedHarnessFile>, KvasirClientError> {
    let claude_settings_path = config.claude_settings_path.as_path().to_path_buf();
    let copilot_profile_path = config.copilot_profile_path.as_path().to_path_buf();
    let opencode_config_path = config.opencode_config_path.as_path().to_path_buf();
    let opencode_env_path = config.opencode_env_path.as_path().to_path_buf();
    let zsh_profile_path = config.zsh_profile_path.as_path().to_path_buf();
    let bash_profile_path = config.bash_profile_path.as_path().to_path_buf();
    let zsh_repo_hook_path = config.zsh_repo_hook_path.as_path().to_path_buf();
    let bash_repo_hook_path = config.bash_repo_hook_path.as_path().to_path_buf();

    let claude_settings =
        ClaudeCodeSettings::generate(&read_optional_string(&claude_settings_path)?, setup_config)
            .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let copilot_profile =
        CopilotShellProfile::generate(&read_optional_string(&copilot_profile_path)?, setup_config)
            .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let opencode_setup =
        OpenCodeSetup::generate(&read_optional_string(&opencode_config_path)?, setup_config)
            .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let zsh_profile = RepoInjectionShellProfile::generate(
        &read_optional_string(&zsh_profile_path)?,
        &zsh_repo_hook_path,
    )
    .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    let bash_profile = RepoInjectionShellProfile::generate(
        &read_optional_string(&bash_profile_path)?,
        &bash_repo_hook_path,
    )
    .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;

    Ok(vec![
        GeneratedHarnessFile {
            target_path: claude_settings_path,
            contents: claude_settings.as_str().to_owned(),
        },
        GeneratedHarnessFile {
            target_path: copilot_profile_path,
            contents: copilot_profile.as_str().to_owned(),
        },
        GeneratedHarnessFile {
            target_path: opencode_config_path,
            contents: opencode_setup.opencode_json().to_owned(),
        },
        GeneratedHarnessFile {
            target_path: opencode_env_path,
            contents: opencode_env_file(&opencode_setup),
        },
        GeneratedHarnessFile {
            target_path: zsh_profile_path,
            contents: zsh_profile.as_str().to_owned(),
        },
        GeneratedHarnessFile {
            target_path: bash_profile_path,
            contents: bash_profile.as_str().to_owned(),
        },
        GeneratedHarnessFile {
            target_path: zsh_repo_hook_path,
            contents: RepoInjectionShellHook::generate(RepoInjectionShell::Zsh)
                .as_str()
                .to_owned(),
        },
        GeneratedHarnessFile {
            target_path: bash_repo_hook_path,
            contents: RepoInjectionShellHook::generate(RepoInjectionShell::Bash)
                .as_str()
                .to_owned(),
        },
    ])
}

fn install_generated_harness_files(files: Vec<GeneratedHarnessFile>) -> std::io::Result<()> {
    for file in files {
        install_generated_harness_file(file)?;
    }
    Ok(())
}

fn generated_harness_files_are_current(
    files: &[GeneratedHarnessFile],
) -> Result<bool, KvasirClientError> {
    for file in files {
        if !managed_file_is_current(&file.target_path, &file.contents)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn managed_file_is_current(path: &Path, desired_contents: &str) -> Result<bool, KvasirClientError> {
    if read_optional_string(path)? != desired_contents {
        return Ok(false);
    }
    if !state_path_exists(&installed_path(path)).map_err(|_| KvasirClientError::Filesystem)?
        || !backup_state_exists(path).map_err(|_| KvasirClientError::Filesystem)?
    {
        return Ok(false);
    }
    let installed_contents =
        fs::read_to_string(installed_path(path)).map_err(|_| KvasirClientError::Filesystem)?;
    Ok(installed_contents == desired_contents)
}

fn backup_state_exists(path: &Path) -> std::io::Result<bool> {
    Ok(state_path_exists(&backup_path(path))? || state_path_exists(&missing_backup_path(path))?)
}

fn install_generated_harness_file(file: GeneratedHarnessFile) -> std::io::Result<()> {
    if read_optional_string_io(&file.target_path)? == file.contents {
        ensure_backup_state(&file.target_path)?;
        write_installed_state(&file.target_path, &file.contents)?;
        return Ok(());
    }
    ensure_backup_state(&file.target_path)?;
    replace_file(&file.target_path, &file.contents)?;
    write_installed_state(&file.target_path, &file.contents)
}

fn setup_managed_paths(config: &KvasirHarnessTelemetrySetup) -> Vec<PathBuf> {
    vec![
        config.codex_config_path.as_path().to_path_buf(),
        config.claude_settings_path.as_path().to_path_buf(),
        config.copilot_profile_path.as_path().to_path_buf(),
        config.opencode_config_path.as_path().to_path_buf(),
        config.opencode_env_path.as_path().to_path_buf(),
        config.zsh_profile_path.as_path().to_path_buf(),
        config.bash_profile_path.as_path().to_path_buf(),
        config.zsh_repo_hook_path.as_path().to_path_buf(),
        config.bash_repo_hook_path.as_path().to_path_buf(),
    ]
}

fn uninstall_managed_file(path: &Path) -> Result<(), KvasirClientError> {
    let backup_path = backup_path(path);
    let missing_backup_path = missing_backup_path(path);
    let installed_path = installed_path(path);
    let installed_state_exists =
        state_path_exists(&installed_path).map_err(|_| KvasirClientError::Filesystem)?;
    let backup_state_exists =
        state_path_exists(&backup_path).map_err(|_| KvasirClientError::Filesystem)?;
    let missing_state_exists =
        state_path_exists(&missing_backup_path).map_err(|_| KvasirClientError::Filesystem)?;
    if installed_state_exists {
        let installed_contents =
            fs::read_to_string(&installed_path).map_err(|_| KvasirClientError::Filesystem)?;
        let current_contents = read_optional_string(path)?;
        if current_contents != installed_contents {
            if backup_state_exists {
                let backup_contents =
                    fs::read_to_string(&backup_path).map_err(|_| KvasirClientError::Filesystem)?;
                if current_contents == backup_contents {
                    fs::remove_file(&backup_path).map_err(|_| KvasirClientError::Filesystem)?;
                    let _ = fs::remove_file(&missing_backup_path);
                    let _ = fs::remove_file(&installed_path);
                    return Ok(());
                }
            }
            if missing_state_exists && !path.exists() {
                fs::remove_file(&missing_backup_path).map_err(|_| KvasirClientError::Filesystem)?;
                let _ = fs::remove_file(&backup_path);
                let _ = fs::remove_file(&installed_path);
                return Ok(());
            }
            return Err(KvasirClientError::HarnessTelemetryUninstallConflict);
        }
    }
    if backup_state_exists {
        let backup_contents =
            fs::read_to_string(&backup_path).map_err(|_| KvasirClientError::Filesystem)?;
        replace_file_with_permissions(path, &backup_contents, current_permissions(&backup_path))
            .map_err(|_| KvasirClientError::Filesystem)?;
        fs::remove_file(&backup_path).map_err(|_| KvasirClientError::Filesystem)?;
        let _ = fs::remove_file(&missing_backup_path);
        let _ = fs::remove_file(&installed_path);
        return Ok(());
    }
    if missing_state_exists {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(KvasirClientError::Filesystem),
        }
        fs::remove_file(missing_backup_path).map_err(|_| KvasirClientError::Filesystem)?;
        let _ = fs::remove_file(&installed_path);
    } else if installed_state_exists {
        return Err(KvasirClientError::Filesystem);
    }
    Ok(())
}

fn read_optional_string(path: &Path) -> Result<String, KvasirClientError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err(KvasirClientError::Filesystem),
    }
}

fn read_optional_string_io(path: &Path) -> std::io::Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

fn ensure_backup_state(path: &Path) -> std::io::Result<()> {
    let backup_path = backup_path(path);
    let missing_backup_path = missing_backup_path(path);
    if state_path_exists(&backup_path)? || state_path_exists(&missing_backup_path)? {
        return Ok(());
    }
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let contents = fs::read(path)?;
        create_state_file_new(&backup_path, &contents, current_permissions(path))?;
    } else {
        create_state_file_new(&missing_backup_path, b"", private_file_permissions())?;
    }
    Ok(())
}

fn replace_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let previous_config = match fs::read_to_string(path) {
        Ok(contents) => PreviousCodexConfig::Present {
            contents,
            permissions: current_permissions(path),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PreviousCodexConfig::Missing,
        Err(err) => return Err(err),
    };
    replace_file_with_permissions(path, contents, replacement_permissions(&previous_config))
}

fn replace_file_with_permissions(
    path: &Path,
    contents: &str,
    permissions: Option<fs::Permissions>,
) -> std::io::Result<()> {
    let writable_path = writable_file_path(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = writable_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = write_replacement_file(&writable_path, contents, permissions)?;
    fs::rename(&temp_path, &writable_path)?;
    sync_parent_directory(&writable_path)
}

fn writable_file_path(path: &Path) -> std::io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => path.canonicalize(),
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err),
    }
}

fn backup_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-backup")
}

fn missing_backup_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-missing")
}

fn installed_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-installed")
}

fn sibling_setup_state_path(path: &Path, suffix: &str) -> PathBuf {
    let parent = writable_parent(path);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    parent.join(format!(".{file_name}.{suffix}"))
}

fn create_state_file_new(
    path: &Path,
    contents: &[u8],
    permissions: Option<fs::Permissions>,
) -> std::io::Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "setup state file already exists",
        ));
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    if let Some(permissions) = permissions {
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn write_installed_state(path: &Path, contents: &str) -> std::io::Result<()> {
    let state_path = installed_path(path);
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !state_path_exists(&state_path)? {
        return create_state_file_new(&state_path, contents.as_bytes(), private_file_permissions());
    }
    replace_file(&state_path, contents)
}

fn state_path_exists(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "setup state file must not be a symlink",
        )),
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn opencode_env_file(setup: &OpenCodeSetup) -> String {
    let endpoint = setup.otlp_endpoint_variable();
    let headers = setup.otlp_headers_variable();
    format!(
        "{}={}\n{}={}\n",
        endpoint.key().as_str(),
        shell_single_quote(endpoint.value()),
        headers.key().as_str(),
        shell_single_quote(headers.value())
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
        backup_path(self.as_path())
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
        backup_path(self.as_path())
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
        missing_backup_path(self.as_path())
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
        PreviousCodexConfig::Missing => writable_file_path(path)
            .map_err(|_| ())
            .and_then(|path| fs::remove_file(&path).map(|_| path).map_err(|_| ()))
            .and_then(|path| sync_parent(&path).map_err(|_| ())),
        PreviousCodexConfig::Present {
            contents,
            permissions,
        } => replace_file_with_permissions(path, &contents, permissions).map_err(|_| ()),
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

#[cfg(unix)]
fn private_file_permissions() -> Option<fs::Permissions> {
    Some(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn private_file_permissions() -> Option<fs::Permissions> {
    None
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

        assert!(matches!(error, KvasirClientError::Filesystem));
        assert!(fs::read_to_string(config.codex_config_path.as_path())?.contains("[otel]"));
        assert!(installed_path(config.codex_config_path.as_path()).exists());
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
        assert!(!installed_path(config.codex_config_path.as_path()).exists());
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
