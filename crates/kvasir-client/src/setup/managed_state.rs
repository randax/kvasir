use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::error::KvasirClientError;

use super::fs_atomic::{
    current_permissions, private_file_permissions, read_optional_string, replace_file,
    writable_parent,
};

pub(super) struct ManagedFileState {
    pub(super) backup_path: PathBuf,
    pub(super) missing_backup_path: PathBuf,
    pub(super) installed_path: PathBuf,
    pub(super) target_path: PathBuf,
    pub(super) installed_exists: bool,
    pub(super) backup_exists: bool,
    pub(super) missing_exists: bool,
    pub(super) target_exists: bool,
}

impl ManagedFileState {
    pub(super) fn load(path: &Path) -> Result<Self, KvasirClientError> {
        Self::load_io(path).map_err(|_| KvasirClientError::Filesystem)
    }

    fn load_io(path: &Path) -> std::io::Result<Self> {
        let backup_path = backup_path(path);
        let missing_backup_path = missing_backup_path(path);
        let installed_path = installed_path(path);
        let target_path = target_path(path);
        Ok(Self {
            backup_exists: state_path_exists(&backup_path)?,
            missing_exists: state_path_exists(&missing_backup_path)?,
            installed_exists: state_path_exists(&installed_path)?,
            target_exists: state_path_exists(&target_path)?,
            backup_path,
            missing_backup_path,
            installed_path,
            target_path,
        })
    }

    fn has_complete_install_state(&self) -> bool {
        self.installed_exists && self.target_exists && (self.backup_exists ^ self.missing_exists)
    }
}

pub(super) fn backup_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-backup")
}

pub(super) fn missing_backup_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-missing")
}

pub(super) fn installed_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-installed")
}

fn target_path(path: &Path) -> PathBuf {
    sibling_setup_state_path(path, "kvasir-target")
}

fn sibling_setup_state_path(path: &Path, suffix: &str) -> PathBuf {
    let parent = writable_parent(path);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    parent.join(format!(".{file_name}.{suffix}"))
}

pub(super) fn ensure_backup_state(path: &Path) -> std::io::Result<()> {
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

pub(super) fn ensure_installable_state(path: &Path) -> std::io::Result<()> {
    let state = ManagedFileState::load_io(path)?;
    if !state.installed_exists
        && !state.backup_exists
        && !state.missing_exists
        && !state.target_exists
    {
        return ensure_backup_state(path);
    }
    if state.has_complete_install_state() {
        ensure_current_matches_installed(path, &state)?;
        ensure_target_matches_state(path, &state)?;
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "setup state is incomplete",
    ))
}

pub(super) fn write_installed_state(path: &Path, contents: &str) -> std::io::Result<()> {
    let state_path = installed_path(path);
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !state_path_exists(&state_path)? {
        create_state_file_new(&state_path, contents.as_bytes(), private_file_permissions())?;
    } else {
        replace_file(&state_path, contents)?;
    }
    write_target_state(path)
}

pub(super) fn managed_file_is_current(
    path: &Path,
    desired_contents: &str,
) -> Result<bool, KvasirClientError> {
    if read_optional_string(path).map_err(|_| KvasirClientError::Filesystem)? != desired_contents {
        return Ok(false);
    }
    let state = ManagedFileState::load(path)?;
    if !state.has_complete_install_state() {
        return Ok(false);
    }
    if !target_matches_state(path, &state)? {
        return Ok(false);
    }
    let installed_contents =
        fs::read_to_string(state.installed_path).map_err(|_| KvasirClientError::Filesystem)?;
    Ok(installed_contents == desired_contents)
}

pub(super) fn cleanup_state_files(state: &ManagedFileState) -> Result<(), KvasirClientError> {
    if state.backup_exists {
        fs::remove_file(&state.backup_path).map_err(|_| KvasirClientError::Filesystem)?;
    }
    if state.missing_exists {
        fs::remove_file(&state.missing_backup_path).map_err(|_| KvasirClientError::Filesystem)?;
    }
    if state.installed_exists {
        fs::remove_file(&state.installed_path).map_err(|_| KvasirClientError::Filesystem)?;
    }
    if state.target_exists {
        fs::remove_file(&state.target_path).map_err(|_| KvasirClientError::Filesystem)?;
    }
    Ok(())
}

pub(super) fn target_matches_state(
    path: &Path,
    state: &ManagedFileState,
) -> Result<bool, KvasirClientError> {
    if !state.target_exists {
        return Err(KvasirClientError::Filesystem);
    }
    let expected_target =
        fs::read_to_string(&state.target_path).map_err(|_| KvasirClientError::Filesystem)?;
    let current_target = current_symlink_target(path).map_err(|_| KvasirClientError::Filesystem)?;
    Ok(current_target == expected_target)
}

fn ensure_current_matches_installed(path: &Path, state: &ManagedFileState) -> std::io::Result<()> {
    let current_contents = read_optional_string(path)?;
    let installed_contents = fs::read_to_string(&state.installed_path)?;
    if current_contents == installed_contents {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "managed file has changed since setup",
    ))
}

fn ensure_target_matches_state(path: &Path, state: &ManagedFileState) -> std::io::Result<()> {
    let expected_target = fs::read_to_string(&state.target_path)?;
    let current_target = current_symlink_target(path)?;
    if current_target == expected_target {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "managed file symlink target changed since setup",
    ))
}

fn write_target_state(path: &Path) -> std::io::Result<()> {
    let state_path = target_path(path);
    let contents = current_symlink_target(path)?;
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !state_path_exists(&state_path)? {
        return create_state_file_new(&state_path, contents.as_bytes(), private_file_permissions());
    }
    replace_file(&state_path, &contents)
}

fn current_symlink_target(path: &Path) -> std::io::Result<String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Ok(path.canonicalize()?.to_string_lossy().into_owned())
        }
        Ok(_) => Ok(String::new()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
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
