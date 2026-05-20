# Install Pearl

## Goal

Install the `pearl` binary and verify that the command is available.

## Supported platforms

- macOS Apple Silicon
- macOS Intel
- Linux x86_64
- Linux arm64

## Install methods

### Method 1: Homebrew (recommended for end users)

```bash
brew tap eggyShrimp/tap
brew update
brew install eggyShrimp/tap/pearl
pearl --version
```

If already installed, refresh it:

```bash
brew update
brew reinstall eggyShrimp/tap/pearl
pearl --version
```

### Method 2: Build from source (development)

```bash
cd /path/to/pearl
cargo build --release
cp target/release/pearl ~/.local/bin/pearl
pearl --version
```

Or use `cargo install`:

```bash
cd /path/to/pearl
cargo install --path .
# Installs to: ~/.cargo/bin/pearl
```

**Important:** If `~/.local/bin` is higher priority in PATH than `~/.cargo/bin`,
copy the binary manually:

```bash
cp ~/.cargo/bin/pearl ~/.local/bin/pearl
```

Verify the active binary location:

```bash
which pearl
pearl --version
```

## Verify binary install (Homebrew)

After Homebrew installation, confirm it is using the release archive and not a
source build:

```bash
brew cat eggyShrimp/tap/pearl
brew deps eggyShrimp/tap/pearl
```

The formula must contain release URLs such as:

```text
https://github.com/eggyShrimp/pearl/releases/download/v0.1.0/pearl-aarch64-apple-darwin.tar.gz
```

`brew deps eggyShrimp/tap/pearl` should print no build dependency such as
`rust`.

## Post-install setup

If the user wants to use an Obsidian vault immediately, run:

```bash
pearl init
pearl index
pearl search "test" --json
```

If the current directory is not inside an Obsidian vault, pass the vault path:

```bash
pearl init --vault /path/to/vault
pearl index --vault /path/to/vault
pearl search --vault /path/to/vault "test" --json
```

## Failure handling

- If `brew` is missing, stop and tell the user Homebrew must be installed first.
- If the formula still points to Git source or mentions `cargo install`, run
  `brew untap eggyShrimp/tap`, then repeat the install steps.
- If the release archive download fails, retry once after `brew update`.
- If verification still fails, report the exact command and output.
