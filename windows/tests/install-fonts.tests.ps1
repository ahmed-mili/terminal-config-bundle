# install-fonts.tests.ps1 -- regression tests for per-user font registration.
#
# ASCII-only: this file must remain compatible with Windows PowerShell 5.1.

$ErrorActionPreference = 'Stop'

$TestsDir = Split-Path -Parent $PSCommandPath
$InstallPath = Join-Path (Split-Path -Parent $TestsDir) 'install.ps1'
$TmpDir = Join-Path ([IO.Path]::GetTempPath()) ("font-tests-" + [guid]::NewGuid().ToString('N'))
$RegistryPath = 'HKCU:\Software\DevEnvironmentFontTests\' + [guid]::NewGuid().ToString('N')
$SystemFontRegistry = 'HKCU:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts'
$OriginalLocalAppData = $env:LOCALAPPDATA
$RuntimeEntryName = $null

$script:Pass = 0
$script:Fail = 0

function Write-Result($Ok, $Label, $Detail) {
    if ($Ok) {
        $script:Pass++
        Write-Host ("  PASS  " + $Label) -ForegroundColor Green
    } else {
        $script:Fail++
        Write-Host ("  FAIL  " + $Label) -ForegroundColor Red
        if ($Detail) { Write-Host ("        " + $Detail) -ForegroundColor DarkGray }
    }
}

try {
    New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null
    New-Item -Path $RegistryPath -Force | Out-Null

    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        $InstallPath,
        [ref]$tokens,
        [ref]$errors
    )
    if ($errors.Count -gt 0) {
        throw "install.ps1 has parser errors: $($errors -join '; ')"
    }

    foreach ($functionName in @('Register-UserFontFiles', 'Install-UserFont')) {
        $functionAst = $ast.Find(
            {
                param($Node)
                $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $Node.Name -eq $functionName
            },
            $true
        )
        if (-not $functionAst) {
            throw "$functionName is missing from install.ps1"
        }
        . ([scriptblock]::Create($functionAst.Extent.Text))
    }

    Add-Type -TypeDefinition @'
using System;
public static class FontBroadcast {
    public static int AddCount = 0;
    public static int SendCount = 0;
    public static uint LastFlags = 0;
    public static int AddFontResource(string path) {
        AddCount++;
        return 1;
    }
    public static IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result) {
        SendCount++;
        LastFlags = flags;
        result = IntPtr.Zero;
        return IntPtr.Zero;
    }
}
'@
    function Write-Ok($Message) {}
    $SkipWinget = $false

    $sourceDir = Join-Path $TmpDir 'source'
    $fontsDir = Join-Path $TmpDir 'fonts'
    New-Item -ItemType Directory -Path $sourceDir -Force | Out-Null
    $sourceFont = Join-Path $sourceDir 'Demo-Regular.ttf'
    [IO.File]::WriteAllBytes($sourceFont, [byte[]](1, 2, 3, 4))

    $registered = @(Register-UserFontFiles `
        -FontFiles @(Get-Item -LiteralPath $sourceFont) `
        -UserFontsDir $fontsDir `
        -RegistryPath $RegistryPath)

    $installedFont = Join-Path $fontsDir 'Demo-Regular.ttf'
    $entryName = 'Demo-Regular (TrueType)'
    $entryValue = (Get-ItemProperty -Path $RegistryPath -Name $entryName).$entryName

    Write-Result (Test-Path -LiteralPath $installedFont) `
        'a new font file is copied to the user font directory' $installedFont
    Write-Result ($registered.Count -eq 1 -and $registered[0] -eq $installedFont) `
        'the helper returns the installed font path' ($registered -join ', ')
    Write-Result ($entryValue -eq $installedFont) `
        'a new font file is registered under HKCU' $entryValue

    Remove-ItemProperty -Path $RegistryPath -Name $entryName
    Register-UserFontFiles `
        -FontFiles @(Get-Item -LiteralPath $installedFont) `
        -UserFontsDir $fontsDir `
        -RegistryPath $RegistryPath | Out-Null
    $repairedValue = (Get-ItemProperty -Path $RegistryPath -Name $entryName).$entryName

    Write-Result ($repairedValue -eq $installedFont) `
        'an existing font file repairs a missing registry entry' $repairedValue

    $runtimeFileName = 'DshFontTest-' + [guid]::NewGuid().ToString('N') + '.ttf'
    $runtimeFontsDir = Join-Path $TmpDir 'Microsoft\Windows\Fonts'
    New-Item -ItemType Directory -Path $runtimeFontsDir -Force | Out-Null
    $runtimeFont = Join-Path $runtimeFontsDir $runtimeFileName
    Copy-Item -LiteralPath $installedFont -Destination $runtimeFont
    $RuntimeEntryName = "$([IO.Path]::GetFileNameWithoutExtension($runtimeFileName)) (TrueType)"
    New-ItemProperty -Path $SystemFontRegistry -Name $RuntimeEntryName -Value $runtimeFont -PropertyType String -Force | Out-Null
    $env:LOCALAPPDATA = $TmpDir

    [FontBroadcast]::AddCount = 0
    [FontBroadcast]::SendCount = 0
    Install-UserFont `
        -DisplayName 'Demo' `
        -Url 'unused.zip' `
        -FilePattern 'DshFontTest-*.ttf' `
        -MarkerFile $runtimeFileName
    Write-Result ([FontBroadcast]::AddCount -eq 0 -and [FontBroadcast]::SendCount -eq 0) `
        'an already registered font does not rebroadcast WM_FONTCHANGE' `
        "add=$([FontBroadcast]::AddCount), send=$([FontBroadcast]::SendCount)"

    Remove-ItemProperty -Path $SystemFontRegistry -Name $RuntimeEntryName
    [FontBroadcast]::AddCount = 0
    [FontBroadcast]::SendCount = 0
    [FontBroadcast]::LastFlags = 0
    Install-UserFont `
        -DisplayName 'Demo' `
        -Url 'unused.zip' `
        -FilePattern 'DshFontTest-*.ttf' `
        -MarkerFile $runtimeFileName
    Write-Result ([FontBroadcast]::SendCount -eq 1) `
        'a repaired registration broadcasts WM_FONTCHANGE once' `
        "send=$([FontBroadcast]::SendCount)"
    Write-Result ([FontBroadcast]::LastFlags -eq 2) `
        'the broadcast aborts instead of waiting on hung windows' `
        "flags=$([FontBroadcast]::LastFlags)"
} catch {
    Write-Result $false 'font registration test completed' $_.Exception.Message
} finally {
    $env:LOCALAPPDATA = $OriginalLocalAppData
    if ($RuntimeEntryName) {
        Remove-ItemProperty -Path $SystemFontRegistry -Name $RuntimeEntryName -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $RegistryPath -Recurse -Force -ErrorAction SilentlyContinue
}

$color = 'Green'
if ($script:Fail -gt 0) { $color = 'Red' }
Write-Host ''
Write-Host ("Result: {0} pass / {1} fail" -f $script:Pass, $script:Fail) -ForegroundColor $color
if ($script:Fail -gt 0) { exit 1 }
exit 0
