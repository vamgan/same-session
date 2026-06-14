use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use ulid::Ulid;

const PROTOCOL: &str = "same-session/v1";
const VERSION: &[u8] = b"1\n";
const LOCAL_PREFIX: &str = "refs/samesession/local";
const REMOTE_PREFIX: &str = "refs/samesession/remotes";
const PUBLISHED_PREFIX: &str = "refs/heads/same-session/v1";
const LOCAL_LEASE_PREFIX: &str = "refs/samesession/leases/local";
const REMOTE_LEASE_PREFIX: &str = "refs/samesession/leases/remotes";
const PUBLISHED_LEASE_PREFIX: &str = "refs/heads/same-session-leases/v1";

#[derive(Debug, Error)]
pub enum GitStoreError {
    #[error("invalid ref segment: {0}")]
    InvalidRefSegment(String),
    #[error("Git command failed: git {arguments}\n{stderr}")]
    Git { arguments: String, stderr: String },
    #[error("checkpoint ref does not exist: {0}")]
    MissingRef(String),
    #[error("checkpoint tree is missing {0}")]
    MissingTreeEntry(String),
    #[error("checkpoint payload hash does not match public metadata")]
    PayloadHashMismatch,
    #[error("fetched checkpoint refs disagree for portable session {0}")]
    DivergentRemoteRefs(String),
    #[error("session lease is held by {holder} until {expires_at}")]
    LeaseHeld { holder: String, expires_at: String },
    #[error("lease TTL must be greater than zero")]
    InvalidLeaseTtl,
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("checkpoint JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("time formatting failed: {0}")]
    Time(#[from] time::error::Format),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicCheckpoint {
    pub protocol: String,
    pub checkpoint_id: String,
    pub portable_session_id: String,
    pub created_at: String,
    pub creator: String,
    pub cipher: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredCheckpoint {
    pub oid: String,
    pub reference: String,
    pub public: PublicCheckpoint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseRecord {
    pub protocol: String,
    pub portable_session_id: String,
    pub holder_device_id: String,
    pub source_checkpoint: String,
    pub acquired_at: String,
    pub expires_at: String,
    pub takeover_reason: Option<String>,
    #[serde(default)]
    pub released: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredLease {
    pub oid: String,
    pub reference: String,
    pub lease: LeaseRecord,
}

#[derive(Clone, Debug)]
pub struct GitStore {
    repository: PathBuf,
    repository_key: String,
}

impl GitStore {
    /// Opens a Git repository as a `SameSession` checkpoint store.
    ///
    /// # Errors
    ///
    /// Returns an error when `repository` is not readable by Git.
    pub fn open(repository: impl Into<PathBuf>) -> Result<Self, GitStoreError> {
        let repository = repository.into();
        let identity = git_optional_text(&repository, &["config", "--get", "remote.origin.url"])?
            .map_or_else(
            || git_text(&repository, &["rev-parse", "--show-toplevel"]),
            Ok,
        )?;
        let repository_key = short_hash(identity.trim().as_bytes());
        Ok(Self {
            repository,
            repository_key,
        })
    }

    #[must_use]
    pub fn repository_key(&self) -> &str {
        &self.repository_key
    }

    /// Appends an encrypted payload to an isolated local checkpoint ref.
    ///
    /// # Errors
    ///
    /// Returns an error if IDs are unsafe, the payload cannot be read, Git
    /// plumbing fails, or another writer advances the local ref concurrently.
    pub fn append(
        &self,
        payload: &Path,
        portable_session_id: Option<&str>,
        creator: &str,
    ) -> Result<StoredCheckpoint, GitStoreError> {
        let portable_session_id =
            portable_session_id.map_or_else(|| format!("sss_{}", Ulid::new()), str::to_owned);
        validate_segment(&portable_session_id)?;
        let checkpoint_id = format!("ssc_{}", Ulid::new());
        let reference = self.local_ref(&portable_session_id)?;
        let local_parent = self.resolve_optional(&reference)?;
        let parent = match &local_parent {
            Some(parent) => Some(parent.clone()),
            None => self.remote_parent(&portable_session_id)?,
        };
        let payload_bytes = fs::read(payload)?;
        let payload_sha256 = sha256(&payload_bytes);
        let public = PublicCheckpoint {
            protocol: PROTOCOL.to_owned(),
            checkpoint_id,
            portable_session_id,
            created_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
            creator: creator.to_owned(),
            cipher: "age".to_owned(),
            payload_sha256: payload_sha256.clone(),
            payload_bytes: payload_bytes.len().try_into().map_err(io::Error::other)?,
        };
        let public_bytes = serde_json::to_vec_pretty(&public)?;
        let version_oid = self.hash_blob(VERSION)?;
        let public_oid = self.hash_blob(&public_bytes)?;
        let payload_oid = self.hash_blob(&payload_bytes)?;
        let hash_oid = self.hash_blob(format!("{payload_sha256}\n").as_bytes())?;
        let tree = self.make_tree(&[
            ("version", &version_oid),
            ("public.json", &public_oid),
            ("payload.age", &payload_oid),
            ("payload.sha256", &hash_oid),
        ])?;
        let oid = self.commit_tree(&tree, parent.as_deref(), &public.checkpoint_id)?;
        self.update_ref(&reference, &oid, local_parent.as_deref())?;
        Ok(StoredCheckpoint {
            oid,
            reference,
            public,
        })
    }

    /// Lists checkpoint tips from local and fetched-remote `SameSession` refs.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot enumerate or read a checkpoint ref.
    pub fn list(&self) -> Result<Vec<StoredCheckpoint>, GitStoreError> {
        let output = git_text(
            &self.repository,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                LOCAL_PREFIX,
                REMOTE_PREFIX,
            ],
        )?;
        let mut checkpoints = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (oid, reference) = line
                    .split_once(' ')
                    .ok_or_else(|| GitStoreError::MissingRef(line.to_owned()))?;
                self.inspect_oid(oid, reference)
            })
            .collect::<Result<Vec<_>, _>>()?;
        checkpoints.sort_by(|left, right| right.public.created_at.cmp(&left.public.created_at));
        Ok(checkpoints)
    }

    /// Inspects and verifies the public metadata for a checkpoint ref or OID.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision or required tree entries are invalid.
    pub fn inspect(&self, revision: &str) -> Result<StoredCheckpoint, GitStoreError> {
        if revision == "latest" {
            return self
                .list()?
                .into_iter()
                .next()
                .ok_or_else(|| GitStoreError::MissingRef(revision.to_owned()));
        }
        let oid = match self.resolve_optional(revision)? {
            Some(oid) => Some(oid),
            None if revision.starts_with("sss_") => {
                validate_segment(revision)?;
                let local = self.local_ref(revision)?;
                match self.resolve_optional(&local)? {
                    Some(oid) => Some(oid),
                    None => self.remote_parent(revision)?,
                }
            }
            None => None,
        }
        .ok_or_else(|| GitStoreError::MissingRef(revision.to_owned()))?;
        self.inspect_oid(&oid, revision)
    }

    /// Extracts and verifies an encrypted payload to a newly created path.
    ///
    /// # Errors
    ///
    /// Returns an error if the checkpoint is malformed, tampered, or the
    /// output path already exists.
    pub fn extract_payload(&self, revision: &str, output: &Path) -> Result<(), GitStoreError> {
        if output.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "output exists").into());
        }
        let checkpoint = self.inspect(revision)?;
        let payload = self.show_blob(&format!("{}:payload.age", checkpoint.oid))?;
        if sha256(&payload) != checkpoint.public.payload_sha256 {
            return Err(GitStoreError::PayloadHashMismatch);
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&payload)?;
        temporary
            .persist_noclobber(output)
            .map_err(|error| error.error)?;
        Ok(())
    }

    /// Pushes one local checkpoint chain with an explicit refspec.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe ref segments or a rejected Git push.
    pub fn push(&self, remote: &str, portable_session_id: &str) -> Result<(), GitStoreError> {
        validate_segment(remote)?;
        validate_segment(portable_session_id)?;
        let source = self.local_ref(portable_session_id)?;
        let destination = format!(
            "{PUBLISHED_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        );
        git_output(
            &self.repository,
            &["push", remote, &format!("{source}:{destination}")],
            None,
        )?;
        Ok(())
    }

    /// Fetches only `SameSession` checkpoint refs from one remote.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe remote name or a failed Git fetch.
    pub fn fetch(&self, remote: &str) -> Result<(), GitStoreError> {
        self.fetch_with_prune(remote, false)
    }

    /// Fetches only `SameSession` refs and optionally prunes stale fetched refs.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe remote name or a failed Git fetch.
    pub fn fetch_with_prune(&self, remote: &str, prune: bool) -> Result<(), GitStoreError> {
        validate_segment(remote)?;
        let source = format!("{PUBLISHED_PREFIX}/{}/*", self.repository_key);
        let destination = format!("{REMOTE_PREFIX}/{remote}/{}/*", self.repository_key);
        let lease_source = format!("{PUBLISHED_LEASE_PREFIX}/{}/*", self.repository_key);
        let lease_destination = format!("{REMOTE_LEASE_PREFIX}/{remote}/{}/*", self.repository_key);
        let checkpoint_refspec = format!("+{source}:{destination}");
        let lease_refspec = format!("+{lease_source}:{lease_destination}");
        let mut arguments = vec!["fetch", "--no-tags"];
        if prune {
            arguments.push("--prune");
        }
        arguments.extend([remote, checkpoint_refspec.as_str(), lease_refspec.as_str()]);
        git_output(&self.repository, &arguments, None)?;
        Ok(())
    }

    /// Deletes local checkpoint and lease refs for one portable session.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe session ID or a failed ref deletion.
    pub fn delete_local(&self, portable_session_id: &str) -> Result<(), GitStoreError> {
        let checkpoint = self.local_ref(portable_session_id)?;
        let lease = self.local_lease_ref(portable_session_id)?;
        git_output(&self.repository, &["update-ref", "-d", &checkpoint], None)?;
        git_output(&self.repository, &["update-ref", "-d", &lease], None)?;
        Ok(())
    }

    /// Deletes published checkpoint and lease refs for one portable session.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe ref segments or a rejected Git push.
    pub fn delete_remote(
        &self,
        remote: &str,
        portable_session_id: &str,
    ) -> Result<(), GitStoreError> {
        validate_segment(remote)?;
        validate_segment(portable_session_id)?;
        let checkpoint = format!(
            "{PUBLISHED_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        );
        let lease = format!(
            "{PUBLISHED_LEASE_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        );
        git_output(
            &self.repository,
            &[
                "push",
                remote,
                &format!(":{checkpoint}"),
                &format!(":{lease}"),
            ],
            None,
        )?;
        Ok(())
    }

    /// Runs Git's conservative automatic object maintenance.
    ///
    /// # Errors
    ///
    /// Returns an error when Git maintenance fails.
    pub fn gc(&self) -> Result<(), GitStoreError> {
        git_output(&self.repository, &["gc", "--auto"], None)?;
        Ok(())
    }

    /// Acquires or renews an advisory append-only session lease.
    ///
    /// An active lease owned by a different device requires a non-empty
    /// takeover reason.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe IDs, invalid lease state, an active
    /// conflicting lease, or a failed compare-and-swap ref update.
    pub fn acquire_lease(
        &self,
        portable_session_id: &str,
        holder_device_id: &str,
        source_checkpoint: &str,
        ttl_seconds: i64,
        takeover_reason: Option<&str>,
    ) -> Result<StoredLease, GitStoreError> {
        validate_segment(portable_session_id)?;
        validate_segment(holder_device_id)?;
        if ttl_seconds <= 0 {
            return Err(GitStoreError::InvalidLeaseTtl);
        }
        let reference = self.local_lease_ref(portable_session_id)?;
        let local_parent = self.resolve_optional(&reference)?;
        let parent = match &local_parent {
            Some(parent) => Some(parent.clone()),
            None => self.remote_lease_parent(portable_session_id)?,
        };
        let now = OffsetDateTime::now_utc();
        if let Some(parent) = &parent {
            let current = self.inspect_lease_oid(parent, &reference)?;
            let expiry = OffsetDateTime::parse(&current.lease.expires_at, &Rfc3339)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let conflicting = expiry > now && current.lease.holder_device_id != holder_device_id;
            if conflicting && takeover_reason.is_none_or(str::is_empty) {
                return Err(GitStoreError::LeaseHeld {
                    holder: current.lease.holder_device_id,
                    expires_at: current.lease.expires_at,
                });
            }
        }
        let lease = LeaseRecord {
            protocol: PROTOCOL.to_owned(),
            portable_session_id: portable_session_id.to_owned(),
            holder_device_id: holder_device_id.to_owned(),
            source_checkpoint: source_checkpoint.to_owned(),
            acquired_at: now.format(&Rfc3339)?,
            expires_at: (now + time::Duration::seconds(ttl_seconds)).format(&Rfc3339)?,
            takeover_reason: takeover_reason.map(str::to_owned),
            released: false,
        };
        let version_oid = self.hash_blob(VERSION)?;
        let lease_oid = self.hash_blob(&serde_json::to_vec_pretty(&lease)?)?;
        let tree = self.make_tree(&[("version", &version_oid), ("lease.json", &lease_oid)])?;
        let oid = self.commit_tree(&tree, parent.as_deref(), "SameSession lease")?;
        self.update_ref(&reference, &oid, local_parent.as_deref())?;
        Ok(StoredLease {
            oid,
            reference,
            lease,
        })
    }

    /// Reads the latest known lease for a portable session.
    ///
    /// # Errors
    ///
    /// Returns an error when local and fetched lease refs disagree or a lease
    /// commit is malformed.
    pub fn lease_status(
        &self,
        portable_session_id: &str,
    ) -> Result<Option<StoredLease>, GitStoreError> {
        validate_segment(portable_session_id)?;
        let local = self.local_lease_ref(portable_session_id)?;
        if let Some(oid) = self.resolve_optional(&local)? {
            return self.inspect_lease_oid(&oid, &local).map(Some);
        }
        self.remote_lease_parent(portable_session_id)?
            .map(|oid| self.inspect_lease_oid(&oid, "fetched-remote"))
            .transpose()
    }

    /// Releases a lease owned by the specified device.
    ///
    /// # Errors
    ///
    /// Returns an error if no lease exists, another device owns the active
    /// lease, or the append-only release record cannot be written.
    pub fn release_lease(
        &self,
        portable_session_id: &str,
        holder_device_id: &str,
    ) -> Result<StoredLease, GitStoreError> {
        validate_segment(portable_session_id)?;
        validate_segment(holder_device_id)?;
        let reference = self.local_lease_ref(portable_session_id)?;
        let local_parent = self.resolve_optional(&reference)?;
        let parent = match &local_parent {
            Some(parent) => parent.clone(),
            None => self
                .remote_lease_parent(portable_session_id)?
                .ok_or_else(|| GitStoreError::MissingRef(portable_session_id.to_owned()))?,
        };
        let current = self.inspect_lease_oid(&parent, &reference)?;
        let now = OffsetDateTime::now_utc();
        let expiry = OffsetDateTime::parse(&current.lease.expires_at, &Rfc3339)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if expiry > now
            && !current.lease.released
            && current.lease.holder_device_id != holder_device_id
        {
            return Err(GitStoreError::LeaseHeld {
                holder: current.lease.holder_device_id,
                expires_at: current.lease.expires_at,
            });
        }
        let lease = LeaseRecord {
            protocol: PROTOCOL.to_owned(),
            portable_session_id: portable_session_id.to_owned(),
            holder_device_id: holder_device_id.to_owned(),
            source_checkpoint: current.lease.source_checkpoint,
            acquired_at: now.format(&Rfc3339)?,
            expires_at: now.format(&Rfc3339)?,
            takeover_reason: Some("released".to_owned()),
            released: true,
        };
        let version_oid = self.hash_blob(VERSION)?;
        let lease_oid = self.hash_blob(&serde_json::to_vec_pretty(&lease)?)?;
        let tree = self.make_tree(&[("version", &version_oid), ("lease.json", &lease_oid)])?;
        let oid = self.commit_tree(&tree, Some(&parent), "SameSession lease release")?;
        self.update_ref(&reference, &oid, local_parent.as_deref())?;
        Ok(StoredLease {
            oid,
            reference,
            lease,
        })
    }

    /// Pushes one lease chain with an explicit refspec.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe ref segments or a rejected Git push.
    pub fn push_lease(&self, remote: &str, portable_session_id: &str) -> Result<(), GitStoreError> {
        validate_segment(remote)?;
        validate_segment(portable_session_id)?;
        let source = self.local_lease_ref(portable_session_id)?;
        let destination = format!(
            "{PUBLISHED_LEASE_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        );
        git_output(
            &self.repository,
            &["push", remote, &format!("{source}:{destination}")],
            None,
        )?;
        Ok(())
    }

    fn local_ref(&self, portable_session_id: &str) -> Result<String, GitStoreError> {
        validate_segment(portable_session_id)?;
        Ok(format!(
            "{LOCAL_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        ))
    }

    fn local_lease_ref(&self, portable_session_id: &str) -> Result<String, GitStoreError> {
        validate_segment(portable_session_id)?;
        Ok(format!(
            "{LOCAL_LEASE_PREFIX}/{}/{}",
            self.repository_key, portable_session_id
        ))
    }

    fn resolve_optional(&self, revision: &str) -> Result<Option<String>, GitStoreError> {
        git_optional_text(
            &self.repository,
            &["rev-parse", "--verify", "--end-of-options", revision],
        )
    }

    fn remote_parent(&self, portable_session_id: &str) -> Result<Option<String>, GitStoreError> {
        let refs = git_text(
            &self.repository,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                REMOTE_PREFIX,
            ],
        )?;
        unique_remote_tip(&refs, &self.repository_key, portable_session_id)
    }

    fn remote_lease_parent(
        &self,
        portable_session_id: &str,
    ) -> Result<Option<String>, GitStoreError> {
        let refs = git_text(
            &self.repository,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                REMOTE_LEASE_PREFIX,
            ],
        )?;
        unique_remote_tip(&refs, &self.repository_key, portable_session_id)
    }

    fn hash_blob(&self, bytes: &[u8]) -> Result<String, GitStoreError> {
        git_text_with_input(&self.repository, &["hash-object", "-w", "--stdin"], bytes)
    }

    fn make_tree(&self, entries: &[(&str, &String)]) -> Result<String, GitStoreError> {
        let mut input = String::new();
        for (name, oid) in entries {
            use std::fmt::Write as _;
            writeln!(&mut input, "100644 blob {oid}\t{name}")
                .expect("writing to a string cannot fail");
        }
        git_text_with_input(&self.repository, &["mktree"], input.as_bytes())
    }

    fn commit_tree(
        &self,
        tree: &str,
        parent: Option<&str>,
        message: &str,
    ) -> Result<String, GitStoreError> {
        let mut arguments = vec!["commit-tree", tree];
        if let Some(parent) = parent {
            arguments.extend(["-p", parent]);
        }
        git_text_with_input(
            &self.repository,
            &arguments,
            format!("{message}\n").as_bytes(),
        )
    }

    fn update_ref(
        &self,
        reference: &str,
        oid: &str,
        old: Option<&str>,
    ) -> Result<(), GitStoreError> {
        let old = old.unwrap_or("0000000000000000000000000000000000000000");
        git_output(&self.repository, &["update-ref", reference, oid, old], None)?;
        Ok(())
    }

    fn inspect_oid(&self, oid: &str, reference: &str) -> Result<StoredCheckpoint, GitStoreError> {
        let version = self.show_blob(&format!("{oid}:version"))?;
        if version != VERSION {
            return Err(GitStoreError::MissingTreeEntry(
                "supported version".to_owned(),
            ));
        }
        let public: PublicCheckpoint =
            serde_json::from_slice(&self.show_blob(&format!("{oid}:public.json"))?)?;
        let expected_hash =
            String::from_utf8_lossy(&self.show_blob(&format!("{oid}:payload.sha256"))?)
                .trim()
                .to_owned();
        if public.protocol != PROTOCOL || expected_hash != public.payload_sha256 {
            return Err(GitStoreError::PayloadHashMismatch);
        }
        Ok(StoredCheckpoint {
            oid: oid.to_owned(),
            reference: reference.to_owned(),
            public,
        })
    }

    fn inspect_lease_oid(&self, oid: &str, reference: &str) -> Result<StoredLease, GitStoreError> {
        let version = self.show_blob(&format!("{oid}:version"))?;
        if version != VERSION {
            return Err(GitStoreError::MissingTreeEntry(
                "supported version".to_owned(),
            ));
        }
        let lease: LeaseRecord =
            serde_json::from_slice(&self.show_blob(&format!("{oid}:lease.json"))?)?;
        if lease.protocol != PROTOCOL {
            return Err(GitStoreError::MissingTreeEntry(
                "supported lease protocol".to_owned(),
            ));
        }
        Ok(StoredLease {
            oid: oid.to_owned(),
            reference: reference.to_owned(),
            lease,
        })
    }

    fn show_blob(&self, revision: &str) -> Result<Vec<u8>, GitStoreError> {
        Ok(git_output(&self.repository, &["show", revision], None)?.stdout)
    }
}

fn unique_remote_tip(
    refs: &str,
    repository_key: &str,
    portable_session_id: &str,
) -> Result<Option<String>, GitStoreError> {
    let suffix = format!("/{repository_key}/{portable_session_id}");
    let mut matches = refs
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, reference)| reference.ends_with(&suffix))
        .map(|(oid, _)| oid.to_owned())
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(None),
        [oid] => Ok(Some(oid.clone())),
        _ => Err(GitStoreError::DivergentRemoteRefs(
            portable_session_id.to_owned(),
        )),
    }
}

fn validate_segment(segment: &str) -> Result<(), GitStoreError> {
    if segment.is_empty()
        || !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(GitStoreError::InvalidRefSegment(segment.to_owned()));
    }
    Ok(())
}

fn short_hash(bytes: &[u8]) -> String {
    sha256(bytes)[7..15].to_owned()
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(7 + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

fn git_text(repository: &Path, arguments: &[&str]) -> Result<String, GitStoreError> {
    Ok(trim_output(
        &git_output(repository, arguments, None)?.stdout,
    ))
}

fn git_optional_text(
    repository: &Path,
    arguments: &[&str],
) -> Result<Option<String>, GitStoreError> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(arguments)
        .output()?;
    if output.status.success() {
        Ok(Some(trim_output(&output.stdout)))
    } else {
        Ok(None)
    }
}

fn git_text_with_input(
    repository: &Path,
    arguments: &[&str],
    input: &[u8],
) -> Result<String, GitStoreError> {
    Ok(trim_output(
        &git_output(repository, arguments, Some(input))?.stdout,
    ))
}

fn git_output(
    repository: &Path,
    arguments: &[&str],
    input: Option<&[u8]>,
) -> Result<Output, GitStoreError> {
    let mut child = Command::new("git")
        .current_dir(repository)
        .args([
            "-c",
            "user.name=SameSession",
            "-c",
            "user.email=samesession@localhost",
        ])
        .args(arguments)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("piped stdin is available")
            .write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(GitStoreError::Git {
            arguments: arguments.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output)
}

fn trim_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use tempfile::tempdir;

    use super::{GitStore, GitStoreError};

    fn init_repository() -> tempfile::TempDir {
        let repository = tempdir().expect("repository");
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(repository.path())
            .status()
            .expect("git");
        assert!(status.success());
        repository
    }

    fn add_origin(repository: &Path, origin: &Path) {
        let status = Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(origin)
            .current_dir(repository)
            .status()
            .expect("git remote");
        assert!(status.success());
    }

    #[test]
    fn appends_isolated_checkpoint_chain_without_mutating_head() {
        let repository = init_repository();
        let payload = repository.path().join("payload.age");
        fs::write(&payload, b"encrypted one").expect("payload");
        let store = GitStore::open(repository.path()).expect("store");
        let status_before =
            super::git_text(repository.path(), &["status", "--porcelain"]).expect("status");
        let first = store
            .append(&payload, Some("sss_test"), "device")
            .expect("first");
        fs::write(&payload, b"encrypted two").expect("payload");
        let second = store
            .append(&payload, Some("sss_test"), "device")
            .expect("second");

        assert_eq!(store.list().expect("list"), vec![second.clone()]);
        let parents = super::git_text(
            repository.path(),
            &["rev-list", "--parents", "-1", &second.oid],
        )
        .expect("parents");
        assert_eq!(parents, format!("{} {}", second.oid, first.oid));
        assert_eq!(
            super::git_text(repository.path(), &["status", "--porcelain"]).expect("status"),
            status_before
        );
    }

    #[test]
    fn extracts_verified_payload() {
        let repository = init_repository();
        let payload = repository.path().join("payload.age");
        let extracted = repository.path().join("extracted.age");
        fs::write(&payload, b"encrypted").expect("payload");
        let store = GitStore::open(repository.path()).expect("store");
        let checkpoint = store.append(&payload, None, "device").expect("checkpoint");

        store
            .extract_payload(&checkpoint.oid, &extracted)
            .expect("extract");

        assert_eq!(fs::read(extracted).expect("extracted"), b"encrypted");
    }

    #[test]
    fn rejects_hostile_ref_segments() {
        let repository = init_repository();
        let payload = repository.path().join("payload.age");
        fs::write(&payload, b"encrypted").expect("payload");
        let store = GitStore::open(repository.path()).expect("store");

        let error = store
            .append(&payload, Some("../main"), "device")
            .expect_err("hostile ref");

        assert!(matches!(error, GitStoreError::InvalidRefSegment(_)));
    }

    #[test]
    fn pushes_fetches_and_continues_remote_checkpoint_chain() {
        let remote = tempdir().expect("remote");
        let status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .current_dir(remote.path())
            .status()
            .expect("bare git");
        assert!(status.success());
        let source = init_repository();
        let destination = init_repository();
        add_origin(source.path(), remote.path());
        add_origin(destination.path(), remote.path());
        let source_payload = source.path().join("payload.age");
        let destination_payload = destination.path().join("payload.age");
        fs::write(&source_payload, b"source payload").expect("source payload");
        fs::write(&destination_payload, b"destination payload").expect("destination payload");
        let source_store = GitStore::open(source.path()).expect("source store");
        let destination_store = GitStore::open(destination.path()).expect("destination store");
        assert_eq!(
            source_store.repository_key(),
            destination_store.repository_key()
        );

        let first = source_store
            .append(&source_payload, Some("sss_test"), "source")
            .expect("first");
        source_store.push("origin", "sss_test").expect("first push");
        destination_store
            .fetch("origin")
            .expect("destination fetch");
        assert_eq!(
            destination_store
                .inspect("sss_test")
                .expect("inspect portable session")
                .oid,
            first.oid
        );
        assert_eq!(
            destination_store
                .inspect("latest")
                .expect("inspect latest")
                .oid,
            first.oid
        );
        let second = destination_store
            .append(&destination_payload, Some("sss_test"), "destination")
            .expect("second");
        destination_store
            .push("origin", "sss_test")
            .expect("second push");
        source_store.fetch("origin").expect("source fetch");

        let remote_tip = source_store
            .list()
            .expect("list")
            .into_iter()
            .find(|checkpoint| checkpoint.public.creator == "destination")
            .expect("remote tip");
        let parents = super::git_text(
            source.path(),
            &["rev-list", "--parents", "-1", &remote_tip.oid],
        )
        .expect("parents");
        assert_eq!(remote_tip.oid, second.oid);
        assert_eq!(parents, format!("{} {}", second.oid, first.oid));
    }

    #[test]
    fn lease_blocks_other_device_without_takeover_reason() {
        let repository = init_repository();
        let store = GitStore::open(repository.path()).expect("store");
        let first = store
            .acquire_lease("sss_test", "device_one", "checkpoint", 3600, None)
            .expect("first lease");

        let error = store
            .acquire_lease("sss_test", "device_two", "checkpoint", 3600, None)
            .expect_err("must block");
        let takeover = store
            .acquire_lease(
                "sss_test",
                "device_two",
                "checkpoint",
                3600,
                Some("source unavailable"),
            )
            .expect("takeover");

        assert!(matches!(error, GitStoreError::LeaseHeld { .. }));
        assert_eq!(
            store.lease_status("sss_test").expect("status"),
            Some(takeover.clone())
        );
        let parents = super::git_text(
            repository.path(),
            &["rev-list", "--parents", "-1", &takeover.oid],
        )
        .expect("parents");
        assert_eq!(parents, format!("{} {}", takeover.oid, first.oid));
    }

    #[test]
    fn lease_owner_can_release_for_another_device() {
        let repository = init_repository();
        let store = GitStore::open(repository.path()).expect("store");
        store
            .acquire_lease("sss_test", "device_one", "checkpoint", 3600, None)
            .expect("lease");

        let released = store
            .release_lease("sss_test", "device_one")
            .expect("release");
        let acquired = store
            .acquire_lease("sss_test", "device_two", "checkpoint", 3600, None)
            .expect("new lease");

        assert!(released.lease.released);
        assert_eq!(acquired.lease.holder_device_id, "device_two");
    }

    #[test]
    fn pushes_and_fetches_lease_refs() {
        let remote = tempdir().expect("remote");
        let status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .current_dir(remote.path())
            .status()
            .expect("bare git");
        assert!(status.success());
        let source = init_repository();
        let destination = init_repository();
        add_origin(source.path(), remote.path());
        add_origin(destination.path(), remote.path());
        let source_store = GitStore::open(source.path()).expect("source store");
        let destination_store = GitStore::open(destination.path()).expect("destination store");
        let lease = source_store
            .acquire_lease("sss_test", "device_one", "checkpoint", 3600, None)
            .expect("lease");
        source_store
            .push_lease("origin", "sss_test")
            .expect("push lease");

        destination_store.fetch("origin").expect("fetch");

        assert_eq!(
            destination_store
                .lease_status("sss_test")
                .expect("status")
                .expect("lease")
                .oid,
            lease.oid
        );
    }

    #[test]
    fn deletes_selected_local_and_remote_refs() {
        let remote = tempdir().expect("remote");
        let status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .current_dir(remote.path())
            .status()
            .expect("bare git");
        assert!(status.success());
        let source = init_repository();
        let destination = init_repository();
        add_origin(source.path(), remote.path());
        add_origin(destination.path(), remote.path());
        let payload = source.path().join("payload.age");
        fs::write(&payload, b"payload").expect("payload");
        let source_store = GitStore::open(source.path()).expect("source store");
        let destination_store = GitStore::open(destination.path()).expect("destination store");
        source_store
            .append(&payload, Some("sss_test"), "source")
            .expect("append");
        source_store.push("origin", "sss_test").expect("push");
        destination_store.fetch("origin").expect("fetch");
        assert_eq!(destination_store.list().expect("list").len(), 1);

        source_store
            .delete_remote("origin", "sss_test")
            .expect("delete remote");
        source_store.delete_local("sss_test").expect("delete local");
        destination_store
            .fetch_with_prune("origin", true)
            .expect("prune fetch");

        assert!(source_store.list().expect("source list").is_empty());
        assert!(
            destination_store
                .list()
                .expect("destination list")
                .is_empty()
        );
    }
}
