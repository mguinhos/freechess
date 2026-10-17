#!/usr/bin/env bash
# Folder assets are processed by dx; restore the pinned vendor bytes before packaging.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="${1:?Usage: copy-stockfish-assets.sh <built-public-directory>}"
test -f "$DIST/index.html"
mkdir -p "$DIST/assets/stockfish"
cp -R "$ROOT/ui/assets/stockfish/." "$DIST/assets/stockfish/"
(
  cd "$DIST/assets/stockfish"
  if command -v sha256sum >/dev/null; then
    sha256sum -c SHA256SUMS
  else
    shasum -a 256 -c SHA256SUMS
  fi
)
