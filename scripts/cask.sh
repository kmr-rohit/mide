#!/bin/sh
# Print a Homebrew cask for a release, using the checksum files that scripts/pkg.sh wrote.
#
#   scripts/cask.sh > mide.rb
#
# Put the result in your tap (a repo named homebrew-mide, in Casks/mide.rb); users then run
#   brew install --cask <you>/mide/mide
# See docs/notes/RELEASING.md.
set -e
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)
REPO=$(python3 -c 'import json;print(json.load(open("site-src/config.json"))["repo"])')
SHA_FILE="dist/mide-$VERSION-universal.pkg.sha256"
[ -f "$SHA_FILE" ] || { echo "missing $SHA_FILE: run scripts/pkg.sh --universal (or download it from the release)" >&2; exit 1; }
SHA=$(cut -d' ' -f1 "$SHA_FILE")

cat <<RUBY
cask "mide" do
  version "$VERSION"
  sha256 "$SHA"

  url "https://github.com/$REPO/releases/download/v#{version}/mide-#{version}-universal.pkg"
  name "mide"
  desc "Native code editor with git history and terminal sessions"
  homepage "https://github.com/$REPO"

  depends_on macos: ">= :monterey"

  pkg "mide-#{version}-universal.pkg"

  uninstall pkgutil: "dev.mide.editor",
            delete:  [
              "/Applications/mide.app",
              "/usr/local/bin/mide",
            ]

  zap trash: "~/.config/mide"
end
RUBY
