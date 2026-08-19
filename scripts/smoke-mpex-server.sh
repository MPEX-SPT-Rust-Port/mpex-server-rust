#!/usr/bin/env bash
# End-to-end proof of the Rust launcher: Debug-publish the C# server (framework-dependent),
# boot through the published mpex-server, assert /health answers.
# Prerequisites: scripts/decompress-assets.sh has run; dotnet SDK + cargo on PATH.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

# Preflight: an already-running server on :6969 would answer the poll below and false-pass.
if curl -ksf https://127.0.0.1:6969/health > /dev/null 2>&1; then
    echo "SMOKE ABORT: :6969 already answers /health — stop that server first" >&2
    exit 1
fi
out="$(mktemp -d)"
server_pid=""
trap '{ kill "$server_pid" && wait "$server_pid"; } 2>/dev/null || true; rm -rf "$out"' EXIT

# The publish itself ships mpex-server into $out (IncludeMpexServerLauncher target), so the
# smoke fails if that wiring regresses — no separate cargo invocation or copy needed.
dotnet publish "$root/SPTarkov.Server/SPTarkov.Server.csproj" -c Debug -o "$out"

cd "$out"   # sptLogger.Development.json lands here; the server requires it in CWD
./mpex-server &
server_pid=$!

for _ in $(seq 1 90); do
    if curl -ksf https://127.0.0.1:6969/health > /dev/null 2>&1; then
        echo "SMOKE OK: /health answered through the Rust launcher"
        exit 0
    fi
    if ! kill -0 "$server_pid" 2> /dev/null; then
        echo "SMOKE FAIL: launcher exited before /health answered" >&2
        exit 1
    fi
    sleep 1
done
echo "SMOKE FAIL: /health never answered within 90s" >&2
exit 1
