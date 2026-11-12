param(
    [Parameter(Mandatory = $true)][string]$ExecutablePath,
    [switch]$Unregister
)

$ErrorActionPreference = 'Stop'
$classes = 'HKCU:\Software\Classes'
$extensions = @('.pdf', '.md', '.tex', '.ltx')
$classIds = @{
    '.pdf' = 'Folio.PDF'
    '.md' = 'Folio.Markdown'
    '.tex' = 'Folio.LaTeX'
    '.ltx' = 'Folio.LaTeX'
}
$progIds = @('Folio.PDF', 'Folio.Markdown', 'Folio.LaTeX')
$capabilitiesPath = 'Software\Folio\Capabilities'
$registeredApplications = 'HKCU:\Software\RegisteredApplications'
$capabilities = 'HKCU:\Software\Folio\Capabilities'

if ($Unregister) {
    Remove-ItemProperty -LiteralPath $registeredApplications -Name 'Folio' -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $capabilities -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($progId in ($progIds | Select-Object -Unique)) {
        Remove-Item -LiteralPath (Join-Path $classes $progId) -Recurse -Force -ErrorAction SilentlyContinue
    }
    foreach ($extension in $extensions) {
        $openWith = Join-Path (Join-Path $classes $extension) 'OpenWithProgids'
        Remove-ItemProperty -LiteralPath $openWith -Name $classIds[$extension] -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath (Join-Path $classes 'Applications\Folio.exe') -Recurse -Force -ErrorAction SilentlyContinue
    exit 0
}

$exe = [System.IO.Path]::GetFullPath($ExecutablePath)
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "Folio executable does not exist: $exe"
}
$icon = '"' + $exe + '",0'
$openCommand = '"' + $exe + '" --open "%1"'
$displayNames = @{
    'Folio.PDF' = 'Folio PDF 文档'
    'Folio.Markdown' = 'Folio Markdown 文档'
    'Folio.LaTeX' = 'Folio LaTeX 文档'
}

foreach ($progId in ($progIds | Select-Object -Unique)) {
    $progIdPath = Join-Path $classes $progId
    New-Item -Path $progIdPath -Force | Out-Null
    Set-Item -LiteralPath $progIdPath -Value $displayNames[$progId]
    New-Item -Path (Join-Path $progIdPath 'DefaultIcon') -Force | Out-Null
    Set-Item -LiteralPath (Join-Path $progIdPath 'DefaultIcon') -Value $icon
    $openPath = Join-Path $progIdPath 'shell\open\command'
    New-Item -Path $openPath -Force | Out-Null
    Set-Item -LiteralPath $openPath -Value $openCommand
}

$applicationPath = Join-Path $classes 'Applications\Folio.exe'
New-Item -Path $applicationPath -Force | Out-Null
New-ItemProperty -LiteralPath $applicationPath -Name 'FriendlyAppName' -Value 'Folio' -PropertyType String -Force | Out-Null
New-Item -Path (Join-Path $applicationPath 'SupportedTypes') -Force | Out-Null
foreach ($extension in $extensions) {
    New-ItemProperty -LiteralPath (Join-Path $applicationPath 'SupportedTypes') -Name $extension -Value '' -PropertyType String -Force | Out-Null
    $openWith = Join-Path (Join-Path $classes $extension) 'OpenWithProgids'
    New-Item -Path $openWith -Force | Out-Null
    New-ItemProperty -LiteralPath $openWith -Name $classIds[$extension] -Value '' -PropertyType String -Force | Out-Null
}

New-Item -Path $capabilities -Force | Out-Null
New-ItemProperty -LiteralPath $capabilities -Name 'ApplicationName' -Value 'Folio' -PropertyType String -Force | Out-Null
New-ItemProperty -LiteralPath $capabilities -Name 'ApplicationDescription' -Value 'Folio 本地 PDF、Markdown 与 LaTeX 工作台' -PropertyType String -Force | Out-Null
$fileAssociations = Join-Path $capabilities 'FileAssociations'
New-Item -Path $fileAssociations -Force | Out-Null
foreach ($extension in $extensions) {
    New-ItemProperty -LiteralPath $fileAssociations -Name $extension -Value $classIds[$extension] -PropertyType String -Force | Out-Null
}
New-Item -Path $registeredApplications -Force | Out-Null
New-ItemProperty -LiteralPath $registeredApplications -Name 'Folio' -Value $capabilitiesPath -PropertyType String -Force | Out-Null
