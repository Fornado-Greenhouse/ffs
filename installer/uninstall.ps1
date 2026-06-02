# installer/uninstall.ps1 — reverse what install.ps1 did.

[CmdletBinding()]
param(
    [string]$Vault = $env:FFS_VAULT,
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'FFS'),
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'

function Say($msg) { Write-Host "[uninstall] $msg" }

$BinDir = Join-Path $Prefix 'bin'

# Scheduled task.
$TaskName = 'FFS Daemon'
if (Get-Command -Name Unregister-ScheduledTask -ErrorAction SilentlyContinue) {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Say "removed scheduled task '$TaskName'"
    }
}

# Binaries.
foreach ($name in @('ffs.exe', 'ffs-daemon.exe', 'ffs-mcp.exe')) {
    $p = Join-Path $BinDir $name
    if (Test-Path $p) {
        Remove-Item -Force $p
        Say "removed $p"
    }
}

# User data location (also the canonical vault per task_30).
$DataDir = Join-Path $env:USERPROFILE '.ffs'

# Obsidian plugin — default substrate-is-vault location.
$DefaultPlugin = Join-Path $DataDir '.obsidian\plugins\ffs'
if (Test-Path $DefaultPlugin) {
    Remove-Item -Recurse -Force $DefaultPlugin
    Say "removed Obsidian plugin at $DefaultPlugin"
}

# External vault (legacy / opt-in).
if ($Vault -and ($Vault -ne $DataDir)) {
    $plug = Join-Path $Vault '.obsidian\plugins\ffs'
    if (Test-Path $plug) {
        Remove-Item -Recurse -Force $plug
        Say "removed external-vault Obsidian plugin at $plug"
    }
}
if ($Purge) {
    if (Test-Path $DataDir) {
        Remove-Item -Recurse -Force $DataDir
        Say "PURGED $DataDir"
    }
} else {
    Say "preserved $DataDir — pass -Purge to also delete it"
}
