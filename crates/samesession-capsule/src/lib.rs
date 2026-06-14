use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use age::{Decryptor, Encryptor, x25519};
use samesession_core::{
    ArtifactClassification, CapsuleArtifact, NativeCapsule, NativeSession, Provider,
    RepositorySnapshot, RewritePolicy,
};
use samesession_policy::{FindingKind, scan_path};
use semver::Version;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use thiserror::Error;
use walkdir::WalkDir;

const CAPSULE_MANIFEST: &str = "capsule.json";
const ARTIFACTS_PREFIX: &str = "artifacts";
const SINGLE_FILE_LIMIT: u64 = 25 * 1024 * 1024;
const CAPSULE_LIMIT: u64 = 100 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum CapsuleError {
    #[error("artifact {path} has non-exportable classification {classification:?}")]
    ForbiddenArtifact {
        path: PathBuf,
        classification: ArtifactClassification,
    },
    #[error("artifact {path} is outside provider home {home}")]
    OutsideProviderHome { path: PathBuf, home: PathBuf },
    #[error("unsafe archive path: {0}")]
    UnsafePath(PathBuf),
    #[error("unsupported archive entry type for {0}")]
    UnsupportedEntry(PathBuf),
    #[error("duplicate archive path: {0}")]
    DuplicatePath(PathBuf),
    #[error("duplicate artifact install path: {0}")]
    DuplicateInstallPath(PathBuf),
    #[error("capsule manifest is missing")]
    MissingManifest,
    #[error("capsule schema {0} is not supported")]
    UnsupportedSchema(String),
    #[error("artifact hash mismatch for {0}")]
    HashMismatch(PathBuf),
    #[error("artifact changed while checkpointing: {0}")]
    ArtifactChanged(PathBuf),
    #[error("restore destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("checkpoint output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("capsule source bundle is missing")]
    MissingSourceBundle,
    #[error("blocked secret finding {kind:?} in {path}")]
    SecretFound { path: PathBuf, kind: FindingKind },
    #[error("{path} exceeds the {limit}-byte capsule limit")]
    SizeLimit { path: PathBuf, limit: u64 },
    #[error("capsule provider {actual} does not match expected provider {expected}")]
    ProviderMismatch {
        expected: Provider,
        actual: Provider,
    },
    #[error(
        "native session version {source_version} is incompatible with destination version {destination_version}"
    )]
    IncompatibleVersion {
        source_version: String,
        destination_version: String,
    },
    #[error("invalid age identity")]
    InvalidIdentity,
    #[error("invalid age recipient")]
    InvalidRecipient,
    #[error("capsule encryption failed: {0}")]
    Encrypt(#[from] age::EncryptError),
    #[error("capsule decryption failed: {0}")]
    Decrypt(#[from] age::DecryptError),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("capsule JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("directory traversal failed: {0}")]
    Walk(#[from] walkdir::Error),
}

#[derive(Clone)]
pub struct DeviceIdentity {
    identity: x25519::Identity,
}

#[derive(Clone, Copy, Debug)]
pub struct RestorePolicy<'a> {
    pub expected_provider: Provider,
    pub destination_version: Option<&'a str>,
    pub force_native: bool,
    pub source_bundle_output: Option<&'a Path>,
}

#[derive(Clone, Copy, Debug)]
pub struct SourceBundle<'a> {
    pub path: &'a Path,
    pub snapshot: &'a RepositorySnapshot,
}

impl DeviceIdentity {
    #[must_use]
    pub fn generate() -> Self {
        Self {
            identity: x25519::Identity::generate(),
        }
    }

    #[must_use]
    pub fn recipient(&self) -> String {
        self.identity.to_public().to_string()
    }

    #[must_use]
    pub fn device_id(&self) -> String {
        let digest = Sha256::digest(self.recipient().as_bytes());
        let mut encoded = String::from("device_");
        for byte in &digest[..8] {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
        }
        encoded
    }

    #[must_use]
    pub fn expose_secret(&self) -> String {
        age::secrecy::ExposeSecret::expose_secret(&self.identity.to_string()).to_owned()
    }

    /// Parses an age X25519 private identity.
    ///
    /// # Errors
    ///
    /// Returns [`CapsuleError::InvalidIdentity`] when `value` is invalid.
    pub fn parse(value: &str) -> Result<Self, CapsuleError> {
        Ok(Self {
            identity: x25519::Identity::from_str(value)
                .map_err(|_| CapsuleError::InvalidIdentity)?,
        })
    }

    /// Saves this private identity to a newly created file.
    ///
    /// On Unix, the file is created with mode `0600`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the destination exists or cannot be written.
    pub fn save_private(&self, path: &Path) -> Result<(), CapsuleError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        writeln!(file, "{}", self.expose_secret())?;
        Ok(())
    }

    /// Loads an age X25519 private identity from a file.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file cannot be read or
    /// [`CapsuleError::InvalidIdentity`] when its contents are invalid.
    pub fn load_private(path: &Path) -> Result<Self, CapsuleError> {
        Self::parse(fs::read_to_string(path)?.trim())
    }
}

#[derive(Debug)]
struct PendingArtifact {
    source: PathBuf,
    install_path: PathBuf,
    role: String,
    required: bool,
}

/// Creates a byte-preserving encrypted native session capsule.
///
/// # Errors
///
/// Returns an error when an artifact is forbidden or outside `provider_home`,
/// a recipient is invalid, or packaging and encryption fail.
pub fn create_encrypted(
    session: &NativeSession,
    provider_home: &Path,
    recipients: &[String],
    output: &Path,
) -> Result<NativeCapsule, CapsuleError> {
    create_encrypted_with_source(session, provider_home, recipients, output, None)
}

/// Creates an encrypted native session capsule with an optional source bundle.
///
/// # Errors
///
/// Returns an error when native or source artifacts fail policy, packaging, or
/// encryption.
pub fn create_encrypted_with_source(
    session: &NativeSession,
    provider_home: &Path,
    recipients: &[String],
    output: &Path,
    source: Option<SourceBundle<'_>>,
) -> Result<NativeCapsule, CapsuleError> {
    let recipients = recipients
        .iter()
        .map(|recipient| {
            x25519::Recipient::from_str(recipient).map_err(|_| CapsuleError::InvalidRecipient)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pending = collect_artifacts(session, provider_home)?;
    let capsule = build_manifest(session, &pending, source)?;

    if output.exists() {
        return Err(CapsuleError::OutputExists(output.to_path_buf()));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let file = temporary.reopen()?;
    let encryptor = Encryptor::with_recipients(
        recipients
            .iter()
            .map(|recipient| recipient as &dyn age::Recipient),
    )?;
    let age_writer = encryptor.wrap_output(file)?;
    let zstd_writer = zstd::Encoder::new(age_writer, 9)?;
    let mut tar = Builder::new(zstd_writer);
    tar.mode(tar::HeaderMode::Deterministic);
    append_bytes(
        &mut tar,
        Path::new(CAPSULE_MANIFEST),
        &serde_json::to_vec_pretty(&capsule)?,
    )?;
    for (index, artifact) in pending.iter().enumerate() {
        append_artifact(&mut tar, index, artifact)?;
    }
    if let Some(source) = source {
        append_file(&mut tar, Path::new("source/commits.bundle"), source.path)?;
    }
    verify_capture_stable(&pending, &capsule, source)?;
    let zstd_writer = tar.into_inner()?;
    let age_writer = zstd_writer.finish()?;
    age_writer.finish()?;
    temporary
        .persist_noclobber(output)
        .map_err(|error| error.error)?;
    Ok(capsule)
}

fn verify_capture_stable(
    pending: &[PendingArtifact],
    capsule: &NativeCapsule,
    source: Option<SourceBundle<'_>>,
) -> Result<(), CapsuleError> {
    for (pending, artifact) in pending.iter().zip(&capsule.artifacts) {
        if hash_path(&pending.source)? != artifact.sha256 {
            return Err(CapsuleError::ArtifactChanged(pending.source.clone()));
        }
    }
    if let Some(source) = source
        && hash_path(source.path)? != source.snapshot.bundle_sha256
    {
        return Err(CapsuleError::ArtifactChanged(source.path.to_path_buf()));
    }
    Ok(())
}

/// Decrypts, verifies, and installs a native session capsule.
///
/// All artifact hashes are verified before installation begins.
///
/// # Errors
///
/// Returns an error when decryption, archive validation, hash verification, or
/// installation fails.
pub fn restore_encrypted(
    input: &Path,
    identity: &DeviceIdentity,
    destination_home: &Path,
    expected_provider: Provider,
) -> Result<NativeCapsule, CapsuleError> {
    restore_encrypted_with_policy(
        input,
        identity,
        destination_home,
        RestorePolicy {
            expected_provider,
            destination_version: None,
            force_native: false,
            source_bundle_output: None,
        },
    )
}

/// Decrypts, verifies, checks compatibility, and installs a native session.
///
/// # Errors
///
/// Returns an error when decryption, archive validation, compatibility checks,
/// hash verification, or installation fails.
pub fn restore_encrypted_with_policy(
    input: &Path,
    identity: &DeviceIdentity,
    destination_home: &Path,
    policy: RestorePolicy<'_>,
) -> Result<NativeCapsule, CapsuleError> {
    let file = File::open(input)?;
    let decryptor = Decryptor::new_buffered(BufReader::new(file))?;
    let reader = decryptor.decrypt(std::iter::once(&identity.identity as &dyn age::Identity))?;
    let decoder = zstd::Decoder::new(reader)?;
    let mut archive = Archive::new(decoder);
    fs::create_dir_all(destination_home)?;
    let staging = tempfile::Builder::new()
        .prefix(".samesession-restore-")
        .tempdir_in(destination_home)?;
    restore_archive(&mut archive, staging.path(), destination_home, policy)
}

fn collect_artifacts(
    session: &NativeSession,
    provider_home: &Path,
) -> Result<Vec<PendingArtifact>, CapsuleError> {
    let provider_home = provider_home.canonicalize()?;
    let mut pending = Vec::new();
    let mut install_paths = BTreeSet::new();
    let mut total_size = 0_u64;
    for artifact in &session.artifacts {
        if !artifact.classification.is_exportable() {
            return Err(CapsuleError::ForbiddenArtifact {
                path: artifact.path.clone(),
                classification: artifact.classification,
            });
        }
        let source = artifact.path.canonicalize()?;
        if let Some(finding) = scan_path(&source)
            .map_err(|error| io::Error::other(error.to_string()))?
            .into_iter()
            .next()
        {
            return Err(CapsuleError::SecretFound {
                path: finding.path,
                kind: finding.kind,
            });
        }
        let install_path = source
            .strip_prefix(&provider_home)
            .map_err(|_| CapsuleError::OutsideProviderHome {
                path: source.clone(),
                home: provider_home.clone(),
            })?
            .to_path_buf();
        validate_relative(&install_path)?;
        if !install_paths.insert(install_path.clone()) {
            return Err(CapsuleError::DuplicateInstallPath(install_path));
        }
        total_size =
            total_size
                .checked_add(path_size(&source)?)
                .ok_or_else(|| CapsuleError::SizeLimit {
                    path: source.clone(),
                    limit: CAPSULE_LIMIT,
                })?;
        if total_size > CAPSULE_LIMIT {
            return Err(CapsuleError::SizeLimit {
                path: source,
                limit: CAPSULE_LIMIT,
            });
        }
        pending.push(PendingArtifact {
            source,
            install_path,
            role: artifact.role.clone(),
            required: artifact.classification == ArtifactClassification::Required,
        });
    }
    pending.sort_by(|left, right| left.install_path.cmp(&right.install_path));
    Ok(pending)
}

fn path_size(path: &Path) -> Result<u64, CapsuleError> {
    let mut total = 0_u64;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            return Err(CapsuleError::UnsupportedEntry(entry.path().to_path_buf()));
        }
        if entry.file_type().is_file() {
            let size = entry.metadata()?.len();
            if size > SINGLE_FILE_LIMIT {
                return Err(CapsuleError::SizeLimit {
                    path: entry.path().to_path_buf(),
                    limit: SINGLE_FILE_LIMIT,
                });
            }
            total = total
                .checked_add(size)
                .ok_or_else(|| CapsuleError::SizeLimit {
                    path: path.to_path_buf(),
                    limit: CAPSULE_LIMIT,
                })?;
        } else if !entry.file_type().is_dir() {
            return Err(CapsuleError::UnsupportedEntry(entry.path().to_path_buf()));
        }
    }
    Ok(total)
}

fn build_manifest(
    session: &NativeSession,
    artifacts: &[PendingArtifact],
    source: Option<SourceBundle<'_>>,
) -> Result<NativeCapsule, CapsuleError> {
    let mut manifest_artifacts = Vec::new();
    for artifact in artifacts {
        manifest_artifacts.push(CapsuleArtifact {
            logical_role: artifact.role.clone(),
            install_path: artifact.install_path.clone(),
            sha256: hash_path(&artifact.source)?,
            required: artifact.required,
            rewrite_policy: RewritePolicy::BytePreserve,
        });
    }
    Ok(NativeCapsule {
        schema: NativeCapsule::SCHEMA.to_owned(),
        provider: session.provider,
        source_version: session.agent_version.clone(),
        native_session_id: session.id.clone(),
        original_cwd: session.cwd.clone(),
        artifacts: manifest_artifacts,
        repository: source.map(|source| source.snapshot.clone()),
    })
}

fn append_artifact<W: Write>(
    tar: &mut Builder<W>,
    index: usize,
    artifact: &PendingArtifact,
) -> Result<(), CapsuleError> {
    let root = PathBuf::from(ARTIFACTS_PREFIX).join(index.to_string());
    if artifact.source.is_file() {
        append_file(tar, &root.join("content"), &artifact.source)?;
    } else {
        append_directory(tar, &root.join("content"))?;
        let mut entries = WalkDir::new(&artifact.source)
            .follow_links(false)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        for entry in entries {
            let relative = entry
                .path()
                .strip_prefix(&artifact.source)
                .expect("walk entries are rooted under the artifact");
            if relative.as_os_str().is_empty() {
                continue;
            }
            validate_relative(relative)?;
            if entry.file_type().is_symlink() {
                return Err(CapsuleError::UnsupportedEntry(entry.path().to_path_buf()));
            }
            if entry.file_type().is_dir() {
                append_directory(tar, &root.join("content").join(relative))?;
            } else if entry.file_type().is_file() {
                append_file(tar, &root.join("content").join(relative), entry.path())?;
            } else {
                return Err(CapsuleError::UnsupportedEntry(entry.path().to_path_buf()));
            }
        }
    }
    Ok(())
}

fn append_bytes<W: Write>(
    tar: &mut Builder<W>,
    path: &Path,
    bytes: &[u8],
) -> Result<(), CapsuleError> {
    let mut header = deterministic_header(bytes.len().try_into().map_err(io::Error::other)?);
    tar.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn append_file<W: Write>(
    tar: &mut Builder<W>,
    archive_path: &Path,
    source: &Path,
) -> Result<(), CapsuleError> {
    validate_relative(archive_path)?;
    let mut file = File::open(source)?;
    let size = file.metadata()?.len();
    if size > SINGLE_FILE_LIMIT {
        return Err(CapsuleError::SizeLimit {
            path: source.to_path_buf(),
            limit: SINGLE_FILE_LIMIT,
        });
    }
    let mut header = deterministic_header(size);
    tar.append_data(&mut header, archive_path, &mut file)?;
    Ok(())
}

fn append_directory<W: Write>(tar: &mut Builder<W>, path: &Path) -> Result<(), CapsuleError> {
    validate_relative(path)?;
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_size(0);
    header.set_mode(0o755);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, path, io::empty())?;
    Ok(())
}

fn deterministic_header(size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(size);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    header
}

fn restore_archive<R: Read>(
    archive: &mut Archive<R>,
    staging: &Path,
    destination_home: &Path,
    policy: RestorePolicy<'_>,
) -> Result<NativeCapsule, CapsuleError> {
    let mut seen = BTreeSet::new();
    let mut manifest = None;
    let mut total_size = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        validate_relative(&path)?;
        if !seen.insert(path.clone()) {
            return Err(CapsuleError::DuplicatePath(path));
        }
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            return Err(CapsuleError::UnsupportedEntry(path));
        }
        let size = entry.header().size()?;
        if size > SINGLE_FILE_LIMIT {
            return Err(CapsuleError::SizeLimit {
                path,
                limit: SINGLE_FILE_LIMIT,
            });
        }
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| CapsuleError::SizeLimit {
                path: PathBuf::from("capsule"),
                limit: CAPSULE_LIMIT,
            })?;
        if total_size > CAPSULE_LIMIT {
            return Err(CapsuleError::SizeLimit {
                path: PathBuf::from("capsule"),
                limit: CAPSULE_LIMIT,
            });
        }
        if path == Path::new(CAPSULE_MANIFEST) {
            manifest = Some(serde_json::from_reader(&mut entry)?);
        } else if !entry.unpack_in(staging)? {
            return Err(CapsuleError::UnsafePath(path));
        }
    }
    let capsule: NativeCapsule = manifest.ok_or(CapsuleError::MissingManifest)?;
    if capsule.schema != NativeCapsule::SCHEMA {
        return Err(CapsuleError::UnsupportedSchema(capsule.schema));
    }
    if capsule.provider != policy.expected_provider {
        return Err(CapsuleError::ProviderMismatch {
            expected: policy.expected_provider,
            actual: capsule.provider,
        });
    }
    check_version_compatibility(&capsule, policy)?;
    verify_source_bundle(&capsule, staging)?;
    preflight_source_output(&capsule, policy.source_bundle_output)?;
    verify_and_install(&capsule, staging, destination_home)?;
    copy_source_bundle(&capsule, staging, policy.source_bundle_output)?;
    Ok(capsule)
}

fn verify_source_bundle(capsule: &NativeCapsule, staging: &Path) -> Result<(), CapsuleError> {
    let Some(repository) = &capsule.repository else {
        return Ok(());
    };
    let bundle = staging.join("source/commits.bundle");
    if !bundle.is_file() {
        return Err(CapsuleError::MissingSourceBundle);
    }
    if hash_path(&bundle)? != repository.bundle_sha256 {
        return Err(CapsuleError::HashMismatch(PathBuf::from(
            "source/commits.bundle",
        )));
    }
    Ok(())
}

fn preflight_source_output(
    capsule: &NativeCapsule,
    output: Option<&Path>,
) -> Result<(), CapsuleError> {
    let (Some(_), Some(output)) = (&capsule.repository, output) else {
        return Ok(());
    };
    if output.exists() {
        return Err(CapsuleError::DestinationExists(output.to_path_buf()));
    }
    Ok(())
}

fn copy_source_bundle(
    capsule: &NativeCapsule,
    staging: &Path,
    output: Option<&Path>,
) -> Result<(), CapsuleError> {
    let (Some(_), Some(output)) = (&capsule.repository, output) else {
        return Ok(());
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(staging.join("source/commits.bundle"), output)?;
    Ok(())
}

fn check_version_compatibility(
    capsule: &NativeCapsule,
    policy: RestorePolicy<'_>,
) -> Result<(), CapsuleError> {
    let (Some(source), Some(destination)) = (
        capsule.source_version.as_deref(),
        policy.destination_version,
    ) else {
        return Ok(());
    };
    if policy.force_native || compatible_versions(source, destination) {
        return Ok(());
    }
    Err(CapsuleError::IncompatibleVersion {
        source_version: source.to_owned(),
        destination_version: destination.to_owned(),
    })
}

fn compatible_versions(source: &str, destination: &str) -> bool {
    match (parse_version(source), parse_version(destination)) {
        (Some(source), Some(destination)) => {
            source.major == destination.major && source.minor == destination.minor
        }
        _ => source.trim() == destination.trim(),
    }
}

fn parse_version(value: &str) -> Option<Version> {
    value
        .split_whitespace()
        .find_map(|part| Version::parse(part.trim_start_matches('v')).ok())
}

fn verify_and_install(
    capsule: &NativeCapsule,
    staging: &Path,
    destination_home: &Path,
) -> Result<(), CapsuleError> {
    for (index, artifact) in capsule.artifacts.iter().enumerate() {
        validate_relative(&artifact.install_path)?;
        let source = staging
            .join(ARTIFACTS_PREFIX)
            .join(index.to_string())
            .join("content");
        if hash_path(&source)? != artifact.sha256 {
            return Err(CapsuleError::HashMismatch(artifact.install_path.clone()));
        }
    }
    fs::create_dir_all(destination_home)?;
    let mut moves = Vec::new();
    for artifact in &capsule.artifacts {
        let destination = destination_home.join(&artifact.install_path);
        reject_symlink_ancestors(destination_home, &artifact.install_path)?;
        if destination.exists() {
            return Err(CapsuleError::DestinationExists(destination));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
    }
    for (index, artifact) in capsule.artifacts.iter().enumerate() {
        let source = staging
            .join(ARTIFACTS_PREFIX)
            .join(index.to_string())
            .join("content");
        let destination = destination_home.join(&artifact.install_path);
        if let Err(error) = fs::rename(&source, &destination) {
            for (installed_source, installed_destination) in moves.iter().rev() {
                let _ = fs::rename(installed_destination, installed_source);
            }
            return Err(error.into());
        }
        moves.push((source, destination));
    }
    Ok(())
}

fn reject_symlink_ancestors(root: &Path, relative: &Path) -> Result<(), CapsuleError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(CapsuleError::UnsafePath(relative.to_path_buf()));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CapsuleError::UnsafePath(relative.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn hash_path(path: &Path) -> Result<String, CapsuleError> {
    let mut hasher = Sha256::new();
    if path.is_file() {
        hash_file(&mut hasher, path)?;
    } else {
        let mut entries = WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        for entry in entries {
            if entry.file_type().is_symlink() {
                return Err(CapsuleError::UnsupportedEntry(entry.path().to_path_buf()));
            }
            let relative = entry
                .path()
                .strip_prefix(path)
                .expect("walk entries are rooted under path");
            hasher.update(relative.as_os_str().as_encoded_bytes());
            if entry.file_type().is_file() {
                hash_file(&mut hasher, entry.path())?;
            }
        }
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

fn hash_file(hasher: &mut Sha256, path: &Path) -> Result<(), CapsuleError> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}

fn validate_relative(path: &Path) -> Result<(), CapsuleError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(CapsuleError::UnsafePath(path.to_path_buf()));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CapsuleError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use samesession_core::{
        ArtifactClassification, NativeArtifact, NativeSession, Provider, RepositorySnapshot,
    };
    use tempfile::tempdir;

    use super::{
        CapsuleError, DeviceIdentity, RestorePolicy, SourceBundle, create_encrypted,
        create_encrypted_with_source, restore_encrypted, restore_encrypted_with_policy,
    };

    fn session(path: &Path, classification: ArtifactClassification) -> NativeSession {
        NativeSession {
            provider: Provider::Codex,
            id: "session-1".to_owned(),
            transcript_path: path.to_path_buf(),
            cwd: Some(PathBuf::from("/repo")),
            agent_version: Some("1.0.0".to_owned()),
            timestamp: None,
            artifacts: vec![NativeArtifact {
                role: "primary-transcript".to_owned(),
                path: path.to_path_buf(),
                classification,
            }],
        }
    }

    use std::path::{Path, PathBuf};

    #[test]
    fn encrypted_round_trip_preserves_bytes() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let transcript = source.path().join("sessions/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        let bytes = b"{\"binary\":\"\\u0000\"}\n\x00\xff";
        fs::write(&transcript, bytes).expect("transcript");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");

        create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &capsule,
        )
        .expect("create");
        restore_encrypted(&capsule, &identity, destination.path(), Provider::Codex)
            .expect("restore");

        assert_eq!(
            fs::read(destination.path().join("sessions/session.jsonl")).expect("restored"),
            bytes
        );
    }

    #[test]
    fn rejects_non_exportable_artifact() {
        let source = tempdir().expect("source");
        let transcript = source.path().join("auth.json");
        fs::write(&transcript, "{}").expect("artifact");
        let identity = DeviceIdentity::generate();

        let error = create_encrypted(
            &session(&transcript, ArtifactClassification::Unsafe),
            source.path(),
            &[identity.recipient()],
            &source.path().join("session.age"),
        )
        .expect_err("must reject unsafe artifact");

        assert!(matches!(error, CapsuleError::ForbiddenArtifact { .. }));
    }

    #[test]
    fn rejects_high_confidence_secret_in_transcript() {
        let source = tempdir().expect("source");
        let transcript = source.path().join("sessions/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        fs::write(&transcript, "token=ghp_abcdefghijklmnopqrstuvwxyz1234").expect("artifact");
        let identity = DeviceIdentity::generate();

        let error = create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &source.path().join("session.age"),
        )
        .expect_err("must reject secret");

        assert!(matches!(error, CapsuleError::SecretFound { .. }));
    }

    #[test]
    fn rejects_artifact_outside_provider_home() {
        let provider = tempdir().expect("provider");
        let outside = tempdir().expect("outside");
        let transcript = outside.path().join("session.jsonl");
        fs::write(&transcript, "{}").expect("artifact");
        let identity = DeviceIdentity::generate();

        let error = create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            provider.path(),
            &[identity.recipient()],
            &provider.path().join("session.age"),
        )
        .expect_err("must reject outside artifact");

        assert!(matches!(error, CapsuleError::OutsideProviderHome { .. }));
    }

    #[test]
    fn restores_directory_artifacts() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let directory = source.path().join("tasks/session-1");
        fs::create_dir_all(directory.join("nested")).expect("directory");
        fs::write(directory.join("nested/task.json"), b"task bytes").expect("artifact");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");

        create_encrypted(
            &session(&directory, ArtifactClassification::Associated),
            source.path(),
            &[identity.recipient()],
            &capsule,
        )
        .expect("create");
        restore_encrypted(&capsule, &identity, destination.path(), Provider::Codex)
            .expect("restore");

        assert_eq!(
            fs::read(destination.path().join("tasks/session-1/nested/task.json"))
                .expect("restored"),
            b"task bytes"
        );
    }

    #[test]
    fn refuses_provider_mismatch_before_installing() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let transcript = source.path().join("sessions/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        fs::write(&transcript, "{}").expect("artifact");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");
        create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &capsule,
        )
        .expect("create");

        let error = restore_encrypted(
            &capsule,
            &identity,
            destination.path(),
            Provider::ClaudeCode,
        )
        .expect_err("must reject provider mismatch");

        assert!(matches!(error, CapsuleError::ProviderMismatch { .. }));
        assert!(!destination.path().join("sessions/session.jsonl").exists());
    }

    #[test]
    fn refuses_incompatible_native_version_before_installing() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let transcript = source.path().join("sessions/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        fs::write(&transcript, "{}").expect("artifact");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");
        let mut native_session = session(&transcript, ArtifactClassification::Required);
        native_session.agent_version = Some("1.2.3".to_owned());
        create_encrypted(
            &native_session,
            source.path(),
            &[identity.recipient()],
            &capsule,
        )
        .expect("create");

        let error = restore_encrypted_with_policy(
            &capsule,
            &identity,
            destination.path(),
            RestorePolicy {
                expected_provider: Provider::Codex,
                destination_version: Some("codex-cli 1.3.0"),
                force_native: false,
                source_bundle_output: None,
            },
        )
        .expect_err("must reject incompatible version");

        assert!(matches!(error, CapsuleError::IncompatibleVersion { .. }));
        assert!(!destination.path().join("sessions/session.jsonl").exists());
    }

    #[test]
    fn refuses_existing_restore_destination() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let transcript = source.path().join("sessions/session.jsonl");
        let existing = destination.path().join("sessions/session.jsonl");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("source sessions");
        fs::create_dir_all(existing.parent().expect("parent")).expect("destination sessions");
        fs::write(&transcript, "new").expect("artifact");
        fs::write(&existing, "existing").expect("existing");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");
        create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &capsule,
        )
        .expect("create");

        let error = restore_encrypted(&capsule, &identity, destination.path(), Provider::Codex)
            .expect_err("must reject collision");

        assert!(matches!(error, CapsuleError::DestinationExists(_)));
        assert_eq!(fs::read_to_string(existing).expect("existing"), "existing");
    }

    #[test]
    fn refuses_to_replace_checkpoint_output() {
        let source = tempdir().expect("source");
        let transcript = source.path().join("sessions/session.jsonl");
        let output = source.path().join("session.age");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        fs::write(&transcript, "{}").expect("artifact");
        fs::write(&output, "existing").expect("existing");
        let identity = DeviceIdentity::generate();

        let error = create_encrypted(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &output,
        )
        .expect_err("must reject existing output");

        assert!(matches!(error, CapsuleError::OutputExists(_)));
        assert_eq!(fs::read_to_string(output).expect("existing"), "existing");
    }

    #[cfg(unix)]
    #[test]
    fn saves_private_identity_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempdir().expect("temp");
        let path = temp.path().join("identity.age");
        let identity = DeviceIdentity::generate();
        identity.save_private(&path).expect("save");

        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn encrypted_round_trip_preserves_source_bundle() {
        let source = tempdir().expect("source");
        let destination = tempdir().expect("destination");
        let transcript = source.path().join("sessions/session.jsonl");
        let bundle = source.path().join("commits.bundle");
        let restored_bundle = destination.path().join("commits.bundle");
        fs::create_dir_all(transcript.parent().expect("parent")).expect("sessions");
        fs::write(&transcript, "{}").expect("transcript");
        fs::write(&bundle, b"git bundle bytes").expect("bundle");
        let identity = DeviceIdentity::generate();
        let capsule = source.path().join("session.age");
        let snapshot = RepositorySnapshot {
            root_hint: "repo".to_owned(),
            head_oid: "abc123".to_owned(),
            snapshot_oid: "def456".to_owned(),
            bundle_ref: "refs/samesession/capture/def456".to_owned(),
            head_ref: Some("main".to_owned()),
            dirty: false,
            bundle_sha256: super::hash_path(&bundle).expect("hash"),
        };
        create_encrypted_with_source(
            &session(&transcript, ArtifactClassification::Required),
            source.path(),
            &[identity.recipient()],
            &capsule,
            Some(SourceBundle {
                path: &bundle,
                snapshot: &snapshot,
            }),
        )
        .expect("create");

        let restored = restore_encrypted_with_policy(
            &capsule,
            &identity,
            destination.path(),
            RestorePolicy {
                expected_provider: Provider::Codex,
                destination_version: None,
                force_native: false,
                source_bundle_output: Some(&restored_bundle),
            },
        )
        .expect("restore");

        assert_eq!(restored.repository, Some(snapshot));
        assert_eq!(
            fs::read(restored_bundle).expect("restored bundle"),
            b"git bundle bytes"
        );
    }
}
