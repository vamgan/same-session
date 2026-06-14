# SameSession

**Same session. Different machine.**

SameSession is an open-source CLI for moving native coding-agent sessions
between machines while preserving the original provider, transcript, history,
and session ID.

It is intentionally not a cross-provider converter or semantic handoff tool.

## Current Status

SameSession is an early implementation. The first release discovers and
inspects native Codex CLI and Claude Code sessions without modifying them.

```bash
cargo run -p samesession -- status
cargo run -p samesession -- sessions list
cargo run -p samesession -- sessions inspect <session-id>
```

## Product Direction

```text
Mac                                  Cloud desktop
──────────────────────               ──────────────────────
samesession move current             samesession resume latest
        │                                      ▲
        └── encrypted native capsule via Git ──┘
```

The target migration guarantee is:

- Same provider
- Same native session bytes
- Same session ID
- Same conversation history
- Independent destination authentication and approvals

## Supported Agents

| Agent | Discovery | Inspection | Move/restore |
|---|---:|---:|---:|
| Codex CLI | Yes | Yes | Planned |
| Claude Code | Yes | Yes | Planned |

## Safety

SameSession treats agent transcripts as sensitive. Authentication files,
credentials, machine trust, and previous approvals must never be migrated.

The current commands are read-only.

## Architecture

- `samesession-core`: provider-neutral native-session model and adapter contract
- `samesession-adapter-codex`: Codex rollout discovery
- `samesession-adapter-claude`: Claude Code transcript discovery
- `samesession-cli`: user-facing CLI

The full architecture and migration protocol are documented in
[DESIGN.md](DESIGN.md).

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

## License

MIT
