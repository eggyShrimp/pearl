# Install Pearl

Install the `pearl` binary and verify it is available.

## Supported platforms

- macOS Apple Silicon (aarch64-apple-darwin)
- macOS Intel (x86_64-apple-darwin)
- Linux x86_64 (x86_64-unknown-linux-gnu)
- Linux arm64 (aarch64-unknown-linux-gnu)

## Step 1: Install via Homebrew

Run these commands in order:

```bash
brew tap eggyShrimp/tap
brew update
brew install eggyShrimp/tap/pearl
```

Verify:

```bash
pearl --version
```

If `pearl --version` succeeds, skip to **Post-install setup**.

If any command fails, proceed to Step 2.

## Step 2: Build from source (fallback)

Use this method if Homebrew is unavailable or failed.

### Prerequisites

- Rust toolchain (`rustup` or `cargo` must be available)

### Build and install

```bash
cargo build --release
mkdir -p ~/.local/bin
cp target/release/pearl ~/.local/bin/pearl
```

Make sure `~/.local/bin` is in your PATH, or copy to a directory that is:

```bash
cp target/release/pearl /usr/local/bin/pearl
```

Verify:

```bash
pearl --version
```

## Post-install setup

After `pearl --version` succeeds, ask the user:

> Pearl is installed. Do you want to initialize it now? This will create a config file and index your vault for search.

If the user agrees, run:

```bash
pearl init
pearl index
pearl search "test" --json
```

To check if the current directory is inside a vault, look for a `.obsidian` directory in the current or parent directories. If not found, ask the user for their vault path and include a hint based on their platform:

- macOS: usually in `~/Documents/` or `~/`, the folder name is your vault name
- Linux: usually in `~/Documents/` or `~/`

You can also suggest: open Obsidian → right-click a vault → "Reveal in Finder/Files" to see the full path.

Then pass the path explicitly:

```bash
pearl init --vault /path/to/vault
pearl index --vault /path/to/vault
pearl search --vault /path/to/vault "test" --json
```

If the user declines, stop here.

## Failure handling

- If `brew` is missing or fails, skip directly to Step 2 (build from source).
- If `cargo` is also missing, report: "Rust toolchain required. Install via https://rustup.rs"
- If both methods fail, report the exact error output.
