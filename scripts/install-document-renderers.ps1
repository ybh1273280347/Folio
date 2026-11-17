param([string]$SkillLibrary = (Join-Path $env:USERPROFILE '.config/magpie/library/skills'))
$ErrorActionPreference = 'Stop'
$sourceDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'resources/pi-skills'
foreach ($skill in @('pdf', 'docx', 'pptx')) {
    $destination = Join-Path $SkillLibrary $skill
    $instructions = Join-Path $destination 'SKILL.md'
    if (-not (Test-Path -LiteralPath $instructions)) { throw "Existing skill not found: $instructions" }
    $null = New-Item -ItemType Directory -Path (Join-Path $destination 'scripts') -Force
    Copy-Item -LiteralPath (Join-Path $sourceDirectory "$skill/scripts/render_pages.py") -Destination (Join-Path $destination 'scripts/render_pages.py') -Force
    $section = Get-Content -LiteralPath (Join-Path $sourceDirectory "$skill/windows-rendering.md") -Raw
    $heading = ($section -split "`n", 2)[0].Trim()
    if (-not (Get-Content -LiteralPath $instructions -Raw).Contains($heading)) {
        Add-Content -LiteralPath $instructions -Value "`n$section" -Encoding utf8
    }
}
Write-Output 'Installed selected-page renderers into the existing Magpie skills.'
