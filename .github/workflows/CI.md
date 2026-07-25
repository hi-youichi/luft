# GitHub CI Documentation

This document describes the GitHub CI/CD setup for the Luft project, which automates building, testing, and releasing binary artifacts across multiple platforms.

## Workflows Overview

The project includes two main GitHub Actions workflows:

- **CI Workflow**: Continuous integration for testing and validation
- **Release Workflow**: Automated release creation and binary distribution

## CI Workflow

**File**: `.github/workflows/ci.yml`

### Triggers
- Push to `main` or `develop` branches
- Pull requests targeting `main` or `develop` branches

### Jobs

#### Test Job
- **Platforms**: Ubuntu, Windows, macOS
- **Actions**:
  - Code formatting check with `cargo fmt -- --check`
  - Linting with `cargo clippy --workspace --all-targets -- -D warnings`
  - Full test suite execution with `cargo test --workspace`
- **Caching**: Cargo registry, index, and build artifacts for faster builds

#### Build Job
- **Platforms**: Ubuntu, Windows, macOS
- **Actions**:
  - Release builds with `cargo build --release`
  - Artifact uploads for each platform's binary

### Environment Variables
```yaml
CARGO_TERM_COLOR: always
RUST_BACKTRACE: 1
```

## Release Workflow

**File**: `.github/workflows/release.yml`

### Triggers
- Git tags matching pattern `v*` (e.g., `v0.3.3`, `v1.0.0`)

### Jobs

#### Create Release Job
- **Platform**: Ubuntu
- **Actions**:
  - Creates a GitHub release with the tag name
  - Outputs upload URL for binary artifacts

#### Build and Upload Job
- **Platforms**: Ubuntu, Windows, macOS with cross-compilation support
- **Target Platforms**:
  - `x86_64-unknown-linux-gnu` (Linux x64)
  - `x86_64-unknown-linux-musl` (Linux x64 static)
  - `x86_64-pc-windows-msvc` (Windows x64)
  - `x86_64-apple-darwin` (macOS Intel)
  - `aarch64-apple-darwin` (macOS Apple Silicon)

- **Actions**:
  - Cross-compilation setup for each target
  - Release builds with target-specific optimization
  - Binary stripping for reduced file size
  - Artifact compression (gzip for Unix, zip for Windows)
  - Upload to GitHub Release as release assets

### Required Permissions
```yaml
permissions:
  contents: write
```

## Usage

### Daily Development
```bash
# Push to main or develop branch triggers CI
git push origin main
git push origin develop
```

### Creating a Release
```bash
# Tag a commit with version number
git tag -a v0.3.3 -m "Release v0.3.3"

# Push the tag to trigger release workflow
git push origin v0.3.3
```

### Downloading Release Binaries

Release binaries are automatically uploaded to GitHub Releases under the specific release version. Available formats:

- **Linux**: `luft-x86_64-linux-gnu.gz` and `luft-x86_64-linux-musl.gz`
- **Windows**: `luft-x86_64-windows-msvc.exe.zip`
- **macOS**: `luft-x86_64-apple-darwin.gz` and `luft-aarch64-apple-darwin.gz`

### Installation

After downloading and extracting:
```bash
# Linux/macOS
gunzip luft-x86_64-linux-gnu.gz
chmod +x luft-x86_64-linux-gnu
./luft-x86_64-linux-gnu --version

# Windows
unzip luft-x86_64-windows-msvc.exe.zip
luft-x86_64-windows-msvc.exe --version
```

## Build Optimization

### Caching Strategy
- **Cargo Registry**: Caches downloaded crates registry
- **Cargo Index**: Caches crate index for faster dependency resolution
- **Build Artifacts**: Caches compiled dependencies across builds

### Cross-Compilation Setup
- Linux musl target requires `musl-tools` and `musl-dev` packages
- macOS builds support both Intel and Apple Silicon architectures
- Windows builds use MSVC toolchain

### Binary Optimization
- Release builds use `--release` flag for maximum optimization
- `strip` command removes debugging symbols for smaller file sizes
- Compression reduces download size significantly

## Troubleshooting

### CI Failures
1. Check formatting: `cargo fmt -- --check`
2. Check linting: `cargo clippy --workspace --all-targets -- -D warnings`
3. Run tests locally: `cargo test --workspace`

### Release Failures
1. Ensure tag follows semver format: `vX.Y.Z`
2. Verify GitHub token permissions in repository settings
3. Check that release name doesn't already exist

### Build Issues
- Clear caches: Delete workflow runs and caches from GitHub Actions tab
- Check Rust version compatibility in workflow files
- Verify all dependencies are properly declared in Cargo.toml

## Contributing

When contributing to the Luft project:

1. Ensure all CI checks pass before creating PR
2. Follow semantic versioning for release tags
3. Test changes locally with `cargo test --workspace`
4. Run formatting: `cargo fmt`
5. Run linting: `cargo clippy --workspace --all-targets`

## Additional Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Rust and Cargo Book](https://doc.rust-lang.org/book/)
- [Cross-Compilation Guide](https://rust-lang.github.io/rustup/cross-compilation.html)