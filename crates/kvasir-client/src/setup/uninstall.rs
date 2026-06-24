use std::fs;
use std::path::{Path, PathBuf};

use crate::error::KvasirClientError;

use super::KvasirHarnessTelemetrySetup;
use super::fs_atomic::{current_permissions, read_optional_string, replace_file_with_permissions};
use super::managed_state::{ManagedFileState, cleanup_state_files, target_matches_state};

pub(super) fn setup_managed_paths(config: &KvasirHarnessTelemetrySetup) -> Vec<PathBuf> {
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

pub(super) fn uninstall_managed_file(path: &Path) -> Result<(), KvasirClientError> {
    let state = ManagedFileState::load(path)?;
    if !state.installed_exists
        && !state.backup_exists
        && !state.missing_exists
        && !state.target_exists
    {
        return Ok(());
    }
    if state.backup_exists == state.missing_exists {
        return Err(KvasirClientError::HarnessTelemetryUninstallConflict);
    }
    if !target_matches_state(path, &state)? {
        return Err(KvasirClientError::HarnessTelemetryUninstallConflict);
    }

    let installed_contents = read_installed_contents(&state)?;
    if installed_contents.is_none() {
        if state.backup_exists {
            let current_contents =
                read_optional_string(path).map_err(|_| KvasirClientError::Filesystem)?;
            return handle_backup_without_installed_state(path, &state, &current_contents);
        }
        if state.missing_exists {
            return handle_missing_without_installed_state(path, &state);
        }
    }

    if let Some(installed_contents) = installed_contents.as_deref() {
        let current_contents =
            read_optional_string(path).map_err(|_| KvasirClientError::Filesystem)?;
        if current_contents != installed_contents {
            return handle_uninstall_mismatch(path, &state, &current_contents);
        }
    }

    if state.backup_exists {
        return restore_from_backup(path, &state);
    }
    if state.missing_exists {
        return remove_setup_created_file(path, &state);
    }
    if state.installed_exists {
        return Err(KvasirClientError::Filesystem);
    }
    Ok(())
}

fn read_installed_contents(state: &ManagedFileState) -> Result<Option<String>, KvasirClientError> {
    if !state.installed_exists {
        return Ok(None);
    }
    fs::read_to_string(&state.installed_path)
        .map(Some)
        .map_err(|_| KvasirClientError::Filesystem)
}

fn handle_uninstall_mismatch(
    path: &Path,
    state: &ManagedFileState,
    current_contents: &str,
) -> Result<(), KvasirClientError> {
    if state.backup_exists
        && path.exists()
        && current_contents == read_backup_contents(state)?.as_str()
    {
        return cleanup_state_files(state);
    }
    if state.missing_exists && !path.exists() {
        return cleanup_state_files(state);
    }
    Err(KvasirClientError::HarnessTelemetryUninstallConflict)
}

fn handle_backup_without_installed_state(
    path: &Path,
    state: &ManagedFileState,
    current_contents: &str,
) -> Result<(), KvasirClientError> {
    if path.exists() && current_contents == read_backup_contents(state)?.as_str() {
        return cleanup_state_files(state);
    }
    if state.missing_exists && !path.exists() {
        return cleanup_state_files(state);
    }
    Err(KvasirClientError::HarnessTelemetryUninstallConflict)
}

fn handle_missing_without_installed_state(
    path: &Path,
    state: &ManagedFileState,
) -> Result<(), KvasirClientError> {
    if !path.exists() {
        return cleanup_state_files(state);
    }
    Err(KvasirClientError::HarnessTelemetryUninstallConflict)
}

fn restore_from_backup(path: &Path, state: &ManagedFileState) -> Result<(), KvasirClientError> {
    let backup_contents = read_backup_contents(state)?;
    replace_file_with_permissions(
        path,
        &backup_contents,
        current_permissions(&state.backup_path),
    )
    .map_err(|_| KvasirClientError::Filesystem)?;
    cleanup_state_files(state)
}

fn remove_setup_created_file(
    path: &Path,
    state: &ManagedFileState,
) -> Result<(), KvasirClientError> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(KvasirClientError::Filesystem),
    }
    cleanup_state_files(state)
}

fn read_backup_contents(state: &ManagedFileState) -> Result<String, KvasirClientError> {
    fs::read_to_string(&state.backup_path).map_err(|_| KvasirClientError::Filesystem)
}
