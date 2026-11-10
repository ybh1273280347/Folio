$ErrorActionPreference = 'Stop'
$prototypeDirectory = Split-Path -Parent $PSScriptRoot
$runtimeDirectory = Join-Path $prototypeDirectory 'runtime'
New-Item -ItemType Directory -Path $runtimeDirectory -Force | Out-Null
$archive = Join-Path $runtimeDirectory 'pdfium-win-x64.tgz'
Invoke-WebRequest 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8138/pdfium-win-x64.tgz' -OutFile $archive
tar -xzf $archive -C $runtimeDirectory
if ($LASTEXITCODE -ne 0) { throw 'PDFium archive extraction failed.' }
Write-Output (Join-Path $runtimeDirectory 'bin/pdfium.dll')
