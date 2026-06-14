use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("configuration parsing failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuration serialization failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("unsupported configuration version {0}")]
    UnsupportedVersion(u32),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectConfig {
    pub version: u32,
    pub store: StoreConfig,
    pub encryption: EncryptionConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoreConfig {
    pub remote: String,
    pub auto_push: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EncryptionConfig {
    pub recipients: Vec<String>,
}

impl ProjectConfig {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub fn new(remote: String, auto_push: bool, recipients: Vec<String>) -> Self {
        Self {
            version: Self::VERSION,
            store: StoreConfig { remote, auto_push },
            encryption: EncryptionConfig { recipients },
        }
    }
}

#[must_use]
pub fn project_config_path(repository: &Path) -> PathBuf {
    repository.join(".samesession/config.toml")
}

/// Loads project configuration when it exists.
///
/// # Errors
///
/// Returns an error when the file cannot be read, parsed, or uses an
/// unsupported version.
pub fn load_project(repository: &Path) -> Result<Option<ProjectConfig>, ConfigError> {
    let path = project_config_path(repository);
    match fs::read_to_string(path) {
        Ok(contents) => {
            let config: ProjectConfig = toml::from_str(&contents)?;
            if config.version != ProjectConfig::VERSION {
                return Err(ConfigError::UnsupportedVersion(config.version));
            }
            Ok(Some(config))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Writes project configuration without replacing an existing file.
///
/// # Errors
///
/// Returns an error when serialization, directory creation, or writing fails.
pub fn create_project(repository: &Path, config: &ProjectConfig) -> Result<PathBuf, ConfigError> {
    let path = project_config_path(repository);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(toml::to_string_pretty(config)?.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{ProjectConfig, create_project, load_project};

    #[test]
    fn creates_and_loads_project_configuration() {
        let repository = tempdir().expect("repository");
        let config =
            ProjectConfig::new("origin".to_owned(), true, vec!["age1recipient".to_owned()]);

        create_project(repository.path(), &config).expect("create");

        assert_eq!(load_project(repository.path()).expect("load"), Some(config));
    }
}
