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
pub struct ClaudeAdapter {
    home: PathBuf,
}

impl ClaudeAdapter {
    #[must_use]
    pub fn detect() -> Self {
        let home = std::env::var_os("SAMESESSION_CLAUDE_HOME").map_or_else(
            || {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".claude")
            },
            PathBuf::from,
        );
        Self { home }
    }

    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn add_tree_artifact(artifacts: &mut Vec<NativeArtifact>, path: PathBuf, role: &str) {
        if path.exists() {
            artifacts.push(NativeArtifact {
                role: role.to_owned(),
                path,
                classification: ArtifactClassification::Associated,
            });
        }
    }

    fn parse_transcript(&self, path: &Path) -> Result<Option<NativeSession>, AdapterError> {
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Ok(None);
        };
        let file = File::open(path).map_err(|source| AdapterError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let reader = BufReader::new(file);
        let mut cwd = None;
        let mut version = None;
        let mut timestamp = None;
        let mut confirms_id = false;

        for line in reader.lines().take(200) {
            let line = line.map_err(|source| AdapterError::Read {
                path: path.to_path_buf(),
                source,
            })?;
            let value: Value =
                serde_json::from_str(&line).map_err(|source| AdapterError::Parse {
                    path: path.to_path_buf(),
                    source,
                })?;
            if value.get("sessionId").and_then(Value::as_str) == Some(id) {
                confirms_id = true;
            }
            cwd = cwd.or_else(|| value.get("cwd").and_then(Value::as_str).map(PathBuf::from));
            version = version.or_else(|| {
                value
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            timestamp = timestamp.or_else(|| {
                value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            if confirms_id && cwd.is_some() && version.is_some() {
                break;
            }
        }
        if !confirms_id {
            return Ok(None);
        }

        let mut artifacts = vec![NativeArtifact {
            role: "primary-transcript".to_owned(),
            path: path.to_path_buf(),
            classification: ArtifactClassification::Required,
        }];
        Self::add_tree_artifact(
            &mut artifacts,
            path.with_extension(""),
            "session-artifact-directory",
        );
        Self::add_tree_artifact(
            &mut artifacts,
            self.home.join("tasks").join(id),
            "session-tasks",
        );
        Self::add_tree_artifact(
            &mut artifacts,
            self.home.join("session-env").join(id),
            "session-environment",
        );
        let short_id = id.split('-').next().unwrap_or(id);
        Self::add_tree_artifact(
            &mut artifacts,
            self.home.join("jobs").join(short_id),
            "session-jobs",
        );

        Ok(Some(NativeSession {
            provider: Provider::ClaudeCode,
            id: id.to_owned(),
            transcript_path: path.to_path_buf(),
            cwd,
            agent_version: version,
            timestamp,
            artifacts,
        }))
    }
}

impl SessionAdapter for ClaudeAdapter {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn home(&self) -> &Path {
        &self.home
    }

    fn discover(&self) -> Result<Vec<NativeSession>, AdapterError> {
        let projects = self.home.join("projects");
        if !projects.is_dir() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for entry in WalkDir::new(projects)
            .min_depth(2)
            .max_depth(2)
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

    use super::ClaudeAdapter;

    #[test]
    fn discovers_transcript_and_associated_directory() {
        let temp = tempdir().expect("tempdir");
        let project = temp.path().join("projects/-repo");
        fs::create_dir_all(&project).expect("project");
        let id = "6b38a80a-b6da-4e84-b091-c5f596680546";
        fs::write(
            project.join(format!("{id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{id}\",\"cwd\":\"/repo\",\"version\":\"2.1.177\",\"timestamp\":\"2026-06-13T00:00:00Z\"}}\n"
            ),
        )
        .expect("transcript");
        fs::create_dir_all(project.join(id).join("subagents")).expect("artifacts");

        let sessions = ClaudeAdapter::new(temp.path().to_path_buf())
            .discover()
            .expect("discover");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].artifacts.len(), 2);
    }
}
