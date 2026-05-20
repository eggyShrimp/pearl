#!/usr/bin/env bash
set -euo pipefail

: "${REPOSITORY:?REPOSITORY is required, for example eggyShrimp/pearl}"
: "${TAG_NAME:?TAG_NAME is required, for example v0.2.0}"

TAP_DIR="${TAP_DIR:-homebrew-tap}"
FORMULA_PATH="${FORMULA_PATH:-${TAP_DIR}/Formula/pearl.rb}"
VERSION="${TAG_NAME#v}"
BASE_URL="${BASE_URL:-https://github.com/${REPOSITORY}/releases/download/${TAG_NAME}}"

ASSETS=(
  "pearl-aarch64-apple-darwin.tar.gz"
  "pearl-aarch64-unknown-linux-gnu.tar.gz"
  "pearl-x86_64-unknown-linux-gnu.tar.gz"
)

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if [[ -n "${ASSET_DIR:-}" ]]; then
  WORKDIR="${ASSET_DIR}"
  CLEANUP_WORKDIR=0
else
  WORKDIR="$(mktemp -d)"
  CLEANUP_WORKDIR=1
fi

cleanup() {
  if [[ "${CLEANUP_WORKDIR}" == "1" ]]; then
    rm -rf "${WORKDIR}"
  fi
}
trap cleanup EXIT

for asset in "${ASSETS[@]}"; do
  if [[ -z "${ASSET_DIR:-}" ]]; then
    curl --fail --location --silent --show-error \
      --output "${WORKDIR}/${asset}" \
      "${BASE_URL}/${asset}"
  fi

  if [[ ! -s "${WORKDIR}/${asset}" ]]; then
    echo "Missing release asset: ${asset}" >&2
    exit 1
  fi
done

sha_macos_arm="$(sha256 "${WORKDIR}/pearl-aarch64-apple-darwin.tar.gz")"
sha_linux_arm="$(sha256 "${WORKDIR}/pearl-aarch64-unknown-linux-gnu.tar.gz")"
sha_linux_intel="$(sha256 "${WORKDIR}/pearl-x86_64-unknown-linux-gnu.tar.gz")"

mkdir -p "$(dirname "${FORMULA_PATH}")"
cat > "${FORMULA_PATH}" <<EOF
# typed: false
# frozen_string_literal: true

class Pearl < Formula
  desc "Local-first semantic search for Obsidian vaults"
  homepage "https://github.com/${REPOSITORY}"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "${BASE_URL}/pearl-aarch64-apple-darwin.tar.gz"
      sha256 "${sha_macos_arm}"
    end
  end

  on_linux do
    on_arm do
      url "${BASE_URL}/pearl-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_arm}"
    end

    on_intel do
      url "${BASE_URL}/pearl-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_intel}"
    end
  end

  def install
    bin.install "pearl"
  end

  test do
    assert_match "pearl", shell_output("\#{bin}/pearl --version")
  end
end
EOF

ruby -c "${FORMULA_PATH}"
