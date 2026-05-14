# typed: false
# frozen_string_literal: true

# Homebrew formula template for vault-search.
# The release workflow writes the final formula to:
#   github.com/eggyShrimp/homebrew-tap/Formula/vault-search.rb
#
# To use the published formula:
#   brew tap eggyShrimp/tap && brew install vault-search
#
# SHA256 values are filled from the release assets by:
#   .github/scripts/update-homebrew-formula.sh

class VaultSearch < Formula
  desc "Local-first semantic search MCP server for Obsidian vaults"
  homepage "https://github.com/eggyShrimp/vault-search"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/eggyShrimp/vault-search/releases/download/v#{version}/vault-search-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/eggyShrimp/vault-search/releases/download/v#{version}/vault-search-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
    on_intel do
      url "https://github.com/eggyShrimp/vault-search/releases/download/v#{version}/vault-search-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  def install
    bin.install "vault-search"
  end

  test do
    assert_match "vault-search", shell_output("#{bin}/vault-search --version")
  end
end
