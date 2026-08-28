# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
scripts/decompress-assets.sh                        # REQUIRED before first build (or .ps1 on Windows) - unpacks looseLoot.7z
# rustup (Rust 1.98) is REQUIRED: dotnet build invokes cargo for rust/spt-native
dotnet build                                        # solution is server-csharp.slnx
dotnet test                                         # all tests (Testing/UnitTests, NUnit)
dotnet test --filter "FullyQualifiedName~MongoIdTests"   # single fixture
dotnet test --filter "Name=EveryRegisteredServiceCanBeResolved"   # single test
dotnet build && dotnet test --no-build --filter "..."    # iterating on one filter: build once, rerun tests without rebuilding
csharpier format .                                  # run before opening a PR
cd rust && cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings   # Rust checks
scripts/smoke-mpex-server.sh                         # e2e: Rust mpex-server launcher boots the CLR, asserts /health
```

The server refuses to start unless the working directory contains `sptLogger.json`/`sptLogger.Development.json`, so run
the built `SPT.Server` executable from its output directory (`SPTarkov.Server/bin/<Config>/net10.0`), not via
`dotnet run` from the repo root. IDE profiles for this are in `SPTarkov.Server/Properties/launchSettings.json`.

Publish flags (`dotnet publish`) feed the generated `ProgramStatics` class: `-p:SptVersion=`, `-p:SptCommit=`,
`-p:SptBuildTime=`, `-p:SptBuildType=` (`LOCAL`/`DEBUG`/`RELEASE`). Defaults live in
`Build.props`.

`cargo` must be on `PATH` for **any** `dotnet build` of the solution — `SPTarkov.Server.Core` builds `rust/spt-native`
first, and a missing toolchain fails the build with `MSB3073`. Publishing for a RID other than the build host's needs
`-p:SptNativeRid=<rid>` as well (`dotnet publish -r` alone never reaches a RID-agnostic project reference, so cargo
would silently emit a host-triple library); `Build.props` maps the RID to a Rust target triple and
`SPTarkov.Server.csproj` errors out for unmapped RIDs.

Release builds also regenerate `SPT_Data/checks.dat` by running the `gen_checks` bin in `rust/spt-native`
(`cargo run --bin gen_checks`).

## Pull Requests
Pull requests will target the `dev` branch unless explicitly told otherwise.

## Supported CPU Architecture
Only `x86_64` (64-bit) CPUs are supported.
- Rust's `cargo` targets `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.
- Rust also compiles with `target-cpu=x86-64-v3` for modern CPU extensions.
- Docker/Podman's Containerfiles target `fedora-minimal:44-x86_64`.

## Architecture

Full reference: [ARCHITECTURE.md](ARCHITECTURE.md) — solution layout, request pipeline, DI, startup order,
persistence, websockets, admin panel, mods, build-time codegen. The rules that keep changes correct:

- No MVC controllers or attribute routing. An endpoint = router entry (`Routers/Static` or `Routers/Dynamic`) +
  callback + controller — not an `[HttpGet]`. Item-moving actions go through `Routers/ItemEvents/`; profile-load
  patches through `Routers/SaveLoad/`. Four routes are registered as minimal APIs instead and bypass the pipeline:
  `/health` (`Program.cs`) and the admin panel's login, logout, and profile-download routes (`SPTarkov.Server.Web/SPTWeb.cs`).
- Mark classes `[Injectable]`; every registration lives in `ProgramHelpers.RegisterSptServicesAsync`.
  `DependencyInjectionValidationTests` rebuilds that exact container (mods on and off), so a bad registration fails
  the test run, not a launch.
- Startup work implements `IOnLoad`, ordered by `OnLoadOrder`; anything below `GameCallbacks` runs before Kestrel
  binds. Periodic work implements `IOnUpdate` (5s poll).
- Never edit `Utils/ProgramStatics.Generated.cs` (build-generated). On Release *or any publish*, `Tools/Ceciler`
  IL-rewrites `SPTarkov.Server.Core.dll` (injects `[JsonExtensionData]` into `Models` types), so a rewritten binary
  differs structurally from a plain Debug build.
- The mod-loading split in `Program.StartServerAfterModLoading` is deliberate: merging it back breaks prepatching.
- `DatabaseImporter` hash-verifies `SPT_Data` against `checks.dat` at startup outside DEBUG builds.

## Style

**Rust Porting** - Follow the nomenclature and naming scheme of the C# you are replacing.
[RUST-ROADMAP.md](RUST-ROADMAP.md) is the status/guidelines reference, [RUST-LEDGER.md](RUST-LEDGER.md) the
append-only decision history (flip/phase ledgers, PR ledger), [rust/ARCHITECTURE.md](rust/ARCHITECTURE.md) the
FFI/wire-format one. Adding or changing an export means bumping `ABI_VERSION` in `rust/spt-native/src/lib.rs` *and*
`SptNative.ExpectedAbiVersion` in `Libraries/SPTarkov.Server.Core/Native/SptNative.cs` — they are asserted equal at
startup and in `ffi.rs` tests.

**No line numbers in docs and trackers.** Cite symbols (`Type.Member`, `module::fn`), never
`Example.cs:123` — line numbers churn unpredictably with every edit and rot silently. Line-level
granularity is reserved for cases where a symbol genuinely cannot locate the thing: a bug pinned
inside one large body, or short-lived notes/plans/specs for *active* work. Historical documents
(RUST-LEDGER.md entries, closed specs) keep their as-of-writing line refs unmaintained.

CSharpier plus `.editorconfig` handle formatting. The rules a formatter can't catch:

- Always brace single-line bodies.
- File-scoped namespaces; `using` directives outside the namespace, `System.*` first.
- No `this.` qualification. Private/internal fields are `_camelCase`; consts are `PascalCase`.
- Language keywords over BCL types (`string`, not `String`).
- Block bodies for methods/constructors/properties/accessors — no expression-bodied members (lambdas are fine).

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
