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

# Obsidian plugin.
if ($Vault) {
    $plug = Join-Path $Vault '.obsidian\plugins\ffs'
    if (Test-Path $plug) {
        Remove-Item -Recurse -Force $plug
        Say "removed Obsidian plugin at $plug"
    }
}

# User data.
$DataDir = Join-Path $env:USERPROFILE '.ffs'
if ($Purge) {
    if (Test-Path $DataDir) {
        Remove-Item -Recurse -Force $DataDir
        Say "PURGED $DataDir"
    }
} else {
    Say "preserved $DataDir — pass -Purge to also delete it"
}
