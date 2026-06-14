use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Codex,
    ClaudeCode,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex => formatter.write_str("codex"),
            Self::ClaudeCode => formatter.write_str("claude-code"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactClassification {
    Required,
    Associated,
    Derived,
    Global,
    Unsafe,
    Unknown,
}

impl ArtifactClassification {
    #[must_use]
    pub const fn is_exportable(self) -> bool {
        matches!(self, Self::Required | Self::Associated | Self::Derived)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeArtifact {
    pub role: String,
    pub path: PathBuf,
    pub classification: ArtifactClassification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeSession {
    pub provider: Provider,
    pub id: String,
    pub transcript_path: PathBuf,
    pub cwd: Option<PathBuf>,
    pub agent_version: Option<String>,
    pub timestamp: Option<String>,
    pub artifacts: Vec<NativeArtifact>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RewritePolicy {
    BytePreserve,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapsuleArtifact {
    pub logical_role: String,
    pub install_path: PathBuf,
    pub sha256: String,
    pub required: bool,
    pub rewrite_policy: RewritePolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeCapsule {
    pub schema: String,
    pub provider: Provider,
    pub source_version: Option<String>,
    pub native_session_id: String,
    pub original_cwd: Option<PathBuf>,
    pub artifacts: Vec<CapsuleArtifact>,
    #[serde(default)]
    pub repository: Option<RepositorySnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositorySnapshot {
    pub root_hint: String,
    pub head_oid: String,
    #[serde(default)]
    pub snapshot_oid: String,
    #[serde(default)]
    pub bundle_ref: String,
    pub head_ref: Option<String>,
    pub dirty: bool,
    pub bundle_sha256: String,
}

impl NativeCapsule {
    pub const SCHEMA: &'static str = "same-session/native-capsule/v1";
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("unable to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unable to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("session {0} was not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

pub trait SessionAdapter {
    fn provider(&self) -> Provider;
    fn home(&self) -> &std::path::Path;

    /// Discovers native sessions without mutating provider state.
    ///
    /// # Errors
    ///
    /// Returns an adapter error when a required native artifact cannot be read
    /// or parsed.
    fn discover(&self) -> Result<Vec<NativeSession>, AdapterError>;

    /// Finds a native session by its provider-owned session ID.
    ///
    /// # Errors
    ///
    /// Returns an adapter error when discovery fails or the ID is not found.
    fn inspect(&self, id: &str) -> Result<NativeSession, AdapterError> {
        self.discover()?
            .into_iter()
            .find(|session| session.id == id)
            .ok_or_else(|| AdapterError::NotFound(id.to_owned()))
    }
}
