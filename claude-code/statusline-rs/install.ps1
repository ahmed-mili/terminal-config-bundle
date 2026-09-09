#Requires -Version 5.1
<#
.SYNOPSIS
    Standalone installer for the Claude Code statusline (single command).

.DESCRIPTION
    Downloads the latest prebuilt statusline.exe from the dev-environment
    GitHub Releases (rolling tag "statusline", rebuilt by
    .github/workflows/statusline-release.yml on every push), installs it to
    %USERPROFILE%\.claude\statusline.exe, and wires it up in
    %USERPROFILE%\.claude\settings.json by merging the "statusLine" key --
    every other key in settings.json is left untouched.

    No Rust toolchain, no repo clone: the binary ships prebuilt.

.LINK
    https://github.com/ahmed-mili/dev-environment

.EXAMPLE
    irm https://raw.githubusercontent.com/ahmed-mili/dev-environment/main/claude-code/statusline-rs/install.ps1 | iex
#>

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

$claudeDir    = Join-Path $env:USERPROFILE '.claude'
$exePath      = Join-Path $claudeDir 'statusline.exe'
$settingsPath = Join-Path $claudeDir 'settings.json'
$downloadUrl  = 'https://github.com/ahmed-mili/dev-environment/releases/download/statusline/statusline.exe'

Write-Host "==> Installation du statusline Claude Code" -ForegroundColor Blue

if (-not (Test-Path $claudeDir)) {
    New-Item -ItemType Directory -Path $claudeDir -Force | Out-Null
}

Write-Host "    Telechargement de statusline.exe..."
$tmpPath = "$exePath.download"
try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tmpPath -UseBasicParsing
} catch {
    Write-Host "    ECHEC telechargement: $($_.Exception.Message)" -ForegroundColor Red
    throw
}
Move-Item -Path $tmpPath -Destination $exePath -Force
Write-Host "    OK: $exePath" -ForegroundColor Green

# --- settings.json : fusionne la cle statusLine sans ecraser le reste ---
$settings = $null
if (Test-Path $settingsPath) {
    $raw = Get-Content -Raw -Path $settingsPath
    if ($raw -and $raw.Trim().Length -gt 0) {
        $settings = $raw | ConvertFrom-Json
    }
}
if (-not $settings) { $settings = [PSCustomObject]@{} }

$statusLineValue = [PSCustomObject]@{
    type            = 'command'
    command         = '~/.claude/statusline.exe'
    refreshInterval = 1
}

if ($settings.PSObject.Properties.Name -contains 'statusLine') {
    $settings.statusLine = $statusLineValue
} else {
    $settings | Add-Member -MemberType NoteProperty -Name 'statusLine' -Value $statusLineValue
}

$json = $settings | ConvertTo-Json -Depth 20
# Set-Content -Encoding UTF8 prepend un BOM sur Windows PowerShell 5.1 ; on
# ecrit nous-memes en UTF-8 sans BOM pour rester coherent avec le reste du repo.
[System.IO.File]::WriteAllText($settingsPath, $json, [System.Text.UTF8Encoding]::new($false))
Write-Host "    OK: statusLine configure dans settings.json" -ForegroundColor Green

Write-Host ""
Write-Host "Termine. Redemarre Claude Code pour voir le statusline." -ForegroundColor Blue
