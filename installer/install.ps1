# installer/install.ps1 — Windows installer for FFS.
#
# Lays down:
#   - Binaries (ffs.exe, ffs-daemon.exe, ffs-mcp.exe) under
#     $env:LOCALAPPDATA\FFS\bin.
#   - %USERPROFILE%\.ffs\{config\predicates, config\templates,
#     skills, run, log}.
#   - Starter predicate-spec library and Tera templates (idempotent
#     copy — existing user-edited files are preserved).
#   - Python skill bundles copied under %USERPROFILE%\.ffs\skills.
#   - A Scheduled Task ("FFS Daemon") that launches ffs-daemon.exe
#     at user logon.
#   - Optional Obsidian plugin registration into a configured vault
#     via -Vault <path>.
#
# Re-running the installer is safe — every step is idempotent.

[CmdletBinding()]
param(
    [string]$Vault = $env:FFS_VAULT,
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'FFS'),
    [switch]$SkipService,
    [switch]$SkipPlugin,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Say($msg) { Write-Host "[install] $msg" }
function Run($block) {
    if ($DryRun) {
        Write-Host "+ $block"
    } else {
        & $block
    }
}

$BinDir = Join-Path $Prefix 'bin'
$DataDir = Join-Path $env:USERPROFILE '.ffs'

Say "FFS installer — prefix=$Prefix dry_run=$DryRun vault=$Vault"

# -------- locate sources --------

$ScriptHome = Split-Path -Parent $MyInvocation.MyCommand.Path

function Locate-Binary($name) {
    $candidates = @(
        (Join-Path $ScriptHome "bin\windows-x86_64\$name"),
        (Join-Path $ScriptHome "bin\$name"),
        (Join-Path $ScriptHome "..\target\release\$name"),
        (Join-Path $ScriptHome "..\..\target\release\$name")
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { return (Resolve-Path $c).Path }
    }
    throw "install.ps1: cannot locate binary '$name'"
}

# -------- bin placement --------

Say "installing binaries to $BinDir"
Run { New-Item -ItemType Directory -Force -Path $BinDir | Out-Null }
foreach ($name in @('ffs.exe', 'ffs-daemon.exe', 'ffs-mcp.exe')) {
    $src = Locate-Binary $name
    Run { Copy-Item -Force -Path $src -Destination (Join-Path $BinDir $name) }
}

# Append the bin dir to the user PATH (idempotent).
$current = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $current) { $current = '' }
if (-not ($current.Split(';') -contains $BinDir)) {
    if ($DryRun) {
        Write-Host "+ add $BinDir to user PATH"
    } else {
        [Environment]::SetEnvironmentVariable('Path', "$current;$BinDir", 'User')
        Say "added $BinDir to user PATH (open a new shell to pick it up)"
    }
}

# -------- runtime dirs --------

foreach ($d in @(
    (Join-Path $DataDir 'config\predicates'),
    (Join-Path $DataDir 'config\templates'),
    (Join-Path $DataDir 'skills'),
    (Join-Path $DataDir 'run'),
    (Join-Path $DataDir 'log')
)) {
    Run { New-Item -ItemType Directory -Force -Path $d | Out-Null }
}

# -------- seed starter library --------

function Install-Seed($src, $dst) {
    if (Test-Path $dst) {
        Say "preserving existing $dst"
    } else {
        Run { Copy-Item -Force -Path $src -Destination $dst }
    }
}

$StarterRoot = Join-Path $ScriptHome 'starter'
if (-not (Test-Path $StarterRoot)) {
    $StarterRoot = Join-Path $ScriptHome '..\starter'
}

foreach ($f in Get-ChildItem -Path (Join-Path $StarterRoot 'predicates') -Filter '*.toml') {
    Install-Seed $f.FullName (Join-Path $DataDir "config\predicates\$($f.Name)")
}
foreach ($f in Get-ChildItem -Path (Join-Path $StarterRoot 'templates') -Filter '*.tera') {
    Install-Seed $f.FullName (Join-Path $DataDir "config\templates\$($f.Name)")
}

# -------- skill bundles --------

$SkillsRoot = Join-Path $ScriptHome 'skills'
if (-not (Test-Path $SkillsRoot)) {
    $SkillsRoot = Join-Path $ScriptHome '..\skills'
}
foreach ($skill in @('auditor', 'librarian', 'scribe', '_lib')) {
    $srcSkill = Join-Path $SkillsRoot $skill
    if (Test-Path $srcSkill) {
        $dstSkill = Join-Path $DataDir "skills\$skill"
        Run { New-Item -ItemType Directory -Force -Path $dstSkill | Out-Null }
        Run { Copy-Item -Recurse -Force -Path "$srcSkill\*" -Destination $dstSkill }
    }
}

# -------- scheduled task wiring --------

if ($SkipService) {
    Say "skipping scheduled-task wiring (-SkipService)"
} else {
    $TaskName = 'FFS Daemon'
    $DaemonExe = Join-Path $BinDir 'ffs-daemon.exe'
    if (Get-Command -Name Register-ScheduledTask -ErrorAction SilentlyContinue) {
        $action = New-ScheduledTaskAction -Execute $DaemonExe
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
            -DontStopIfGoingOnBatteries `
            -StartWhenAvailable `
            -ExecutionTimeLimit ([TimeSpan]::Zero)
        $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive
        Run {
            Register-ScheduledTask `
                -TaskName $TaskName `
                -Action $action `
                -Trigger $trigger `
                -Settings $settings `
                -Principal $principal `
                -Description 'Foley File System personal substrate daemon (per-user).' `
                -Force | Out-Null
        }
        Say "registered scheduled task '$TaskName' for at-logon launch"
    } else {
        Say "WARN: Register-ScheduledTask not available — install.ps1 needs Windows 8+ with the ScheduledTasks module"
    }
}

# -------- obsidian plugin --------

if ($SkipPlugin) {
    Say "skipping Obsidian plugin registration (-SkipPlugin)"
} elseif (-not $Vault) {
    Say "no vault path provided (-Vault); skipping plugin registration"
} elseif (-not (Test-Path (Join-Path $Vault '.obsidian'))) {
    Say "WARN: $Vault\.obsidian does not exist — skipping plugin registration"
} else {
    $PluginDst = Join-Path $Vault '.obsidian\plugins\ffs'
    Run { New-Item -ItemType Directory -Force -Path $PluginDst | Out-Null }
    $PluginSrc = Join-Path $ScriptHome 'obsidian-plugin'
    if (-not (Test-Path $PluginSrc)) {
        $PluginSrc = Join-Path $ScriptHome '..\obsidian-plugin\dist'
    }
    if (-not (Test-Path $PluginSrc)) {
        $PluginSrc = Join-Path $ScriptHome '..\obsidian-plugin'
    }
    if (Test-Path $PluginSrc) {
        foreach ($f in @('main.js', 'manifest.json', 'styles.css')) {
            $src = Join-Path $PluginSrc $f
            if (Test-Path $src) {
                Run { Copy-Item -Force -Path $src -Destination (Join-Path $PluginDst $f) }
            }
        }
        Say "registered Obsidian plugin at $PluginDst"
    } else {
        Say "WARN: Obsidian plugin source not found; skipping"
    }
}

# -------- keychain bootstrap (deferred) --------

Say "keychain bootstrap: deferred to first-run interactive (see README)"
Say "done — try: $BinDir\ffs.exe health"
Say "next steps — see docs\onboarding\technical-friend-checklist.md and docs\onboarding\first-use-guide.md"
Say "trouble?  see docs\onboarding\troubleshooting.md"
