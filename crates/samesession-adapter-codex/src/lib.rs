use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use samesession_core::{
    AdapterError, ArtifactClassification, NativeArtifact, NativeSession, Provider, SessionAdapter,
};
use serde_json::Value;
use walkdir::WalkDir;

#[derive(Debug)]
pub struct CodexAdapter {
    home: PathBuf,
}

impl CodexAdapter {
    #[must_use]
    pub fn detect() -> Self {
        let home = std::env::var_os("CODEX_HOME").map_or_else(
            || {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".codex")
            },
            PathBuf::from,
        );
        Self { home }
    }

    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn parse_transcript(&self, path: &Path) -> Result<Option<NativeSession>, AdapterError> {
        let file = File::open(path).map_err(|source| AdapterError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut first = String::new();
        BufReader::new(file)
            .read_line(&mut first)
            .map_err(|source| AdapterError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        let value: Value =
            serde_json::from_str(first.trim()).map_err(|source| AdapterError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            return Ok(None);
        }

        let Some(payload) = value.get("payload") else {
            return Ok(None);
        };
        let Some(id) = payload.get("id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let mut artifacts = vec![NativeArtifact {
            role: "primary-transcript".to_owned(),
            path: path.to_path_buf(),
            classification: ArtifactClassification::Required,
        }];
        let snapshots = self.home.join("shell_snapshots");
        if snapshots.is_dir() {
            for entry in WalkDir::new(snapshots)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
            {
                if entry.file_name().to_string_lossy().starts_with(id) {
                    artifacts.push(NativeArtifact {
                        role: "shell-snapshot".to_owned(),
                        path: entry.into_path(),
                        classification: ArtifactClassification::Derived,
                    });
                }
            }
        }

        Ok(Some(NativeSession {
            provider: Provider::Codex,
            id: id.to_owned(),
            transcript_path: path.to_path_buf(),
            cwd: payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            agent_version: payload
                .get("cli_version")
                .and_then(Value::as_str)
                .map(str::to_owned),
            timestamp: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_owned),
            artifacts,
        }))
    }
}

impl SessionAdapter for CodexAdapter {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn home(&self) -> &Path {
        &self.home
    }

    fn discover(&self) -> Result<Vec<NativeSession>, AdapterError> {
        let sessions = self.home.join("sessions");
        if !sessions.is_dir() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for entry in WalkDir::new(sessions)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
        {
            if let Some(session) = self.parse_transcript(entry.path())? {
                found.push(session);
            }
        }
        found.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use samesession_core::SessionAdapter;
    use tempfile::tempdir;

    use super::CodexAdapter;

    #[test]
    fn discovers_rollout_and_shell_snapshot() {
        let temp = tempdir().expect("tempdir");
        let sessions = temp.path().join("sessions/2026/06/13");
        fs::create_dir_all(&sessions).expect("sessions");
        fs::create_dir_all(temp.path().join("shell_snapshots")).expect("snapshots");
        let id = "019f-test-session";
        fs::write(
            sessions.join(format!("rollout-{id}.jsonl")),
            format!(
                "{{\"timestamp\":\"2026-06-13T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"/repo\",\"cli_version\":\"0.137.0\"}}}}\n"
            ),
        )
        .expect("transcript");
        fs::write(
            temp.path().join(format!("shell_snapshots/{id}.sh")),
            "export A=1",
        )
        .expect("snapshot");

        let sessions = CodexAdapter::new(temp.path().to_path_buf())
            .discover()
            .expect("discover");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].artifacts.len(), 2);
    }
}
