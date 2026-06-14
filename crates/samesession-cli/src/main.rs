use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use samesession_adapter_claude::ClaudeAdapter;
use samesession_adapter_codex::CodexAdapter;
use samesession_core::{NativeSession, SessionAdapter};

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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status { json } => {
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
        }
        Command::Sessions {
            command: SessionsCommand::List { provider, json },
        } => {
            let sessions = discover(provider)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                print_sessions(&sessions);
            }
        }
        Command::Sessions {
            command: SessionsCommand::Inspect { id, provider, json },
        } => {
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
