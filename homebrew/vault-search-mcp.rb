# typed: false
# frozen_string_literal: true

# Homebrew formula for vault-search-mcp
# To use: brew tap <your-gh-user>/tap && brew install vault-search-mcp
#
# This file should be placed in a separate tap repository:
#   github.com/<your-gh-user>/homebrew-tap/Formula/vault-search-mcp.rb

class VaultSearchMcp < Formula
  desc "Local-first semantic search MCP server for Obsidian vaults"
  homepage "https://github.com/<your-gh-user>/vault-search-mcp"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/<your-gh-user>/vault-search-mcp/releases/download/v#{version}/vault-search-mcp-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
    on_intel do
      url "https://github.com/<your-gh-user>/vault-search-mcp/releases/download/v#{version}/vault-search-mcp-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/<your-gh-user>/vault-search-mcp/releases/download/v#{version}/vault-search-mcp-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
    on_intel do
      url "https://github.com/<your-gh-user>/vault-search-mcp/releases/download/v#{version}/vault-search-mcp-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  def install
    bin.install "vault-search-mcp"
  end

  test do
    assert_match "vault-search-mcp", shell_output("#{bin}/vault-search-mcp --version")
  end
end
