param([switch]$SkipBuild, [switch]$Launch, [switch]$KeepRunning)
$ErrorActionPreference = 'Stop'
$projectDirectory = Split-Path -Parent $PSScriptRoot
$compiler = (Get-Command gcc).Source
if (-not $SkipBuild) {
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $compiler
    Push-Location $projectDirectory
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'Folio release build failed.' }
    } finally { Pop-Location }
}
$releaseName = 'Folio-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
$destination = Join-Path $projectDirectory ("dist\releases\$releaseName")
$executable = Join-Path $destination 'Folio.exe'
$releaseRoot = (Join-Path $projectDirectory 'dist\releases').TrimEnd('\') + '\'
$legacyExecutable = Join-Path $projectDirectory 'dist\Folio\Folio.exe'
if (-not $KeepRunning) {
foreach ($process in Get-Process -Name Folio -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $legacyExecutable -or $_.Path.StartsWith($releaseRoot, [StringComparison]::OrdinalIgnoreCase) }) {
    $null = $process.CloseMainWindow()
    if (-not $process.WaitForExit(5000)) {
        throw 'Save your drafts and close Folio GPUI, then run this script again.'
    }
}
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $projectDirectory 'target\release\folio-gpui-prototype.exe') -Destination $executable
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\icon.ico') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\latex-rust\LICENSE-MIT') -Destination (Join-Path $destination 'LaTeX-Rust-LICENSE-MIT.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\latex-rust\NOTICE') -Destination (Join-Path $destination 'LaTeX-Rust-NOTICE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\gpui-kit-markdown-editor\LICENSE-APACHE') -Destination (Join-Path $destination 'Markdown-Editor-LICENSE-APACHE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\gpui-pdf\LICENSE') -Destination (Join-Path $destination 'GPUI-PDF-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'README.md') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\fonts\OFL.txt') -Destination (Join-Path $destination 'STIX-Fonts-OFL.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\fonts\GUST-FONT-LICENSE.txt') -Destination $destination
$desktop = [Environment]::GetFolderPath('Desktop')
$shortcutPath = Join-Path $desktop 'Folio GPUI.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $executable
$shortcut.WorkingDirectory = $destination
$shortcut.IconLocation = (Join-Path $destination 'icon.ico')
$shortcut.Description = 'Folio native GPUI workspace'
$shortcut.Save()
$launcherPath = Join-Path $projectDirectory '启动 Folio.cmd'
Set-Content -LiteralPath $launcherPath -Encoding Ascii -Value "@echo off`r`nstart `"`" `"%~dp0dist\releases\$releaseName\Folio.exe`" %*"
Write-Output "Published: $executable"
Write-Output "Desktop shortcut: $shortcutPath"
if ($Launch) {
    Start-Process -FilePath $executable -WorkingDirectory $destination -WindowStyle Normal
}
