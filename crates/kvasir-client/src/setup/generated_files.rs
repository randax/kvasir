use std::path::PathBuf;

use kvasir_core::{
    ClaudeCodeSettings, CopilotShellProfile, OpenCodeSetup, RepoInjectionShell,
    RepoInjectionShellHook, RepoInjectionShellProfile, SetupConfig,
};

use crate::error::KvasirClientError;

use super::fs_atomic::{read_optional_string, replace_file};
use super::managed_state::{
    ensure_installable_state, managed_file_is_current, write_installed_state,
};
use super::{KvasirHarnessTelemetrySetup, setup_error_to_client_error};

pub(super) struct GeneratedHarnessFile {
    target_path: PathBuf,
    contents: String,
}

pub(super) fn prepare_generated_harness_files(
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

    let claude_settings = ClaudeCodeSettings::generate(
        &read_optional_string(&claude_settings_path).map_err(|_| KvasirClientError::Filesystem)?,
        setup_config,
    )
    .map_err(setup_error_to_client_error)?;
    let copilot_profile = CopilotShellProfile::generate(
        &read_optional_string(&copilot_profile_path).map_err(|_| KvasirClientError::Filesystem)?,
        setup_config,
    )
    .map_err(setup_error_to_client_error)?;
    let opencode_setup = OpenCodeSetup::generate(
        &read_optional_string(&opencode_config_path).map_err(|_| KvasirClientError::Filesystem)?,
        setup_config,
    )
    .map_err(setup_error_to_client_error)?;
    let zsh_profile = RepoInjectionShellProfile::generate(
        &read_optional_string(&zsh_profile_path).map_err(|_| KvasirClientError::Filesystem)?,
        &zsh_repo_hook_path,
    )
    .map_err(setup_error_to_client_error)?;
    let bash_profile = RepoInjectionShellProfile::generate(
        &read_optional_string(&bash_profile_path).map_err(|_| KvasirClientError::Filesystem)?,
        &bash_repo_hook_path,
    )
    .map_err(setup_error_to_client_error)?;

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

pub(super) fn install_generated_harness_files(
    files: Vec<GeneratedHarnessFile>,
) -> std::io::Result<()> {
    for file in files {
        install_generated_harness_file(file)?;
    }
    Ok(())
}

pub(super) fn generated_harness_files_are_current(
    files: &[GeneratedHarnessFile],
) -> Result<bool, KvasirClientError> {
    for file in files {
        if !managed_file_is_current(&file.target_path, &file.contents)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn install_generated_harness_file(file: GeneratedHarnessFile) -> std::io::Result<()> {
    if read_optional_string(&file.target_path)? == file.contents {
        ensure_installable_state(&file.target_path)?;
        write_installed_state(&file.target_path, &file.contents)?;
        return Ok(());
    }
    ensure_installable_state(&file.target_path)?;
    replace_file(&file.target_path, &file.contents)?;
    write_installed_state(&file.target_path, &file.contents)
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
