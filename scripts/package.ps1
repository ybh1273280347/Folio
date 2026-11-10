param([switch]$SkipBuild, [switch]$Launch)
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
foreach ($process in Get-Process -Name Folio -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $legacyExecutable -or $_.Path.StartsWith($releaseRoot, [StringComparison]::OrdinalIgnoreCase) }) {
    $null = $process.CloseMainWindow()
    if (-not $process.WaitForExit(5000)) {
        throw 'Save your drafts and close Folio GPUI, then run this script again.'
    }
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $projectDirectory 'target\release\folio-gpui-prototype.exe') -Destination $executable
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\icon.ico') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'runtime\bin\pdfium.dll') -Destination $destination
$katexSource = Join-Path $projectDirectory 'resources\katex'
$katexDestination = Join-Path $destination 'katex'
New-Item -ItemType Directory -Path (Join-Path $katexDestination 'contrib') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $katexSource 'katex.min.css'), (Join-Path $katexSource 'katex.min.js') -Destination $katexDestination
Copy-Item -LiteralPath (Join-Path $katexSource 'contrib\auto-render.min.js') -Destination (Join-Path $katexDestination 'contrib')
Copy-Item -LiteralPath (Join-Path $katexSource 'fonts') -Destination $katexDestination -Recurse -Force
Copy-Item -LiteralPath (Join-Path $katexSource 'LICENSE') -Destination (Join-Path $destination 'KaTeX-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'runtime\LICENSE') -Destination (Join-Path $destination 'PDFium-LICENSE.txt')
$licenses = Join-Path $destination 'pdfium-licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item -Path (Join-Path $projectDirectory 'runtime\licenses\*') -Destination $licenses -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectDirectory 'README.md') -Destination $destination
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
