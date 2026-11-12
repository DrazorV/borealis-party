# Builds the party desktop app (Windows). Run from the repository root.
#   party/desktop/build-desktop.ps1              the program, target\party-desktop\release\borealis-party.exe
#   party/desktop/build-desktop.ps1 -Installer   also the NSIS installer (Tauri downloads NSIS the first time)
#   party/desktop/build-desktop.ps1 -Installer -Config <file.json>   one more Tauri config on top (a test feed, say)
#
# Built from the main Cargo workspace (src-tauri), not party/Cargo.toml: that one swaps tauri for the server's stand-ins.
# tauri.party.desktop.json gives the app its own identifier, app.borealis.party, so its data folder is
# %APPDATA%\app.borealis.party and never desktop Borealis's %APPDATA%\app.borealis. It also gives upstream's updater the
# party release feed and key (party/desktop/updates.md) and turns off file associations, which belong to desktop
# Borealis. The app's version is the party version (party/Cargo.toml), which the updater compares with the feed.
#
# With TAURI_SIGNING_PRIVATE_KEY (and _PASSWORD) set, -Installer also writes the signed update file next to the
# installer (`*-setup.exe.sig`).
#
# Set CARGO_HOME to a short path such as C:\c: a dependency's checkout is too deep for the default one (even with the
# llama feature off, cargo still checks it out to resolve the workspace).
param(
    [switch] $Installer,
    [string] $Config
)
# "Continue": Windows PowerShell 5.1 would stop at the first line the CLI writes to stderr.
$ErrorActionPreference = "Continue"
if ($env:CARGO_HOME) {
    # Tauri looks for cargo inside CARGO_HOME, so a custom one needs the rustup proxies.
    New-Item -ItemType Directory -Force "$env:CARGO_HOME\bin" | Out-Null
    foreach ($tool in "cargo", "rustc", "rustup") {
        if (-not (Test-Path "$env:CARGO_HOME\bin\$tool.exe")) {
            Copy-Item "$env:USERPROFILE\.cargo\bin\$tool.exe" "$env:CARGO_HOME\bin\"
        }
    }
    if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = "$env:USERPROFILE\.rustup" }
    $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
}
# Its own cargo target folder, so this program is never mistaken for a desktop Borealis build in target\release.
$env:CARGO_TARGET_DIR = (Join-Path (Get-Location) "target\party-desktop")

$cliArgs = @("build", "--config", "party/tauri.party.json", "--config", "party/tauri.party.desktop.json")
# The party version, not upstream's: the updater compares it with the feed's.
$version = (Select-String -Path party/Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw "No version in party/Cargo.toml" }
New-Item -ItemType Directory -Force $env:CARGO_TARGET_DIR | Out-Null
$generated = Join-Path $env:CARGO_TARGET_DIR "party-desktop-build.json"
$extra = @{ version = $version }
if ($Installer -and $env:TAURI_SIGNING_PRIVATE_KEY) { $extra.bundle = @{ createUpdaterArtifacts = $true } }
[IO.File]::WriteAllText($generated, ($extra | ConvertTo-Json -Compress))
$cliArgs += @("--config", $generated)
if ($Config) { $cliArgs += @("--config", $Config) }
if (-not $Installer) { $cliArgs += "--no-bundle" }
# Without upstream's default features: device-link (device-to-device transfer, which a party app doesn't offer) and
# llama (on-device semantic search). The characters and rules come from the server.
$cliArgs += @("--", "--no-default-features")
node ui/node_modules/@tauri-apps/cli/tauri.js @cliArgs
if ($LASTEXITCODE -ne 0) { throw "The desktop build failed" }
Get-ChildItem "$env:CARGO_TARGET_DIR\release" -Filter "borealis-party*.exe" | ForEach-Object { $_.FullName }
if ($Installer) { Get-ChildItem "$env:CARGO_TARGET_DIR\release\bundle\nsis" -Include *.exe, *.sig -Recurse | ForEach-Object { $_.FullName } }
