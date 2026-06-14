# SameSession: Production Design

Status: Draft architecture specification  
Working name: `same-session`  
CLI binary: `samesession`

## 1. Product Definition

SameSession migrates an active coding-agent session and its required
session-scoped artifacts into an encrypted, Git-backed native session capsule
that the same vendor CLI can restore and resume on another machine.

The product preserves the vendor's native session instead of summarizing,
converting, or recreating it. A migrated Codex session resumes through
`codex resume`; a migrated Claude Code session resumes through
`claude --resume`.

The first supported agents are Codex CLI and Claude Code CLI.

### Positioning

> Git moves code. SameSession moves the unfinished agent work around the code.

## 2. Goals

- Checkpoint without modifying the current branch, index, or worktree.
- Store checkpoints in Git using ordinary commits and remote-compatible branch
  refs.
- Support reliable Codex-to-Codex and Claude-to-Claude native resume.
- Discover and preserve every required session-scoped native artifact.
- Preserve native session files byte-for-byte unless an adapter performs an
  explicit, versioned path relocation.
- Bind a session capsule to the repository revision and filesystem assumptions
  required for safe resume.
- Optionally carry dirty workspace state when Git alone cannot recreate it.
- Prevent accidental concurrent resume of the same migrated native session.
- Encrypt sensitive payloads before they enter Git object storage.
- Never migrate credentials, authentication state, or prior approvals.
- Detect incompatible, incomplete, stale, or unsafe checkpoints before restore.
- Provide deterministic, scriptable behavior suitable for CI and automation.

## 3. Non-Goals

- Migrating running processes or open network connections.
- Replaying prior approvals on another machine.
- Copying agent authentication tokens or API keys.
- Guaranteeing native compatibility after a vendor changes its private format.
- Real-time multi-device synchronization in the first release.
- Supporting desktop-app-only session stores in the first release.
- Cross-agent migration such as Codex-to-Claude or Claude-to-Codex.
- Semantic summaries, generated handoffs, or transcript conversion.
- General-purpose repository or development-environment backup.

## 4. Product Invariants

These rules are mandatory:

1. `samesession checkpoint` never changes the current branch, index, worktree, or
   stash list.
2. Every checkpoint is immutable and content-addressed.
3. Published checkpoints are append-only. Updating a session creates a new
   checkpoint commit.
4. Sensitive payloads are encrypted before `git hash-object`, `git add`, or any
   equivalent operation writes them into Git.
5. Authentication material and previous approval state are never exported.
6. Restore never overwrites a dirty destination worktree without explicit user
   confirmation or `--force`.
7. Restore verifies hashes, schema compatibility, repository identity, and
   source revision before changing the destination.
8. If native compatibility cannot be proven, restore fails safely instead of
   silently starting a different or reconstructed session.
9. Destructive operations require an interactive confirmation unless an
   explicit non-interactive policy flag is supplied.
10. All machine-readable output is stable, versioned JSON.
11. A session snapshot is captured only after the adapter proves its native
    artifacts are quiescent and internally consistent.
12. Migration ownership uses an explicit lease. SameSession never silently resumes
    the same native session on two machines.

## 5. Primary User Workflows

### 5.1 First-Time Setup

```bash
cd ~/Projects/payments
samesession init
samesession doctor
```

`samesession init`:

- Detects the Git repository and default remote.
- Detects installed agent adapters.
- Creates user encryption identity if one does not exist.
- Configures one or more recipients.
- Writes repository-local non-secret policy to `.samesession/config.toml` only
  after confirmation.
- Does not write private keys into the repository.

### 5.2 Checkpoint and Push

```bash
samesession move --session current --push
```

Example output:

```text
Agent:       codex
Session:     019ebc40-23d5-7410-a1aa-5c3fa58dcdf0
Repository:  github.com/acme/payments
Source:      feature/retries @ 7bc91c2
Workspace:   4 modified, 2 staged, 1 untracked
Payload:     3.8 MiB encrypted
Checkpoint:  ssc_01JXQ8K2C8MVF2N4TN51YQ4Y3P
Remote ref:  refs/heads/same-session/v1/5f2c9a71/sss_01JXQ8...

Pushed checkpoint and transferred the SameSession lease successfully.
```

### 5.3 Restore and Resume Elsewhere

```bash
git clone git@github.com:acme/payments.git
cd payments
samesession fetch
samesession restore latest --resume
```

Restore performs a preflight, presents a plan, restores the workspace, installs
the native session when compatible, and launches the agent.

### 5.4 Session-Only Migration

When repository state is already available from its normal Git remote:

```bash
samesession checkpoint --session current --session-only --push
```

The destination must have the required source revision and a clean compatible
worktree before SameSession installs and resumes the native session.

### 5.5 Sidecar Repository for Public Repositories

```bash
samesession init --store git@github.com:vamgan/private-sessions.git
samesession checkpoint --push
```

For public source repositories, a private sidecar repository is the recommended
store. Encryption is still mandatory.

## 6. CLI Contract

### 6.1 Command Tree

```text
samesession
├── init
├── doctor
├── status
├── session
│   ├── list
│   ├── inspect
│   └── current
├── checkpoint
├── move
├── list
├── inspect
├── fetch
├── restore
├── resume
├── push
├── delete
├── gc
├── lease
│   ├── status
│   ├── release
│   └── takeover
├── config
│   ├── get
│   ├── set
│   └── list
├── recipient
│   ├── list
│   ├── add
│   ├── remove
│   └── rotate
├── device
│   ├── init
│   ├── show
│   ├── enroll
│   └── revoke
└── adapter
    ├── list
    ├── inspect
    └── doctor
```

### 6.2 Global Flags

```text
--repo <path>             Repository path; defaults to current repository
--config <path>           Additional config file
--json                    Emit versioned JSON only
--no-color                Disable color
--non-interactive         Never prompt
--yes                     Accept non-destructive confirmations
--verbose                 Include diagnostic details
--trace                   Include subprocess and adapter tracing
--log-file <path>         Write redacted logs to a file
--version                 Print CLI version
```

`--yes` does not authorize destructive restore, secret inclusion, or unsafe
native session installation. Those require their specific policy flags.

### 6.3 `samesession init`

```text
samesession init
  [--store <git-url-or-remote>]
  [--recipient <age-recipient>...]
  [--agent <name>...]
  [--no-project-config]
```

Creates configuration and performs capability discovery.

### 6.4 `samesession device`

```text
samesession device init [--name <name>]
samesession device show [--public]
samesession device enroll <public-recipient> [--name <name>]
samesession device revoke <device-id>
```

Each machine creates its own identity. `device enroll` adds only the
destination machine's public recipient to the repository or team policy. It
never transfers a private identity.

Before the first Mac-to-cloud checkpoint:

```bash
# Cloud desktop
samesession device init --name cloud-dev
samesession device show --public

# Mac, using the printed public recipient
samesession device enroll age1... --name cloud-dev
samesession checkpoint --push
```

Revocation prevents future checkpoints from being encrypted to a device. It
cannot revoke access to checkpoints the device could already decrypt.

### 6.5 `samesession status`

```text
samesession status [--porcelain]
```

Reports:

- Repository identity and source revision.
- Detected current agent session.
- Workspace changes by category.
- Configured store and recipient status.
- Whether the current state differs from the latest checkpoint.
- Blocking safety or compatibility issues.

### 6.6 `samesession checkpoint`

```text
samesession checkpoint
  [--session <id|current>]
  [--agent <name>]
  [--message <text>]
  [--session-only]
  [--include-workspace]
  [--include-untracked]
  [--include-ignored <glob>...]
  [--exclude <glob>...]
  [--max-payload <size>]
  [--push]
  [--dry-run]
```

Default behavior:

- Include tracked staged and unstaged changes.
- Include untracked files after showing them in the plan.
- Exclude ignored files.
- Include the complete native session capsule discovered by the adapter.
- Include workspace state only when explicitly requested or required because
  the source state is not reproducible from a remote Git revision.
- Encrypt all payload archives.
- Create a local checkpoint commit.
- Do not push unless repository config enables auto-push or `--push` is passed.

`--dry-run` performs discovery, secret scanning, sizing, and compatibility
checks without creating Git objects.

### 6.7 `samesession move`

```text
samesession move
  [--session <id|current>]
  [--push]
  [--deactivate-source]
  [--lease-ttl <duration>]
  [checkpoint options...]
```

`move` is the primary native-migration command. It:

1. Quiesces and snapshots the selected native session.
2. Creates and optionally pushes a checkpoint.
3. Transfers the advisory SameSession lease away from the source device.
4. Marks the local session as migrated in SameSession state.

`--deactivate-source` asks the adapter to move the local native session into a
vendor-compatible archive or backup location after the remote checkpoint is
verified. It is never the default because vendor behavior differs.

`checkpoint` creates a resumable copy without transferring ownership. It is
intended for backup and testing; the CLI warns about concurrent native resume.

### 6.8 `samesession fetch`

```text
samesession fetch [<remote>] [--prune]
```

Fetches only configured SameSession refs. It must not alter normal branch fetch
configuration.

### 6.9 `samesession list`

```text
samesession list
  [--local]
  [--remote]
  [--agent <name>]
  [--session <id>]
  [--since <duration-or-date>]
  [--all-repos]
```

### 6.10 `samesession inspect`

```text
samesession inspect <checkpoint|latest>
  [--decrypt]
  [--show-files]
  [--show-capsule]
```

Metadata that can leak repository details is minimized in plaintext. Decryption
is required to inspect file lists, prompts, transcripts, and native artifacts.

### 6.11 `samesession restore`

```text
samesession restore <checkpoint|latest>
  [--agent <name>]
  [--resume]
  [--into <path>]
  [--strategy abort|new-worktree|apply]
  [--path-map <old=new>...]
  [--force-native]
  [--force]
  [--allow-dirty]
  [--dry-run]
```

Default strategy:

- Existing clean repository: restore into a new Git worktree.
- Existing dirty repository: abort.
- Missing repository: clone/fetch using manifest source information, subject to
  user confirmation.

`--strategy apply` applies directly to the current worktree. It is intended for
advanced users and automation with controlled state.

### 6.12 `samesession resume`

```text
samesession resume [<checkpoint|latest>] [--agent <name>]
```

Convenience command equivalent to restore-if-needed followed by adapter launch.
It acquires the session lease before launch.

### 6.13 `samesession push`

```text
samesession push [<checkpoint|latest>] [--remote <name-or-url>]
```

Pushes one append-only session ref using an explicit refspec. It never uses
`git push --all`, `--mirror`, or broad force pushes.

### 6.14 `samesession lease`

```text
samesession lease status [<checkpoint|session>]
samesession lease release <session>
samesession lease takeover <session> --reason <text>
```

Leases reduce accidental split-brain resume. They are advisory because a user
can bypass SameSession and launch the vendor CLI directly. Takeover requires an
explicit reason and records an audit event.

### 6.15 `samesession delete` and `samesession gc`

```text
samesession delete <checkpoint|session> [--remote] [--confirm <id>]
samesession gc [--older-than 30d] [--keep-last 5] [--remote]
```

Deletion updates only the selected SameSession ref. It cannot guarantee immediate
removal from a hosting provider's object retention, backups, or forks.

## 7. Exit Codes

```text
0   Success
1   General failure
2   Invalid command or configuration
3   No supported repository found
4   No matching agent session found
5   Dirty destination or restore conflict
6   Compatibility check failed
7   Integrity verification failed
8   Encryption or decryption failed
9   Secret policy violation
10  Git transport failure
11  Adapter failure
12  User confirmation required in non-interactive mode
13  Payload exceeds configured limit
14  Authentication required
15  Partial restore; rollback completed
16  Partial restore; manual recovery required
```

## 8. Git Storage Protocol

### 8.1 Why Git

Git already provides authenticated transport, distributed replication,
content-addressed objects, append-only workflows, remote authorization, and
offline operation. SameSession uses Git as a checkpoint transport, not as a
substitute for encryption or lifecycle policy.

### 8.2 Ref Layout

For maximum remote compatibility, version 1 uses ordinary branch refs:

```text
refs/heads/same-session/v1/<repository-key>/<portable-session-id>
```

Fetched locally as:

```text
refs/samesession/remotes/<remote>/<repository-key>/<portable-session-id>
```

Example explicit push:

```bash
git push origin \
  <checkpoint-commit>:refs/heads/same-session/v1/5f2c9a71/sss_01JXQ8...
```

The CLI does not check out these branches. Hosting providers may display them
as branches, but they remain isolated from source branches and pull requests.

Future stores may support custom refs:

```text
refs/same-session/v1/<repository-key>/<portable-session-id>
```

Custom refs are not the default because remote support and visibility differ.

`repository-key` is a short hash of the canonical repository identity.
`portable-session-id` is generated by SameSession and is not the vendor's native
session ID. Vendor session IDs remain encrypted.

### 8.3 Checkpoint Commit Graph

Each agent session has one append-only commit chain:

```text
C1 <- C2 <- C3
```

Each commit:

- Has the previous checkpoint as its only parent.
- Uses a tree containing checkpoint artifacts only.
- Does not use the source commit as a parent.
- Records source repository and revision information inside the encrypted
  manifest.
- Is signed when signing is configured.

The first checkpoint is a root commit. Because the source repository tree is
not part of the checkpoint branch, routine source files are not duplicated in
checkpoint trees.

### 8.4 Tree Layout

```text
version
public.json
payload.age
payload.sha256
signature/
  checkpoint.sig
```

`public.json` contains only what is required to list, route, and reject an
incompatible checkpoint before decryption:

```json
{
  "protocol": "same-session/v1",
  "checkpoint_id": "ssc_01JXQ8K2C8MVF2N4TN51YQ4Y3P",
  "portable_session_id": "sss_01JXQ8K0E2KQH7H6TR8V5ZT7AB",
  "created_at": "2026-06-12T15:20:00Z",
  "creator": "vamgan",
  "cipher": "age",
  "payload_sha256": "sha256:...",
  "payload_bytes": 3981201,
  "expires_at": "2026-06-19T15:20:00Z"
}
```

Repository URL, source branch, paths, prompts, file names, agent identity, and
session IDs are encrypted or hashed.

### 8.5 Payload Layout

After decryption and decompression:

```text
manifest.json
workspace/
  tracked.patch
  index.patch
  untracked.tar.zst
  ignored.tar.zst
  attributes.json
source/
  commits.bundle
session/
  capsule.json
  artifacts.tar.zst
  artifact-hashes.json
environment/
  summary.json
  commands.json
integrity/
  files.sha256
```

The entire payload directory is packaged deterministically and encrypted into
one `payload.age` file. Deterministic packaging does not imply deterministic
encryption; encryption must use fresh randomness.

### 8.6 Source Commit Handling

The manifest records:

- `head_oid`
- `head_ref`
- `upstream_ref`
- `remote_urls`
- `merge_base_oid`
- `local_commits`

If all required commits are reachable from a configured destination remote, no
source bundle is included.

If local-only commits exist, SameSession includes `source/commits.bundle`. The
restore verifies the bundle and fetches its commits into a temporary namespace
before creating the destination worktree.

### 8.7 Workspace Capture

Workspace state is captured without stashing or changing the index:

- `index.patch`: staged changes relative to `HEAD`.
- `tracked.patch`: unstaged tracked changes relative to the index.
- `untracked.tar.zst`: selected untracked files.
- `ignored.tar.zst`: only ignored files explicitly included by policy or flag.
- `attributes.json`: executable bits, symlinks, submodule state, and relevant
  filesystem metadata.

Binary changes that cannot be represented safely as patches are stored as
content-addressed files inside the encrypted payload.

Submodules are recorded by commit and dirty-state metadata. Recursive submodule
checkpointing is opt-in.

### 8.8 Size Policy

Defaults:

```text
Warning threshold:       25 MiB
Hard checkpoint limit:  100 MiB
Single included file:    25 MiB
```

Larger payloads require an external blob-store adapter or explicit override.
Git LFS is not required for the MVP because it adds remote-specific setup,
separate availability semantics, and cleanup complexity.

## 9. Portable Schemas

All schemas use JSON Schema and explicit protocol versions.

### 9.1 Manifest

```json
{
  "schema": "same-session/manifest/v1",
  "checkpoint": {
    "id": "ssc_01JXQ8K2C8MVF2N4TN51YQ4Y3P",
    "portable_session_id": "sss_01JXQ8K0E2KQH7H6TR8V5ZT7AB",
    "created_at": "2026-06-12T15:20:00Z",
    "parent_id": "ssc_01JXQ7...",
    "message": "Retry implementation partially complete"
  },
  "repository": {
    "identity": "sha256:canonical-remote-and-root",
    "root_hint": "payments",
    "head_oid": "7bc91c2...",
    "head_ref": "refs/heads/feature/retries",
    "upstream_ref": "refs/remotes/origin/feature/retries",
    "remote_hints": ["git@github.com:acme/payments.git"]
  },
  "workspace": {
    "dirty": true,
    "staged_files": 2,
    "modified_files": 4,
    "untracked_files": 1,
    "required_case_sensitivity": true
  },
  "agent": {
    "provider": "openai",
    "product": "codex-cli",
    "version": "0.137.0",
    "session_id": "019ebc40-...",
    "native_format": "codex-rollout-jsonl",
    "native_format_version": "unknown",
    "native_included": true
  },
  "restore": {
    "preferred_mode": "native",
    "original_cwd": "/Users/vamgan/Projects/payments",
    "path_dependencies": ["/Users/vamgan/Projects/payments"],
    "requires": ["git>=2.40", "codex-cli"]
  }
}
```

### 9.2 Native Session Capsule

```json
{
  "schema": "same-session/native-capsule/v1",
  "provider": "openai",
  "product": "codex-cli",
  "source_version": "0.137.0",
  "native_session_id": "019ebc40-23d5-7410-a1aa-5c3fa58dcdf0",
  "original_cwd": "/Users/vamgan/Projects/payments",
  "artifacts": [
    {
      "logical_role": "primary-transcript",
      "source_path": "$CODEX_HOME/sessions/2026/06/12/rollout-....jsonl",
      "install_path": "$CODEX_HOME/sessions/2026/06/12/rollout-....jsonl",
      "sha256": "sha256:...",
      "required": true,
      "rewrite_policy": "byte-preserve"
    },
    {
      "logical_role": "shell-snapshot",
      "source_path": "$CODEX_HOME/shell_snapshots/019ebc40....sh",
      "install_path": "$CODEX_HOME/shell_snapshots/019ebc40....sh",
      "sha256": "sha256:...",
      "required": false,
      "rewrite_policy": "byte-preserve"
    }
  ],
  "compatibility": {
    "supported_destination_versions": ">=0.137.0,<0.138.0",
    "requires_exact_cwd": false,
    "supports_cwd_override": true
  },
  "launch": {
    "command": "codex",
    "arguments": ["resume", "019ebc40-23d5-7410-a1aa-5c3fa58dcdf0"],
    "cwd": "${RESTORED_REPOSITORY}"
  }
}
```

Adapters preserve artifact bytes by default. `rewrite_policy` may permit a
specific path relocation only when the adapter implements and tests that
transformation for the detected native format and vendor version.

## 10. Adapter Architecture

### 10.1 Provider-Neutral Core Contract

```ts
export interface AgentAdapter {
  readonly descriptor: AdapterDescriptor;

  detect(ctx: DetectContext): Promise<DetectedAgent[]>;
  listSessions(ctx: SessionQueryContext): Promise<AgentSession[]>;
  inspectSession(ctx: InspectContext): Promise<SessionInspection>;
  discoverCapsule(ctx: ExportContext): Promise<NativeSessionCapsule>;
  exportCapsule(ctx: ExportContext): Promise<NativeExport>;
  checkRestore(ctx: RestoreCheckContext): Promise<RestoreCompatibility>;
  restoreNative(ctx: NativeRestoreContext): Promise<NativeRestoreResult>;
  launch(ctx: LaunchContext): Promise<LaunchResult>;
}
```

Core owns:

- Git inspection and workspace capture.
- Packaging, encryption, integrity, and signatures.
- Store operations and ref lifecycle.
- Restore planning, rollback, and policy.
- Stable schemas and JSON output.

Adapters own:

- Session discovery.
- Vendor-format inspection.
- Native export file selection.
- Native compatibility checks.
- Destination path rewriting where explicitly supported.
- Vendor-native restore and launch commands.
- Vendor-specific redaction rules.

Adapters must not independently push Git refs, capture workspace state, or
handle encryption.

### 10.2 Codex Adapter

Initial discovery sources:

```text
$CODEX_HOME/sessions
$CODEX_HOME/archived_sessions
```

Native resume target:

```bash
codex resume <session-id>
```

Adapter requirements:

- Parse rollout metadata without assuming every event type is known.
- Preserve unknown events byte-for-byte.
- Export the selected rollout JSONL, its session-index entry, and all
  session-scoped shell snapshots discovered for that session.
- Treat the rollout JSONL as required and index/shell artifacts as
  version-dependent until compatibility tests prove otherwise.
- Never export authentication files.
- Allow destination working-directory override.
- Verify the installed Codex version and native session readability before
  installation.

### 10.3 Claude Code Adapter

Initial discovery source:

```text
~/.claude/projects/<encoded-project-path>/
```

Native resume target:

```bash
claude --resume <session-id>
```

Adapter requirements:

- Treat absolute project path as a compatibility constraint.
- Export the selected main transcript plus its session directory containing
  subagent transcripts and tool results when present.
- Discover session-associated task, job, and session-environment artifacts and
  classify each as required, optional, or unsafe.
- Exclude project memory by default because it is project-scoped rather than
  session-scoped.
- Never export authentication or global user configuration.
- Support exact-path restore by default.
- Treat path rewriting as experimental and require a backup plus explicit
  confirmation.

### 10.4 Adapter Compatibility Result

```ts
type RestoreCompatibility = {
  native: "compatible" | "incompatible" | "unsafe" | "unknown";
  reasons: CompatibilityReason[];
  requiredActions: RequiredAction[];
  warnings: Warning[];
};
```

Unknown native compatibility blocks restore unless the user passes
`--force-native`. Forced restores are installed into a backup-protected staging
location before launch.

### 10.5 Native Capsule Discovery Rules

Every discovered artifact is classified:

```text
required      Native resume cannot work without it.
associated    Belongs to the session and should migrate when present.
derived       Can be safely rebuilt on the destination.
global        Shared across sessions and excluded by default.
unsafe        Authentication, approvals, credentials, or machine trust; never migrate.
unknown       Session relationship is unclear; block or require explicit policy.
```

Adapters discover artifacts by session ID, native references, and known
vendor-version layout rules. They must not export an entire vendor home
directory as a shortcut.

Current Codex adapter inventory:

```text
$CODEX_HOME/sessions/.../rollout-<session-id>.jsonl   required
$CODEX_HOME/session_index.jsonl entry                 derived
$CODEX_HOME/shell_snapshots/<session-id>.*.sh         derived/optional
$CODEX_HOME/auth.json                                 unsafe
$CODEX_HOME/config.toml                               global
$CODEX_HOME/history.jsonl                             global
$CODEX_HOME/*.sqlite                                  global/unknown
```

Current Claude Code adapter inventory:

```text
~/.claude/projects/<encoded-cwd>/<session-id>.jsonl   required
~/.claude/projects/<encoded-cwd>/<session-id>/        associated
  subagents/
  tool-results/
~/.claude/tasks/<session-id>/                         associated when present
~/.claude/session-env/<session-id>/                   associated when present
~/.claude/jobs/<session-id-prefix>/                   associated/unknown
~/.claude/history.jsonl                               global
~/.claude/projects/<encoded-cwd>/memory/              project-scoped, excluded
Claude authentication and permission state           unsafe
```

These classifications are adapter-version data, not permanent assumptions.
Each supported vendor version has fixture-backed discovery rules.

Observed compatibility evidence:

```text
Codex CLI 0.137.0:
  A clean isolated CODEX_HOME containing only the selected rollout JSONL
  successfully reached native resume for the original session ID through
  `codex resume <session-id>`. Authentication was supplied independently.
  Session index and shell snapshot artifacts were not required for explicit
  resume in this probe.

Claude Code 2.1.177:
  An isolated HOME containing destination-global Claude configuration and only
  the selected project transcript JSONL successfully loaded the original
  session ID and displayed its full native history through
  `claude --resume <session-id>`. Authentication was independent and absent.
  Associated task, job, subagent, tool-result, and session-environment
  artifacts were not required to load this tested session, but may still be
  required to preserve capabilities of sessions that reference them.
```

### 10.6 Native Migration Success Criteria

A migration is successful only when all of the following hold:

1. Every required artifact was installed and hash-verified.
2. The destination vendor CLI discovers the original native session ID.
3. The vendor CLI resumes that session without format or path errors.
4. The existing conversation history is visible to the resumed agent.
5. A new turn appends to the same native session ID.
6. Previous approvals and credentials were not restored.
7. Repository revision and working-directory assumptions match the capsule or
   were relocated through an adapter-supported transformation.

The CLI distinguishes `installed`, `discoverable`, `resumed`, and
`round_trip_verified`; it does not report a generic success after merely
copying files.

### 10.7 Plugin Model

V1 ships first-party adapters in the main distribution. The adapter API becomes
public only after the core lifecycle stabilizes.

Future external adapters are separate executables using a versioned JSON-RPC
stdio protocol:

```text
samesession-adapter-<name>
```

Out-of-process plugins prevent third-party adapter crashes from corrupting core
state and permit adapters written in any language.

## 11. Security Model

### 11.1 Threat Model

Assume:

- Git remotes and repository administrators can read all Git objects.
- Checkpoint branches may be fetched by unintended collaborators.
- Agent transcripts may contain secrets, source code, prompts, and command
  output.
- A malicious checkpoint may attempt path traversal or command execution.
- Destination machines may have different trust and authorization boundaries.

### 11.2 Encryption

- Payload encryption is mandatory before Git object creation.
- V1 uses `age`-compatible recipient encryption.
- Multiple recipients are supported.
- Each device has a distinct identity and public recipient.
- Private identities remain outside repositories and are never transferred by
  SameSession.
- Repository config may contain public recipients only.
- Passphrase-only encryption is allowed for local/offline export but discouraged
  for shared Git transport.

New devices must enroll their public recipient before they can decrypt newly
created checkpoints. Teams may use centrally managed recipients or supported
hardware-backed identity plugins. Recipient removal affects future checkpoints
only; rotating access to historical checkpoints requires decrypting and
reencrypting them.

### 11.3 Authentication and Signing

Encryption provides confidentiality, not creator identity. Checkpoints may be
signed using SSH signing or Sigstore.

Restore policy can require:

```toml
[trust]
require_signature = true
allowed_signers = ["SHA256:..."]
```

### 11.4 Secret Scanning

Before packaging, SameSession scans:

- Selected workspace files.
- Native transcript export.
- Environment summary and command output.

Policy outcomes:

```text
block       Known high-confidence secret
warn        Possible secret or sensitive path
allow       Explicit repository policy exception
redact      Adapter-declared safe redaction
```

The default is to block high-confidence findings. `--allow-secret` is not a
global bypass; exceptions require a finding ID and are logged in the encrypted
manifest.

### 11.5 Mandatory Exclusions

Always excluded:

```text
Agent authentication files
API tokens and credential stores
SSH private keys
Cloud credential directories
Prior approval/permission grants
Shell history
Unrelated global agent configuration
```

### 11.6 Safe Extraction

Restore rejects:

- Absolute archive paths.
- `..` traversal.
- Symlinks escaping the restore root.
- Device files, sockets, and FIFOs.
- Duplicate paths with conflicting types.
- Files exceeding configured extraction limits.

No restored command executes automatically. `environment/commands.json` is
informational until the user explicitly runs or approves a command.

## 12. Restore Transaction

Restore is a staged transaction:

```text
DISCOVER
  -> FETCH
  -> VERIFY_PUBLIC_METADATA
  -> DECRYPT
  -> VERIFY_PAYLOAD
  -> PREFLIGHT
  -> PREPARE_DESTINATION
  -> RESTORE_SOURCE_COMMITS
  -> APPLY_WORKSPACE
  -> INSTALL_NATIVE_SESSION
  -> VERIFY_RESULT
  -> OPTIONAL_LAUNCH
```

### 12.1 Preflight Checks

- Destination repository identity matches.
- Required source commit exists or bundle verifies.
- Destination filesystem supports required case behavior and symlinks.
- Destination has sufficient disk space.
- Worktree strategy is safe.
- Agent version and adapter compatibility are acceptable.
- Path mapping is complete.
- Payload signature and hashes verify.

### 12.2 Rollback

The preferred restore strategy creates a new worktree and staging directories,
making rollback a directory/ref removal rather than destructive patch reversal.

For direct apply:

- SameSession first creates a local rollback checkpoint.
- Native session files are installed through atomic rename.
- On failure, workspace and native files are restored from the rollback
  checkpoint.
- Exit code distinguishes complete rollback from manual recovery.

### 12.3 Resume Safety

Before launching an agent, SameSession injects a system-visible samesession notice or
initial user prompt:

```text
This session was restored on a different machine.
Re-check repository state, environment, credentials, and permissions before
continuing. Previous approvals do not apply.
```

## 13. Configuration

Precedence, highest first:

```text
CLI flags
Environment variables
.samesession/config.local.toml
.samesession/config.toml
~/.config/same-session/config.toml
Built-in defaults
```

Example project config:

```toml
version = 1

[store]
type = "git"
remote = "origin"
ref_prefix = "refs/heads/same-session/v1"
auto_push = false

[checkpoint]
include_untracked = true
max_payload_bytes = 104857600
expires_after = "7d"

[encryption]
recipients = [
  "age1...",
  "age1..."
]

[security]
secret_policy = "block"
require_encryption = true
require_signature = false

[restore]
strategy = "new-worktree"
native_policy = "compatible-only"

[adapter.codex]
enabled = true

[adapter.claude]
enabled = true
include_project_memory = false
```

Environment variables must not accept raw private keys. They may point to
identity providers or files:

```text
SAMESESSION_CONFIG
SAMESESSION_STORE
SAMESESSION_IDENTITY_FILE
SAMESESSION_LOG
SAMESESSION_NON_INTERACTIVE
```

## 14. Machine-Readable Output

Every command supports `--json`. Output is one JSON object for bounded commands
and JSON Lines events for long-running commands.

Example:

```json
{
  "schema": "same-session/cli-result/v1",
  "ok": true,
  "command": "checkpoint",
  "checkpoint_id": "ssc_01JXQ8K2C8MVF2N4TN51YQ4Y3P",
  "warnings": [],
  "result": {
    "commit_oid": "a14f...",
    "remote_ref": "refs/heads/same-session/v1/5f2c9a71/sss_01JXQ8...",
    "pushed": true
  }
}
```

Logs go to stderr. JSON result data goes to stdout. Sensitive values are
redacted in both.

## 15. Implementation Architecture

Recommended language: Rust.

Reasons:

- Single static binaries for macOS, Linux, and Windows.
- Strong filesystem and archive safety controls.
- Reliable subprocess orchestration.
- Mature Git, serialization, compression, and cryptography ecosystems.
- Suitable for a security-sensitive CLI.

Initial repository layout:

```text
same-session/
  crates/
    samesession-cli/
    samesession-core/
    samesession-git/
    samesession-crypto/
    samesession-policy/
    samesession-schema/
    samesession-adapter-codex/
    samesession-adapter-claude/
    samesession-test-support/
  schemas/
  docs/
  fixtures/
  deny.toml
  rust-toolchain.toml
```

This should begin as one implementation repository because the protocol,
schemas, adapters, and CLI will change together during early development.
Adapters can become separate repositories once the external adapter protocol is
stable.

### 15.1 Internal Boundaries

```text
CLI
  -> Application services
      -> Core checkpoint/restore domain
          -> Agent adapters
          -> Workspace capture
          -> Security policy
          -> Crypto
          -> Store interface
              -> Git store
```

### 15.2 Store Contract

```ts
interface CheckpointStore {
  list(query: CheckpointQuery): Promise<CheckpointHeader[]>;
  fetch(ref: CheckpointRef): Promise<EncryptedCheckpoint>;
  append(input: AppendCheckpoint): Promise<StoredCheckpoint>;
  publish(ref: CheckpointRef): Promise<PublishResult>;
  delete(ref: CheckpointRef): Promise<void>;
}
```

The core does not assume all future stores are Git-backed.

### 15.3 State Database

Use a small local SQLite database for:

- Checkpoint index and cached headers.
- Session-to-ref mapping.
- Push/fetch state.
- Restore transaction journal.
- Secret finding exceptions.
- Adapter compatibility cache.

The database is a cache and journal, not the source of truth. It must be
rebuildable from refs and local filesystem discovery.

## 16. Concurrency and Consistency

- Acquire a repository-scoped lock during checkpoint creation and restore.
- Acquire an adapter-specific lock while installing native session files.
- Refuse capture while the vendor session is actively mutating unless the
  adapter supports a cooperative quiesce operation.
- For append-only transcript formats, verify a stable snapshot by comparing
  size, modification metadata, and hashes across the capture boundary.
- Use optimistic append: push only if the remote session ref still matches the
  fetched parent OID.
- On concurrent checkpoints, fetch and create a merge-free successor or create
  a distinct actor/session ref. Never force-overwrite another checkpoint.
- Write payloads and session files through temporary files plus atomic rename.
- Interrupted operations are recovered using the transaction journal.

### 16.1 Session Leases

Lease state is stored in a separate append-only Git ref:

```text
refs/heads/same-session-leases/v1/<repository-key>/<portable-session-id>
```

A lease records the holder device ID, source checkpoint, acquisition time,
expiry, and previous lease commit. Acquisition uses a compare-and-swap push
against the previously fetched lease OID.

The destination must acquire the lease before native resume. An unexpired lease
owned by another device blocks resume unless the user performs an audited
takeover. Because Git transport may be offline or bypassed, the lease is a
split-brain safety mechanism rather than a distributed-consensus guarantee.

## 17. Error Handling and Diagnostics

Errors have stable codes and structured remediation:

```json
{
  "code": "SAMESESSION_RESTORE_DIRTY_DESTINATION",
  "message": "Destination worktree contains changes.",
  "remediation": [
    "Use --strategy new-worktree",
    "Checkpoint or commit the destination changes"
  ],
  "details": {
    "modified_files": 3
  }
}
```

`samesession doctor` verifies:

- Git version and required commands.
- Store reachability and ref push permission.
- Encryption recipients and identity availability.
- Adapter installation and supported versions.
- Secret scanner availability.
- Filesystem capabilities.
- A create/fetch/decrypt/delete canary checkpoint when `--live` is passed.

## 18. Testing Strategy

### 18.1 Unit Tests

- Schema validation and forward-compatible parsing.
- Canonical repository identity.
- Ref construction and validation.
- Path traversal rejection.
- Secret policy.
- Encryption/decryption and hash verification.
- Compatibility decisions.
- Exit-code mapping.

### 18.2 Golden Fixture Tests

Maintain sanitized fixtures for multiple Codex and Claude Code versions:

```text
fixtures/codex/<version>/
fixtures/claude/<version>/
```

Tests must preserve unknown native events and reject malformed inputs.

### 18.3 Git Integration Tests

Run against local bare remotes:

- Append and fetch checkpoint refs.
- Concurrent pushes.
- Local-only source commits.
- Shallow clones.
- Detached HEAD.
- Worktrees and submodules.
- Remote branch deletion and GC behavior.

### 18.4 End-to-End Matrix

```text
macOS arm64 -> Linux x64
Linux x64   -> macOS arm64
Linux x64   -> Linux x64

Codex       -> Codex native
Claude      -> Claude native
```

Each E2E test:

1. Creates a source repository and agent fixture.
2. Produces staged, unstaged, binary, and untracked state.
3. Checkpoints and pushes to a bare remote.
4. Restores on a clean simulated destination.
5. Verifies byte-level workspace state.
6. Verifies native session discoverability and successful resume invocation.

### 18.5 Security Tests

- Malicious archives and path traversal.
- Symlink escape.
- Oversized decompression payloads.
- Tampered ciphertext and signatures.
- Credential and token fixtures.
- Unsafe command injection in metadata.
- Hostile Git refs and repository configuration.
- Fuzzing for manifest, archive, and native transcript parsers.

## 19. Release and Supply Chain

- Reproducible release builds.
- Signed release artifacts and checksums.
- SBOM generation.
- Dependency license and vulnerability checks.
- Rust unsafe-code policy and dependency auditing.
- macOS, Linux, and Windows binaries.
- Homebrew, npm wrapper, and GitHub Releases distribution after core binaries
  are stable.
- Protocol schemas published independently from CLI releases.

Compatibility policy:

- Patch releases do not change protocol semantics.
- Minor releases may add optional fields and capabilities.
- Major releases may change required schema behavior.
- V1 readers ignore unknown optional fields and reject unknown required
  capabilities.

## 20. Observability and Privacy

Telemetry is disabled by default.

Optional telemetry may include:

- Command name.
- Adapter name and compatibility result.
- Payload size bucket.
- Success/failure code.
- Duration bucket.

It must never include repository URLs, paths, session IDs, prompts, source,
transcripts, file names, or command output.

`samesession diagnostics export` creates a redacted bundle and prints its exact
contents before writing it.

## 21. MVP and Release Plan

### Phase 0: Protocol Spike

- Read-only Codex and Claude session discovery.
- Native capsule capture without mutation.
- Encrypted payload packaging.
- Local bare-Git checkpoint store.
- Restore into isolated vendor homes.

Exit gate: the original native session ID and full history load from a
byte-identical primary transcript round trip with independently supplied
destination authentication.

### Phase 1: Useful Alpha

- `init`, `doctor`, `status`, `checkpoint`, `move`, `list`, `inspect`, `fetch`,
  `push`, `restore`, and `lease`.
- Git remote branch transport.
- Codex native resume.
- Claude Code native resume with exact path.
- Native capsule artifact inventory and byte-level verification.
- Repository revision validation; source code arrives through normal Git.
- Advisory session leases.
- Secret scanning and mandatory encryption.

Exit gate: reliable Mac-to-Linux restore through GitHub and a self-hosted bare
Git remote.

### Phase 2: Public Beta

- Version-aware native capsule discovery.
- Tested path relocation where vendor formats safely permit it.
- Optional dirty-workspace capture.
- New-worktree restore transaction and rollback.
- Signatures and trust policy.
- Sidecar repository workflow.
- Retention and GC.
- Full JSON automation contract.

Exit gate: published E2E matrix, security review, and format compatibility
fixtures for multiple vendor versions.

### Phase 3: Stable V1

- Stable protocol and CLI.
- External adapter JSON-RPC specification.
- Windows support.
- Team recipients and policy configuration.
- Documented migration and recovery procedures.

## 22. Decisions and Tradeoffs

### Use Git branches first, not custom refs

Ordinary branch refs work across common Git hosting providers. The cost is
branch-list visibility. The branch contains encrypted artifacts only and uses a
reserved prefix.

### Encrypt one payload, not individual files

One encrypted payload minimizes plaintext metadata leakage and simplifies
integrity verification. It reduces deduplication, which is acceptable because
transcripts are sensitive and checkpoint retention is bounded.

### Native compatibility is the product

Vendor-native formats are private and changeable, so adapters and compatibility
fixtures are the product's core engineering investment. SameSession preserves
unknown artifacts and fields, refuses unproven migrations by default, and
publishes a compatibility matrix for each supported vendor version.

### Begin in one repository

Early protocol and adapter changes require coordinated releases. Split adapters
only after the external adapter protocol is stable.

### Default to a new worktree on restore

This minimizes destructive behavior, gives rollback a clear boundary, and lets
users compare restored work with an existing checkout.

## 23. Open Questions Requiring Prototypes

1. Which Git hosting providers permit and conveniently hide custom refs?
2. What minimum Codex session file subset supports native resume across
   machines and versions?
3. What exact Claude Code path fields can be safely rewritten, if any?
4. Which session-scoped artifacts are required versus optional for each vendor
   version?
5. Which secret scanner offers the best embeddable, cross-platform behavior?
6. Should large payloads use a standard external blob protocol or a dedicated
   SameSession blob-store interface first?
7. How should session checkpoints interact with repositories containing
   multiple linked worktrees?

## 24. Recommended First Implementation Slice

Implement one complete vertical path:

```text
Codex current session
  -> prove primary transcript is quiescent
  -> package primary native transcript and capsule manifest
  -> encrypt
  -> append checkpoint commit
  -> push ordinary checkpoint branch
  -> transfer advisory session lease
  -> fetch on Linux
  -> validate repository revision from normal Git
  -> install transcript into isolated destination CODEX_HOME
  -> codex resume <session-id>
```

Do not begin with cross-agent conversion, summaries, or a hosted control plane.
The first credible milestone is a real Mac-to-cloud Codex native resume through
an encrypted Git checkpoint with zero mutation of the source workspace.
Associated artifacts and dirty-workspace transport follow only after the
primary native-session round trip is proven.

## 25. Primary References

- Codex CLI session resume:
  <https://developers.openai.com/codex/cli/features>
- Codex local session transcript locations:
  <https://developers.openai.com/codex/app/troubleshooting>
- Claude Code CLI session resume:
  <https://docs.anthropic.com/en/docs/claude-code/cli-reference>
- Claude Code local transcript storage and retention:
  <https://docs.anthropic.com/en/docs/claude-code/data-usage>
- Git push refspec behavior:
  <https://git-scm.com/docs/git-push>
- Git bundle format and transport:
  <https://git-scm.com/docs/git-bundle>
- GitHub repository and large-file limits:
  <https://docs.github.com/en/repositories/creating-and-managing-repositories/repository-limits>
- Age encryption implementation:
  <https://github.com/FiloSottile/age>
