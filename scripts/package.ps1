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
$destination = Join-Path $projectDirectory 'dist\Folio'
$executable = Join-Path $destination 'Folio.exe'
# Ask only this native build to close; never terminate the original Electron app.
foreach ($process in Get-Process -Name Folio -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $executable }) {
    $null = $process.CloseMainWindow()
    if (-not $process.WaitForExit(5000)) {
        throw 'Save your drafts and close Folio GPUI, then run this script again.'
    }
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $projectDirectory 'target\release\folio-gpui-prototype.exe') -Destination $executable
Copy-Item -LiteralPath (Join-Path $projectDirectory 'resources\icon.ico') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'runtime\bin\pdfium.dll') -Destination $destination
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
Write-Output "Published: $executable"
Write-Output "Desktop shortcut: $shortcutPath"
if ($Launch) {
    Start-Process -FilePath $executable -WorkingDirectory $destination -WindowStyle Normal
}
