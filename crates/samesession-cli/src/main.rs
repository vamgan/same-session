use std::{
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use samesession_adapter_claude::ClaudeAdapter;
use samesession_adapter_codex::CodexAdapter;
use samesession_capsule::{
    DeviceIdentity, RestorePolicy, SourceBundle, create_encrypted_with_source,
    restore_encrypted_with_policy,
};
use samesession_core::{NativeCapsule, NativeSession, SessionAdapter};
use samesession_git::{GitStore, StoredLease};
use samesession_workspace::{capture_source, restore_source};

#[derive(Debug, Parser)]
#[command(
    name = "samesession",
    version,
    about = "Move native coding-agent sessions between machines"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create and inspect this device's encryption identity.
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Create an encrypted native session capsule.
    Checkpoint {
        /// Provider-owned native session ID.
        id: String,
        #[arg(long)]
        provider: ProviderArg,
        /// One or more age X25519 recipients.
        #[arg(long, required = true)]
        recipient: Vec<String>,
        /// Optional standalone encrypted capsule output path.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Git repository used for isolated checkpoint storage.
        #[arg(long)]
        repository: Option<PathBuf>,
        /// Existing portable session ID when appending a checkpoint.
        #[arg(long)]
        portable_session: Option<String>,
        /// Push the checkpoint to this Git remote after creation.
        #[arg(long)]
        push: Option<String>,
        /// Emit stable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Restore an encrypted capsule into a provider's native session home.
    Restore {
        /// Encrypted capsule path.
        capsule: PathBuf,
        #[arg(long)]
        provider: ProviderArg,
        /// Private age identity path.
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Read the positional argument as a checkpoint ref or OID in this repository.
        #[arg(long)]
        repository: Option<PathBuf>,
        /// Bypass provider-native version compatibility checks.
        #[arg(long)]
        force_native: bool,
        /// Emit stable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List local and fetched Git checkpoint tips.
    List {
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Inspect public checkpoint metadata without decrypting it.
    Inspect {
        revision: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Fetch only `SameSession` checkpoint refs.
    Fetch {
        #[arg(default_value = "origin")]
        remote: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
    },
    /// Push one append-only checkpoint chain.
    Push {
        portable_session: String,
        #[arg(default_value = "origin")]
        remote: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
    },
    /// Checkpoint a session and mark it in transit.
    Move {
        id: String,
        #[arg(long)]
        provider: ProviderArg,
        #[arg(long, required = true)]
        recipient: Vec<String>,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        portable_session: Option<String>,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long)]
        push: Option<String>,
        #[arg(long, default_value_t = 86_400)]
        lease_ttl: i64,
        #[arg(long)]
        json: bool,
    },
    /// Restore, acquire ownership, and optionally launch a native session.
    Resume {
        revision: String,
        #[arg(long)]
        provider: ProviderArg,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        takeover_reason: Option<String>,
        #[arg(long, default_value_t = 3_600)]
        lease_ttl: i64,
        #[arg(long)]
        no_launch: bool,
        /// Bypass provider-native version compatibility checks.
        #[arg(long)]
        force_native: bool,
        #[arg(long)]
        json: bool,
    },
    /// Inspect and manage advisory session ownership.
    Lease {
        #[command(subcommand)]
        command: LeaseCommand,
    },
    /// Show detected agent homes and native session counts.
    Status {
        /// Emit stable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Discover and inspect native sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// Generate a new device identity without replacing an existing one.
    Init {
        /// Private identity destination.
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Emit stable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Print this device's public recipient.
    Show {
        /// Private identity path.
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Emit stable JSON output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum LeaseCommand {
    /// Show the latest known lease.
    Status {
        portable_session: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Acquire, renew, or explicitly take over a lease.
    Acquire {
        portable_session: String,
        #[arg(long)]
        source_checkpoint: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long, default_value_t = 3_600)]
        ttl: i64,
        #[arg(long)]
        takeover_reason: Option<String>,
        #[arg(long)]
        push: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Take over a lease and record the required reason.
    Takeover {
        portable_session: String,
        #[arg(long)]
        source_checkpoint: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long, default_value_t = 3_600)]
        ttl: i64,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        push: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Release a lease owned by this device.
    Release {
        portable_session: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long)]
        push: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SessionsCommand {
    /// List locally discoverable native sessions.
    List {
        #[arg(long)]
        provider: Option<ProviderArg>,
        #[arg(long)]
        json: bool,
    },
    /// Inspect a native session and its known artifacts.
    Inspect {
        id: String,
        #[arg(long)]
        provider: Option<ProviderArg>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderArg {
    Codex,
    Claude,
}

fn adapters(provider: Option<ProviderArg>) -> Vec<Box<dyn SessionAdapter>> {
    match provider {
        Some(ProviderArg::Codex) => vec![Box::new(CodexAdapter::detect())],
        Some(ProviderArg::Claude) => vec![Box::new(ClaudeAdapter::detect())],
        None => vec![
            Box::new(CodexAdapter::detect()),
            Box::new(ClaudeAdapter::detect()),
        ],
    }
}

fn adapter(provider: ProviderArg) -> Box<dyn SessionAdapter> {
    match provider {
        ProviderArg::Codex => Box::new(CodexAdapter::detect()),
        ProviderArg::Claude => Box::new(ClaudeAdapter::detect()),
    }
}

fn default_identity_path() -> Result<PathBuf> {
    let config = dirs::config_dir().context("unable to locate the user configuration directory")?;
    Ok(config.join("same-session").join("identity.age"))
}

fn identity_path(path: Option<PathBuf>) -> Result<PathBuf> {
    path.map_or_else(default_identity_path, Ok)
}

fn print_device(identity: &DeviceIdentity, path: &Path, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "identity_path": path,
                "device_id": identity.device_id(),
                "recipient": identity.recipient(),
            }))?
        );
    } else {
        println!("{}", identity.recipient());
    }
    Ok(())
}

fn discover(provider: Option<ProviderArg>) -> Result<Vec<NativeSession>> {
    let mut sessions = Vec::new();
    for adapter in adapters(provider) {
        sessions.extend(
            adapter
                .discover()
                .with_context(|| format!("failed to discover {} sessions", adapter.provider()))?,
        );
    }
    sessions.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
    Ok(sessions)
}

fn print_sessions(sessions: &[NativeSession]) {
    for session in sessions {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            session.provider,
            session.id,
            session.agent_version.as_deref().unwrap_or("-"),
            session
                .cwd
                .as_deref()
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
            session.transcript_path.display()
        );
    }
}

fn run_device(command: DeviceCommand) -> Result<()> {
    match command {
        DeviceCommand::Init { identity, json } => {
            let path = identity_path(identity)?;
            let identity = DeviceIdentity::generate();
            identity.save_private(&path).with_context(|| {
                format!("failed to save private identity at {}", path.display())
            })?;
            print_device(&identity, &path, json)?;
        }
        DeviceCommand::Show { identity, json } => {
            let path = identity_path(identity)?;
            let identity = DeviceIdentity::load_private(&path).with_context(|| {
                format!("failed to load private identity at {}", path.display())
            })?;
            print_device(&identity, &path, json)?;
        }
    }
    Ok(())
}

struct CheckpointOptions {
    id: String,
    provider: ProviderArg,
    recipient: Vec<String>,
    output: Option<PathBuf>,
    repository: Option<PathBuf>,
    portable_session: Option<String>,
    push: Option<String>,
    json: bool,
}

fn run_checkpoint(options: CheckpointOptions) -> Result<()> {
    if options.output.is_none() && options.repository.is_none() {
        bail!("checkpoint requires --output, --repository, or both");
    }
    let adapter = adapter(options.provider);
    let session = adapter.inspect(&options.id).with_context(|| {
        format!(
            "failed to inspect {} session {}",
            adapter.provider(),
            options.id
        )
    })?;
    let temporary = tempfile::tempdir().context("failed to create checkpoint staging directory")?;
    let source_bundle_path = temporary.path().join("commits.bundle");
    let source_snapshot = options
        .repository
        .as_deref()
        .map(|repository| capture_source(repository, &source_bundle_path))
        .transpose()?;
    if source_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.dirty)
    {
        bail!(
            "repository has uncommitted changes; commit them before checkpointing until dirty-workspace capture is enabled"
        );
    }
    let capsule_path = options
        .output
        .as_deref()
        .map_or_else(|| temporary.path().join("payload.age"), Path::to_path_buf);
    let source = source_snapshot.as_ref().map(|snapshot| SourceBundle {
        path: &source_bundle_path,
        snapshot,
    });
    let capsule = create_encrypted_with_source(
        &session,
        adapter.home(),
        &options.recipient,
        &capsule_path,
        source,
    )
    .with_context(|| format!("failed to create checkpoint {}", capsule_path.display()))?;
    let stored = options
        .repository
        .as_deref()
        .map(|repository| {
            let store = GitStore::open(repository)?;
            let creator = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown".to_owned());
            let checkpoint =
                store.append(&capsule_path, options.portable_session.as_deref(), &creator)?;
            if let Some(remote) = options.push.as_deref() {
                store.push(remote, &checkpoint.public.portable_session_id)?;
            }
            Ok::<_, anyhow::Error>(checkpoint)
        })
        .transpose()?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "capsule": capsule,
                "checkpoint": stored,
                "output": options.output,
            }))?
        );
    } else {
        if let Some(output) = options.output {
            println!(
                "Created encrypted {} checkpoint for {} at {}",
                capsule.provider,
                capsule.native_session_id,
                output.display()
            );
        }
        if let Some(stored) = stored {
            println!(
                "Stored checkpoint {} on {}",
                stored.public.checkpoint_id, stored.reference
            );
        }
    }
    Ok(())
}

fn run_restore(
    capsule: &Path,
    provider: ProviderArg,
    identity: Option<PathBuf>,
    repository: Option<&Path>,
    force_native: bool,
    json: bool,
) -> Result<()> {
    let restored = restore_capsule(capsule, provider, identity, repository, force_native)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&restored)?);
    } else {
        println!(
            "Restored native {} session {}",
            restored.provider, restored.native_session_id
        );
    }
    Ok(())
}

fn restore_capsule(
    capsule: &Path,
    provider: ProviderArg,
    identity: Option<PathBuf>,
    repository: Option<&Path>,
    force_native: bool,
) -> Result<NativeCapsule> {
    let path = identity_path(identity)?;
    let identity = DeviceIdentity::load_private(&path)
        .with_context(|| format!("failed to load private identity at {}", path.display()))?;
    let adapter = adapter(provider);
    let temporary = tempfile::tempdir().context("failed to create restore staging directory")?;
    let source_bundle = repository.map(|_| temporary.path().join("commits.bundle"));
    let capsule_path = repository.map_or_else(
        || Ok(capsule.to_path_buf()),
        |repository| {
            let extracted = temporary.path().join("payload.age");
            GitStore::open(repository)?.extract_payload(&capsule.to_string_lossy(), &extracted)?;
            Ok::<_, anyhow::Error>(extracted)
        },
    )?;
    let destination_version = detect_agent_version(provider);
    restore_encrypted_with_policy(
        &capsule_path,
        &identity,
        adapter.home(),
        RestorePolicy {
            expected_provider: adapter.provider(),
            destination_version: destination_version.as_deref(),
            force_native,
            source_bundle_output: source_bundle.as_deref(),
        },
    )
    .with_context(|| format!("failed to restore checkpoint {}", capsule.display()))
    .and_then(|restored| {
        if let (Some(repository), Some(snapshot), Some(bundle)) = (
            repository,
            restored.repository.as_ref(),
            source_bundle.as_deref(),
        ) {
            let length = 16.min(snapshot.head_oid.len());
            let segment = format!("source_{}", &snapshot.head_oid[..length]);
            restore_source(repository, bundle, snapshot, &segment)?;
        }
        Ok(restored)
    })
}

fn detect_agent_version(provider: ProviderArg) -> Option<String> {
    let executable = match provider {
        ProviderArg::Codex => "codex",
        ProviderArg::Claude => "claude",
    };
    let output = ProcessCommand::new(executable)
        .arg("--version")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn run_list(repository: &Path, json: bool) -> Result<()> {
    let checkpoints = GitStore::open(repository)?.list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&checkpoints)?);
    } else {
        for checkpoint in checkpoints {
            println!(
                "{}\t{}\t{}\t{}",
                checkpoint.public.created_at,
                checkpoint.public.checkpoint_id,
                checkpoint.public.portable_session_id,
                checkpoint.reference
            );
        }
    }
    Ok(())
}

fn run_inspect(repository: &Path, revision: &str, json: bool) -> Result<()> {
    let checkpoint = GitStore::open(repository)?.inspect(revision)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&checkpoint)?);
    } else {
        println!(
            "{}\t{}\t{}\t{}\t{} bytes",
            checkpoint.public.created_at,
            checkpoint.public.checkpoint_id,
            checkpoint.public.portable_session_id,
            checkpoint.reference,
            checkpoint.public.payload_bytes
        );
    }
    Ok(())
}

fn run_fetch(repository: &Path, remote: &str) -> Result<()> {
    GitStore::open(repository)?.fetch(remote)?;
    Ok(())
}

fn run_push(repository: &Path, remote: &str, portable_session: &str) -> Result<()> {
    GitStore::open(repository)?.push(remote, portable_session)?;
    Ok(())
}

struct MoveOptions {
    id: String,
    provider: ProviderArg,
    recipient: Vec<String>,
    repository: PathBuf,
    portable_session: Option<String>,
    identity: Option<PathBuf>,
    push: Option<String>,
    lease_ttl: i64,
    json: bool,
}

fn run_move(options: MoveOptions) -> Result<()> {
    let identity_path = identity_path(options.identity)?;
    let identity = DeviceIdentity::load_private(&identity_path).with_context(|| {
        format!(
            "failed to load private identity at {}",
            identity_path.display()
        )
    })?;
    let adapter = adapter(options.provider);
    let session = adapter.inspect(&options.id).with_context(|| {
        format!(
            "failed to inspect {} session {}",
            adapter.provider(),
            options.id
        )
    })?;
    let temporary = tempfile::tempdir().context("failed to create move staging directory")?;
    let payload = temporary.path().join("payload.age");
    let source_bundle_path = temporary.path().join("commits.bundle");
    let source_snapshot = capture_source(&options.repository, &source_bundle_path)?;
    if source_snapshot.dirty {
        bail!(
            "repository has uncommitted changes; commit them before moving until dirty-workspace capture is enabled"
        );
    }
    create_encrypted_with_source(
        &session,
        adapter.home(),
        &options.recipient,
        &payload,
        Some(SourceBundle {
            path: &source_bundle_path,
            snapshot: &source_snapshot,
        }),
    )?;
    let store = GitStore::open(&options.repository)?;
    let checkpoint = store.append(
        &payload,
        options.portable_session.as_deref(),
        &identity.device_id(),
    )?;
    let lease = store.acquire_lease(
        &checkpoint.public.portable_session_id,
        "in_transit",
        &checkpoint.oid,
        options.lease_ttl,
        Some("session moved from source device"),
    )?;
    if let Some(remote) = options.push.as_deref() {
        store.push(remote, &checkpoint.public.portable_session_id)?;
        store.push_lease(remote, &checkpoint.public.portable_session_id)?;
    }
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checkpoint": checkpoint,
                "lease": lease,
            }))?
        );
    } else {
        println!(
            "Moved checkpoint {} as {}",
            checkpoint.public.checkpoint_id, checkpoint.public.portable_session_id
        );
    }
    Ok(())
}

fn print_lease(lease: Option<&StoredLease>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&lease)?);
    } else if let Some(lease) = lease {
        println!(
            "{}\t{}\t{}\t{}",
            lease.lease.portable_session_id,
            lease.lease.holder_device_id,
            lease.lease.expires_at,
            lease.lease.source_checkpoint
        );
    } else {
        println!("No lease found");
    }
    Ok(())
}

struct LeaseAcquireOptions {
    portable_session: String,
    source_checkpoint: String,
    repository: PathBuf,
    identity: Option<PathBuf>,
    ttl: i64,
    takeover_reason: Option<String>,
    push: Option<String>,
    json: bool,
}

fn run_lease_acquire(options: LeaseAcquireOptions) -> Result<()> {
    let path = identity_path(options.identity)?;
    let identity = DeviceIdentity::load_private(&path)?;
    let store = GitStore::open(options.repository)?;
    let lease = store.acquire_lease(
        &options.portable_session,
        &identity.device_id(),
        &options.source_checkpoint,
        options.ttl,
        options.takeover_reason.as_deref(),
    )?;
    if let Some(remote) = options.push {
        store.push_lease(&remote, &options.portable_session)?;
    }
    print_lease(Some(&lease), options.json)
}

fn run_lease(command: LeaseCommand) -> Result<()> {
    match command {
        LeaseCommand::Status {
            portable_session,
            repository,
            json,
        } => {
            let lease = GitStore::open(repository)?.lease_status(&portable_session)?;
            print_lease(lease.as_ref(), json)
        }
        LeaseCommand::Acquire {
            portable_session,
            source_checkpoint,
            repository,
            identity,
            ttl,
            takeover_reason,
            push,
            json,
        } => run_lease_acquire(LeaseAcquireOptions {
            portable_session,
            source_checkpoint,
            repository,
            identity,
            ttl,
            takeover_reason,
            push,
            json,
        }),
        LeaseCommand::Takeover {
            portable_session,
            source_checkpoint,
            repository,
            identity,
            ttl,
            reason,
            push,
            json,
        } => run_lease_acquire(LeaseAcquireOptions {
            portable_session,
            source_checkpoint,
            repository,
            identity,
            ttl,
            takeover_reason: Some(reason),
            push,
            json,
        }),
        LeaseCommand::Release {
            portable_session,
            repository,
            identity,
            push,
            json,
        } => {
            let path = identity_path(identity)?;
            let identity = DeviceIdentity::load_private(&path)?;
            let store = GitStore::open(repository)?;
            let lease = store.release_lease(&portable_session, &identity.device_id())?;
            if let Some(remote) = push {
                store.push_lease(&remote, &portable_session)?;
            }
            print_lease(Some(&lease), json)
        }
    }
}

struct ResumeOptions {
    revision: String,
    provider: ProviderArg,
    repository: PathBuf,
    identity: Option<PathBuf>,
    remote: Option<String>,
    takeover_reason: Option<String>,
    lease_ttl: i64,
    no_launch: bool,
    force_native: bool,
    json: bool,
}

fn run_resume(options: ResumeOptions) -> Result<()> {
    let path = identity_path(options.identity.clone())?;
    let identity = DeviceIdentity::load_private(&path)?;
    let store = GitStore::open(&options.repository)?;
    if let Some(remote) = options.remote.as_deref() {
        store.fetch(remote)?;
    }
    let checkpoint = store.inspect(&options.revision)?;
    let status = store.lease_status(&checkpoint.public.portable_session_id)?;
    let automatic_takeover = status
        .as_ref()
        .filter(|lease| lease.lease.holder_device_id == "in_transit")
        .map(|_| "claim moved session");
    let lease = store.acquire_lease(
        &checkpoint.public.portable_session_id,
        &identity.device_id(),
        &checkpoint.oid,
        options.lease_ttl,
        options.takeover_reason.as_deref().or(automatic_takeover),
    )?;
    if let Some(remote) = options.remote.as_deref() {
        store.push_lease(remote, &checkpoint.public.portable_session_id)?;
    }
    let restored = restore_capsule(
        Path::new(&options.revision),
        options.provider,
        options.identity,
        Some(&options.repository),
        options.force_native,
    )?;
    if !options.no_launch {
        launch_native(options.provider, &restored)?;
    }
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checkpoint": checkpoint,
                "lease": lease,
                "restored": restored,
                "launched": !options.no_launch,
            }))?
        );
    } else {
        println!(
            "Resumed {} session {}",
            restored.provider, restored.native_session_id
        );
    }
    Ok(())
}

fn launch_native(provider: ProviderArg, capsule: &NativeCapsule) -> Result<()> {
    let mut command = match provider {
        ProviderArg::Codex => {
            let mut command = ProcessCommand::new("codex");
            command.args(["resume", &capsule.native_session_id]);
            command
        }
        ProviderArg::Claude => {
            let mut command = ProcessCommand::new("claude");
            command.args(["--resume", &capsule.native_session_id]);
            command
        }
    };
    if let Some(cwd) = capsule.original_cwd.as_deref().filter(|cwd| cwd.is_dir()) {
        command.current_dir(cwd);
    }
    let status = command.status().context("failed to launch native agent")?;
    if !status.success() {
        bail!("native agent exited with status {status}");
    }
    Ok(())
}

fn run_status(json: bool) -> Result<()> {
    let statuses = adapters(None)
        .into_iter()
        .map(|adapter| {
            let count = adapter.discover().map_or(0, |sessions| sessions.len());
            serde_json::json!({
                "provider": adapter.provider(),
                "home": adapter.home(),
                "session_count": count,
            })
        })
        .collect::<Vec<_>>();
    if json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
    } else {
        for status in statuses {
            println!(
                "{}\t{}\t{} sessions",
                status["provider"].as_str().unwrap_or("-"),
                status["home"].as_str().unwrap_or("-"),
                status["session_count"].as_u64().unwrap_or(0)
            );
        }
    }
    Ok(())
}

fn run_sessions(command: SessionsCommand) -> Result<()> {
    match command {
        SessionsCommand::List { provider, json } => {
            let sessions = discover(provider)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                print_sessions(&sessions);
            }
        }
        SessionsCommand::Inspect { id, provider, json } => {
            let Some(session) = discover(provider)?
                .into_iter()
                .find(|session| session.id == id)
            else {
                bail!("native session {id} was not found");
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&session)?);
            } else {
                print_sessions(std::slice::from_ref(&session));
                for artifact in session.artifacts {
                    println!(
                        "  {:?}\t{}\t{}",
                        artifact.classification,
                        artifact.role,
                        artifact.path.display()
                    );
                }
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Device { command } => run_device(command),
        Command::Checkpoint {
            id,
            provider,
            recipient,
            output,
            repository,
            portable_session,
            push,
            json,
        } => run_checkpoint(CheckpointOptions {
            id,
            provider,
            recipient,
            output,
            repository,
            portable_session,
            push,
            json,
        }),
        Command::Restore {
            capsule,
            provider,
            identity,
            repository,
            force_native,
            json,
        } => run_restore(
            &capsule,
            provider,
            identity,
            repository.as_deref(),
            force_native,
            json,
        ),
        Command::List { repository, json } => run_list(&repository, json),
        Command::Inspect {
            revision,
            repository,
            json,
        } => run_inspect(&repository, &revision, json),
        Command::Fetch { remote, repository } => run_fetch(&repository, &remote),
        Command::Push {
            portable_session,
            remote,
            repository,
        } => run_push(&repository, &remote, &portable_session),
        Command::Move {
            id,
            provider,
            recipient,
            repository,
            portable_session,
            identity,
            push,
            lease_ttl,
            json,
        } => run_move(MoveOptions {
            id,
            provider,
            recipient,
            repository,
            portable_session,
            identity,
            push,
            lease_ttl,
            json,
        }),
        Command::Resume {
            revision,
            provider,
            repository,
            identity,
            remote,
            takeover_reason,
            lease_ttl,
            no_launch,
            force_native,
            json,
        } => run_resume(ResumeOptions {
            revision,
            provider,
            repository,
            identity,
            remote,
            takeover_reason,
            lease_ttl,
            no_launch,
            force_native,
            json,
        }),
        Command::Lease { command } => run_lease(command),
        Command::Status { json } => run_status(json),
        Command::Sessions { command } => run_sessions(command),
    }
}
