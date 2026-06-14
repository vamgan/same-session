use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use walkdir::WalkDir;

const SCAN_FILE_LIMIT: u64 = 25 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    PrivateKey,
    AwsAccessKey,
    GithubToken,
    AnthropicApiKey,
    OpenAiApiKey,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecretFinding {
    pub path: PathBuf,
    pub kind: FindingKind,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("directory traversal failed: {0}")]
    Walk(#[from] walkdir::Error),
}

/// Scans a file or directory for high-confidence secret patterns.
///
/// # Errors
///
/// Returns an error when selected files cannot be traversed or read.
pub fn scan_path(path: &Path) -> Result<Vec<SecretFinding>, PolicyError> {
    let mut findings = Vec::new();
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() || entry.metadata()?.len() > SCAN_FILE_LIMIT {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        for kind in scan_bytes(&bytes) {
            findings.push(SecretFinding {
                path: entry.path().to_path_buf(),
                kind,
            });
        }
    }
    Ok(findings)
}

#[must_use]
pub fn scan_bytes(bytes: &[u8]) -> Vec<FindingKind> {
    let mut findings = Vec::new();
    if contains(bytes, b"-----BEGIN OPENSSH PRIVATE KEY-----")
        || contains(bytes, b"-----BEGIN RSA PRIVATE KEY-----")
        || contains(bytes, b"-----BEGIN EC PRIVATE KEY-----")
        || contains(bytes, b"-----BEGIN PRIVATE KEY-----")
    {
        findings.push(FindingKind::PrivateKey);
    }
    if has_prefixed_secret(bytes, b"AKIA", 16, is_upper_alphanumeric) {
        findings.push(FindingKind::AwsAccessKey);
    }
    if has_prefixed_secret(bytes, b"ghp_", 24, is_token_character)
        || has_prefixed_secret(bytes, b"github_pat_", 24, is_token_character)
    {
        findings.push(FindingKind::GithubToken);
    }
    if has_prefixed_secret(bytes, b"sk-ant-", 24, is_token_character) {
        findings.push(FindingKind::AnthropicApiKey);
    }
    if has_prefixed_secret(bytes, b"sk-proj-", 24, is_token_character) {
        findings.push(FindingKind::OpenAiApiKey);
    }
    findings
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn has_prefixed_secret(
    bytes: &[u8],
    prefix: &[u8],
    minimum_suffix: usize,
    allowed: fn(u8) -> bool,
) -> bool {
    bytes
        .windows(prefix.len())
        .enumerate()
        .filter(|(_, window)| *window == prefix)
        .any(|(index, _)| {
            bytes[index + prefix.len()..]
                .iter()
                .take_while(|byte| allowed(**byte))
                .count()
                >= minimum_suffix
        })
}

fn is_upper_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

fn is_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{FindingKind, scan_bytes, scan_path};

    #[test]
    fn identifies_high_confidence_tokens() {
        assert_eq!(
            scan_bytes(b"token=ghp_abcdefghijklmnopqrstuvwxyz1234"),
            vec![FindingKind::GithubToken]
        );
        assert_eq!(
            scan_bytes(b"-----BEGIN OPENSSH PRIVATE KEY-----"),
            vec![FindingKind::PrivateKey]
        );
    }

    #[test]
    fn scans_directory_files() {
        let directory = tempdir().expect("directory");
        fs::write(
            directory.path().join("transcript.jsonl"),
            b"key=sk-ant-abcdefghijklmnopqrstuvwxyz1234",
        )
        .expect("file");

        let findings = scan_path(directory.path()).expect("scan");

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::AnthropicApiKey);
    }
}
