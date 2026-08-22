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

# Phase 6b: the published tree must be single-linkage. mpex-server carries spt-native as an rlib and
# exports its symbols; a cdylib beside it would be a second copy of every static.
# Retention is all-or-nothing - the linker keeps the referenced rlib whole or discards it whole -
# so any count above zero means the anchor held. Asserting an exact count would only add a site to
# update whenever an export is added.
exported="$(nm -D --defined-only "$out/mpex-server" | grep -c ' T spt_' || true)"
if [ "$exported" -eq 0 ]; then
    echo "SMOKE FAIL: mpex-server exports no spt_* symbols." >&2
    echo "  The usual cause is that rust/mpex-server/src/main.rs lost its reference to spt_native," >&2
    echo "  so the linker dropped the rlib; restore the anchor call in run(). If that is intact," >&2
    echo "  check that --export-dynamic survived into the link: cd rust && cargo build -v 2>&1 |" >&2
    echo "  grep -c export-dynamic  (rust/.cargo/config.toml is read from the working directory)." >&2
    exit 1
fi
if [ -e "$out/libspt_native.so" ]; then
    echo "SMOKE FAIL: publish shipped a cdylib alongside the rlib-linked launcher" >&2
    exit 1
fi

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
