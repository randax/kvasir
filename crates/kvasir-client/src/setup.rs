use std::fs;
use std::path::PathBuf;

use kvasir_core::{
    CodexConfigToml, KvasirEndpoint, RawBodyDirectory, SetupConfig, SetupSecretSource,
};
#[cfg(test)]
use kvasir_core::{SetupCredential, resolve_setup_config};

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
    let setup_config =
        SetupSecretSource::claude_code_keychain(&PathBuf::from(&config.claude_settings_path))
            .resolve(
                KvasirEndpoint::new(&config.otlp_endpoint),
                RawBodyDirectory::new(PathBuf::from(&config.raw_body_directory)),
            )
            .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    write_codex_telemetry_config(config, &setup_config)
}

#[cfg(test)]
fn configure_kvasir_harness_telemetry_with_credential(
    config: KvasirHarnessTelemetrySetup,
    credential: &dyn SetupCredential,
) -> Result<(), KvasirClientError> {
    let setup_config = resolve_setup_config(
        credential,
        KvasirEndpoint::new(&config.otlp_endpoint),
        RawBodyDirectory::new(PathBuf::from(&config.raw_body_directory)),
    )
    .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;
    write_codex_telemetry_config(config, &setup_config)
}

fn write_codex_telemetry_config(
    config: KvasirHarnessTelemetrySetup,
    setup_config: &SetupConfig,
) -> Result<(), KvasirClientError> {
    let codex_config_path = PathBuf::from(config.codex_config_path);
    if let Some(parent) = codex_config_path.parent() {
        fs::create_dir_all(parent).map_err(|_| KvasirClientError::Filesystem)?;
    }

    let existing_config = match fs::read_to_string(&codex_config_path) {
        Ok(config) => config,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(KvasirClientError::Filesystem),
    };
    let generated = CodexConfigToml::generate(&existing_config, setup_config)
        .map_err(|_| KvasirClientError::HarnessTelemetrySetup)?;

    if generated.as_str() != existing_config {
        fs::write(codex_config_path, generated.as_str())
            .map_err(|_| KvasirClientError::Filesystem)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

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

    #[derive(Clone, Default)]
    struct MemorySetupCredential {
        password: Rc<RefCell<Option<String>>>,
    }

    impl SetupCredential for MemorySetupCredential {
        fn read(&self) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.password.borrow().clone())
        }

        fn write(&self, password: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.password.replace(Some(password.to_owned()));
            Ok(())
        }
    }
}
