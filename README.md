# SameSession

**Same session. Different machine or teammate.**

SameSession moves a native Codex CLI or Claude Code session to another machine.
It can also hand the active session to an authorized teammate. It preserves the
provider-owned session ID, native transcript bytes, local Git commits, and dirty
workspace state. It does not convert sessions between providers or generate a
semantic handoff.

```text
Mac                                        Cloud desktop
────────────────────────                   ─────────────────────────
samesession move <native-id>               samesession resume <portable-id>
        │                                              ▲
        └── age-encrypted capsule in isolated Git refs ┘
```

## Status

SameSession is pre-release software. The implemented migration path supports:

- Native Codex CLI and Claude Code discovery, inspection, restore, and resume
- Age-encrypted, byte-preserving native session capsules
- Local commits plus staged, unstaged, deleted, binary, and untracked files
- Append-only checkpoint transport through isolated Git refs
- Detached destination worktrees without changing the active branch or `HEAD`
- Advisory ownership leases, explicit takeover, deletion, pruning, and GC
- Provider/version compatibility checks, secret policy checks, and operation locks

Provider-native formats are private implementation details and can change.
Review a test migration before relying on SameSession for important work.

## Install From Source

Requires Git and Rust 1.88 or newer.

```bash
git clone https://github.com/vamgan/same-session.git
cd same-session
cargo install --path crates/samesession-cli
```

## Move A Session

Create an identity on each machine. The private identity never leaves that
machine; exchange only the public `age1...` recipient.

```bash
# Source machine
samesession device init

# Destination machine
samesession device init
```

Initialize the source repository with both public recipients and optional
automatic pushes:

```bash
samesession init \
  --repository . \
  --recipient <destination-age-recipient> \
  --auto-push
```

Find the provider-owned native session ID, then move it:

```bash
samesession sessions list --provider codex
samesession move current --provider codex --repository .
```

The command prints a portable session ID such as `sss_01...`. On the
destination, clone the same Git remote and resume it:

```bash
samesession resume latest \
  --provider codex \
  --repository . \
  --remote origin
```

Use an explicit native session ID or portable session ID when `current` or
`latest` is not the intended session.

Resume decrypts and installs the native session, imports the captured source
commit into an isolated ref, creates a sibling detached worktree, acquires the
lease, and launches the provider CLI from that worktree. Use `--no-launch` to
restore without launching or `--into <path>` to choose the worktree path.

For Claude Code, replace `--provider codex` with `--provider claude`.

## Hand Off To A Teammate

Enroll the teammate's public age recipient and ensure they can access the Git
remote. The next moved capsule can then be decrypted and resumed by that
teammate:

```bash
samesession init \
  --repository . \
  --recipient <teammate-age-recipient> \
  --auto-push

samesession move current --provider codex --repository .
```

The teammate clones or fetches the shared remote and runs `samesession resume
latest`. They receive the native session and captured workspace, but must
authenticate with the provider and approve actions independently. Credentials,
machine trust, and previous approvals are never transferred.

## Standalone Checkpoints

An encrypted capsule can also be created and restored without Git transport:

```bash
samesession checkpoint <native-session-id> \
  --provider codex \
  --recipient <age-recipient> \
  --output session.age

samesession restore session.age --provider codex
```

Use `--repository .` on checkpoint and restore to include and import source
state. Add `--into <path>` on restore to materialize it as a detached worktree.

## Operations

```bash
samesession doctor
samesession list
samesession inspect <checkpoint-ref-or-oid>
samesession fetch origin --prune
samesession push <portable-session-id> origin
samesession lease status <portable-session-id>
samesession lease takeover <portable-session-id> --source-checkpoint <oid> --reason <reason>
samesession delete <portable-session-id> --confirm <portable-session-id>
samesession gc
```

All commands that support `--json` emit machine-readable output.

## Safety Model

- Capsules are encrypted to explicit age recipients.
- Authentication, credentials, machine trust, and previous approvals are not migrated.
- Native artifacts must remain inside the provider home and match exportable classifications.
- High-confidence secrets block capture before a capsule is created.
- Restore verifies hashes, rejects unsafe archive paths, and installs native files rollback-safely.
- Source snapshots use a temporary Git index and transient ref; the source index, worktree, branch, and `HEAD` are not modified.
- Restored source state is imported into isolated refs and detached worktrees.
- Leases reduce accidental concurrent resume but remain advisory.

Destination authentication and approvals are always established independently.
See [SECURITY.md](SECURITY.md) for reporting guidance.

## Protocol And Architecture

The complete design is in [DESIGN.md](DESIGN.md). Stable protocol shapes are
published under [`schemas/`](schemas/):

- `native-capsule-v1.json`
- `public-checkpoint-v1.json`
- `lease-v1.json`
- `project-config-v1.json`

Workspace crates separate provider adapters, encrypted capsules, Git transport,
configuration, policy, locking, source capture, and the user-facing CLI.

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

## License

[MIT](LICENSE-MIT)
