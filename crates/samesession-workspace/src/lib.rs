use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

use samesession_core::RepositorySnapshot;
use samesession_policy::{FindingKind, scan_path};
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
    #[error("blocked secret finding {kind:?} in {path}")]
    SecretFound { path: String, kind: FindingKind },
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
    let root = Path::new(&root);
    scan_selected_files(root)?;
    let head_oid = git_text(root, &["rev-parse", "HEAD"])?;
    let head_ref = git_optional_text(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let dirty = !git_text(root, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty();
    let snapshot_oid = if dirty {
        create_synthetic_commit(root)?
    } else {
        head_oid.clone()
    };
    let bundle_ref = format!("refs/samesession/capture/{snapshot_oid}");
    git_output(root, &["update-ref", &bundle_ref, &snapshot_oid])?;
    let bundle_result = git_output(
        root,
        &[
            "bundle",
            "create",
            &bundle_output.to_string_lossy(),
            &bundle_ref,
        ],
    );
    let delete_result = git_output(root, &["update-ref", "-d", &bundle_ref]);
    bundle_result?;
    delete_result?;
    Ok(RepositorySnapshot {
        root_hint: root.file_name().map_or_else(
            || "repository".to_owned(),
            |name| name.to_string_lossy().into(),
        ),
        head_oid,
        snapshot_oid,
        bundle_ref,
        head_ref,
        dirty,
        bundle_sha256: hash_file(bundle_output)?,
    })
}

fn create_synthetic_commit(repository: &Path) -> Result<String, WorkspaceError> {
    let directory = tempfile::tempdir()?;
    let index = directory.path().join("index");
    git_output_with_index(repository, &index, &["read-tree", "HEAD"])?;
    git_output_with_index(repository, &index, &["add", "-A", "--", "."])?;
    let tree = git_text_with_index(repository, &index, &["write-tree"])?;
    git_text_with_input(
        repository,
        &["commit-tree", &tree, "-p", "HEAD"],
        b"SameSession workspace snapshot\n",
    )
}

fn scan_selected_files(repository: &Path) -> Result<(), WorkspaceError> {
    let output = git_output(
        repository,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    for path in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = repository.join(String::from_utf8_lossy(path).as_ref());
        if !path.exists() {
            continue;
        }
        if let Some(finding) = scan_path(&path)
            .map_err(|error| io::Error::other(error.to_string()))?
            .into_iter()
            .next()
        {
            return Err(WorkspaceError::SecretFound {
                path: finding.path.display().to_string(),
                kind: finding.kind,
            });
        }
    }
    Ok(())
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
    let bundle_ref = if snapshot.bundle_ref.is_empty() {
        "HEAD"
    } else {
        &snapshot.bundle_ref
    };
    git_output(
        repository,
        &[
            "fetch",
            &bundle.to_string_lossy(),
            &format!("{bundle_ref}:{reference}"),
        ],
    )?;
    let actual = git_text(
        repository,
        &["rev-parse", "--verify", "--end-of-options", &reference],
    )?;
    let expected = if snapshot.snapshot_oid.is_empty() {
        &snapshot.head_oid
    } else {
        &snapshot.snapshot_oid
    };
    if &actual != expected {
        return Err(WorkspaceError::HeadMismatch {
            reference,
            actual,
            expected: expected.clone(),
        });
    }
    Ok(reference)
}

/// Creates a detached worktree at an imported `SameSession` source ref.
///
/// # Errors
///
/// Returns an error when the destination exists or Git cannot create the
/// worktree.
pub fn create_worktree(
    repository: &Path,
    source_ref: &str,
    destination: &Path,
) -> Result<(), WorkspaceError> {
    if destination.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "worktree exists").into());
    }
    git_output(
        repository,
        &[
            "worktree",
            "add",
            "--detach",
            &destination.to_string_lossy(),
            source_ref,
        ],
    )?;
    Ok(())
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

fn git_text_with_index(
    repository: &Path,
    index: &Path,
    arguments: &[&str],
) -> Result<String, WorkspaceError> {
    Ok(
        String::from_utf8_lossy(&git_output_with_index(repository, index, arguments)?.stdout)
            .trim()
            .to_owned(),
    )
}

fn git_text_with_input(
    repository: &Path,
    arguments: &[&str],
    input: &[u8],
) -> Result<String, WorkspaceError> {
    let mut child = Command::new("git")
        .current_dir(repository)
        .args([
            "-c",
            "user.name=SameSession",
            "-c",
            "user.email=samesession@localhost",
        ])
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped stdin is available")
        .write_all(input)?;
    let output = child.wait_with_output()?;
    checked_output(arguments, output)
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
    checked_output(arguments, output)
}

fn git_output_with_index(
    repository: &Path,
    index: &Path,
    arguments: &[&str],
) -> Result<Output, WorkspaceError> {
    let output = Command::new("git")
        .current_dir(repository)
        .env("GIT_INDEX_FILE", index)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    checked_output(arguments, output)
}

fn checked_output(arguments: &[&str], output: Output) -> Result<Output, WorkspaceError> {
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

    use super::{WorkspaceError, capture_source, create_worktree, restore_source};

    fn git_text(path: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(path)
            .output()
            .expect("git command");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

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

    #[test]
    fn blocks_secret_in_tracked_source_file() {
        let source = tempdir().expect("source");
        init_with_commit(source.path(), "token=ghp_abcdefghijklmnopqrstuvwxyz1234");

        let error = capture_source(source.path(), &source.path().join("source.bundle"))
            .expect_err("must reject secret");

        assert!(matches!(error, WorkspaceError::SecretFound { .. }));
    }

    #[test]
    fn captures_dirty_workspace_without_mutating_source() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let worktree_parent = tempdir().expect("worktree parent");
        let worktree = worktree_parent.path().join("resumed");
        init_with_commit(source.path(), "base");
        init_with_commit(destination.path(), "destination");
        fs::write(source.path().join("file.txt"), "modified").expect("tracked file");
        fs::write(source.path().join("untracked.bin"), [0_u8, 1, 2, 255]).expect("untracked file");
        let source_status = git_text(
            source.path(),
            &["status", "--porcelain=v1", "--untracked-files=all"],
        );
        let destination_head = git_text(destination.path(), &["rev-parse", "HEAD"]);
        let bundle = worktree_parent.path().join("source.bundle");

        let snapshot = capture_source(source.path(), &bundle).expect("capture");
        let reference =
            restore_source(destination.path(), &bundle, &snapshot, "sss_dirty").expect("restore");
        create_worktree(destination.path(), &reference, &worktree).expect("worktree");

        assert!(snapshot.dirty);
        assert_ne!(snapshot.snapshot_oid, snapshot.head_oid);
        assert_eq!(
            git_text(
                source.path(),
                &["status", "--porcelain=v1", "--untracked-files=all"]
            ),
            source_status
        );
        assert_eq!(
            git_text(source.path(), &["for-each-ref", "refs/samesession/capture"]),
            ""
        );
        assert_eq!(
            git_text(destination.path(), &["rev-parse", "HEAD"]),
            destination_head
        );
        assert_eq!(
            fs::read_to_string(worktree.join("file.txt")).expect("tracked"),
            "modified"
        );
        assert_eq!(
            fs::read(worktree.join("untracked.bin")).expect("untracked"),
            [0_u8, 1, 2, 255]
        );
    }
}
