param(
    [string]$Version = 'v32.7.4'
)

$ErrorActionPreference = 'Stop'
$prototypeDirectory = Split-Path -Parent $PSScriptRoot
$targetDirectory = Join-Path $prototypeDirectory 'resources\drawio'
$tempZip = Join-Path $env:TEMP "drawio-$Version.zip"

$downloadUrl = "https://github.com/jgraph/drawio/releases/download/$Version/draw.war"
Write-Output "Downloading draw.io $Version from $downloadUrl ..."
Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip

if (Test-Path -LiteralPath $targetDirectory) {
    Remove-Item -LiteralPath $targetDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $targetDirectory -Force | Out-Null

Write-Output "Extracting archive to $targetDirectory ..."
Expand-Archive -Path $tempZip -DestinationPath $targetDirectory -Force
Remove-Item -LiteralPath $tempZip -Force -ErrorAction SilentlyContinue

# Strip Java servlet container directories (not needed for static WebView hosting)
foreach ($unused in @('WEB-INF', 'META-INF')) {
    $unusedPath = Join-Path $targetDirectory $unused
    if (Test-Path -LiteralPath $unusedPath) {
        Remove-Item -LiteralPath $unusedPath -Recurse -Force
    }
}

$indexPath = Join-Path $targetDirectory 'index.html'
if (-not (Test-Path -LiteralPath $indexPath)) {
    throw "Extraction failed: index.html not found in $targetDirectory"
}

Write-Output "draw.io $Version static assets ready at: $targetDirectory"
