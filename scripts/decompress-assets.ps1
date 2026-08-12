# Extracts the large database JSON files bundled in looseLoot.7z back into
# their normal locations under Libraries/SPTarkov.Server.Assets. Required before
# first build - see CLAUDE.md.

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$Archive = Join-Path $RepoRoot "Libraries/SPTarkov.Server.Assets/looseLoot.7z"

$SevenZip = Get-Command 7z -ErrorAction SilentlyContinue
if (-not $SevenZip) {
    foreach ($candidate in @(
        "$env:ProgramFiles\7-Zip\7z.exe",
        "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
    )) {
        if (Test-Path $candidate) {
            $SevenZip = $candidate
            break
        }
    }
}
if (-not $SevenZip) {
    Write-Error "7z not found. Install 7-Zip (https://www.7-zip.org/) and try again."
    exit 1
}

& $SevenZip x -y $Archive "-o$RepoRoot" | Out-Null
Write-Host "Extracted $Archive"
