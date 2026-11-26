# Contributing to academic-agent

Thank you for helping improve `academic-agent`.

## Development Setup

Requirements:

- Rust `1.82` or newer (`rustup toolchain install stable`) with `rustfmt` and `clippy`
- Git

Run the full offline verification suite before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All tests are self-contained and must pass completely offline without live Discord tokens or external LLM API keys.

## Coding and Documentation Standards

- Keep changes focused and well modularized across `academic-core`, `academic-mcp`, and `academic-cli`.
- Validate untrusted filenames with `sanitize_filename` / `safe_join_within` before touching the filesystem.
- Write concise English documentation without raw HTML clutter, horizontal `---` rules, or em/en-dash characters.
- Never commit `.env` files, real API tokens, or runtime databases.
