use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[derive(Clone)]
pub(super) enum PreviousFile {
    Missing,
    Present {
        contents: String,
        permissions: Option<fs::Permissions>,
    },
}

impl PreviousFile {
    pub(super) fn contents(&self) -> &str {
        match self {
            Self::Missing => "",
            Self::Present { contents, .. } => contents,
        }
    }
}

pub(super) fn read_previous_file(path: &Path) -> std::io::Result<PreviousFile> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(PreviousFile::Present {
            contents,
            permissions: current_permissions(path),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PreviousFile::Missing),
        Err(err) => Err(err),
    }
}

pub(super) fn read_optional_string(path: &Path) -> std::io::Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

pub(super) fn replace_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let previous_file = read_previous_file(path)?;
    replace_file_with_permissions(path, contents, replacement_permissions(&previous_file))
}

pub(super) fn replace_file_with_permissions(
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

pub(super) fn restore_previous_file(
    path: &Path,
    previous_file: PreviousFile,
    sync_parent: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<(), ()> {
    match previous_file {
        PreviousFile::Missing => writable_file_path(path)
            .map_err(|_| ())
            .and_then(|path| fs::remove_file(&path).map(|_| path).map_err(|_| ()))
            .and_then(|path| sync_parent(&path).map_err(|_| ())),
        PreviousFile::Present {
            contents,
            permissions,
        } => replace_file_with_permissions(path, &contents, permissions).map_err(|_| ()),
    }
}

pub(super) fn write_replacement_file(
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

fn writable_file_path(path: &Path) -> std::io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => path.canonicalize(),
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err),
    }
}

pub(super) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = writable_parent(path);
    File::open(parent)?.sync_all()
}

pub(super) fn writable_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
pub(super) fn current_permissions(path: &Path) -> Option<fs::Permissions> {
    fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .ok()
}

#[cfg(not(unix))]
pub(super) fn current_permissions(path: &Path) -> Option<fs::Permissions> {
    fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .ok()
}

#[cfg(unix)]
pub(super) fn replacement_permissions(previous_file: &PreviousFile) -> Option<fs::Permissions> {
    let mode = match previous_file {
        PreviousFile::Missing => 0o600,
        PreviousFile::Present {
            permissions: Some(permissions),
            ..
        } => permissions.mode() & 0o700,
        PreviousFile::Present {
            permissions: None, ..
        } => 0o600,
    };
    Some(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(super) fn replacement_permissions(previous_file: &PreviousFile) -> Option<fs::Permissions> {
    match previous_file {
        PreviousFile::Missing => None,
        PreviousFile::Present { permissions, .. } => permissions.clone(),
    }
}

#[cfg(unix)]
pub(super) fn private_file_permissions() -> Option<fs::Permissions> {
    Some(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub(super) fn private_file_permissions() -> Option<fs::Permissions> {
    None
}
