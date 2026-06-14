use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use samesession_adapter_claude::ClaudeAdapter;
use samesession_adapter_codex::CodexAdapter;
use samesession_capsule::{DeviceIdentity, create_encrypted, restore_encrypted};
use samesession_core::{NativeSession, SessionAdapter};
use samesession_git::GitStore;

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
    let capsule_path = options
        .output
        .as_deref()
        .map_or_else(|| temporary.path().join("payload.age"), Path::to_path_buf);
    let capsule = create_encrypted(&session, adapter.home(), &options.recipient, &capsule_path)
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
    json: bool,
) -> Result<()> {
    let path = identity_path(identity)?;
    let identity = DeviceIdentity::load_private(&path)
        .with_context(|| format!("failed to load private identity at {}", path.display()))?;
    let adapter = adapter(provider);
    let temporary = tempfile::tempdir().context("failed to create restore staging directory")?;
    let capsule_path = repository.map_or_else(
        || Ok(capsule.to_path_buf()),
        |repository| {
            let extracted = temporary.path().join("payload.age");
            GitStore::open(repository)?.extract_payload(&capsule.to_string_lossy(), &extracted)?;
            Ok::<_, anyhow::Error>(extracted)
        },
    )?;
    let restored = restore_encrypted(&capsule_path, &identity, adapter.home(), adapter.provider())
        .with_context(|| format!("failed to restore checkpoint {}", capsule.display()))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&restored)?);
    } else {
        println!(
            "Restored native {} session {} into {}",
            restored.provider,
            restored.native_session_id,
            adapter.home().display()
        );
    }
    Ok(())
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
            json,
        } => run_restore(&capsule, provider, identity, repository.as_deref(), json),
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
        Command::Status { json } => run_status(json),
        Command::Sessions { command } => run_sessions(command),
    }
}
