# Install vault-search with Homebrew

This document is for agents. Follow it directly when a user asks you to install
`vault-search`.

## Goal

Install the published `vault-search` binary with Homebrew and verify that the
command is available. Do not build from source, do not run `cargo install`, and
do not ask the user to run commands manually unless Homebrew itself is missing.

## Supported platforms

- macOS Apple Silicon
- Linux x86_64
- Linux arm64

macOS Intel is not published. If the machine is macOS Intel, stop and report
that this release does not support that platform.

## Install

Run these commands:

```bash
brew tap eggyShrimp/tap
brew update
brew install eggyShrimp/tap/vault-search
vault-search --version
```

If `vault-search` is already installed, refresh it:

```bash
brew update
brew reinstall eggyShrimp/tap/vault-search
vault-search --version
```

## Verify binary install

After installation, confirm Homebrew is using the release archive and not a
source build:

```bash
brew cat eggyShrimp/tap/vault-search
brew deps eggyShrimp/tap/vault-search
```

The formula must contain release URLs such as:

```text
https://github.com/eggyShrimp/vault-search/releases/download/v0.1.0/vault-search-aarch64-apple-darwin.tar.gz
```

`brew deps eggyShrimp/tap/vault-search` should print no build dependency such as
`rust`.

## Post-install setup

If the user wants to use an Obsidian vault immediately, run:

```bash
vault-search init
vault-search index
vault-search search "test" --json
```

If the current directory is not inside an Obsidian vault, pass the vault path:

```bash
vault-search init --vault /path/to/vault
vault-search index --vault /path/to/vault
vault-search search --vault /path/to/vault "test" --json
```

## Failure handling

- If `brew` is missing, stop and tell the user Homebrew must be installed first.
- If the formula still points to Git source or mentions `cargo install`, run
  `brew untap eggyShrimp/tap`, then repeat the install steps.
- If the release archive download fails, retry once after `brew update`.
- If verification still fails, report the exact command and output.
