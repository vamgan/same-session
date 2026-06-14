use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
    process::{Command, Output, Stdio},
};

use samesession_core::RepositorySnapshot;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Git command failed: git {arguments}\n{stderr}")]
    Git { arguments: String, stderr: String },
    #[error("restored source ref {reference} resolved to {actual}, expected {expected}")]
    HeadMismatch {
        reference: String,
        actual: String,
        expected: String,
    },
    #[error("invalid source ref segment: {0}")]
    InvalidRefSegment(String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Captures all commits reachable from `HEAD` in a portable Git bundle.
///
/// The repository index and working tree are not mutated.
///
/// # Errors
///
/// Returns an error when repository metadata cannot be read or Git cannot
/// create the bundle.
pub fn capture_source(
    repository: &Path,
    bundle_output: &Path,
) -> Result<RepositorySnapshot, WorkspaceError> {
    if bundle_output.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "bundle output exists").into());
    }
    if let Some(parent) = bundle_output.parent() {
        fs::create_dir_all(parent)?;
    }
    let root = git_text(repository, &["rev-parse", "--show-toplevel"])?;
    let head_oid = git_text(repository, &["rev-parse", "HEAD"])?;
    let head_ref = git_optional_text(repository, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let dirty = !git_text(
        repository,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?
    .is_empty();
    git_output(
        repository,
        &["bundle", "create", &bundle_output.to_string_lossy(), "HEAD"],
    )?;
    Ok(RepositorySnapshot {
        root_hint: Path::new(&root).file_name().map_or_else(
            || "repository".to_owned(),
            |name| name.to_string_lossy().into(),
        ),
        head_oid,
        head_ref,
        dirty,
        bundle_sha256: hash_file(bundle_output)?,
    })
}

/// Imports a source bundle into an isolated local ref.
///
/// # Errors
///
/// Returns an error when the bundle is invalid, the ref segment is unsafe, Git
/// import fails, or the imported `HEAD` differs from the encrypted snapshot.
pub fn restore_source(
    repository: &Path,
    bundle: &Path,
    snapshot: &RepositorySnapshot,
    ref_segment: &str,
) -> Result<String, WorkspaceError> {
    validate_segment(ref_segment)?;
    let actual_hash = hash_file(bundle)?;
    if actual_hash != snapshot.bundle_sha256 {
        return Err(WorkspaceError::HeadMismatch {
            reference: "bundle-sha256".to_owned(),
            actual: actual_hash,
            expected: snapshot.bundle_sha256.clone(),
        });
    }
    git_output(repository, &["bundle", "verify", &bundle.to_string_lossy()])?;
    let reference = format!("refs/samesession/source/{ref_segment}");
    git_output(
        repository,
        &[
            "fetch",
            &bundle.to_string_lossy(),
            &format!("HEAD:{reference}"),
        ],
    )?;
    let actual = git_text(
        repository,
        &["rev-parse", "--verify", "--end-of-options", &reference],
    )?;
    if actual != snapshot.head_oid {
        return Err(WorkspaceError::HeadMismatch {
            reference,
            actual,
            expected: snapshot.head_oid.clone(),
        });
    }
    Ok(reference)
}

fn validate_segment(segment: &str) -> Result<(), WorkspaceError> {
    if segment.is_empty()
        || !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WorkspaceError::InvalidRefSegment(segment.to_owned()));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, WorkspaceError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(7 + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    Ok(encoded)
}

fn git_text(repository: &Path, arguments: &[&str]) -> Result<String, WorkspaceError> {
    Ok(
        String::from_utf8_lossy(&git_output(repository, arguments)?.stdout)
            .trim()
            .to_owned(),
    )
}

fn git_optional_text(
    repository: &Path,
    arguments: &[&str],
) -> Result<Option<String>, WorkspaceError> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    } else {
        Ok(None)
    }
}

fn git_output(repository: &Path, arguments: &[&str]) -> Result<Output, WorkspaceError> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        return Err(WorkspaceError::Git {
            arguments: arguments.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use tempfile::tempdir;

    use super::{capture_source, restore_source};

    fn init_with_commit(path: &Path, contents: &str) {
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .expect("git init");
        assert!(status.success());
        fs::write(path.join("file.txt"), contents).expect("file");
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "add",
                "file.txt",
            ])
            .current_dir(path)
            .status()
            .expect("git add");
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "test",
            ])
            .current_dir(path)
            .status()
            .expect("git commit");
        assert!(status.success());
    }

    #[test]
    fn imports_local_commits_without_mutating_destination_head() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        init_with_commit(source.path(), "source");
        init_with_commit(destination.path(), "destination");
        let destination_head =
            super::git_text(destination.path(), &["rev-parse", "HEAD"]).expect("destination head");
        let bundle = source.path().join("source.bundle");
        let snapshot = capture_source(source.path(), &bundle).expect("capture");

        let reference =
            restore_source(destination.path(), &bundle, &snapshot, "sss_test").expect("restore");

        assert_eq!(
            super::git_text(destination.path(), &["rev-parse", "HEAD"]).expect("head"),
            destination_head
        );
        assert_eq!(
            super::git_text(destination.path(), &["rev-parse", &reference]).expect("source ref"),
            snapshot.head_oid
        );
    }
}
