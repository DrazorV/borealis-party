# Android smoke run for "Open with Borealis Connect" (plan M7b, I4). Run from the repository root.
#   party/android/smoke-open-with.ps1 -Build        build the x86_64 debug APK first, then run
#   party/android/smoke-open-with.ps1               run with the debug APK already built
#
# On the emulator (AVD BalanceLab_Tablet, started if none is running, set to 1080x2340 at 450 dpi = 384x832 CSS) it
# starts a throwaway party server in WSL (SQLite in a new /tmp folder; never the production server), installs the APK
# fresh, signs in through the app, and opens a .dnd5e, an Aurora .xml, a 5etools .json and a .borealis with the app
# the way the Files app does. It checks the "Import <file> into smoke?" dialog and what each import did on the server,
# and opens the character's sheet preview at Android font sizes 1.0 and 1.15 (no text past a section or the footer,
# text on its ruled lines). Then it stops the server, removes its folder and the pushed files, clears the app's data and resets the screen size.
# Screenshots and results.json: target/party-android-smoke. Exit code 0 when every check passed.
#
# Needs: the Android SDK (ANDROID_HOME, or the default %LOCALAPPDATA%\Android\Sdk), Node 22 or later, WSL with a
# borealis-server build (-ServerBin, default ~/bp-accounts-target/release/borealis-server), and for -Build what
# build-apk.ps1 needs (JDK 17 in JAVA_HOME, CARGO_HOME=C:\c on Windows).
param(
    [switch] $Build,
    [string] $Apk = "",
    [string] $ServerBin = "",
    [int] $Port = 18799,
    [string] $Avd = "BalanceLab_Tablet",
    [string] $Out = "target\party-android-smoke",
    # Leave the server, files and screen size as they are after the run (to look around).
    [switch] $Keep
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path "party\android\smoke\run.mjs")) { throw "Run from the repository root." }
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk" }

if ($Build) {
    & party\android\build-apk.ps1 -Target x86_64 -Debug
    if (-not $?) { throw "The APK build failed" }
    # Gradle and Tauri rewrite these generated upstream files; put them back (party/check-upstream.sh).
    git checkout -- src-tauri/gen/android/app/src/main/AndroidManifest.xml src-tauri/gen/android/gradlew 2>$null
}
if (-not $Apk) {
    $Apk = Get-ChildItem "src-tauri\gen\android\app\build\outputs\apk\x86_64\debug" -Filter *.apk -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Apk) { throw "No debug APK: run with -Build, or build-apk.ps1 -Target x86_64 -Debug" }

$nodeArgs = @("party/android/smoke/run.mjs", "--apk", $Apk, "--port", $Port, "--avd", $Avd, "--out", $Out)
if ($ServerBin) { $nodeArgs += @("--server-bin", $ServerBin) }
if ($Keep) { $nodeArgs += "--keep" }
node @nodeArgs
exit $LASTEXITCODE
