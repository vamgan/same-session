# Contributing

SameSession is building a safety-sensitive migration layer over private,
versioned vendor session formats.

Before submitting a change:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Adapter changes must preserve unknown native fields and include sanitized
fixtures for every newly supported vendor format.

Never commit real transcripts, credentials, authentication files, or approval
state.

