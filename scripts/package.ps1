param([switch]$SkipBuild)
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
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $projectDirectory 'target\release\folio-gpui-prototype.exe') -Destination (Join-Path $destination 'Folio.exe')
Copy-Item -LiteralPath (Join-Path $projectDirectory 'runtime\bin\pdfium.dll') -Destination $destination
Copy-Item -LiteralPath (Join-Path $projectDirectory 'runtime\LICENSE') -Destination (Join-Path $destination 'PDFium-LICENSE.txt')
$licenses = Join-Path $destination 'pdfium-licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item -Path (Join-Path $projectDirectory 'runtime\licenses\*') -Destination $licenses -Recurse -Force
Copy-Item -LiteralPath (Join-Path $projectDirectory 'README.md') -Destination $destination
Write-Output (Join-Path $destination 'Folio.exe')
