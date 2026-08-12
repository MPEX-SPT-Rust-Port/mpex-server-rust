#!/usr/bin/env bash
# Extracts the large database JSON files bundled in looseLoot.7z back into
# their normal locations under Libraries/SPTarkov.Server.Assets. Required before
# first build - see CLAUDE.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVE="$REPO_ROOT/Libraries/SPTarkov.Server.Assets/looseLoot.7z"

if ! command -v 7z >/dev/null 2>&1; then
  echo "error: 7z not found on PATH. Install 7-Zip (https://www.7-zip.org/) and try again." >&2
  exit 1
fi

7z x -y "$ARCHIVE" -o"$REPO_ROOT" >/dev/null
echo "Extracted $ARCHIVE"
