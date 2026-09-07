#!/usr/bin/env bash
# Prints a Homebrew formula for a vivace release, reading tarball checksums
# from a release's SHA256SUMS file. Ready to copy into a tap repository.
# Usage: scripts/brew-formula.sh <version> <sha256sums-file>
set -euo pipefail

version="$1"
sums="$2"
repo="svandragt/vivace"
base_url="https://github.com/$repo/releases/download/v${version}"

sha_for() {
	grep " vivace-v${version}-$1.tar.gz\$" "$sums" | cut -d' ' -f1
}

cat <<RUBY
class Vivace < Formula
  desc "Fast composer install from composer.lock"
  homepage "https://github.com/$repo"
  license "MIT"

  on_macos do
    on_arm do
      url "$base_url/vivace-v${version}-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha_for aarch64-apple-darwin)"
    end
    on_intel do
      url "$base_url/vivace-v${version}-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha_for x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$base_url/vivace-v${version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sha_for aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$base_url/vivace-v${version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sha_for x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "viv"
  end

  test do
    system "#{bin}/viv", "--version"
  end
end
RUBY
