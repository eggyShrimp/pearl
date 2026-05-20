# typed: false
# frozen_string_literal: true

# Homebrew formula template for pearl.
# The release workflow writes the final formula to:
#   github.com/eggyShrimp/homebrew-tap/Formula/pearl.rb
#
# To use the published formula:
#   brew tap eggyShrimp/tap && brew install pearl
#
# SHA256 values are filled from the release assets by:
#   .github/scripts/update-homebrew-formula.sh

class Pearl < Formula
  desc "Local-first semantic search for Obsidian vaults"
  homepage "https://github.com/eggyShrimp/pearl"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/eggyShrimp/pearl/releases/download/v#{version}/pearl-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/eggyShrimp/pearl/releases/download/v#{version}/pearl-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end

    on_intel do
      url "https://github.com/eggyShrimp/pearl/releases/download/v#{version}/pearl-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256"
    end
  end

  def install
    bin.install "pearl"
  end

  test do
    assert_match "pearl", shell_output("#{bin}/pearl --version")
  end
end
