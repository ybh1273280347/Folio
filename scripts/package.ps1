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
$stableDirectory = Join-Path $projectDirectory 'dist\Folio'
$stableExecutable = Join-Path $stableDirectory 'Folio.exe'
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
$webview2Loader = Get-ChildItem -Path (Join-Path $projectDirectory 'target\release\build\webview2-com-sys-*\out\x64\WebView2Loader.dll') -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $webview2Loader) { throw 'WebView2Loader.dll not found in target\release\build.' }
Copy-Item -LiteralPath $webview2Loader.FullName -Destination (Join-Path $destination 'WebView2Loader.dll') -Force
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\icon.ico') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\latex-rust\LICENSE-MIT') -Destination (Join-Path $destination 'LaTeX-Rust-LICENSE-MIT.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\latex-rust\NOTICE') -Destination (Join-Path $destination 'LaTeX-Rust-NOTICE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\gpui-kit-markdown-editor\LICENSE-APACHE') -Destination (Join-Path $destination 'Markdown-Editor-LICENSE-APACHE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\gpui-xterm\LICENSE') -Destination (Join-Path $destination 'GPUI-Xterm-LICENSE-MIT.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'vendor\gpui-pdf\LICENSE') -Destination (Join-Path $destination 'GPUI-PDF-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\rag\Qwen3-Embedding-LICENSE') -Destination (Join-Path $destination 'Qwen3-Embedding-LICENSE.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'README.md') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\fonts\OFL.txt') -Destination (Join-Path $destination 'STIX-Fonts-OFL.txt')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\fonts\GUST-FONT-LICENSE.txt') -Destination $destination
# The SDK stays out of the executable; ship its locked dependencies and isolated fetch environment.
$sdkSource = Join-Path $projectDirectory 'resources/pi-host'
if (-not (Test-Path -LiteralPath (Join-Path $sdkSource 'node_modules'))) { throw 'Run scripts/setup-ai.ps1 before packaging.' }
if (-not (Test-Path -LiteralPath (Join-Path $sdkSource '.venv/Scripts/python.exe'))) { throw 'The web fetch Python environment is missing; run scripts/setup-ai.ps1.' }
foreach ($hostDestination in @((Join-Path $destination 'pi-host'), (Join-Path $stableDirectory 'pi-host'))) {
    $null = New-Item -ItemType Directory -Path $hostDestination -Force
    & robocopy.exe $sdkSource $hostDestination /E /XD test __pycache__ .pytest_cache /XF *.log *.tmp /COPY:DAT /DCOPY:DAT /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "Copying pi-host failed with robocopy exit code $LASTEXITCODE." }
}
$drawioSource = Join-Path $projectDirectory 'resources\drawio'
if (-not (Test-Path -LiteralPath (Join-Path $drawioSource 'index.html'))) { throw 'The offline draw.io assets are missing; run scripts/install-drawio.ps1.' }
foreach ($drawioDestination in @((Join-Path $destination 'drawio'), (Join-Path $stableDirectory 'drawio'))) {
    $null = New-Item -ItemType Directory -Path $drawioDestination -Force
    & robocopy.exe $drawioSource $drawioDestination /E /COPY:DAT /DCOPY:DAT /R:1 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "Copying draw.io assets failed with robocopy exit code $LASTEXITCODE." }
}
$null = New-Item -ItemType Directory -Path $stableDirectory -Force
Copy-Item -LiteralPath $executable -Destination $stableExecutable -Force
Copy-Item -LiteralPath $webview2Loader.FullName -Destination (Join-Path $stableDirectory 'WebView2Loader.dll') -Force
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\icon.ico') -Destination $stableDirectory -Force
Copy-Item -LiteralPath (Join-Path $destination 'Qwen3-Embedding-LICENSE.txt') -Destination $stableDirectory -Force
$null = & (Join-Path $PSScriptRoot 'register-file-associations.ps1') -ExecutablePath $stableExecutable
$desktop = [Environment]::GetFolderPath('Desktop')
$shortcutPath = Join-Path $desktop 'Folio GPUI.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $stableExecutable
$shortcut.WorkingDirectory = $stableDirectory
$shortcut.IconLocation = (Join-Path $stableDirectory 'icon.ico')
$shortcut.Description = 'Folio native GPUI workspace'
$shortcut.Save()
$launcherPath = Join-Path $projectDirectory '启动 Folio.cmd'
Set-Content -LiteralPath $launcherPath -Encoding Ascii -Value "@echo off`r`nstart `"`" `"%~dp0dist\Folio\Folio.exe`" %*"
Write-Output "Published: $executable"
Write-Output "Stable launcher: $stableExecutable"
Write-Output "Desktop shortcut: $shortcutPath"
Write-Output 'PDF/Markdown/LaTeX Open With entries registered for this user; choose Folio in Windows Default Apps to make it the default.'
if ($Launch) {
    Start-Process -FilePath $stableExecutable -WorkingDirectory $stableDirectory -WindowStyle Normal
}
