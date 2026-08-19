#!/usr/bin/env bash
# End-to-end proof of the Rust launcher: Debug-publish the C# server (framework-dependent),
# copy mpex-server beside it, boot through the launcher, assert /health answers.
# Prerequisites: scripts/decompress-assets.sh has run; dotnet SDK + cargo on PATH.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$(mktemp -d)"
server_pid=""
trap '{ kill "$server_pid" && wait "$server_pid"; } 2>/dev/null || true; rm -rf "$out"' EXIT

# BuildSptNative runs `cargo build --locked` at the workspace root during publish, so this
# also produces rust/target/debug/mpex-server — no separate cargo invocation needed.
dotnet publish "$root/SPTarkov.Server/SPTarkov.Server.csproj" -c Debug -o "$out"
cp "$root/rust/target/debug/mpex-server" "$out/"

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
