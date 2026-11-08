# Builds the party Android app. Run from the repository root.
#   party/android/build-apk.ps1                     signed release APK for phones (arm64)
#   party/android/build-apk.ps1 -Target x86_64 -Debug   debug APK for an emulator
#
# Needs JDK 17 in JAVA_HOME, the Android SDK in ANDROID_HOME (ndk;27.2.12479018, platforms;android-37.0,
# build-tools;36.0.0, cmake;3.22.1), and for release ANDROID_KEYSTORE_PATH and ANDROID_KEYSTORE_PASSWORD.
# On Windows set CARGO_HOME to a short path such as C:\c: a dependency's checkout is too deep for the default one.
param(
    [ValidateSet("aarch64", "x86_64")] [string] $Target = "aarch64",
    [switch] $Debug
)
$ErrorActionPreference = "Continue"
$sdk = $env:ANDROID_HOME
$ndk = "$sdk\ndk\27.2.12479018"
$env:ANDROID_SDK_ROOT = $sdk
$env:NDK_HOME = $ndk
$env:ANDROID_NDK_HOME = $ndk
$env:ANDROID_NDK = $ndk
$env:CI = "true"
$env:PATH = "$sdk\cmake\3.22.1\bin;$env:JAVA_HOME\bin;$env:PATH"
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

$abi, $flavor = if ($Target -eq "aarch64") { "arm64-v8a", "Arm64" } else { "x86_64", "X86_64" }
$profile, $variant = if ($Debug) { "debug", "Debug" } else { "release", "Release" }
$rustTarget = if ($Target -eq "aarch64") { "aarch64-linux-android" } else { "x86_64-linux-android" }

# Compiles the library. Packaging is done below, because Tauri can't pass Gradle an init script, and on Windows
# without Developer Mode this step stops at a symlink anyway.
$cliArgs = @("android", "build", "--apk", "--target", $Target, "--config", "party/tauri.party.json")
if ($Debug) { $cliArgs += "--debug" }
# A party client gets its rules from the server: no device transfer or on-device semantic search.
$cliArgs += @("--", "--no-default-features")
$started = Get-Date
$tauriLog = node ui/node_modules/@tauri-apps/cli/tauri.js @cliArgs 2>&1 | ForEach-Object { "$_" }
$tauriLog | Write-Output

$library = "target\$rustTarget\$profile\libborealis_lib.so"
if (-not (Test-Path $library) -or (Get-Item $library).LastWriteTime -lt $started) {
    # Cargo leaves an up-to-date library alone, so also accept one the script's own build just confirmed.
    $current = $tauriLog -match "Finished"
    if (-not ((Test-Path $library) -and $current)) { throw "The Rust build did not produce $library" }
}
$jni = "src-tauri\gen\android\app\src\main\jniLibs"
Remove-Item -Recurse -Force "$jni\*" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$jni\$abi" | Out-Null
Copy-Item $library "$jni\$abi\" -Force

$init = (Resolve-Path "party\android\party.init.gradle").Path
Push-Location "src-tauri\gen\android"
.\gradlew.bat --init-script $init "assemble$flavor$variant" -x "rustBuild$flavor$variant" --console=plain
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw "Gradle failed" }
Get-ChildItem "src-tauri\gen\android\app\build\outputs\apk\$($flavor.ToLower())\$profile" -Filter *.apk | ForEach-Object { $_.FullName }
