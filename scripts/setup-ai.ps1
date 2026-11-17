param([switch]$InstallBrowser)
$ErrorActionPreference = 'Stop'
$hostDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'resources/pi-host'
Push-Location $hostDirectory
try {
    & npm.cmd ci --omit=dev --no-audit --no-fund
    if ($LASTEXITCODE) { throw 'Installing pi SDK host dependencies failed.' }
    if (-not (Test-Path -LiteralPath '.venv/Scripts/python.exe')) {
        & uv venv .venv --python 3.12
        if ($LASTEXITCODE) { throw 'Creating web fetch environment failed.' }
    }
    & uv pip sync --python .venv/Scripts/python.exe requirements.lock
    if ($LASTEXITCODE) { throw 'Installing web fetch dependencies failed.' }
    if ($InstallBrowser) {
        $env:PLAYWRIGHT_SKIP_BROWSER_GC = '1'
        & .venv/Scripts/python.exe -m patchright install chromium
        if ($LASTEXITCODE) { throw 'Installing dynamic fetch browser failed.' }
    }
} finally { Pop-Location }
