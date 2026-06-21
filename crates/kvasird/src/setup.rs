use std::path::{Path, PathBuf};

use kvasir_core::{BearerToken, KvasirEndpoint, RawBodyDirectory, SetupConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct KeychainSetupSecretSource {
    service: &'static str,
    user: String,
}

impl KeychainSetupSecretSource {
    pub fn claude_code_settings(settings_path: &Path) -> Self {
        Self {
            service: "dev.kvasir.setup",
            user: format!(
                "claude-code-settings:{}",
                keychain_path_component(&canonical_config_path(settings_path))
            ),
        }
    }
}

pub enum SetupSecretSource {
    Keychain(KeychainSetupSecretSource),
    #[cfg(test)]
    Credential(Box<dyn SetupCredential>),
}

impl SetupSecretSource {
    pub fn claude_code_keychain(settings_path: &Path) -> Self {
        Self::Keychain(KeychainSetupSecretSource::claude_code_settings(
            settings_path,
        ))
    }

    pub fn resolve(
        &self,
        endpoint: KvasirEndpoint,
        raw_body_directory: RawBodyDirectory,
    ) -> anyhow::Result<SetupConfig> {
        match self {
            Self::Keychain(source) => source.resolve(endpoint, raw_body_directory),
            #[cfg(test)]
            Self::Credential(credential) => {
                resolve_setup_config(credential.as_ref(), endpoint, raw_body_directory)
            }
        }
    }
}

impl KeychainSetupSecretSource {
    fn resolve(
        &self,
        endpoint: KvasirEndpoint,
        raw_body_directory: RawBodyDirectory,
    ) -> anyhow::Result<SetupConfig> {
        let entry = keyring::Entry::new(self.service, &self.user)?;
        resolve_setup_config(
            &KeyringSetupCredential { entry },
            endpoint,
            raw_body_directory,
        )
    }
}

pub trait SetupCredential {
    fn read(&self) -> anyhow::Result<Option<String>>;
    fn write(&self, password: &str) -> anyhow::Result<()>;
}

struct KeyringSetupCredential {
    entry: keyring::Entry,
}

impl SetupCredential for KeyringSetupCredential {
    fn read(&self) -> anyhow::Result<Option<String>> {
        match self.entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write(&self, password: &str) -> anyhow::Result<()> {
        self.entry.set_password(password)?;
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct StoredSetupSecrets {
    endpoint: KvasirEndpoint,
    bearer_token: BearerToken,
}

fn resolve_setup_config(
    credential: &dyn SetupCredential,
    endpoint: KvasirEndpoint,
    raw_body_directory: RawBodyDirectory,
) -> anyhow::Result<SetupConfig> {
    let bearer_token = match credential.read()? {
        Some(encoded) => serde_json::from_str::<StoredSetupSecrets>(&encoded)?.bearer_token,
        None => BearerToken::generate()?,
    };
    let secrets = StoredSetupSecrets {
        endpoint,
        bearer_token,
    };
    credential.write(&serde_json::to_string(&secrets)?)?;

    Ok(SetupConfig::new(
        secrets.endpoint,
        secrets.bearer_token,
        raw_body_directory,
    ))
}

fn canonical_config_path(config_path: &Path) -> PathBuf {
    if let Ok(path) = config_path.canonicalize() {
        return path;
    }

    let absolute_path = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(config_path))
            .unwrap_or_else(|_| config_path.to_path_buf())
    };
    let Some(parent) = absolute_path.parent() else {
        return absolute_path;
    };
    let Some(file_name) = absolute_path.file_name() else {
        return absolute_path;
    };
    parent
        .canonicalize()
        .map(|parent| parent.join(file_name))
        .unwrap_or(absolute_path)
}

fn keychain_path_component(stable_path: &Path) -> String {
    if let Some(path) = stable_path.to_str() {
        return format!("utf8:{path}");
    }

    format!("lossy:{}", stable_path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn setup_secret_resolution_generates_and_persists_endpoint_and_bearer_token()
    -> anyhow::Result<()> {
        let credential = MemorySetupCredential::default();
        let source = SetupSecretSource::Credential(Box::new(credential.clone()));
        let config = source.resolve(
            KvasirEndpoint::new("http://127.0.0.1:4318"),
            RawBodyDirectory::new("/tmp/kvasir/raw-bodies".into()),
        )?;

        assert_eq!(config.endpoint().as_str(), "http://127.0.0.1:4318");
        assert_eq!(config.bearer_token().as_str().len(), 64);
        let stored = credential.stored_secrets()?;
        assert_eq!(
            (stored.endpoint.as_str(), stored.bearer_token.as_str()),
            ("http://127.0.0.1:4318", config.bearer_token().as_str())
        );

        let reloaded = source.resolve(
            KvasirEndpoint::new("http://127.0.0.1:9999"),
            RawBodyDirectory::new("/tmp/kvasir/other-raw-bodies".into()),
        )?;
        assert_eq!(reloaded.endpoint().as_str(), "http://127.0.0.1:9999");
        assert_eq!(reloaded.bearer_token(), config.bearer_token());
        assert_eq!(
            reloaded.raw_body_directory().as_path(),
            std::path::Path::new("/tmp/kvasir/other-raw-bodies")
        );
        let stored = credential.stored_secrets()?;
        assert_eq!(
            (stored.endpoint.as_str(), stored.bearer_token.as_str()),
            ("http://127.0.0.1:9999", config.bearer_token().as_str())
        );

        Ok(())
    }

    #[derive(Clone, Default)]
    struct MemorySetupCredential {
        password: std::rc::Rc<RefCell<Option<String>>>,
    }

    impl MemorySetupCredential {
        fn get_password(&self) -> anyhow::Result<Option<String>> {
            Ok(self.password.borrow().clone())
        }

        fn stored_secrets(&self) -> anyhow::Result<StoredSetupSecrets> {
            let Some(encoded) = self.get_password()? else {
                anyhow::bail!("expected stored setup secrets")
            };
            Ok(serde_json::from_str(&encoded)?)
        }
    }

    impl SetupCredential for MemorySetupCredential {
        fn read(&self) -> anyhow::Result<Option<String>> {
            self.get_password()
        }

        fn write(&self, password: &str) -> anyhow::Result<()> {
            self.password.replace(Some(password.to_owned()));
            Ok(())
        }
    }
}
