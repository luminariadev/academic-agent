# Distribution and Packaging

`academic-agent` is distributed as standalone GitHub Release binaries with SHA-256 checksums, non-root container images on GitHub Container Registry, and package manifests for Scoop and Homebrew.

## Prebuilt GitHub Release Archives

Every semantic version tag (`v*.*.*`) builds release binaries across four targets via `.github/workflows/release.yml` and attaches `SHA256SUMS.txt` on [GitHub Releases](https://github.com/luminariadev/academic-agent/releases):

- `academic-agent-v<version>-x86_64-unknown-linux-gnu.tar.gz`
- `academic-agent-v<version>-x86_64-pc-windows-msvc.zip`
- `academic-agent-v<version>-x86_64-apple-darwin.tar.gz`
- `academic-agent-v<version>-aarch64-apple-darwin.tar.gz`

Verify archive integrity against `SHA256SUMS.txt`:

```bash
curl -fsSLO https://github.com/luminariadev/academic-agent/releases/download/v0.2.0/academic-agent-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
curl -fsSLO https://github.com/luminariadev/academic-agent/releases/download/v0.2.0/SHA256SUMS.txt
sha256sum -c --ignore-missing SHA256SUMS.txt
```

On Windows PowerShell:

```powershell
Get-FileHash .\academic-agent-v0.2.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

## Scoop (Windows)

Install using the manifest at [packaging/scoop/academic-agent.json](../packaging/scoop/academic-agent.json):

```powershell
scoop install https://raw.githubusercontent.com/luminariadev/academic-agent/main/packaging/scoop/academic-agent.json
```

Or from a local checkout:

```powershell
scoop install ./packaging/scoop/academic-agent.json
```

Scoop `autoupdate` resolves new release checksums automatically from `$baseurl/SHA256SUMS.txt`.

## Homebrew (macOS and Linux)

Install using the formula at [packaging/homebrew/academic-agent.rb](../packaging/homebrew/academic-agent.rb):

```bash
brew install --formula https://raw.githubusercontent.com/luminariadev/academic-agent/main/packaging/homebrew/academic-agent.rb
```

Or from a local checkout:

```bash
brew install --formula ./packaging/homebrew/academic-agent.rb
```

Note that [packaging/scoop/academic-agent.json](../packaging/scoop/academic-agent.json) and [packaging/homebrew/academic-agent.rb](../packaging/homebrew/academic-agent.rb) pin SHA-256 digests from published GitHub Release assets (`SHA256SUMS.txt`); after a new release tag finishes publishing its archives, update the manifest version and checksums from the newly published `SHA256SUMS.txt`.

## Docker and GHCR

Prebuilt multi-stage non-root container images are published on version tags:

```bash
docker pull ghcr.io/luminariadev/academic-agent:latest
```

See [docs/docker.md](docker.md) for Compose setup, persistent volumes, and environment variables.

## Publishing Workspace Crates

The Cargo workspace uses explicit internal crate versions so crates can be published in dependency order:

```bash
cargo publish -p academic-core
cargo publish -p academic-mcp
cargo publish -p academic-cli
```

Wait briefly for crates.io index propagation between dependent publishes, and validate locally first using `cargo package --workspace --locked --no-verify`.
