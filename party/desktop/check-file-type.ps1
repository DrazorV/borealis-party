# Checks that the Windows installers register the .borealis file type for Borealis Party, and that uninstalling
# takes it away again. Run by .github/workflows/party-apps.yml after the Windows build; it also works by hand on a
# Windows PC (it installs and then uninstalls the app for the current user, so don't run it where you use the app):
#   party/desktop/check-file-type.ps1 -Setup <...-setup.exe> [-Msi <...msi>]
#
# What the NSIS installer must write under HKCU\Software\Classes (party/tauri.party.desktop.json, fileAssociations):
#   .borealis                                 (default) = BorealisParty.Bundle
#   BorealisParty.Bundle                      (default) = Borealis bundle            Explorer's "Type" column
#   BorealisParty.Bundle\DefaultIcon          (default) = ...\borealis-party.exe,0   the app's icon
#   BorealisParty.Bundle\shell\open\command   (default) = ...\borealis-party.exe "%1"
# .dnd5e stays desktop Borealis's: the party installer must not touch it.
param(
    [Parameter(Mandatory)] [string] $Setup,
    [string] $Msi
)
$ErrorActionPreference = "Stop"
$classes = "HKCU:\Software\Classes"
$progId = "BorealisParty.Bundle"
$failures = [System.Collections.Generic.List[string]]::new()
function Fail([string] $what) { $failures.Add($what); Write-Host "FAIL: $what" }
function Default([string] $key) {
    if (-not (Test-Path -LiteralPath $key)) { return $null }
    (Get-Item -LiteralPath $key).GetValue("")
}

$dnd5eBefore = Default "$classes\.dnd5e"

Write-Host "Installing $Setup silently"
$p = Start-Process -FilePath $Setup -ArgumentList "/S" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "the installer exited with $($p.ExitCode)" }
Get-Process borealis-party -ErrorAction SilentlyContinue | Stop-Process -Force

$ext = Default "$classes\.borealis"
$desc = Default "$classes\$progId"
$icon = Default "$classes\$progId\DefaultIcon"
$open = Default "$classes\$progId\shell\open\command"
Write-Host ".borealis = $ext"
Write-Host "$progId = $desc"
Write-Host "DefaultIcon = $icon"
Write-Host "open = $open"
if ($ext -ne $progId) { Fail ".borealis points at '$ext', not $progId" }
if ($desc -ne "Borealis bundle") { Fail "the type name is '$desc', not 'Borealis bundle'" }
if ($icon -notmatch '\\borealis-party\.exe"?,0$') { Fail "the icon is '$icon', not the app's" }
if ($open -notmatch '\\borealis-party\.exe"? "%1"$') { Fail "the open command is '$open'" }
if ((Default "$classes\.dnd5e") -ne $dnd5eBefore) { Fail ".dnd5e changed: it belongs to desktop Borealis" }

$exe = if ($open -match '^"?(.+?\\borealis-party\.exe)') { $Matches[1] } else { $null }
if (-not $exe -or -not (Test-Path -LiteralPath $exe)) { Fail "the program '$exe' is not installed" }
else {
    $dir = Split-Path $exe
    $uninstaller = Join-Path $dir "uninstall.exe"
    Write-Host "Uninstalling with $uninstaller"
    # _?= keeps the uninstaller from copying itself away and returning at once.
    $u = Start-Process -FilePath $uninstaller -ArgumentList "/S", "_?=$dir" -Wait -PassThru
    if ($u.ExitCode -ne 0) { Fail "the uninstaller exited with $($u.ExitCode)" }
    if (Test-Path -LiteralPath "$classes\$progId") { Fail "$progId is still registered after uninstalling" }
    $after = Default "$classes\.borealis"
    if ($after -eq $progId) { Fail ".borealis still points at $progId after uninstalling" }
    Write-Host ".borealis after uninstalling = '$after'"
}

if ($Msi) {
    # The MSI is read, not installed: its Extension and ProgId tables name the type (installed per machine).
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $installer.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $installer, @((Resolve-Path $Msi).Path, 0))
    function Rows([string] $sql, [int] $columns) {
        $view = $db.GetType().InvokeMember("OpenView", "InvokeMethod", $null, $db, @($sql))
        $view.GetType().InvokeMember("Execute", "InvokeMethod", $null, $view, $null) | Out-Null
        $rows = @()
        while ($rec = $view.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $view, $null)) {
            $rows += , (1..$columns | ForEach-Object { $rec.GetType().InvokeMember("StringData", "GetProperty", $null, $rec, @($_)) })
        }
        $view.GetType().InvokeMember("Close", "InvokeMethod", $null, $view, $null) | Out-Null
        , $rows
    }
    $exts = Rows "SELECT ``Extension``, ``ProgId_`` FROM ``Extension``" 2
    $progs = Rows "SELECT ``ProgId``, ``Description``, ``Icon_`` FROM ``ProgId``" 3
    $exts | ForEach-Object { Write-Host "MSI extension: $($_ -join ' -> ')" }
    $progs | ForEach-Object { Write-Host "MSI ProgId: $($_ -join ' | ')" }
    $bundle = $exts | Where-Object { $_[0] -eq "borealis" }
    if (-not $bundle) { Fail "the MSI does not register .borealis" }
    elseif (-not ($progs | Where-Object { $_[0] -eq $bundle[1] -and $_[1] -eq "Borealis bundle" })) {
        Fail "the MSI's .borealis type is not named 'Borealis bundle'"
    }
    if ($exts | Where-Object { $_[0] -eq "dnd5e" }) { Fail "the MSI registers .dnd5e (desktop Borealis's)" }
}

if ($failures.Count -gt 0) { throw "$($failures.Count) file type check(s) failed" }
Write-Host "ok: .borealis opens with Borealis Party, with its icon, and uninstalling removes it"
